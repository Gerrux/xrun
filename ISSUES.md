# xrun field-issue log

Failures, root causes, and lessons from real arborust runs on Kaggle.
Source-of-truth for ROADMAP planning. Newest first.

---

## 2026-05-05: arborust evening session — all 8 items closed

Issues 1–8 from the 2026-05-04 evening session are fixed and verified by
`cargo test --workspace` + `pytest python/xrun_hook`. See `git log` for the
commits.

| # | Title | Resolution |
|---|---|---|
| 1 | Kaggle dataset version race after `status=ready` | Pin to `currentVersionNumber` via `/api/v1/datasets/view` after readiness; slugs already carrying `/N` are left alone. |
| 2 | `pip install` aborts on missing `xrun_hook`, drops siblings | Wheel base64-embedded in kernel `main.py`; bootstrap installs locally before user setup. |
| 3 | `xrun launch --detach` blocks indefinitely | `kaggle kernels push` pipes drained on background threads — `try_wait` no longer hangs on a full OS pipe buffer. |
| 4 | `xrun pull --ckpt …` printed "not implemented" for Kaggle | CLI now resolves run → vendor adapter → `adapter.pull()`; reports matching files for `best`/`latest`/`all`. |
| 5 | `xrun_hook` install path unclear | Same as #2 (wheel auto-injected) + git-pip command documented. |
| 6 | Duplicate `running:start` events after poller restart | Adapter rehydrates last kernel state from DB on first poll after daemon restart. |
| 7 | `⚠ stale` shown while kernel is healthy | TUI now runs auto-resume every 60 s, so a poller that dies mid-session self-heals without `S`. |
| 8 | No live telemetry on Kaggle | `xrun_hook`'s log streamer also tails `events.jsonl` / `metrics.jsonl` and pushes them as MLflow artifact chunks; the Kaggle adapter ingests new chunks every poll tick. Requires `mlflow.url` in xrun config. |

## Reusable themes (carry into design reviews)

1. **Visibility-first**: when something fails or stalls, xrun should surface
   actionable info before the user has to dig into the Kaggle web UI or
   raw stdout. Issue 8 closed the worst gap; keep raising the bar.

2. **Atomic-ish operations**: pip install resolution, dataset version
   indexing, kernel snapshotting — all "appear to succeed" before they're
   actually consistent. Guard with explicit verification or surface
   "cool-down period" recommendations.

3. **Wheel/sourcing of xrun_hook**: hard requirement for live telemetry.
   Now vendor-installed automatically by xrun in remote environments;
   keep this contract for any new vendor.

4. **Pull from in-flight runs**: `xrun pull` works post-completion. Live
   pull mid-run is still an open want — Kaggle's API gives nothing here
   without MLflow as a side channel. Track future demand before building.
