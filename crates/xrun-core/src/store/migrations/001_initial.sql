CREATE TABLE runs (
    id              TEXT PRIMARY KEY,
    name            TEXT NOT NULL,
    manifest_hash   TEXT NOT NULL,
    manifest_path   TEXT NOT NULL,
    vendor          TEXT NOT NULL,
    instance_id     TEXT,
    status          TEXT NOT NULL,
    created_at      DATETIME NOT NULL,
    started_at      DATETIME,
    ended_at        DATETIME,
    cost_usd        REAL,
    mlflow_run_id   TEXT,
    notes           TEXT
);
CREATE INDEX idx_runs_status ON runs(status);
CREATE INDEX idx_runs_name ON runs(name);
CREATE INDEX idx_runs_manifest_hash ON runs(manifest_hash);

CREATE TABLE events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    ts              DATETIME NOT NULL,
    stage           TEXT NOT NULL,
    status          TEXT NOT NULL,
    msg             TEXT,
    payload_json    TEXT
);
CREATE INDEX idx_events_run ON events(run_id, ts);

CREATE TABLE metrics (
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    step            INTEGER NOT NULL,
    key             TEXT NOT NULL,
    value           REAL NOT NULL,
    ts              DATETIME NOT NULL,
    PRIMARY KEY (run_id, key, step)
);

CREATE TABLE artifacts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    remote_path     TEXT NOT NULL,
    local_path      TEXT,
    size_bytes      INTEGER,
    sha256          TEXT,
    pulled_at       DATETIME,
    is_best         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_artifacts_run ON artifacts(run_id);

CREATE TABLE poll_offsets (
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    file            TEXT NOT NULL,
    offset_bytes    INTEGER NOT NULL DEFAULT 0,
    last_polled_at  DATETIME,
    PRIMARY KEY (run_id, file)
);

CREATE TABLE instances (
    id              TEXT PRIMARY KEY,
    vendor          TEXT NOT NULL,
    run_id          TEXT REFERENCES runs(id),
    gpu_type        TEXT,
    price_per_hour  REAL,
    created_at      DATETIME,
    destroyed_at    DATETIME,
    state_json      TEXT
);

CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version VALUES (1);
