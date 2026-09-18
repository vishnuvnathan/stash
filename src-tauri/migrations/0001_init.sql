CREATE TABLE items (
  id           TEXT PRIMARY KEY,
  kind         TEXT NOT NULL CHECK (kind IN ('clip','note')),
  content_type TEXT NOT NULL,
  content      TEXT,
  blob_path    TEXT,
  hash         TEXT,
  source_app   TEXT,
  title        TEXT,
  pinned       INTEGER NOT NULL DEFAULT 0,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL,
  deleted_at   INTEGER
);

CREATE INDEX idx_items_recent ON items(deleted_at, pinned, updated_at DESC);
CREATE INDEX idx_items_hash   ON items(hash) WHERE deleted_at IS NULL;
CREATE INDEX idx_items_type   ON items(content_type);
CREATE INDEX idx_items_source ON items(source_app);

CREATE TABLE tags (
  id   TEXT PRIMARY KEY,
  name TEXT NOT NULL UNIQUE COLLATE NOCASE
);

CREATE TABLE item_tags (
  item_id TEXT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
  tag_id  TEXT NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
  PRIMARY KEY (item_id, tag_id)
);
