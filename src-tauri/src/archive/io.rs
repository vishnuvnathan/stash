//! Building an archive out of the database, and merging one back in.
//!
//! Kept apart from the document model in `super`, which stays pure and
//! testable. Everything that touches the pool, the blob directory or a secret
//! lives here.

use super::{Archive, ArchiveFolder, ArchiveItem};
use crate::db::{queries, Db};
use crate::secret;
use crate::transform::{b64_decode, b64_encode};
use anyhow::{Context, Result};
use std::collections::HashMap;

/// What an import did. Reported back so the user sees a count rather than a
/// silent success, and so "nothing happened" is distinguishable from "it all
/// already existed".
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: usize,
    /// Items already present -- clips by hash, everything else by exact
    /// content. Re-importing the same archive lands entirely here.
    pub skipped: usize,
    pub folders_created: usize,
    pub secrets_restored: usize,
    /// Rows that could not be written. The import continues past them, because
    /// losing the rest of a restore to one bad row would be worse.
    pub failed: usize,
    /// Set when the archive carried no passwords, so the UI can say why none
    /// were restored instead of leaving the user to wonder.
    pub secrets_absent: bool,
}

/// Reads the whole database into an archive.
///
/// `include_secrets` is threaded down rather than filtered afterwards: a
/// password that is never read cannot be left in a buffer by accident.
pub async fn build(db: &Db, include_secrets: bool) -> Result<Archive> {
    let mut archive = Archive::new(include_secrets);

    let folders = queries::list_folders(&db.pool).await.context("reading folders")?;
    let folder_names: HashMap<String, String> =
        folders.iter().map(|f| (f.id.clone(), f.name.clone())).collect();

    archive.folders = folders
        .iter()
        .map(|f| ArchiveFolder { name: f.name.clone(), sort_order: f.sort_order })
        .collect();

    for item in queries::all_items(&db.pool).await.context("reading items")? {
        let folder = item.folder_id.as_ref().and_then(|id| folder_names.get(id).cloned());
        let mut row = ArchiveItem::from_item(&item, folder);

        // Images are inlined so the archive is one self-contained file. A blob
        // path pointing into an app-data directory that will not exist on the
        // other machine is not a backup.
        if let Some(rel) = &item.blob_path {
            match std::fs::read(db.blob_dir.join(rel)) {
                Ok(bytes) => row.image_base64 = Some(b64_encode(&bytes)),
                Err(e) => {
                    // The row is still worth exporting without its picture.
                    tracing::warn!(rel, error = %e, "could not read blob for export");
                }
            }
        }

        if include_secrets && row.is_credential() {
            if let Some((stored, enc)) = queries::read_secret(&db.pool, &item.id).await? {
                match secret::open(&stored, &enc) {
                    Ok(plain) => row.secret = Some(plain),
                    Err(e) => {
                        // Better to export the credential without its password
                        // than to abort the whole backup.
                        tracing::warn!(error = %e, "could not decrypt a secret for export");
                    }
                }
            }
        }

        archive.items.push(row);
    }

    Ok(archive)
}

/// Merges an archive into the database. Never deletes, never overwrites.
///
/// Items already here are skipped, so restoring the same file twice does not
/// double your history -- the behaviour someone recovering a backup under
/// stress will assume without checking.
pub async fn restore(db: &Db, archive: &Archive) -> Result<ImportReport> {
    let mut report = ImportReport {
        secrets_absent: !archive.includes_secrets,
        ..Default::default()
    };

    // Folders first, so items can be filed as they land. `create_folder` is
    // create-or-get, so an existing name is reused rather than duplicated.
    let mut folder_ids: HashMap<String, String> = HashMap::new();
    let existing = queries::list_folders(&db.pool).await?.len();
    for folder in &archive.folders {
        match queries::create_folder(&db.pool, &folder.name).await {
            Ok(f) => {
                folder_ids.insert(folder.name.clone(), f.id);
            }
            Err(e) => tracing::warn!(name = folder.name, error = %e, "could not create folder"),
        }
    }
    report.folders_created = queries::list_folders(&db.pool).await?.len().saturating_sub(existing);

    for item in &archive.items {
        // An item filed into a folder that was not listed in the archive still
        // gets its folder made, rather than silently arriving unfiled.
        let folder_id = match &item.folder {
            None => None,
            Some(name) => match folder_ids.get(name) {
                Some(id) => Some(id.clone()),
                None => match queries::create_folder(&db.pool, name).await {
                    Ok(f) => {
                        folder_ids.insert(name.clone(), f.id.clone());
                        Some(f.id)
                    }
                    Err(_) => None,
                },
            },
        };

        // Clips carry a hash and are matched on it. Notes and credentials do
        // not, so they fall back to an exact-content match -- without which
        // restoring the same backup twice would duplicate every note.
        let already = match &item.hash {
            Some(hash) => queries::hash_exists(&db.pool, hash).await.unwrap_or(false),
            None => queries::duplicate_exists(
                &db.pool,
                &item.content_type,
                item.title.as_deref(),
                item.content.as_deref(),
                item.created_at,
            )
            .await
            .unwrap_or(false),
        };
        if already {
            report.skipped += 1;
            continue;
        }

        // Write the picture back before the row, so a row never points at a
        // blob that is not there.
        let blob_path = match &item.image_base64 {
            None => None,
            Some(b64) => match restore_blob(db, item.hash.as_deref(), b64) {
                Ok(rel) => Some(rel),
                Err(e) => {
                    tracing::warn!(error = %e, "could not restore an image");
                    None
                }
            },
        };

        let inserted = queries::insert_imported(
            &db.pool,
            &item.kind,
            &item.content_type,
            item.content.as_deref(),
            blob_path.as_deref(),
            item.hash.as_deref(),
            item.source_app.as_deref(),
            item.title.as_deref(),
            item.pinned,
            item.created_at,
            item.updated_at,
            item.use_count,
            item.last_used_at,
            folder_id.as_deref(),
        )
        .await;

        let id = match inserted {
            Ok(id) => id,
            Err(e) => {
                tracing::warn!(error = %e, "could not import an item");
                report.failed += 1;
                continue;
            }
        };
        report.imported += 1;

        // Re-sealed with *this* machine's key, not carried across as whatever
        // the source machine used.
        if let Some(plain) = &item.secret {
            match secret::seal(plain) {
                Ok((stored, enc)) => {
                    match queries::upsert_secret(&db.pool, &id, &stored, enc).await {
                        Ok(()) => report.secrets_restored += 1,
                        Err(e) => tracing::warn!(error = %e, "could not store an imported secret"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "could not seal an imported secret"),
            }
        }
    }

    Ok(report)
}

/// Writes an inlined PNG back into the blob store, sharded the same way the
/// watcher does it. Falls back to hashing the bytes when the archive carried no
/// hash, so the file still lands somewhere deterministic.
fn restore_blob(db: &Db, hash: Option<&str>, b64: &str) -> Result<String> {
    let bytes = b64_decode(b64).map_err(|e| anyhow::anyhow!("bad image data: {e}"))?;
    let hash = match hash {
        Some(h) if h.len() >= 2 => h.to_string(),
        _ => crate::clipboard::hash_bytes(&bytes),
    };

    let shard = &hash[..2];
    let dir = db.blob_dir.join(shard);
    let full = dir.join(format!("{hash}.png"));
    let rel = format!("{shard}/{hash}.png");

    if full.exists() {
        return Ok(rel);
    }
    std::fs::create_dir_all(&dir)?;
    std::fs::write(&full, &bytes)?;
    Ok(rel)
}
