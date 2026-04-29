# xrun — open follow-ups

## Missing commands / features that blocked or slowed today's session

These came up trying to launch a single training run end-to-end. Listed in rough priority
order — top-of-list things were active blockers that forced us to fall back on raw `vastai`
or manual SSH.

### A. Lifecycle commands

1. **`xrun stop <id>`** — currently prints `stop not implemented yet for v0.1 (adapter pending)`.
   When a run is failing in a tight retry loop or the instance is alive but the run has
   gotten wedged, the only way out is `vastai destroy instance <id>` directly. Should:
   - Destroy the underlying instance (if any).
   - Mark the run `cancelled` in the DB.
   - Kill the local poller daemon (`poller.pid` is left behind today).
   `xrun stop <id> --force` should never prompt.

2. **`xrun stop --all`** — destroy every active run + its instance + its poller. Today we
   accumulated 4 orphan vast instances across failed launches because each failure left the
   instance running until we noticed and `vastai destroy`-ed by hand.

3. **`xrun gc`** — sweep dead pollers, dead runs, orphan vast instances tied to xrun-created
   labels, and stale temp dirs. After ~10 failed launches today, `runs.db` and the
   per-run dirs in `%APPDATA%\xrun\data\runs\` are full of dead state.

4. **Auto-destroy on fatal error.** If provision succeeds and any subsequent stage fails,
   xrun does destroy the instance (we saw `instance_destroyed: ok`). But if `vastai create
   instance` itself glitches (transient HTTP error during a follow-up `show_instance`), the
   instance gets created and is *never* tracked, so the user pays for an undestroyed box.
   Repro from today:

   ```
   error: provision failed: failed to parse vastai output:
          http show_instance: error sending request for url
          (https://console.vast.ai/api/v0/instances/35818676/)
   $ vastai show instances        # the instance is alive — xrun never registered it
   35818676  …  RTX_4090  …  $0.21/hr   loading
   ```

   On this kind of partial-create failure, xrun should:
   - Retry `show_instance` 3× with backoff (idempotent GET).
   - If still failing, immediately call `destroy_instance` to release the box, even though
     it never made it into `runs.db`.

### B. Observability while a run is live

5. **`xrun logs <id>` returns empty** even after `train_start: ok`. The training script
   doesn't fail; it's just that xrun has no streaming connection from the remote stdout/log
   files back to the local DB. Workaround today is `ssh -p <port> root@host tail -f
   /workspace/run/stdout.log`. Should be a one-liner:

   ```
   xrun logs <id>            # stream remote stdout.log live
   xrun logs <id> --tail 200 # last 200 lines, exit
   xrun logs <id> --grep 'val_F1' # filtered live tail
   ```

6. **`xrun show <id>` doesn't include the `ssh_host`/`ssh_port` of the instance.** When
   debugging, the very first thing we want is to SSH in. Today we had to grep
   `vastai show instances` for the matching age/uptime, with no guarantee of finding the
   right one across 4 orphaned + 1 active. Suggested:

   ```
   $ xrun show <id>
   …
   ssh:           ssh -p 18296 root@ssh1.vast.ai
   instance_id:   35818174
   stdout:        ssh -p 18296 root@ssh1.vast.ai tail -f /workspace/run/stdout.log
   ```

7. **`xrun shell <id>`** — drop directly into an ssh session on the running instance. Mirrors
   what every other workload-runner (railway, fly.io, modal, etc.) provides on day one.
   Saves the lookup hassle from #6.

8. **Live `xrun events <id> --watch`** — block on the events table and print new rows as
   they appear. Today we polled `xrun show` in a loop. `--watch` should also surface stderr
   dumps from each stage when one fails (we never see `upload.log`/`provision.log` files in
   `%APPDATA%\xrun\data\runs\<id>\` — only `manifest.yaml` and `poller.pid`).

9. **Per-run log files in `runs/<id>/`.** The cheat-sheet implies they should exist
   (`upload.log`, `provision.log`, `stdout.log`), but in practice the run dir only ever
   contains `manifest.yaml` + `poller.pid`. Anything xrun captures in-process is lost as
   soon as the run terminates. Suggested:

   ```
   runs/<id>/manifest.yaml
   runs/<id>/poller.pid
   runs/<id>/provision.log    # tracing capture for the provision stage
   runs/<id>/upload.log       # rsync/tar/ssh stderr+stdout
   runs/<id>/execute.log      # ssh launch invocation + initial echo
   runs/<id>/stdout.log       # streamed remote stdout (continuously appended)
   runs/<id>/events.jsonl     # mirror of the DB events table for offline inspection
   ```

### C. Iteration-speed commands

10. **`xrun rerun <id>`** is documented in the cheat sheet (`xrun rerun <id> --patch
    run.args.--lr=5e-4`) but not implemented. Today every relaunch involved editing the
    YAML in place. A `--patch <key>=<value>` would be ideal for sweeps.

11. **`xrun launch --override price.max_per_hour=0.20`** for ad-hoc cap changes without
    touching the YAML. This is the single most common tweak when chasing offers.

12. **`xrun launch --upload-only` / `xrun launch --no-execute`.** Provision the box, run all
    `data:` uploads, run setup, **stop**, hand the user an SSH command. Once the upload
    works, run `xrun resume <id>` to continue with `run.cmd`. Would have saved at least an
    hour today: instead of paying for 4 GPU minutes per launch attempt to test only the
    upload mechanic, we'd pay for just the one upload and iterate on the launch command
    against the same already-uploaded box.

13. **`xrun pull <id> --ssh`** — when artifacts haven't been declared in the manifest but
    we still want to grab `*.pt` from a known directory. Avoids re-launching just to add a
    pattern.

14. **`xrun launch --reuse-instance <vast-id>`.** After a failure that leaves the instance
    alive, just point xrun at the existing box and have it run upload + execute on it,
    skipping provision entirely. Today we instead had to destroy and reprovision a fresh
    box every time, paying ~$0.20 for each diagnostic round-trip.

### D. Manifest schema gaps

15. **Optional vast.ai search filters in `vast:`.** Today only `gpu.type`, `gpu.count`,
    `gpu.vram_min_gb`, `disk_gb`, `price.max_per_hour` are exposed. Add at least:

    ```yaml
    vast:
      cuda_min: 12.1
      inet_up_min_mbps: 100         # critical for fast 4 GB uploads
      inet_down_min_mbps: 100
      direct_port_count_min: 4
      verified: true                # currently silently always true
      rentable: true                # ditto
      reliability_min: 0.95
      regions: [Europe, North America]
    ```

    The lack of `inet_up_min_mbps` is what made us hit a ~3-min upload window where vast's
    NAT closed the connection (Update 0). Picking only nodes with ≥100 Mbps uplink would
    have closed the upload in ~30 s.

16. **`data: { mode: tar }` as an explicit named mode.** Today `mode: copy` *is* a tar pipe
    after the recent fix, which is great, but the manifest still calls it `copy` — a name
    that no longer matches its behaviour and was the source of the silent-no-op bug we
    spent the morning chasing. Aliasing/renaming to `tar` (or making it the default mode
    when `mode:` is omitted) would prevent that confusion.

17. **`data: { exclude: [...] }`** — rsync-style exclude patterns. We had to upload a 4 GB
    `itd_dataset/` for a smoke run when only ~600 MB of it was actually needed. Without
    exclude patterns the only workaround is staging a temp dir locally before each launch.

18. **`data: { compress: zstd }` / `compress: gzip`.** A 4 GB uncompressed tar takes ~3 min
    over a typical 50 Mbps uplink; the same data zstd-compressed is ~600 MB and lands in
    ~30 s. The new tar-pipe path already has all the plumbing — just needs `tar -cf -
    --zstd …` on the local end and `tar -xzf -` (or `--zstd`) on the remote.

19. **`policy.budget.max_usd: 1.50`** — auto-stop the run when accrued cost crosses the
    cap, regardless of `epochs` finishing. Right now a runaway run can burn the whole
    monthly budget if the user isn't watching.

### E. Diagnostics / dev experience

20. **`xrun launch --trace`** (already requested in §6 of the original list) — print every
    external invocation: REST URL + redacted body, ssh command, tar command, vastai
    sub-process, and their raw outputs. Gated off by default. Would have made every blocker
    in this morning's session debuggable in seconds rather than minutes.

21. **`xrun doctor` should validate the manifest** in addition to checking binaries. Today
    it warned about rsync but said nothing about the missing `vast.api_key` or the
    rejected GPU search query.

22. **Better error attribution in `vastai cp` / tar / ssh failures.** This morning's
    `local tar failed for <src>: exit Some(1)` was actually an ssh-side EPIPE because the
    remote sshd wasn't ready (Update 0). The first-failing process is rarely the root
    cause when both halves of a pipe die together. xrun should annotate which side died
    first, ideally via `wait4` ordering.

23. **`xrun config show --secrets`** — show key prefixes (`xxxxxxxx…ae990e`) so the user
    can confirm "the right key is set" without re-pasting it. Today we had `<set>` with no
    way to verify it was the same key vastai CLI uses.

24. **Manifest-source checksum invalidation.** `xrun show` reports `manifest_hash:
    df7dc708…`, but today after editing the manifest between launches the run dir still
    held the *old* manifest. `xrun launch` should always store the just-loaded manifest in
    `runs/<id>/manifest.yaml`, not symlink to the source path.



The original `provision failed: failed to parse vastai output` blocker and its
four follow-up regressions (key propagation, "no offers", `vastai execute`
rejecting `nohup … &`) are all fixed and shipped. What's left below is the
quality-of-life and robustness work that surfaced during that debugging session
but isn't blocking a launch.

## Open

### 0. ⚠️ BLOCKER — `tar_upload` races SSH-not-ready on fresh provision

**Run `01KQC93D3WTYB67CNRCR8Q61ER` (2026-04-29).** Right after the new tar-pipe upload
shipped:

```
$ xrun launch experiments/ml_detector_3d/offset_v1.yaml --detach
Created run 01KQC93D3WTYB67CNRCR8Q61ER
error: upload failed: vastai CLI failed (exit 1):
       local tar failed for C:/Users/gerrux/Desktop/itd_dataset/: exit Some(1)

Events:
  09:29:28  provision           ok
  09:29:28  upload              start
  09:29:30  instance_destroyed  ok        # 2 s after upload start
```

Failure timing (2 s) is way too short for tar to have processed any of 4 GB.
Sequence is almost certainly:

1. xrun spawns `tar -cf - -C <src> .` and pipes its stdout to `ssh root@host "mkdir … && tar -xf - -C <dst>"`.
2. The ssh process starts before sshd on the just-provisioned instance is accepting
   connections (typical vast.ai cold-boot is ~30–60 s before sshd is ready).
3. ssh exits non-zero immediately, closing stdin.
4. tar gets `EPIPE` writing to its closed stdout, exits 1 (bsdtar) / 141 (linux).
5. xrun checks `tar_status.success()` first and returns
   `local tar failed for <src>: exit Some(1)` — the ssh failure is swallowed.

Smoking gun: `tar -cf - -C C:/Users/gerrux/Desktop/itd_dataset/ . > /dev/null` exits
0 with empty stderr in the same shell (Windows bsdtar 3.8.4 — built-in `tar.exe` xrun
finds first on PATH). So tar isn't broken; it just gets killed by an early-closed pipe.

**Asks:**

1. **Wait for SSH before any upload.** After provisioning (and any time `ssh_host` /
   `ssh_port` change), poll the SSH endpoint until it accepts a connection — e.g.
   `tokio::net::TcpStream::connect((host, port))` with retries every 2–3 s, capped
   at ~120 s, before issuing any `tar_upload` / `run_rsync` / `ssh_exec`.

2. **Surface SSH errors when both halves of the pipe fail.** When ssh exits non-zero,
   tar usually exits non-zero too because of EPIPE. Today only the tar error is
   reported, hiding the real cause. Suggested logic:
   ```rust
   if !ssh_status.status.success() {
       // ssh dying first is the most likely root cause when both fail
       return Err(VastError::CliFailure {
           exit_code: ssh_status.status.code().unwrap_or(-1),
           stderr: format!("upload ssh failed at {}: {}", dst,
               String::from_utf8_lossy(&ssh_status.stderr).trim()),
       });
   }
   if !tar_status.success() { … }   // only report local-tar problems if ssh was happy
   ```

3. **Dummy `ssh_exec true` as a readiness gate.** A 1-line probe before tar-upload
   that runs `ssh ... true` (or `ssh ... echo ready`) with retries has the side
   benefit of warming the StrictHostKeyChecking=no known_hosts entry once, so the
   later upload can't trip on first-contact host-key prompts.

### 1. Tar-mode fallback for data uploads on Windows

Manifests with `data: { mode: rsync }` require `rsync` on PATH on the launching
machine. On Windows that's a hard dependency to install (MSYS2 / WSL / Git for
Windows ≥ 2.42). `xrun doctor` warns about it, but the launch itself still
proceeds and only fails at the upload phase.

**Want:** either bundle a `mode: tar` fallback (`tar -cf - … | ssh … "tar -xf -
-C …"`, which is what we used to do by hand and which finishes a 4 GB upload
in ~5 min on a normal connection), or refuse to start the run with a precise
error at `xrun launch` time when a `mode: rsync` data item is set and `rsync`
isn't on PATH.

### 2. ⚠️ BLOCKER — `mode: copy` upload reports `ok` but transfers nothing

**Confirmed on run `01KQC8EEQMVFP9119089T4F6KY` (2026-04-29).**

Manifest:
```yaml
data:
  - src: "C:/Users/gerrux/Desktop/itd_dataset/"            # ~4 GB
    dst: /workspace/data/itd_dataset
  - src: ".../curated_apex_gt/"                            # ~few MB
    dst: /workspace/data/apex_gt
  - src: ".../experiments/ml_detector_3d/"                 # ~1 MB
    dst: /workspace/code
```

xrun event log says everything succeeded:
```
2026-04-29T09:17:43Z  provision    ok
2026-04-29T09:17:43Z  upload       start
2026-04-29T09:17:46Z  upload       ok       (3 s)
2026-04-29T09:17:50Z  env_ready    ok
2026-04-29T09:17:50Z  train_start  ok
```

**SSH into the instance immediately afterwards shows none of that data exists:**
```
$ ssh -p 17468 root@ssh1.vast.ai 'ls /workspace; find /workspace -maxdepth 3 -type d'
onstart.sh
ports.log
run
/workspace
/workspace/run
$ ssh -p 17468 root@ssh1.vast.ai 'ls /workspace/data/itd_dataset 2>&1'
ls: cannot access '/workspace/data/itd_dataset': No such file or directory
$ ssh -p 17468 root@ssh1.vast.ai 'pgrep -a python'
(no output)
```

So:
- `/workspace/data/itd_dataset` — missing (4 GB source)
- `/workspace/data/apex_gt` — missing
- `/workspace/code` — missing (and therefore `launch_offset_v1.sh` was never on disk)
- The only running process is `bash /.launch` waiting for the launch trigger
- No python, no training, just a billing meter spinning at $0.26/hr

`train_start: ok` is therefore false. xrun returned a green status for a run that
provably never executed any user code.

**Root cause hypothesis:** `vastai cp <local-dir>/ root@host:/dst/` does not
recursively copy directory contents — it likely either expects a single file or
needs `-r`. The 3 s wallclock for "uploading" 4 GB plus apex_gt plus the code dir
is consistent with vastai cp returning success without doing anything. xrun
treats vastai's exit code as truth and never verifies destination size.

**Asks:**
1. Make `mode: copy` actually work for directories — either pass the right flags
   to `vastai cp` (likely `--recursive`), or refuse directory sources in copy
   mode with a clear "use mode: rsync, mode: tar, or pass a single file" error.
2. After every upload, post a `du -sb <dst>` check over SSH and refuse to advance
   to `train_start` if the destination is empty / smaller than expected. Right
   now there is no integrity check at all.
3. Surface the actual upload command(s) xrun ran in `xrun show <id>` events. If
   we'd seen `upload: vastai cp -r src dst` we'd have flagged the missing `-r`
   in seconds.

**Workaround we'd reach for next:** pre-tar the data locally (`tar -cf
itd_dataset.tar -C C:/Users/gerrux/Desktop itd_dataset`) and use the existing
`unpack: { format: tar, into: ... }` path with a single-file copy. But:

- This is annoying (~1 min local tar for 4 GB, plus needing to keep the tar
  fresh after every dataset edit).
- It only covers data items whose source is small enough to tar quickly. The
  code dir would also need similar treatment.
- A `mode: tar` that does `tar -cf - <src> | ssh ... "tar -xf - -C <dst>"`
  in one shot is the right primitive — see issue 1 above.

We are blocked from any real launch until either the `mode: copy` directory bug
is fixed, or `mode: rsync` is fixed (rsync isn't on PATH on this Windows host),
or a working `mode: tar` ships.

### 3. Surface optional constraints in the manifest schema

Today `vast:` accepts `gpu.type`, `gpu.count`, `gpu.vram_min_gb`, `disk_gb`,
`price.max_per_hour`. Missing levers that vast.ai's search supports:
`cuda_min`, `inet_up_min`, `verified` (yes/no), `rentable` (yes/no),
`direct_port_count_min`. Without them the user has no way to relax xrun's
default filters when the auto-search returns 0 offers.

### 4. Document the silent default filters

xrun forces `verified=true`, `rentable=true`, `external=false`, `rented=false`,
`type=on-demand`, `order=score-desc`, `allocated_storage=5.0 GiB` on every
search. These gate out a lot of supply at the cheap end. Defaults are fine,
but users should be able to read what they are without grepping the source.
A short section in `docs/MANIFEST.md` would suffice.

### 5. Show in `xrun show <id>` which calls went via REST vs CLI

When provision/upload/execute fails it would help to see the call path in the
event log: `provision: REST POST /bundles/`, `upload: ssh tar pipe`,
`execute: ssh root@host`. Today the user has to guess from the error string.

### 6. `xrun launch --trace`

Optional flag that prints every external invocation (REST URL + body, SSH
command, vastai sub-process, rsync command) and its raw output, gated off by
default. Would have made every blocker in the original issue debuggable in
seconds rather than minutes.

## Recently fixed (kept here as anchors for regression tests)

- Provision parse-error → REST migration of `search_offers` / `create_instance`
  / `destroy_instance` / `show_instance`. Vast Python CLI is no longer on the
  provision path. `crates/xrun-vast/src/rest.rs`,
  `crates/xrun-vast/tests/rest_contract.rs`.
- `vast.api_key not set` → `launch.rs` and `poll_daemon.rs` now load
  `Credentials` from `config_dir` instead of `VastCredentials::default()`.
  Fix-it text in the error points at the correct subcommand.
- "no offers available matching query" for everything → `gpu_name` wire form
  was wrong (`RTX_4090` instead of `RTX 4090`; vast's CLI does
  `_` → space before posting). Regression test in `rest_contract.rs`. The
  error now also prints the full JSON body of the failed search.
- `vastai execute` rejecting `nohup sh -c '…' & echo $!` → setup + launch
  commands now go over plain SSH (`crates/xrun-vast/src/transfer.rs::ssh_exec`,
  `execute::launch_run` takes `&InstanceHandle`).
