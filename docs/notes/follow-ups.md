# xrun — open follow-ups (post-v0.3.0)

Pruned 2026-04-29. Pre-v0.3.0 sections (A-E, Open #-1..#6) were resolved by
the v0.2/v0.3 lifecycle/manifest/upload PRs and removed. Items below are the
ones still hitting us in real runs after the v0.3.0 release.

## ~~Poller: short runs lose metrics — `done:ok` returns before metric tail~~ FIXED

Fixed 2026-05-01 in commit 79e8adb (`fix(poller): drain metrics + stdout
before terminal-status return`). The events-tail block now latches a
`terminal_after_drain: Option<RunStatus>` instead of returning; the
metrics + stdout tails always run, and the loop finalises (destroy +
update_run_status) only after drain. Same wiring covers the `failed`
branch. Regression test in `crates/xrun-poller/tests/loop_progress.rs`
asserts that a tick which sees `done:ok` and 3 metrics in the same window
lands all 3 in the store.

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
- ~~**Poll the actual launched PID**~~ FIXED 2026-05-01: vast's
  `build_launch_command` now writes `$!` to `/workspace/run/run.pid`. The
  poller calls a new `VendorAdapter::process_alive(handle)` method each tick
  and, once `train_start` has fired, marks the run failed when the PID is
  gone but no `done:ok` was emitted. Synthetic `stage_failed:fail` event
  records the cause. Same wiring on `xrun-ssh`. Regression tests in
  `crates/xrun-poller/tests/loop_progress.rs::test_poller_pid_dead_*`.

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
