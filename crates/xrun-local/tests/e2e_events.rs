//! End-to-end smoke test: a manifest whose `cmd` writes a JSON line into
//! `$XRUN_RUN_DIR/events.jsonl` is correctly tailed back through
//! `LocalAdapter::tail`. This is the same path the poller would walk.

#![deny(unsafe_code)]

use std::path::Path;
use std::time::{Duration, Instant};

use tempfile::TempDir;
use xrun_core::{manifest::Manifest, store::Store, vendor::VendorAdapter};
use xrun_local::LocalAdapter;

fn wait_until_nonempty(path: &Path, deadline: Duration) {
    let start = Instant::now();
    loop {
        if let Ok(meta) = std::fs::metadata(path) {
            if meta.len() > 0 {
                return;
            }
        }
        if start.elapsed() > deadline {
            panic!("file did not grow within {deadline:?}: {path:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn process_writes_events_file_via_xrun_run_dir_env() {
    let td = TempDir::new().unwrap();
    let runs_dir = td.path().join("runs");
    std::fs::create_dir_all(&runs_dir).unwrap();

    let mut store = Store::open(&td.path().join("runs.db")).expect("open");
    let run_id = store
        .create_run("e2e", "deadbeef", "manifest.yaml", "local", &[])
        .expect("create_run");

    // Both pwsh and bash understand single-quoted strings and `>>` for append.
    // Picking commands that work in either: write a fixed JSON line into
    // $XRUN_RUN_DIR/events.jsonl, plus an "ok" to stdout for the poller's
    // stdout-tailing path.
    //
    // PowerShell uses $env:XRUN_RUN_DIR; bash uses $XRUN_RUN_DIR. Pick the
    // right one based on the host platform.
    let cmd = if cfg!(windows) {
        r#"$line = '{"ts":"2026-05-01T00:00:00Z","stage":"train","status":"start"}'; Add-Content -Path "$env:XRUN_RUN_DIR/events.jsonl" -Value $line; Write-Output started"#
    } else {
        r#"echo '{"ts":"2026-05-01T00:00:00Z","stage":"train","status":"start"}' >> "$XRUN_RUN_DIR/events.jsonl"; echo started"#
    };

    let yaml = format!(
        r#"
name: e2e-events
vendor: local
run:
  cmd: |
    {cmd}
"#,
        cmd = cmd
    );
    let manifest: Manifest = Manifest::from_yaml_str(&yaml).expect("parse");

    let adapter = LocalAdapter::with_store_and_runs_dir(store, runs_dir.clone());
    adapter.set_run_id(&run_id);

    let handle = adapter.provision(&manifest).expect("provision");
    adapter.execute(&handle, &manifest.run).expect("execute");

    let run_dir = runs_dir.join(run_id.to_string());
    let events_path = run_dir.join("events.jsonl");
    wait_until_nonempty(&events_path, Duration::from_secs(5));

    // Stdout should also contain "started".
    let stdout_path = run_dir.join("stdout.log");
    wait_until_nonempty(&stdout_path, Duration::from_secs(5));

    // tail() over the absolute paths returns the content the poller would see.
    let events_bytes = adapter
        .tail(&handle, events_path.to_str().unwrap(), 0)
        .expect("tail events");
    let events_str = String::from_utf8_lossy(&events_bytes);
    assert!(
        events_str.contains(r#""stage":"train""#),
        "events.jsonl content: {events_str:?}"
    );
    assert!(
        events_str.contains(r#""status":"start""#),
        "events.jsonl content: {events_str:?}"
    );

    let stdout_bytes = adapter
        .tail(&handle, stdout_path.to_str().unwrap(), 0)
        .expect("tail stdout");
    let stdout_str = String::from_utf8_lossy(&stdout_bytes);
    assert!(
        stdout_str.contains("started"),
        "stdout.log content: {stdout_str:?}"
    );

    // Incremental tail picks up nothing past EOF.
    let after = adapter
        .tail(
            &handle,
            events_path.to_str().unwrap(),
            events_bytes.len() as u64,
        )
        .expect("tail past eof");
    assert!(
        after.is_empty(),
        "expected empty post-EOF tail, got {after:?}"
    );
}
