use super::Db;
use anyhow::Result;
use sqlx::{Row, SqlitePool};
use std::time::Duration;

/// Non-pinned clips kept. Notes and pinned clips are never pruned.
const KEEP_CLIPS: i64 = 1000;
/// How long a soft-deleted row stays recoverable before the blob is unlinked.
const GRACE_MS: i64 = 24 * 60 * 60 * 1000;

const INTERVAL: Duration = Duration::from_secs(5 * 60);
const FIRST_RUN_DELAY: Duration = Duration::from_secs(30);

pub fn spawn(db: Db) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_RUN_DELAY).await;
        let mut ticker = tokio::time::interval(INTERVAL);
        loop {
            if let Err(e) = run_once(&db).await {
                tracing::warn!(error = %e, "retention pass failed");
            }
            ticker.tick().await;
        }
    });
}

/// One pass. Returns early the moment there is nothing to do, so an idle
/// machine performs no writes -- the common case is a single SELECT COUNT.
pub async fn run_once(db: &Db) -> Result<()> {
    let trimmed = trim_overflow(&db.pool).await?;
    if trimmed > 0 {
        tracing::debug!(count = trimmed, "soft-deleted clips over retention limit");
    }
    purge_expired(db).await?;
    Ok(())
}

async fn trim_overflow(pool: &SqlitePool) -> Result<u64> {
    let live: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM items
         WHERE kind = 'clip' AND pinned = 0 AND deleted_at IS NULL",
    )
    .fetch_one(pool)
    .await?;

    if live.0 <= KEEP_CLIPS {
        return Ok(0);
    }

    let res = sqlx::query(
        "UPDATE items SET deleted_at = ?1
         WHERE kind = 'clip' AND pinned = 0 AND deleted_at IS NULL
           AND id NOT IN (
             SELECT id FROM items
             WHERE kind = 'clip' AND pinned = 0 AND deleted_at IS NULL
             ORDER BY updated_at DESC LIMIT ?2
           )",
    )
    .bind(super::model::now_millis())
    .bind(KEEP_CLIPS)
    .execute(pool)
    .await?;

    Ok(res.rows_affected())
}

/// Hard-deletes rows past the grace window and unlinks any blob no longer
/// referenced by a live row. The reference check matters because two clips can
/// share a blob path -- identical images copied from different apps.
async fn purge_expired(db: &Db) -> Result<()> {
    let cutoff = super::model::now_millis() - GRACE_MS;

    let doomed = sqlx::query(
        "SELECT id, blob_path FROM items WHERE deleted_at IS NOT NULL AND deleted_at < ?1",
    )
    .bind(cutoff)
    .fetch_all(&db.pool)
    .await?;

    if doomed.is_empty() {
        return Ok(());
    }

    let blob_paths: Vec<String> = doomed
        .iter()
        .filter_map(|r| r.try_get::<Option<String>, _>("blob_path").ok().flatten())
        .collect();

    sqlx::query("DELETE FROM items WHERE deleted_at IS NOT NULL AND deleted_at < ?1")
        .bind(cutoff)
        .execute(&db.pool)
        .await?;

    for path in blob_paths {
        let still_used: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM items WHERE blob_path = ?1")
                .bind(&path)
                .fetch_one(&db.pool)
                .await?;
        if still_used.0 > 0 {
            continue;
        }
        let full = db.blob_dir.join(&path);
        if let Err(e) = std::fs::remove_file(&full) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(path = %full.display(), error = %e, "could not remove blob");
            }
        }
    }

    Ok(())
}
