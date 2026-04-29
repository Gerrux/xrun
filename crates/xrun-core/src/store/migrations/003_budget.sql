ALTER TABLE instances ADD COLUMN max_lifetime_secs INTEGER;
ALTER TABLE instances ADD COLUMN max_cost_usd REAL;
ALTER TABLE instances ADD COLUMN idle_timeout_secs INTEGER;
ALTER TABLE instances ADD COLUMN accumulated_cost REAL NOT NULL DEFAULT 0;
ALTER TABLE instances ADD COLUMN last_active_at DATETIME;
ALTER TABLE instances ADD COLUMN auto_destroyed_reason TEXT;
UPDATE schema_version SET version = 3;
