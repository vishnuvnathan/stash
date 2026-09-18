CREATE TABLE folders (
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE COLLATE NOCASE,
  sort_order INTEGER NOT NULL DEFAULT 0,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

-- SQLite allows ADD COLUMN with a REFERENCES clause only when the default is
-- NULL, which is exactly what is wanted here.
--
-- SET NULL rather than CASCADE is deliberate: deleting a folder must unfile its
-- items, never destroy them. Losing a credential because you tidied up a folder
-- would be unforgivable.
ALTER TABLE items ADD COLUMN folder_id TEXT REFERENCES folders(id) ON DELETE SET NULL;

CREATE INDEX idx_items_folder ON items(folder_id) WHERE deleted_at IS NULL;
