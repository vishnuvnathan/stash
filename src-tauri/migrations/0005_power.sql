-- Usage-aware ranking.
--
-- How often an item has been used, as opposed to when it was last copied *into*
-- Stash. `updated_at` moves every time the watcher sees the same content again,
-- so it cannot answer "what do I reach for most" -- re-copying something once a
-- day and using a stored snippet forty times look identical through it.
--
-- Both columns default so that existing rows rank exactly as they did before:
-- zero uses, never used.
ALTER TABLE items ADD COLUMN use_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE items ADD COLUMN last_used_at INTEGER;

CREATE INDEX idx_items_used ON items(use_count DESC, last_used_at DESC)
  WHERE deleted_at IS NULL;
