# xrun — open follow-ups (post-v0.3.0)

Pruned 2026-04-29. Pre-v0.3.0 sections (A-E, Open #-1..#6) were resolved by
the v0.2/v0.3 lifecycle/manifest/upload PRs and removed. Items below are the
ones still hitting us in real runs after the v0.3.0 release.

## ~~BLOCKER — `last_active_at` is never written → idle guard always anchors on `created_at`~~ FIXED

Fixed 2026-04-29 in `loop_runner.rs`:
- `progress_this_tick = true` now set on every heartbeat tick and on every
  stdout byte block — both reset `last_active_at` via `update_instance_usage`.
- Idle-kill event message now includes `threshold=Nmin, last_active=HH:MMZ`
  (or `anchor=created_at HH:MMZ` when no activity was ever observed).

Remaining open sub-item (lower priority):
2. **Anchor on `train_start` event timestamp**, not `created_at`, when no
   metric activity has been observed yet. A freshly-provisioned run that
   hasn't started training should be in a different idle regime from a run
   whose script has been silent for 30 min.

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

## `data[*].exclude` pattern semantics need a doc note

`exclude` patterns are matched against the *relative path under `src`*, with
no implicit prefix wildcard. Hit on 2026-04-29: used `cache_*/` expecting it
to match `_cache_zmax_exp/`, `_cache_model_cmp/`, etc. — it didn't (leading
underscore was missing). Result: ~6 GB of cache directories went up that
shouldn't have.

The behaviour is technically correct (`tar --exclude` glob semantics), but
the manifest schema doc should call this out explicitly:

```yaml
exclude:
  - "**/__pycache__"   # match any depth
  - "*.pyc"            # files in any directory
  - "_cache_*"         # MUST include the leading char if present
  - "output/**"        # subtree under src
```

Bonus ask: `xrun doctor --manifest <path> --dry-run-upload` mode that prints
the *list of files that would be uploaded* (or top 20 by size). Lets the
user verify the pattern before paying for the bytes. Today the only
verification is "launch and see how big /workspace/code is on the box".

## Heartbeat / pigz / cost accounting (lower priority)

- **Auto-install pigz in setup** when `compress: gzip` is selected on a
  remote that doesn't have it. Today the upload silently falls back to
  single-threaded `gzip -d` which is ~10% of one core. Adding `apt-get
  install -y pigz` to the env-prep phase is cheap and fixes the 14-min
  extract observed on `01KQCT7FP8FS68478Y5K4160VE`.
- **Overlap upload+extract**: today `upload: ok` only fires after extract
  completes. Stream pipe should let them run concurrently.
- **Cost / time accounting**: with heartbeat metrics now in DB, dashboard
  could show `idle_time_$ / total_$` (provision+upload spend vs train
  spend). On 2026-04-29 a 30-min training cycle had 8% overhead from
  provision+upload time at $0.15/hr — worth surfacing.
