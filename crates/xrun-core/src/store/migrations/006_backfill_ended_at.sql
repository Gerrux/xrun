-- v0.7.1: `ended_at` was never populated by the poller (no setter existed),
-- so every done/failed/cancelled run carried `ended_at = NULL`. The TUI's
-- duration formula falls back to `now - started_at` when `ended_at` is null,
-- which made finished runs grow their displayed duration forever.
--
-- Going forward `update_run_status` stamps `ended_at` on terminal
-- transitions. Here we backfill historical rows from the latest event
-- timestamp, which is the closest signal we have to "when did this finish".

UPDATE runs
SET ended_at = (
    SELECT MAX(ts) FROM events WHERE events.run_id = runs.id
)
WHERE ended_at IS NULL
  AND status IN ('done', 'failed', 'cancelled')
  AND started_at IS NOT NULL
  AND EXISTS (SELECT 1 FROM events WHERE events.run_id = runs.id);

UPDATE schema_version SET version = 6;
