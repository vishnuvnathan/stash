-- Passwords live here and nowhere else.
--
-- The point of a separate table is the FTS index. The triggers in 0002_fts.sql
-- copy items.title and items.content into items_fts unconditionally, so
-- anything stored in those columns becomes searchable text. Nothing touches
-- this table, and no query in queries.rs joins it except the secret helpers --
-- leaking a password would take a JOIN that exists nowhere in the codebase.
--
-- `enc` is the forward-compatibility discriminator read by secret.rs: 'none'
-- today, 'dpapi.v1' once encryption lands. Swapping it is a read/write path
-- change with no schema migration and no data movement.
CREATE TABLE item_secrets (
  item_id    TEXT PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
  secret     TEXT NOT NULL,
  enc        TEXT NOT NULL DEFAULT 'none',
  updated_at INTEGER NOT NULL
);
