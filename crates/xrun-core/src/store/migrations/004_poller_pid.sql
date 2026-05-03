ALTER TABLE runs ADD COLUMN poller_pid INTEGER;
UPDATE schema_version SET version = 4;
