-- v0.8: push notifications + poller heartbeat.
--
-- `poller_heartbeat_at` is stamped by the poll loop on every tick. A run in
-- `running` whose heartbeat is older than `[notify].heartbeat_stale_min`
-- is what `xrun watchdog` treats as "poller dead, instance may still be
-- billing" -- the one failure the poller cannot report on itself.
--
-- `notify_log` is the delivery journal: one row per (notification, channel)
-- attempt. Doubles as the dedupe store -- `xrun watchdog` may run from a
-- scheduler *and* from the TUI's 60s resume tick, and the same
-- `poller.dead` must not fire twice within `[notify].dedupe_min`.

ALTER TABLE runs ADD COLUMN poller_heartbeat_at DATETIME;

CREATE TABLE notify_log (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    ts          DATETIME NOT NULL,
    run_id      TEXT,                         -- NULL for global (watchdog / test)
    kind        TEXT NOT NULL,                -- run.done | budget.warn | poller.dead | ...
    dedupe_key  TEXT NOT NULL,                -- kind + run/instance + threshold
    channel     TEXT NOT NULL,                -- ntfy | telegram | webhook | desktop
    ok          INTEGER NOT NULL,             -- 1 delivered, 0 failed
    title       TEXT NOT NULL,
    body        TEXT,
    error       TEXT
);
CREATE INDEX idx_notify_log_key ON notify_log(dedupe_key, ts);
CREATE INDEX idx_notify_log_run ON notify_log(run_id);
UPDATE schema_version SET version = 7;
