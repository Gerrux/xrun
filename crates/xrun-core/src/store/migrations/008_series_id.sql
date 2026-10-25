-- Schema v8: manifest `series_id` (one campaign = many runs). Copied from the
-- manifest at launch so a run keeps its series after the source file or the
-- worktree it lived in is gone. Older runs stay NULL: no backfill.

ALTER TABLE runs ADD COLUMN series_id TEXT;
UPDATE schema_version SET version = 8;
