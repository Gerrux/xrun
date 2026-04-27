ALTER TABLE runs ADD COLUMN cost_usd_estimate REAL;
UPDATE schema_version SET version = 2;
