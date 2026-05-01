# xrun — open follow-ups (post-v0.3.0)

Pruned 2026-04-29. Pre-v0.3.0 sections (A-E, Open #-1..#6) were resolved by
the v0.2/v0.3 lifecycle/manifest/upload PRs and removed. Items below are the
ones still hitting us in real runs after the v0.3.0 release.

## Poller: short runs lose metrics — `done:ok` returns before metric tail

Surfaced 2026-05-01 by `xrun-local` smoke test (`exp/local_smoke.yaml`).
Run finishes within a single poll tick (~200ms). The events tail picks up
all five events including `done:ok`, the poller returns `RunStatus::Done`
*immediately*, and the metrics tail (next branch in the loop) never runs.
metrics.jsonl on disk has 6 entries; DB has 0.

For vast/kaggle the symptom is rare because training takes longer than one
tick. For local-fast jobs (and CI smoke tests) it bites every time.

Fix candidates:
- After `done` is observed, do one more pass over `metrics_file` (and
  `stdout_file` for tail) before returning. Keeps the loop simple, fixes
  the symptom.
- Or: ingest events and metrics from the same `tail` window into a single
  transaction.

`crates/xrun-poller/src/loop_runner.rs:310-321` is where the early return
happens. Same logic exists for `failed`.

## ~~BLOCKER — `last_active_at` is never written → idle guard always anchors on `created_at`~~ FIXED

Fixed 2026-04-29 in `loop_runner.rs`:
- `progress_this_tick = true` now set on every heartbeat tick and on every
  stdout byte block — both reset `last_active_at` via `update_instance_usage`.
- Idle-kill event message now includes `threshold=Nmin, last_active=HH:MMZ`
  (or `anchor=created_at HH:MMZ` when no activity was ever observed).

Remaining open sub-item:
2. ~~**Anchor on `train_start` event timestamp**, not `created_at`, when no
   metric activity has been observed yet.~~ FIXED 2026-05-01 in `budget::idle_anchor`:
   priority is `last_active_at → train_started_at → created_at`. Poller
   caches the ts and rehydrates from the events table on restart.

## Failure-cause misclassification: process death → "idle_timeout" (partial fix)

On `01KQCT7FP8FS68478Y5K4160VE`, host OOM killer SIGKILL'd the python child
of `bash launch_*.sh`. The actual failure is **stage failure** but the poller
saw bash+sshd still alive and fell through to the idle timer.

Fixed 2026-04-29:
- When `on_stage_failed: stop_instance` fires, the poller now emits an
  explicit `instance.auto_destroyed` event with
  `auto-destroyed: stage_failed (on_stage_failed=stop_instance)` before
  destroying.
- `policy.on_idle_minutes` from manifest already applied to `idle_timeout_secs`
  caps (confirmed in `launch.rs`); was not a real bug.

Still open:
- **Poll the actual launched PID** (captured by `nohup … & echo $!` shim)
  and emit `train_failed` / `stage_failed` as soon as that PID is gone. This
  is the root cause: poller cannot detect child-process death when bash is
  still alive. Requires storing the PID in a remote file and periodically
  running `kill -0 $PID` over SSH.

## ~~TUI logs view stays blank in non-running statuses~~ FIXED

Fixed 2026-04-29 in `run_detail.py`: `_load_logs` now only calls `log.clear()`
when `services.get_logs()` returns non-empty content. An empty result (SSH
gone after instance destroyed) preserves the last displayed snapshot instead
of wiping the panel.

## ~~`data[*].exclude` pattern semantics need a doc note~~ FIXED

FIXED 2026-05-01 in `docs/MANIFEST.md` — expanded the existing exclude
section with `tar --exclude` framing, common-mistake mappings, and a
local-verify recipe. The 6 GB incident is documented as the cautionary
tale.

Still open as a bonus ask (separate from the doc fix):
- `xrun doctor --manifest <path> --dry-run-upload` mode that prints the
  *list of files that would be uploaded* (or top 20 by size). Lets the
  user verify the pattern before paying for the bytes.

## Heartbeat / pigz / cost accounting (lower priority)

- ~~**Auto-install pigz in setup** when `compress: gzip` is selected on a
  remote that doesn't have it.~~ FIXED 2026-05-01 in `xrun-vast/upload.rs`:
  before the gzip tar pipe runs we issue
  `command -v pigz || apt-get install -y pigz` once over SSH. Idempotent;
  no-op on apt-less base images.
- **Overlap upload+extract**: today `upload: ok` only fires after extract
  completes. Stream pipe should let them run concurrently.
- **Cost / time accounting**: with heartbeat metrics now in DB, dashboard
  could show `idle_time_$ / total_$` (provision+upload spend vs train
  spend). On 2026-04-29 a 30-min training cycle had 8% overhead from
  provision+upload time at $0.15/hr — worth surfacing.
