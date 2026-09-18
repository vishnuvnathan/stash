pub mod types;

#[cfg(windows)]
#[path = "win.rs"]
mod platform;

#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod platform;

#[cfg(target_os = "linux")]
#[path = "linux.rs"]
mod platform;

use crate::db::model::{ContentType, ItemKind, NewItem};
use crate::db::queries::{self, Upsert};
use crate::db::Db;
use crate::detect;
use crate::state::SelfCopyGuard;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use types::{PollState, Snapshot, MAX_IMAGE_BYTES};

pub const POLL_INTERVAL: Duration = Duration::from_millis(400);

pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    hex::encode(h.finalize())
}

/// Re-encodes an arbitrary image buffer to PNG. Used by the macOS TIFF path;
/// compiled out elsewhere so it does not show up as dead code.
#[cfg(target_os = "macos")]
pub fn reencode_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let decoded = image::load_from_memory(bytes).ok()?;
    let mut png = Vec::new();
    decoded
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

/// Starts the watcher: a dedicated OS thread does the platform polling (Win32
/// clipboard calls prefer a stable thread, and a plain `sleep` loop allocates
/// nothing while idle), and hands anything interesting to an async task that
/// owns the database work.
pub fn spawn(app: AppHandle, db: Db, guard: SelfCopyGuard) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Captured>();

    std::thread::Builder::new()
        .name("stash-clipboard".into())
        .spawn(move || {
            let mut state = PollState::default();
            loop {
                std::thread::sleep(POLL_INTERVAL);

                // The only work on an unchanged clipboard: one platform call and
                // an integer compare. Nothing is allocated and nothing is sent.
                let snapshot = platform::poll(&mut state);
                let captured = match snapshot {
                    Snapshot::Unchanged | Snapshot::Unsupported => continue,
                    Snapshot::Concealed => {
                        tracing::trace!("clipboard change marked concealed; not recorded");
                        continue;
                    }
                    Snapshot::Text(text) => Captured::Text {
                        hash: hash_bytes(text.as_bytes()),
                        text,
                        source_app: platform::foreground_app(),
                    },
                    Snapshot::Image(png) => {
                        if png.len() > MAX_IMAGE_BYTES {
                            tracing::debug!(bytes = png.len(), "image over limit; skipped");
                            continue;
                        }
                        Captured::Image {
                            hash: hash_bytes(&png),
                            png,
                            source_app: platform::foreground_app(),
                        }
                    }
                };

                if tx.send(captured).is_err() {
                    break; // receiver gone: app is shutting down
                }
            }
        })
        .expect("spawning clipboard watcher thread");

    tauri::async_runtime::spawn(async move {
        while let Some(captured) = rx.recv().await {
            if guard.take_if_matches(captured.hash()) {
                // This is our own `copy_item` write coming back around.
                continue;
            }
            if let Err(e) = ingest(&app, &db, captured).await {
                tracing::warn!(error = %e, "failed to record clipboard entry");
            }
        }
    });
}

enum Captured {
    Text {
        text: String,
        hash: String,
        source_app: Option<String>,
    },
    Image {
        png: Vec<u8>,
        hash: String,
        source_app: Option<String>,
    },
}

impl Captured {
    fn hash(&self) -> &str {
        match self {
            Captured::Text { hash, .. } | Captured::Image { hash, .. } => hash,
        }
    }
}

async fn ingest(app: &AppHandle, db: &Db, captured: Captured) -> anyhow::Result<()> {
    let new = match captured {
        Captured::Text {
            text,
            hash,
            source_app,
        } => {
            let content_type = detect::classify(&text);
            NewItem {
                kind: ItemKind::Clip,
                content_type,
                title: Some(detect::derive_title(&text, 120)),
                content: Some(text),
                blob_path: None,
                hash: Some(hash),
                source_app,
                // Captured clips are always unfiled; filing is a user action.
                folder_id: None,
            }
        }
        Captured::Image {
            png,
            hash,
            source_app,
        } => {
            let rel = write_blob(db, &hash, &png)?;
            NewItem {
                kind: ItemKind::Clip,
                content_type: ContentType::Image,
                title: Some(format!("Image ({} KB)", png.len() / 1024)),
                content: None,
                blob_path: Some(rel),
                hash: Some(hash),
                source_app,
                folder_id: None,
            }
        }
    };

    match queries::upsert_by_hash(&db.pool, new).await? {
        Upsert::Inserted(item) => {
            app.emit("item-added", item)?;
        }
        Upsert::Bumped(id) => {
            // Re-copying the same thing must not grow the list, but the row does
            // move to the top, so the UI still needs to hear about it.
            app.emit("item-bumped", id)?;
        }
    }

    Ok(())
}

/// Blobs are sharded by the first hash byte so one directory never accumulates
/// thousands of entries. An identical image already on disk is not rewritten.
fn write_blob(db: &Db, hash: &str, png: &[u8]) -> anyhow::Result<String> {
    let shard = &hash[..2];
    let rel = format!("{shard}/{hash}.png");
    let dir = db.blob_dir.join(shard);
    let full = dir.join(format!("{hash}.png"));

    if full.exists() {
        return Ok(rel);
    }

    std::fs::create_dir_all(&dir)?;
    std::fs::write(&full, png)?;
    Ok(rel)
}
