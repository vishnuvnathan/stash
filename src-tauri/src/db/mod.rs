pub mod model;
pub mod queries;
pub mod retention;

use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

/// Numbered migrations, embedded at compile time so a packaged build carries no
/// loose SQL files. Order in this slice is the apply order; never reorder or
/// edit an entry that has already shipped -- add a new file instead.
const MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("../../migrations/0001_init.sql")),
    ("0002_fts", include_str!("../../migrations/0002_fts.sql")),
    ("0003_folders", include_str!("../../migrations/0003_folders.sql")),
    ("0004_secrets", include_str!("../../migrations/0004_secrets.sql")),
    ("0005_power", include_str!("../../migrations/0005_power.sql")),
];

/// Shared handle stored in Tauri state. Holds the one pool the whole process
/// uses -- the clipboard watcher, the retention task and every command go
/// through this, so there is a single writer path and a single migration path.
#[derive(Clone)]
pub struct Db {
    pub pool: SqlitePool,
    /// Directory image blobs are written to (`<app_data>/blobs`).
    pub blob_dir: PathBuf,
}

pub async fn init(app_data_dir: &Path) -> Result<Db> {
    std::fs::create_dir_all(app_data_dir)
        .with_context(|| format!("creating app data dir {}", app_data_dir.display()))?;

    let blob_dir = app_data_dir.join("blobs");
    std::fs::create_dir_all(&blob_dir).context("creating blob dir")?;

    let db_path = app_data_dir.join("stash.db");
    // Built from a path rather than a URL: a Windows path carries a drive
    // letter and backslashes, which do not survive URL parsing.
    let opts = SqliteConnectOptions::new()
        .filename(&db_path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(4)
        .connect_with(opts)
        .await
        .context("opening sqlite pool")?;

    run_migrations(&pool).await?;

    Ok(Db { pool, blob_dir })
}

/// Applies any migration whose name is not yet in `_migrations`. Each file runs
/// inside its own transaction, so a failure part-way leaves the database on the
/// last fully applied version rather than half-migrated.
async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (
           name       TEXT PRIMARY KEY,
           applied_at INTEGER NOT NULL
         )",
    )
    .execute(pool)
    .await
    .context("creating _migrations table")?;

    for (name, sql) in MIGRATIONS {
        let already: Option<(String,)> =
            sqlx::query_as("SELECT name FROM _migrations WHERE name = ?1")
                .bind(name)
                .fetch_optional(pool)
                .await?;
        if already.is_some() {
            continue;
        }

        let mut tx = pool.begin().await?;
        // Migration files hold several statements; sqlx executes a multi-statement
        // string only through the raw executor, so feed it the whole file.
        sqlx::raw_sql(sql)
            .execute(&mut *tx)
            .await
            .with_context(|| format!("applying migration {name}"))?;
        sqlx::query("INSERT INTO _migrations (name, applied_at) VALUES (?1, ?2)")
            .bind(name)
            .bind(model::now_millis())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;

        tracing::info!(migration = name, "applied migration");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::model::{ContentType, ItemKind, NewItem};
    use super::*;

    /// Each test gets its own database directory, so they can run in parallel
    /// and a failure leaves its evidence behind rather than poisoning the next.
    async fn fresh_db() -> (Db, PathBuf) {
        let dir = std::env::temp_dir().join(format!("stash-test-{}", uuid::Uuid::new_v4()));
        let db = init(&dir).await.expect("init");
        (db, dir)
    }

    fn clip(title: &str, content: &str) -> NewItem {
        NewItem {
            kind: ItemKind::Clip,
            content_type: ContentType::Text,
            content: Some(content.to_string()),
            blob_path: None,
            hash: None,
            source_app: None,
            title: Some(title.to_string()),
            folder_id: None,
        }
    }

    async fn search_titles(db: &Db, query: &str, filters: &model::Filters) -> Vec<String> {
        queries::search(&db.pool, query, filters, 100, 0)
            .await
            .expect("search")
            .into_iter()
            .filter_map(|i| i.title)
            .collect()
    }

    #[tokio::test]
    async fn every_migration_applies_to_a_fresh_database() {
        let (db, _dir) = fresh_db().await;
        let names: Vec<(String,)> = sqlx::query_as("SELECT name FROM _migrations ORDER BY name")
            .fetch_all(&db.pool)
            .await
            .expect("read _migrations");
        let names: Vec<String> = names.into_iter().map(|(n,)| n).collect();
        assert_eq!(
            names,
            ["0001_init", "0002_fts", "0003_folders", "0004_secrets", "0005_power"]
        );
    }

    /// The load-bearing one. `0003` runs ALTER TABLE ADD COLUMN against `items`,
    /// which is the external-content table behind the FTS5 index. If FTS5 bound
    /// its columns by position rather than by name, this search would fail.
    #[tokio::test]
    async fn fts_still_works_after_the_folder_column_is_added() {
        let (db, _dir) = fresh_db().await;
        queries::insert_item(&db.pool, clip("Cutover plan", "transport request 4471"), 1)
            .await
            .expect("insert");

        let hits = search_titles(&db, "transport", &model::Filters::default()).await;
        assert_eq!(hits, ["Cutover plan"], "FTS index broken by the new column");
    }

    /// A folder chip must narrow results identically whether or not the user is
    /// typing -- the recent-first path and the FTS path both run `push_filters`.
    #[tokio::test]
    async fn folder_filter_applies_on_both_search_paths() {
        let (db, _dir) = fresh_db().await;
        let folder = queries::create_folder(&db.pool, "SAP").await.expect("folder");

        let filed = queries::insert_item(&db.pool, clip("SAP login steps", "transport"), 1)
            .await
            .expect("insert");
        queries::insert_item(&db.pool, clip("Unrelated note", "transport"), 2)
            .await
            .expect("insert");
        queries::set_item_folder(&db.pool, &filed.id, Some(&folder.id))
            .await
            .expect("file it");

        let only_sap = model::Filters {
            folder_ids: vec![folder.id.clone()],
            ..Default::default()
        };

        // Empty query -> recent-first path.
        assert_eq!(search_titles(&db, "", &only_sap).await, ["SAP login steps"]);
        // Non-empty query -> FTS/bm25 path. Both rows match "transport".
        assert_eq!(search_titles(&db, "transport", &only_sap).await, ["SAP login steps"]);
        // Without the filter the FTS path still returns both.
        assert_eq!(
            search_titles(&db, "transport", &model::Filters::default()).await.len(),
            2
        );
    }

    /// Deleting a folder must unfile its items, never delete them.
    #[tokio::test]
    async fn deleting_a_folder_unfiles_its_items() {
        let (db, _dir) = fresh_db().await;
        let folder = queries::create_folder(&db.pool, "Temp").await.expect("folder");
        let item = queries::insert_item(&db.pool, clip("Keep me", "body"), 1)
            .await
            .expect("insert");
        queries::set_item_folder(&db.pool, &item.id, Some(&folder.id))
            .await
            .expect("file it");

        queries::delete_folder(&db.pool, &folder.id).await.expect("delete folder");

        let reloaded = queries::get(&db.pool, &item.id).await.expect("get").expect("still there");
        assert_eq!(reloaded.folder_id, None, "item should be unfiled, not destroyed");
    }

    /// Create-or-get: typing the name of an existing folder returns that folder.
    #[tokio::test]
    async fn creating_an_existing_folder_name_is_idempotent() {
        let (db, _dir) = fresh_db().await;
        let a = queries::create_folder(&db.pool, "SAP").await.expect("first");
        let b = queries::create_folder(&db.pool, "  sap  ").await.expect("second");
        assert_eq!(a.id, b.id, "case-insensitive name should resolve to one folder");
        assert_eq!(queries::list_folders(&db.pool).await.expect("list").len(), 1);
    }

    /// The core security property: a stored password must not be reachable
    /// through search, because it never enters `items` and therefore never
    /// enters the FTS index the triggers build from it.
    #[tokio::test]
    async fn a_stored_password_is_not_searchable() {
        let (db, _dir) = fresh_db().await;

        let cred = queries::insert_item(
            &db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some(r#"{"username":"vnathan"}"#.to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("SAP Production".to_string()),
                folder_id: None,
            },
            1,
        )
        .await
        .expect("insert");

        queries::upsert_secret(&db.pool, &cred.id, "Hunter2-VerifyMe", "none")
            .await
            .expect("store secret");

        let no_filters = model::Filters::default();
        assert!(
            search_titles(&db, "Hunter2", &no_filters).await.is_empty(),
            "the password leaked into the FTS index"
        );
        // ...while the username, which is meant to be findable, still is.
        assert_eq!(search_titles(&db, "vnathan", &no_filters).await, ["SAP Production"]);
    }

    /// `folder:work` resolves through a subquery rather than a prior lookup, so
    /// the only way to know it works is to run it against real SQL.
    #[tokio::test]
    async fn folder_names_resolve_and_ignore_case() {
        let (db, _dir) = fresh_db().await;
        let folder = queries::create_folder(&db.pool, "Work").await.expect("folder");

        let filed = queries::insert_item(&db.pool, clip("filed", "alpha"), 1).await.expect("a");
        queries::set_item_folder(&db.pool, &filed.id, Some(&folder.id)).await.expect("file it");
        queries::insert_item(&db.pool, clip("loose", "alpha"), 2).await.expect("b");

        // Typed as "work", stored as "Work".
        let by_name = model::Filters { folder_names: vec!["work".into()], ..Default::default() };
        assert_eq!(search_titles(&db, "", &by_name).await, ["filed"]);

        // And the id path still works, unchanged.
        let by_id = model::Filters { folder_ids: vec![folder.id.clone()], ..Default::default() };
        assert_eq!(search_titles(&db, "", &by_id).await, ["filed"]);

        // A name nobody has matches nothing rather than everything -- the
        // failure mode that would make the operator look like it does nothing.
        let missing = model::Filters { folder_names: vec!["nope".into()], ..Default::default() };
        assert!(search_titles(&db, "", &missing).await.is_empty());
    }

    /// `sort:used` has to beat recency, or the whole point of counting is lost.
    #[tokio::test]
    async fn sort_used_outranks_recency() {
        let (db, _dir) = fresh_db().await;

        let old = queries::insert_item(&db.pool, clip("reached-for", "alpha"), 1).await.expect("a");
        queries::insert_item(&db.pool, clip("newer", "alpha"), 999).await.expect("b");

        // Default order is recency, so the newer one leads.
        let none = model::Filters::default();
        assert_eq!(search_titles(&db, "", &none).await, ["newer", "reached-for"]);

        for _ in 0..3 {
            queries::bump_usage(&db.pool, &old.id).await.expect("bump");
        }

        let used = model::Filters { most_used: true, ..Default::default() };
        assert_eq!(search_titles(&db, "", &used).await, ["reached-for", "newer"]);

        // Using an item must not reorder the recent-first list underneath it.
        assert_eq!(search_titles(&db, "", &none).await, ["newer", "reached-for"]);
    }

    /// The whole point of export/import: everything survives a trip through a
    /// file into a *different* database. Run against real migrations, with a
    /// real blob on disk and a real sealed secret.
    #[tokio::test]
    async fn an_archive_round_trips_into_another_database() {
        let (source, _d1) = fresh_db().await;

        let folder = queries::create_folder(&source.pool, "Work").await.expect("folder");

        let note = queries::insert_item(
            &source.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Markdown,
                content: Some("# heading\nbody".to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("A note".to_string()),
                folder_id: Some(folder.id.clone()),
            },
            1_700_000_000_000,
        )
        .await
        .expect("note");
        queries::set_pinned(&source.pool, &note.id, true).await.expect("pin");
        queries::bump_usage(&source.pool, &note.id).await.expect("use");

        let cred = queries::insert_item(
            &source.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some(r#"{"username":"vnathan"}"#.to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("SAP".to_string()),
                folder_id: None,
            },
            1_700_000_000_001,
        )
        .await
        .expect("cred");
        let (stored, enc) = crate::secret::seal("Hunter2-VerifyMe").expect("seal");
        queries::upsert_secret(&source.pool, &cred.id, &stored, enc).await.expect("secret");

        // A real image blob, so the base64 inlining is exercised rather than
        // stubbed. Not a valid PNG, but nothing in this path decodes it.
        let png = b"fake-png-bytes-for-the-blob-round-trip";
        let hash = crate::clipboard::hash_bytes(png);
        std::fs::create_dir_all(source.blob_dir.join(&hash[..2])).expect("shard");
        std::fs::write(source.blob_dir.join(format!("{}/{}.png", &hash[..2], hash)), png)
            .expect("blob");
        queries::insert_item(
            &source.pool,
            NewItem {
                kind: ItemKind::Clip,
                content_type: ContentType::Image,
                content: None,
                blob_path: Some(format!("{}/{}.png", &hash[..2], hash)),
                hash: Some(hash.clone()),
                source_app: Some("Snip".to_string()),
                title: Some("Image (1 KB)".to_string()),
                folder_id: None,
            },
            1_700_000_000_002,
        )
        .await
        .expect("image");

        let archive = crate::archive::io::build(&source, true).await.expect("build");
        assert_eq!(archive.items.len(), 3);
        assert!(archive.includes_secrets);

        // Through a real file, as JSON, exactly as the command does it.
        let json = archive.to_json().expect("json");
        let parsed = crate::archive::Archive::from_json(&json).expect("parse");

        let (dest, _d2) = fresh_db().await;
        let report = crate::archive::io::restore(&dest, &parsed).await.expect("restore");
        assert_eq!(report.imported, 3, "not everything landed");
        assert_eq!(report.failed, 0);
        assert_eq!(report.secrets_restored, 1);

        // The note kept its folder, its pin and its use count.
        let filed = model::Filters { folder_names: vec!["Work".into()], ..Default::default() };
        assert_eq!(search_titles(&dest, "", &filed).await, ["A note"]);

        let restored: Vec<_> = queries::all_items(&dest.pool).await.expect("all");
        let note_row = restored.iter().find(|i| i.title.as_deref() == Some("A note")).unwrap();
        assert!(note_row.pinned);
        assert_eq!(note_row.use_count, 1);
        assert_eq!(note_row.created_at, 1_700_000_000_000, "timestamps were restamped");

        // The password comes back out, re-sealed with this build's key.
        let cred_row = restored.iter().find(|i| i.title.as_deref() == Some("SAP")).unwrap();
        let (s, e) = queries::read_secret(&dest.pool, &cred_row.id).await.unwrap().unwrap();
        assert_eq!(crate::secret::open(&s, &e).unwrap(), "Hunter2-VerifyMe");

        // The picture was written into the new blob store, not just referenced.
        let image_row = restored.iter().find(|i| i.blob_path.is_some()).unwrap();
        let blob = dest.blob_dir.join(image_row.blob_path.as_ref().unwrap());
        assert!(blob.exists(), "image blob missing after import");
        assert_eq!(std::fs::read(&blob).unwrap(), png);

        // Importing the same archive again must be a no-op. Only the clip has a
        // hash; the note and the credential are recognised by exact content, and
        // without that half they would silently duplicate.
        let again = crate::archive::io::restore(&dest, &parsed).await.expect("second restore");
        assert_eq!(again.imported, 0, "re-import duplicated rows");
        assert_eq!(again.skipped, 3, "all three should have been recognised");
        assert_eq!(again.folders_created, 0);
        assert_eq!(queries::all_items(&dest.pool).await.unwrap().len(), 3);
    }

    /// An export without secrets must not carry one, and must say so on the way
    /// back in rather than looking like the passwords were lost.
    #[tokio::test]
    async fn an_export_without_secrets_carries_none() {
        let (db, _dir) = fresh_db().await;
        let cred = queries::insert_item(
            &db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some(r#"{"username":"vnathan"}"#.to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("SAP".to_string()),
                folder_id: None,
            },
            1,
        )
        .await
        .expect("cred");
        let (stored, enc) = crate::secret::seal("Hunter2-VerifyMe").expect("seal");
        queries::upsert_secret(&db.pool, &cred.id, &stored, enc).await.expect("secret");

        let archive = crate::archive::io::build(&db, false).await.expect("build");
        let json = archive.to_json().expect("json");
        assert!(!json.contains("Hunter2"), "password leaked into a no-secrets export");

        let (dest, _d2) = fresh_db().await;
        let parsed = crate::archive::Archive::from_json(&json).expect("parse");
        let report = crate::archive::io::restore(&dest, &parsed).await.expect("restore");
        assert_eq!(report.imported, 1);
        assert_eq!(report.secrets_restored, 0);
        assert!(report.secrets_absent, "the report should explain why there are none");
    }

    /// The upgrade pass is the only code that rewrites secrets the user already
    /// has, so it gets tested against real migrations rather than trusted.
    #[cfg(windows)]
    #[tokio::test]
    async fn plaintext_secrets_are_re_sealed_in_place() {
        let (db, _dir) = fresh_db().await;

        let cred = queries::insert_item(
            &db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some(r#"{"username":"vnathan"}"#.to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("Legacy row".to_string()),
                folder_id: None,
            },
            1,
        )
        .await
        .expect("insert");

        // Exactly what a pre-encryption build left behind.
        queries::upsert_secret(&db.pool, &cred.id, "Hunter2-VerifyMe", crate::secret::ENC_NONE)
            .await
            .expect("store plaintext secret");

        let (upgraded, failed) = queries::upgrade_unsealed_secrets(&db.pool)
            .await
            .expect("upgrade pass");
        assert_eq!((upgraded, failed), (1, 0));

        let (stored, enc) = queries::read_secret(&db.pool, &cred.id)
            .await
            .expect("read")
            .expect("row still exists");
        assert_eq!(enc, crate::secret::ENC_DPAPI_V1);
        assert!(!stored.contains("Hunter2"), "plaintext survived the upgrade");

        // The point of the exercise: the password still comes back out.
        assert_eq!(
            crate::secret::open(&stored, &enc).expect("decrypt"),
            "Hunter2-VerifyMe"
        );

        // Idempotent -- startup runs this every time.
        assert_eq!(
            queries::upgrade_unsealed_secrets(&db.pool).await.expect("second pass"),
            (0, 0)
        );
    }

    /// Credentials are stored as notes precisely so retention cannot reap them.
    /// This drives the real `retention::run_once` past its clip limit.
    #[tokio::test]
    async fn retention_never_prunes_a_credential() {
        let (db, _dir) = fresh_db().await;

        let cred = queries::insert_item(
            &db.pool,
            NewItem {
                kind: ItemKind::Note,
                content_type: ContentType::Credential,
                content: Some("{}".to_string()),
                blob_path: None,
                hash: None,
                source_app: None,
                title: Some("SAP Production".to_string()),
                folder_id: None,
            },
            1,
        )
        .await
        .expect("insert credential");

        queries::upsert_secret(&db.pool, &cred.id, "Hunter2-VerifyMe", "none")
            .await
            .expect("store secret");

        // Comfortably past the 1000-clip retention limit.
        for i in 0..1100 {
            queries::insert_item(&db.pool, clip(&format!("clip {i}"), "body"), 10 + i as i64)
                .await
                .expect("insert clip");
        }

        retention::run_once(&db).await.expect("retention pass");

        assert!(
            queries::get(&db.pool, &cred.id).await.expect("get").is_some(),
            "retention deleted a credential"
        );
        assert!(
            queries::read_secret(&db.pool, &cred.id).await.expect("read").is_some(),
            "the credential's secret was orphaned or removed"
        );
    }

    /// `ON DELETE CASCADE` plus `foreign_keys(true)` means a hard-deleted item
    /// takes its secret with it -- no orphan rows left holding a password.
    #[tokio::test]
    async fn hard_deleting_an_item_removes_its_secret() {
        let (db, _dir) = fresh_db().await;
        let item = queries::insert_item(&db.pool, clip("Doomed", "body"), 1)
            .await
            .expect("insert");
        queries::upsert_secret(&db.pool, &item.id, "secret", "none")
            .await
            .expect("store");

        sqlx::query("DELETE FROM items WHERE id = ?1")
            .bind(&item.id)
            .execute(&db.pool)
            .await
            .expect("hard delete");

        assert_eq!(
            queries::read_secret(&db.pool, &item.id).await.expect("read"),
            None,
            "secret outlived the item it belonged to"
        );
    }
}
