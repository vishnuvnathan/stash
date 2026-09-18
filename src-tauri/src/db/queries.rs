use super::model::{now_millis, Filters, Folder, Item, NewItem};
use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

/// Always qualified with `items.` and aliased: the FTS search path joins
/// `items_fts`, which also has `title` and `content`, so bare names there would
/// be ambiguous. The aliases keep the column names row decoding expects.
///
/// This list and `Item::from_row` are one unit -- adding a column to either
/// alone yields a runtime `ColumnNotFound` that breaks every search.
///
/// Note what is absent: `item_secrets` is never joined here. A password cannot
/// reach the frontend through the search path because the column is not in the
/// query at all.
const ITEM_COLS: &str = "items.id AS id, items.kind AS kind,
                         items.content_type AS content_type, items.content AS content,
                         items.blob_path AS blob_path, items.hash AS hash,
                         items.source_app AS source_app, items.title AS title,
                         items.pinned AS pinned, items.created_at AS created_at,
                         items.updated_at AS updated_at, items.folder_id AS folder_id,
                         items.use_count AS use_count, items.last_used_at AS last_used_at";

/// Outcome of feeding clipboard content to the database, so the caller knows
/// which event to emit without running a second query.
pub enum Upsert {
    Inserted(Item),
    Bumped(String),
}

/// Insert, or bump `updated_at` when the same hash is already stored and live.
/// The bump path deliberately performs one UPDATE and nothing else -- repeated
/// copies of the same thing must not grow the table.
pub async fn upsert_by_hash(pool: &SqlitePool, new: NewItem) -> Result<Upsert> {
    let now = now_millis();

    if let Some(hash) = new.hash.as_deref() {
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM items WHERE hash = ?1 AND deleted_at IS NULL LIMIT 1")
                .bind(hash)
                .fetch_optional(pool)
                .await?;
        if let Some((id,)) = existing {
            sqlx::query("UPDATE items SET updated_at = ?1 WHERE id = ?2")
                .bind(now)
                .bind(&id)
                .execute(pool)
                .await?;
            return Ok(Upsert::Bumped(id));
        }
    }

    Ok(Upsert::Inserted(insert_item(pool, new, now).await?))
}

pub async fn insert_item(pool: &SqlitePool, new: NewItem, now: i64) -> Result<Item> {
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO items
           (id, kind, content_type, content, blob_path, hash, source_app, title,
            pinned, created_at, updated_at, deleted_at, folder_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9, ?9, NULL, ?10)",
    )
    .bind(&id)
    .bind(new.kind.as_str())
    .bind(new.content_type.as_str())
    .bind(&new.content)
    .bind(&new.blob_path)
    .bind(&new.hash)
    .bind(&new.source_app)
    .bind(&new.title)
    .bind(now)
    .bind(&new.folder_id)
    .execute(pool)
    .await?;

    Ok(Item {
        id,
        kind: new.kind.as_str().to_string(),
        content_type: new.content_type.as_str().to_string(),
        content: new.content,
        blob_path: new.blob_path,
        hash: new.hash,
        source_app: new.source_app,
        title: new.title,
        pinned: false,
        created_at: now,
        updated_at: now,
        folder_id: new.folder_id,
        use_count: 0,
        last_used_at: None,
    })
}

/// Empty query -> recent-first list. Non-empty query -> FTS5 MATCH ranked by
/// bm25, pinned first. Both paths share the same filter clauses so a chip
/// behaves identically whether or not you are searching.
///
/// `query` here is already the *residual* text: `commands::search_items` runs it
/// through `crate::query::parse` first, so anything that looked like an operator
/// has been lifted into `filters` and never reaches FTS.
pub async fn search(
    pool: &SqlitePool,
    query: &str,
    filters: &Filters,
    limit: i64,
    offset: i64,
) -> Result<Vec<Item>> {
    let trimmed = query.trim();
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new("SELECT ");
    qb.push(ITEM_COLS);

    if trimmed.is_empty() {
        qb.push(" FROM items WHERE items.deleted_at IS NULL");
    } else {
        qb.push(
            " FROM items JOIN items_fts ON items_fts.rowid = items.rowid \
             WHERE items_fts MATCH ",
        );
        qb.push_bind(fts_query(trimmed));
        qb.push(" AND items.deleted_at IS NULL");
    }

    push_filters(&mut qb, filters);

    // `sort:used` overrides both default orders. Pinned still leads: pinning is
    // an explicit instruction and outranks any inferred ranking.
    if filters.most_used {
        qb.push(
            " ORDER BY items.pinned DESC, items.use_count DESC, \
             items.last_used_at DESC, items.updated_at DESC",
        );
    } else if trimmed.is_empty() {
        qb.push(" ORDER BY items.pinned DESC, items.updated_at DESC");
    } else {
        qb.push(" ORDER BY items.pinned DESC, bm25(items_fts) ASC, items.updated_at DESC");
    }

    qb.push(" LIMIT ").push_bind(limit);
    qb.push(" OFFSET ").push_bind(offset);

    let rows = qb.build().fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Item::from_row(row)?);
    }
    Ok(out)
}

/// Both search paths push these onto a query that already has a WHERE, and both
/// call this same function -- which is what makes a chip behave identically
/// whether or not you are typing. Folder filtering rides that seam, so `search`
/// itself needed no change to gain it.
fn push_filters(qb: &mut QueryBuilder<Sqlite>, filters: &Filters) {
    if filters.pinned_only {
        qb.push(" AND items.pinned = 1");
    }
    if filters.unfiled_only {
        qb.push(" AND items.folder_id IS NULL");
    }
    push_in_clause(qb, "items.kind", &filters.kinds);
    push_in_clause(qb, "items.content_type", &filters.content_types);
    push_in_clause(qb, "items.source_app", &filters.source_apps);
    push_folder_clause(qb, filters);
}

/// Folders can arrive two ways at once -- a clicked chip carries an id, a typed
/// `folder:work` carries a name -- and they must be OR-ed, not AND-ed: asking
/// for two folders means "in either", the same as asking for two source apps.
/// Names are resolved by subquery rather than a prior lookup, which keeps
/// `query::parse` a pure function with no database access.
fn push_folder_clause(qb: &mut QueryBuilder<Sqlite>, filters: &Filters) {
    if filters.folder_ids.is_empty() && filters.folder_names.is_empty() {
        return;
    }

    qb.push(" AND (");
    let mut first = true;

    if !filters.folder_ids.is_empty() {
        qb.push("items.folder_id IN (");
        let mut sep = qb.separated(", ");
        for v in &filters.folder_ids {
            sep.push_bind(v.clone());
        }
        qb.push(")");
        first = false;
    }

    if !filters.folder_names.is_empty() {
        if !first {
            qb.push(" OR ");
        }
        // COLLATE NOCASE matches how folder names are stored and compared
        // everywhere else, so `folder:Work` finds the folder called "work".
        qb.push("items.folder_id IN (SELECT id FROM folders WHERE name COLLATE NOCASE IN (");
        let mut sep = qb.separated(", ");
        for v in &filters.folder_names {
            sep.push_bind(v.clone());
        }
        qb.push("))");
    }

    qb.push(")");
}

fn push_in_clause(qb: &mut QueryBuilder<Sqlite>, column: &str, values: &[String]) {
    if values.is_empty() {
        return;
    }
    qb.push(format!(" AND {column} IN ("));
    let mut sep = qb.separated(", ");
    for v in values {
        sep.push_bind(v.clone());
    }
    qb.push(")");
}

/// Turns user input into an FTS5 expression. Every token is quoted so that
/// punctuation the user pasted in cannot be read as FTS syntax, and the last
/// token gets a `*` so results narrow as they type.
fn fts_query(input: &str) -> String {
    let tokens: Vec<String> = input
        .split_whitespace()
        .map(|t| t.replace('"', " ").trim().to_string())
        .filter(|t| !t.is_empty())
        .collect();

    if tokens.is_empty() {
        return String::from("\"\"");
    }

    let last = tokens.len() - 1;
    tokens
        .iter()
        .enumerate()
        .map(|(i, t)| if i == last { format!("\"{t}\"*") } else { format!("\"{t}\"") })
        .collect::<Vec<_>>()
        .join(" AND ")
}

pub async fn get(pool: &SqlitePool, id: &str) -> Result<Option<Item>> {
    let sql = format!("SELECT {ITEM_COLS} FROM items WHERE items.id = ?1 AND items.deleted_at IS NULL");
    let row = sqlx::query(&sql).bind(id).fetch_optional(pool).await?;
    row.as_ref().map(Item::from_row).transpose().map_err(Into::into)
}

pub async fn soft_delete(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE items SET deleted_at = ?1, updated_at = ?1 WHERE id = ?2")
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Records that an item was actually used -- copied or pasted *out* of Stash.
///
/// Deliberately does not touch `updated_at`. That column drives the recent-first
/// list, and bumping it here would shuffle the list under the user every time
/// they copied something, which is the opposite of what a history is for.
pub async fn bump_usage(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query(
        "UPDATE items SET use_count = use_count + 1, last_used_at = ?1 WHERE id = ?2",
    )
    .bind(now_millis())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn set_pinned(pool: &SqlitePool, id: &str, pinned: bool) -> Result<()> {
    sqlx::query("UPDATE items SET pinned = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(if pinned { 1 } else { 0 })
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn update_note(pool: &SqlitePool, id: &str, title: &str, content: &str) -> Result<()> {
    sqlx::query("UPDATE items SET title = ?1, content = ?2, updated_at = ?3 WHERE id = ?4")
        .bind(title)
        .bind(content)
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_source_apps(pool: &SqlitePool) -> Result<Vec<String>> {
    let rows = sqlx::query(
        "SELECT DISTINCT source_app FROM items
         WHERE source_app IS NOT NULL AND source_app <> '' AND deleted_at IS NULL
         ORDER BY source_app COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().filter_map(|r| r.try_get::<String, _>(0).ok()).collect())
}

/* -- folders --------------------------------------------------------------- */

const FOLDER_COLS: &str = "id, name, sort_order, created_at, updated_at";

/// Ordered by `sort_order` then name. The order must be stable across calls
/// because Alt+1..9 in the panel selects a folder by position.
pub async fn list_folders(pool: &SqlitePool) -> Result<Vec<Folder>> {
    let sql = format!("SELECT {FOLDER_COLS} FROM folders ORDER BY sort_order, name COLLATE NOCASE");
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Folder::from_row(row)?);
    }
    Ok(out)
}

/// Create-or-get. The UNIQUE COLLATE NOCASE index means typing the name of a
/// folder that already exists hands back that folder instead of erroring --
/// the user asked for a folder of that name, and now there is one.
pub async fn create_folder(pool: &SqlitePool, name: &str) -> Result<Folder> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("folder name cannot be empty");
    }

    let now = now_millis();
    sqlx::query(
        "INSERT INTO folders (id, name, sort_order, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4)
         ON CONFLICT(name) DO NOTHING",
    )
    .bind(uuid::Uuid::new_v4().to_string())
    .bind(name)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    let sql = format!("SELECT {FOLDER_COLS} FROM folders WHERE name = ?1 COLLATE NOCASE");
    let row = sqlx::query(&sql).bind(name).fetch_one(pool).await?;
    Ok(Folder::from_row(&row)?)
}

pub async fn rename_folder(pool: &SqlitePool, id: &str, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("folder name cannot be empty");
    }
    sqlx::query("UPDATE folders SET name = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(name)
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// The items survive: `folder_id` is ON DELETE SET NULL, and `foreign_keys` is
/// on for the pool, so they simply become unfiled.
pub async fn delete_folder(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM folders WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_item_folder(pool: &SqlitePool, id: &str, folder_id: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE items SET folder_id = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(folder_id)
        .bind(now_millis())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/* -- credentials and secrets ----------------------------------------------- */

/// Updates the searchable half of a credential. Separate from `update_note` so
/// that command can refuse credentials outright -- it would overwrite the JSON
/// body with markdown and orphan the secret row.
pub async fn update_credential(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    content: &str,
    folder_id: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE items SET title = ?1, content = ?2, folder_id = ?3, updated_at = ?4
         WHERE id = ?5",
    )
    .bind(title)
    .bind(content)
    .bind(folder_id)
    .bind(now_millis())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// `stored`/`enc` come from `secret::seal`; this function never sees plaintext
/// it has not been handed, and never interprets it.
pub async fn upsert_secret(pool: &SqlitePool, item_id: &str, stored: &str, enc: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO item_secrets (item_id, secret, enc, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(item_id) DO UPDATE SET
           secret = excluded.secret, enc = excluded.enc, updated_at = excluded.updated_at",
    )
    .bind(item_id)
    .bind(stored)
    .bind(enc)
    .bind(now_millis())
    .execute(pool)
    .await?;
    Ok(())
}

/// Returns `(stored, enc)` for `secret::open`. The only read path for a secret.
pub async fn read_secret(pool: &SqlitePool, item_id: &str) -> Result<Option<(String, String)>> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT secret, enc FROM item_secrets WHERE item_id = ?1")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    Ok(row)
}

/// Existence without disclosure -- what `CredentialView.has_password` is built
/// from, so the UI can show a filled field without the value crossing IPC.
pub async fn has_secret(pool: &SqlitePool, item_id: &str) -> Result<bool> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM item_secrets WHERE item_id = ?1")
        .bind(item_id)
        .fetch_one(pool)
        .await?;
    Ok(row.0 > 0)
}

pub async fn delete_secret(pool: &SqlitePool, item_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM item_secrets WHERE item_id = ?1")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Re-seals every secret still stored as plaintext, in place.
///
/// Without this, a credential saved before encryption landed stays readable in
/// `stash.db` until the user happens to edit it again -- which may be never.
/// Encryption that only applies to future writes is not encryption.
///
/// Idempotent: the second run matches no rows and returns 0. A row that fails
/// to seal is left exactly as it was and the pass continues, because losing one
/// password to a hard abort would be worse than the plaintext it replaced.
pub async fn upgrade_unsealed_secrets(pool: &SqlitePool) -> Result<(usize, usize)> {
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT item_id, secret FROM item_secrets WHERE enc = ?1")
            .bind(crate::secret::ENC_NONE)
            .fetch_all(pool)
            .await?;

    if rows.is_empty() {
        return Ok((0, 0));
    }

    let mut upgraded = 0usize;
    let mut failed = 0usize;

    for (item_id, plain) in rows {
        match crate::secret::seal(&plain) {
            Ok((stored, enc)) => {
                // Still `enc = 'none'` on a build that cannot seal; writing that
                // back would be a pointless rewrite of the same bytes.
                if crate::secret::is_unsealed(enc) {
                    continue;
                }
                match upsert_secret(pool, &item_id, &stored, enc).await {
                    Ok(()) => upgraded += 1,
                    Err(e) => {
                        tracing::warn!(item_id, error = %e, "could not store re-sealed secret");
                        failed += 1;
                    }
                }
            }
            Err(e) => {
                tracing::warn!(item_id, error = %e, "could not seal existing secret");
                failed += 1;
            }
        }
    }

    Ok((upgraded, failed))
}

/* -- export / import -------------------------------------------------------- */

/// Every live item, oldest first, with no limit.
///
/// Separate from `search` on purpose: that one clamps to 500 rows for the
/// panel, and a backup that silently stopped at 500 items would be worse than
/// no backup. Oldest first so an archive reads chronologically.
pub async fn all_items(pool: &SqlitePool) -> Result<Vec<Item>> {
    let sql = format!(
        "SELECT {ITEM_COLS} FROM items
          WHERE items.deleted_at IS NULL
          ORDER BY items.created_at ASC"
    );
    let rows = sqlx::query(&sql).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in &rows {
        out.push(Item::from_row(row)?);
    }
    Ok(out)
}

/// True when a live item already carries this hash. The import skip-check.
pub async fn hash_exists(pool: &SqlitePool, hash: &str) -> Result<bool> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM items WHERE hash = ?1 AND deleted_at IS NULL",
    )
    .bind(hash)
    .fetch_one(pool)
    .await?;
    Ok(row.0 > 0)
}

/// Inserts an item from an archive, preserving the timestamps and counters it
/// carried.
///
/// A fresh id is generated rather than reusing the archive's: ids mean nothing
/// across databases, and reusing one risks colliding with a row that already
/// exists here. `insert_item` cannot be reused because it stamps `created_at`
/// and `updated_at` with the import time, which would make every restored item
/// look like it was copied today.
#[allow(clippy::too_many_arguments)]
pub async fn insert_imported(
    pool: &SqlitePool,
    kind: &str,
    content_type: &str,
    content: Option<&str>,
    blob_path: Option<&str>,
    hash: Option<&str>,
    source_app: Option<&str>,
    title: Option<&str>,
    pinned: bool,
    created_at: i64,
    updated_at: i64,
    use_count: i64,
    last_used_at: Option<i64>,
    folder_id: Option<&str>,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO items
           (id, kind, content_type, content, blob_path, hash, source_app, title,
            pinned, created_at, updated_at, deleted_at, folder_id,
            use_count, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?13, ?14)",
    )
    .bind(&id)
    .bind(kind)
    .bind(content_type)
    .bind(content)
    .bind(blob_path)
    .bind(hash)
    .bind(source_app)
    .bind(title)
    .bind(if pinned { 1 } else { 0 })
    .bind(created_at)
    .bind(updated_at)
    .bind(folder_id)
    .bind(use_count)
    .bind(last_used_at)
    .execute(pool)
    .await?;

    Ok(id)
}

/// Whether an identical item is already stored.
///
/// The hash check covers clips, but notes and credentials are inserted with
/// `hash = NULL`, so re-importing an archive would duplicate every one of them.
/// `created_at` is preserved across an import, which makes the quadruple below
/// a workable identity for "this exact row, from this exact archive" without
/// inventing a new column.
///
/// Deliberately not a uniqueness constraint: two notes with the same text
/// written at the same millisecond are indistinguishable anyway, and a user is
/// still free to create a genuine duplicate by hand.
pub async fn duplicate_exists(
    pool: &SqlitePool,
    content_type: &str,
    title: Option<&str>,
    content: Option<&str>,
    created_at: i64,
) -> Result<bool> {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM items
          WHERE deleted_at IS NULL
            AND content_type = ?1
            AND created_at = ?2
            AND title IS ?3
            AND content IS ?4",
    )
    .bind(content_type)
    .bind(created_at)
    .bind(title)
    .bind(content)
    .fetch_one(pool)
    .await?;
    Ok(row.0 > 0)
}
