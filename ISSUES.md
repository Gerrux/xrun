# xrun field-issue log

Failures, root causes, and lessons from real arborust runs on Kaggle.
Source-of-truth for ROADMAP planning. Newest first.

---

## 2026-05-06: `{user}` placeholder not expanded in `datasets:` list (open)

**Symptom**: in a manifest with
```yaml
kaggle:
  kernel_slug: "{user}/treetop3d-pertree-mass"
  datasets:
    - "{user}/arborust-pertree-code"
```
xrun expands `kernel_slug` correctly (resolves to
`fakefentus/treetop3d-pertree-mass` per `resolve_user()`), but pushes
the literal `{user}/arborust-pertree-code` into Kaggle's
`kernel-metadata.json` `dataset_sources`. Kaggle accepts the kernel
push (no validation on attached-dataset slugs), the kernel runs, the
dataset never mounts, and the cmd fails with whatever it does when
the input is missing — in our case `find /kaggle/input … cache_mc_v5
→ empty → ERROR: cache not found`. xrun's launch log shows the
warning `could not check dataset status for '{user}/arborust-pertree-code'`
but treats it as non-fatal and proceeds.

**Repro**: any manifest using `{user}` in both `kernel_slug` and
`datasets`. Latest run that hit it: `01KQZ9MNWCFZ23RGWQT1KW772B` —
fail at `running:start` after 41s with `KernelWorkerStatus.ERROR`.

**Fix idea**: same expansion pass that handles `kernel_slug` should
walk `kaggle.datasets`, `kaggle.competition`, `data[*].dst` for
placeholders. Alternatively: hard-fail at validation when any kaggle
field still contains `{user}` after expansion — surfaces the bug
loudly before a billable run.

**Workaround**: hardcode the username in `datasets:`. Defeats the
point of token-derived `{user}`, but unblocks launches.

---

## 2026-05-06: token-only auth resolves wrong username (open, follow-up)

**Symptom**: user updates `kaggle.token` in xrun config (new API token
for account `fakefentus`), but `xrun dataset push --slug <name>` still
auto-prefixes with `kartaviychert/<name>` — the OLD account that lives
in `~/.kaggle/kaggle.json`. xrun's `resolve_user()`
(`crates/xrun-kaggle/src/adapter.rs:144`) tries:
1. `credentials.username` (`kaggle.username` in xrun config) — usually
   empty for token-only auth.
2. `cli.authenticate()` → calls Python `KaggleApi.authenticate()`
   which reads `username` from `~/.kaggle/kaggle.json` (legacy field).

Token (`kaggle.token` / `kaggle.json.key`) and username are
**independent stores**; updating just the token does not refresh the
username. Surprising from the user's perspective — they think a new
token = new identity.

**Fix idea**: when only a token is configured, derive username from
the token by hitting `https://www.kaggle.com/api/v1/users/me` (or the
Python module's `KaggleApi.config_values.username` after
`authenticate()`). Today the CLI takes the first non-empty name from
either source, even if it points at a different account than the
token. At minimum: `xrun doctor` should compare the
`kaggle.json.username` against whatever `users/me` returns for the
active token and warn loudly on mismatch.

**Workaround**: either (a) replace `~/.kaggle/kaggle.json` with the
new account's downloaded JSON, or (b) `xrun config set
kaggle.username <new_name>` to override the resolution chain.

---

## 2026-05-06: auto-detect Kaggle nickname (open)

**Symptom**: every manifest hard-codes `kernel_slug:
<owner>/<name>` and every `xrun dataset push --slug <owner>/<name>`
takes the owner. When the user changes Kaggle accounts (e.g. from
`kartaviychert` to `fakefentus`) all manifests still point at the old
account; pushes go to the wrong owner or 401, and kernel slugs that
previously resolved silently route to a different owner's namespace.
The user has the correct nickname in `~/.kaggle/kaggle.json`
(`username` field) but xrun never reads it.

**Repro**: `cat ~/.kaggle/kaggle.json` shows `username=A`, then user
asks "запусти X на Kaggle"; manifests in `exp/` still reference
`username=B`. xrun has no way to flag the mismatch.

**Suggested fix**:
1. `xrun doctor` should compare `kaggle.json` username against owners
   appearing in any `kernel_slug` / `datasets:` entry of recently
   launched manifests, warn on mismatch.
2. New `xrun config kaggle.owner` (read-only or override) — by default
   resolves from `~/.kaggle/kaggle.json` username, used by
   `init-manifest` to fill the slug placeholder, and by
   `dataset push --slug NAME` (without owner) to prefix automatically.
3. For `kernel_slug`, accept `<name>` short-form and prefix with
   detected owner the same way.

**Workaround for now**: user must manually update both `kaggle.json`
(with new account's API token) and every `slug` reference in
manifests when switching accounts. Field-reported by gerrux during
voxel-formation experiment launch.

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
