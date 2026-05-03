"""Step renderers for the first-run wizard.

These are free functions that take the WizardScreen as their first argument
so the screen module stays focused on event wiring and state transitions.
"""
from __future__ import annotations

from typing import TYPE_CHECKING

from textual.containers import Vertical
from textual.widgets import (
    Checkbox,
    Input,
    LoadingIndicator,
    RadioButton,
    RadioSet,
    Static,
)

from xrun_tui.screens.wizard.catalog import (
    KAGGLE_FIELDS,
    MLFLOW_FIELDS,
    SINKS,
    SINK_BY_ID,
    SSH_FIELDS,
    VENDOR_BY_ID,
    VENDOR_CARDS,
)
from xrun_tui.services import doctor as _doctor
from xrun_tui.services import probe as _probe

if TYPE_CHECKING:
    from xrun_tui.screens.wizard.screen import WizardScreen


# ── Step 1: local probe ──────────────────────────────────────────────────────


async def render_local(screen: "WizardScreen", body: Vertical) -> None:
    if not screen._probe_done:
        await body.mount(Static(
            "[bold #c0caf5]Step 1 — Local capabilities[/]\n\n"
            "[#565f89]Detecting OS and GPU…[/]",
            classes="wizard-text",
        ))
        await body.mount(LoadingIndicator())
        return

    gpus = screen._probe.get("gpus", [])
    os_str = screen._probe.get("os", "?")
    arch = screen._probe.get("arch", "?")
    lines = [
        "[bold #c0caf5]Step 1 — Local capabilities[/]",
        "",
        f"[#565f89]OS:[/]   [#c0caf5]{os_str}[/]  [#414868]({arch})[/]",
    ]
    if gpus:
        lines.append(f"[#565f89]GPU:[/]  [#9ece6a]{len(gpus)} detected[/]")
        for g in gpus:
            lines.append(f"       • {g}")
        lines.append("")
        lines.append("[#9ece6a]Local vendor available[/] — you can run smoke "
                     "tests and small jobs without any cloud account.")
    else:
        lines.append(f"[#565f89]GPU:[/]  [#e0af68]none detected[/] — "
                     "CPU-only, fine for smoke tests.")
        lines.append("")
        lines.append("Add a cloud vendor on the next step to actually train.")
    await body.mount(Static("\n".join(lines), classes="wizard-text"))


# ── Step 2: vendors ──────────────────────────────────────────────────────────


async def render_vendors(screen: "WizardScreen", body: Vertical) -> None:
    await body.mount(Static(
        "[bold #c0caf5]Step 2 — Vendors[/]\n\n"
        "[#565f89]Tab to move between cards · Space to toggle · "
        "[/][bold]o[/][#565f89] opens the API-key page of the focused card.[/]",
        classes="wizard-text",
    ))
    for vid, label, desc, _url, available, takes_key in VENDOR_CARDS:
        row = Vertical(classes="wizard-vendor-row")
        await body.mount(row)
        badge = "" if available else "  [#e0af68][v0.7+][/]"
        cb = Checkbox(
            f"[bold]{label}[/]{badge}  [#565f89]{desc}[/]",
            value=(vid in screen._selected_vendors and available),
            id=f"wiz-vendor-cb-{vid}",
            disabled=not available,
        )
        await row.mount(cb)
        if vid == "kaggle":
            await _mount_kaggle_form(screen, row)
        elif takes_key:
            await _mount_vendor_key_form(screen, row, vid, label)
        elif vid == "ssh":
            await _mount_ssh_form(screen, row)


async def _mount_kaggle_form(screen: "WizardScreen", row: Vertical) -> None:
    kform = Vertical(id="wiz-kaggle-form", classes="wizard-ssh-form")
    kform.display = "kaggle" in screen._selected_vendors
    await row.mount(kform)
    await kform.mount(Static(
        "[#565f89]Either fill the JWT token, or username + API key. "
        "Leave blank if [/][bold]~/.kaggle/kaggle.json[/][#565f89] is "
        "already on this machine — xrun imports it automatically.[/]",
        classes="wizard-text",
    ))
    await kform.mount(Static(
        " Get token ↗  Open Kaggle Account → Tokens ",
        id="wiz-open-kaggle",
        classes="wizard-link-btn",
    ))
    for field, placeholder, password in KAGGLE_FIELDS:
        await kform.mount(Input(
            value=screen._kaggle_fields.get(field, ""),
            placeholder=placeholder,
            password=password,
            id=f"wiz-kaggle-{field}",
            classes="wizard-input",
        ))


async def _mount_vendor_key_form(
    screen: "WizardScreen", row: Vertical, vid: str, label: str,
) -> None:
    vform = Vertical(id=f"wiz-vendor-form-{vid}", classes="wizard-ssh-form")
    vform.display = vid in screen._selected_vendors
    await row.mount(vform)
    await vform.mount(Static(
        f" Get key ↗  Open {label} API-key page ",
        id=f"wiz-open-{vid}",
        classes="wizard-link-btn",
    ))
    await vform.mount(Input(
        value=screen._pasted_keys.get(vid, ""),
        password=True,
        placeholder=f"Paste {label} API key (optional — can also set later)",
        id=f"wiz-vendor-input-{vid}",
        classes="wizard-input",
    ))


async def _mount_ssh_form(screen: "WizardScreen", row: Vertical) -> None:
    ssh_form = Vertical(id="wiz-ssh-form", classes="wizard-ssh-form")
    ssh_form.display = "ssh" in screen._selected_vendors
    await row.mount(ssh_form)
    for field, _flbl, placeholder, _req in SSH_FIELDS:
        await ssh_form.mount(Input(
            value=screen._ssh_fields.get(field, ""),
            placeholder=placeholder,
            id=f"wiz-ssh-{field}",
            classes="wizard-input",
        ))


# ── Step 3: logging ──────────────────────────────────────────────────────────


async def render_logging(screen: "WizardScreen", body: Vertical) -> None:
    kaggle_selected = "kaggle" in screen._selected_vendors
    await body.mount(Static(
        "[bold #c0caf5]Step 3 — Live logging[/]\n\n"
        "[#565f89]How should xrun stream events and metrics during a run?"
        + (" [#e0af68]Kaggle is selected — pick 'Mirror' for live logs.[/]"
           if kaggle_selected else "")
        + "[/]",
        classes="wizard-text",
    ))
    rs = RadioSet(
        RadioButton(
            "Off — local SQLite only (no live updates in TUI)",
            value=screen._log_mode == "off",
            id="wiz-mode-off",
        ),
        RadioButton(
            "Polling — TUI tails JSONL from instance (vast / ssh / local). Default.",
            value=screen._log_mode == "polling",
            id="wiz-mode-polling",
        ),
        RadioButton(
            "Mirror — also push metrics to MLflow tracking server "
            "(required for Kaggle live logs)",
            value=screen._log_mode == "mirror",
            id="wiz-mode-mirror",
        ),
        id="wiz-mode-radio",
    )
    await body.mount(rs)

    sinks_box = Vertical(id="wiz-sinks-box")
    await body.mount(sinks_box)
    if screen._log_mode == "mirror":
        await mount_sinks(screen, sinks_box)


async def mount_sinks(screen: "WizardScreen", container: Vertical) -> None:
    await container.remove_children()
    await container.mount(Static(
        "\n[bold #c0caf5]Mirror sinks[/]  "
        "[#565f89]Press [/][bold]o[/][#565f89] to open the docs of the "
        "focused sink.[/]",
        classes="wizard-text",
    ))
    for sid, label, available, _url in SINKS:
        badge = "" if available else "  [#e0af68][v0.8][/]"
        cb = Checkbox(
            f"[bold]{label}[/]{badge}",
            value=(sid in screen._selected_sinks and available),
            id=f"wiz-sink-cb-{sid}",
            disabled=not available,
        )
        await container.mount(cb)
        if sid == "mlflow":
            await _mount_mlflow_form(screen, container)


async def _mount_mlflow_form(screen: "WizardScreen", container: Vertical) -> None:
    mform = Vertical(id="wiz-mlflow-form", classes="wizard-ssh-form")
    mform.display = "mlflow" in screen._selected_sinks
    await container.mount(mform)
    await mform.mount(Static(
        "[#565f89]Tracking server URL is required. Auth is optional: "
        "use a Bearer token, OR username+password, OR leave blank for an "
        "anonymous local server. Stored in[/] [bold]credentials.toml[/][#565f89] "
        "(except URL — goes to[/] [bold]config.toml[/][#565f89]).[/]",
        classes="wizard-text",
    ))
    await mform.mount(Static(
        " Docs ↗  MLflow tracking-server setup ",
        id="wiz-open-mlflow",
        classes="wizard-link-btn",
    ))
    for field, placeholder, password in MLFLOW_FIELDS:
        await mform.mount(Input(
            value=screen._mlflow_fields.get(field, ""),
            placeholder=placeholder,
            password=password,
            id=f"wiz-mlflow-{field}",
            classes="wizard-input",
        ))


# ── Step 4: recap ────────────────────────────────────────────────────────────


async def render_recap(screen: "WizardScreen", body: Vertical) -> None:
    sinks = sorted(screen._selected_sinks) if screen._log_mode == "mirror" else []
    active_vendors = [v for v in screen._selected_vendors
                      if VENDOR_BY_ID.get(v, (None,) * 5)[4]]
    keys_set = [v for v, k in screen._pasted_keys.items() if k.strip()]
    if "kaggle" in screen._selected_vendors:
        ktok = screen._kaggle_fields.get("token", "")
        kusr = screen._kaggle_fields.get("username", "")
        kkey = screen._kaggle_fields.get("key", "")
        if ktok:
            keys_set.append("kaggle (token)")
        elif kusr and kkey:
            keys_set.append(f"kaggle (legacy, {kusr})")
        else:
            keys_set.append("kaggle (auto-import from ~/.kaggle/)")

    ssh_line = "[#414868]not configured[/]"
    if "ssh" in screen._selected_vendors and screen._ssh_fields.get("alias"):
        a = screen._ssh_fields["alias"]
        u = screen._ssh_fields.get("user", "?")
        h = screen._ssh_fields.get("host", "?")
        ssh_line = f"[#9ece6a]{a}[/] → [#c0caf5]{u}@{h}[/]"

    mlflow_line = "[#414868]none[/]"
    if "mlflow" in screen._selected_sinks and screen._log_mode == "mirror":
        url = screen._mlflow_fields.get("url", "")
        if url:
            tok = screen._mlflow_fields.get("token", "")
            usr = screen._mlflow_fields.get("username", "")
            pwd = screen._mlflow_fields.get("password", "")
            if tok:
                auth = "Bearer token"
            elif usr and pwd:
                auth = f"Basic ({usr})"
            else:
                auth = "anonymous"
            mlflow_line = f"[#9ece6a]{url}[/]  [#565f89]({auth})[/]"
        else:
            mlflow_line = "[#f7768e]selected but URL empty[/]"

    gpu_line = (f"[#9ece6a]{len(screen._probe.get('gpus', []))} detected[/]"
                if screen._probe.get("gpus") else "[#e0af68]none[/]")
    lines = [
        "[bold #c0caf5]Step 4 — Recap[/]",
        "",
        f"[#565f89]Local GPU:[/]      {gpu_line}",
        f"[#565f89]Vendors:[/]        "
        + (", ".join(f"[#9ece6a]{v}[/]" for v in active_vendors)
           if active_vendors else "[#414868]none[/]"),
        f"[#565f89]Keys staged:[/]    "
        + (", ".join(f"[#9ece6a]{v}[/]" for v in keys_set)
           if keys_set else "[#414868]none[/]"),
        f"[#565f89]SSH host:[/]       {ssh_line}",
        f"[#565f89]Logging mode:[/]   [#c0caf5]{screen._log_mode}[/]",
        f"[#565f89]Sinks:[/]          "
        + (", ".join(f"[#9ece6a]{s}[/]" for s in sinks)
           if sinks else "[#414868]none[/]"),
        f"[#565f89]MLflow:[/]         {mlflow_line}",
        "",
        "[#565f89]Pressing [/][bold]Finish[/][#565f89] writes the config and "
        "marks the wizard as done. Re-run any time with[/] [bold]xrun init[/][#565f89].[/]",
        "",
        "[bold #c0caf5]Environment check (xrun doctor)[/]",
    ]
    await body.mount(Static("\n".join(lines), classes="wizard-text"))
    await body.mount(LoadingIndicator(id="wiz-doctor-spinner"))
    screen._doctor_loaded = False
    screen.run_worker(fill_doctor(screen, body), exclusive=True, group="doctor")

    await body.mount(Static(
        "\n[bold #c0caf5]Connection tests[/]  "
        "[#565f89](probe each selected target with the staged credentials)[/]",
        classes="wizard-text",
    ))
    await body.mount(LoadingIndicator(id="wiz-probe-spinner"))
    screen._probe_results = []
    screen.run_worker(fill_probes(screen, body), exclusive=True, group="probes")


async def fill_doctor(screen: "WizardScreen", body: Vertical) -> None:
    ok, data, err = await _doctor()
    try:
        spinner = screen.query_one("#wiz-doctor-spinner", LoadingIndicator)
    except Exception:
        return  # step changed
    await spinner.remove()
    if not ok:
        await body.mount(Static(
            f"[#f7768e]doctor failed:[/] {err or 'unknown error'}",
            classes="wizard-text",
        ))
        return
    rows = []
    for entry in data if isinstance(data, list) else []:
        name = entry.get("check", "?")
        status = entry.get("status", "?")
        detail = entry.get("detail", "")
        glyph = {"OK": "[#9ece6a]✓[/]",
                 "WARN": "[#e0af68]⚠[/]",
                 "FAIL": "[#f7768e]✗[/]"}.get(status, "·")
        rows.append(f"  {glyph} [bold]{name:<22}[/] [#565f89]{detail}[/]")
    if not rows:
        rows.append("[#414868]  (no checks reported)[/]")
    await body.mount(Static("\n".join(rows), classes="wizard-text"))
    screen._doctor_loaded = True


async def fill_probes(screen: "WizardScreen", body: Vertical) -> None:
    """Run live connectivity probes for whatever the user selected.

    Always probes `local` (cheap and surfaces GPU info). Probes a vendor only
    when its credentials are staged; probes mlflow only when mirror+mlflow is
    on AND a URL is set. Skips Kaggle when only the JWT token is staged — the
    CLI side returns a `WARN`-style "cannot validate locally" message which we
    surface as a yellow row, not a failure.
    """
    import asyncio as _asyncio

    targets = list(_probe_targets(screen))
    if not targets:
        try:
            spinner = screen.query_one("#wiz-probe-spinner", LoadingIndicator)
            await spinner.remove()
        except Exception:
            pass
        await body.mount(Static(
            "  [#414868]· nothing to probe — no vendors / sinks selected[/]",
            classes="wizard-text",
        ))
        return

    coros = [_probe(t["vendor"], env=t["env"], extra_args=t["args"]) for t in targets]
    results = await _asyncio.gather(*coros, return_exceptions=False)

    try:
        spinner = screen.query_one("#wiz-probe-spinner", LoadingIndicator)
        await spinner.remove()
    except Exception:
        return  # step changed

    rows = []
    for t, res in zip(targets, results):
        ok = bool(res.get("ok"))
        detail = (res.get("detail") or "").strip()
        # Kaggle JWT case: ok=true but detail signals deferred validation.
        warn_only = ok and "JWT" in detail and "first launch" in detail
        if warn_only:
            glyph = "[#e0af68]⚠[/]"
        else:
            glyph = "[#9ece6a]✓[/]" if ok else "[#f7768e]✗[/]"
        ms = res.get("elapsed_ms", 0)
        ms_str = f"[#414868]({ms} ms)[/]" if ms else ""
        rows.append(
            f"  {glyph} [bold]{t['label']:<22}[/] [#565f89]{detail}[/]  {ms_str}"
        )

    await body.mount(Static("\n".join(rows), classes="wizard-text"))
    screen._probe_results = [
        {"label": t["label"], "ok": bool(r.get("ok")), "detail": r.get("detail", "")}
        for t, r in zip(targets, results)
    ]
    failed = [r for r in screen._probe_results if not r["ok"]]
    if failed:
        await body.mount(Static(
            f"\n[#e0af68]{len(failed)} probe(s) failed.[/] "
            "[#565f89]You can still press[/] [bold]Finish[/] [#565f89]to save the "
            "config and fix later, or go[/] [bold]Back[/] [#565f89]to correct the "
            "fields.[/]",
            classes="wizard-text",
        ))


def _probe_targets(screen: "WizardScreen") -> list[dict]:
    """Build the list of probe specs for the wizard's current state.

    Each spec is a dict {label, vendor, env, args}. `env` only contains the
    XRUN_PROBE_* keys we want to inject; `_run` merges them over os.environ.
    """
    targets: list[dict] = []

    # Local is always informative — even if the user only picked cloud vendors,
    # the GPU detection is reassurance that the host is wired up.
    targets.append({"label": "local", "vendor": "local", "env": None, "args": []})

    for vid in sorted(screen._selected_vendors):
        if vid == "vast":
            key = screen._pasted_keys.get("vast", "").strip()
            if key:
                targets.append({
                    "label": "vast.ai",
                    "vendor": "vast",
                    "env": {"XRUN_PROBE_VAST_KEY": key},
                    "args": [],
                })
        elif vid == "kaggle":
            tok = screen._kaggle_fields.get("token", "").strip()
            usr = screen._kaggle_fields.get("username", "").strip()
            kk = screen._kaggle_fields.get("key", "").strip()
            env: dict[str, str] = {}
            if tok:
                env["XRUN_PROBE_KAGGLE_TOKEN"] = tok
            elif usr and kk:
                env["XRUN_PROBE_KAGGLE_USERNAME"] = usr
                env["XRUN_PROBE_KAGGLE_KEY"] = kk
            if env:
                targets.append({
                    "label": "kaggle",
                    "vendor": "kaggle",
                    "env": env,
                    "args": [],
                })
        elif vid == "ssh":
            host = screen._ssh_fields.get("host", "").strip()
            user = screen._ssh_fields.get("user", "").strip()
            if host and user:
                args = ["--ssh-host", host, "--ssh-user", user]
                port = screen._ssh_fields.get("port", "").strip()
                if port:
                    args += ["--ssh-port", port]
                key_path = screen._ssh_fields.get("key", "").strip()
                if key_path:
                    args += ["--ssh-key", key_path]
                alias = screen._ssh_fields.get("alias", "").strip() or "ssh"
                targets.append({
                    "label": f"ssh:{alias}",
                    "vendor": "ssh",
                    "env": None,
                    "args": args,
                })

    if (
        screen._log_mode == "mirror"
        and "mlflow" in screen._selected_sinks
    ):
        url = screen._mlflow_fields.get("url", "").strip()
        if url:
            env = {}
            tok = screen._mlflow_fields.get("token", "").strip()
            usr = screen._mlflow_fields.get("username", "").strip()
            pwd = screen._mlflow_fields.get("password", "").strip()
            if tok:
                env["XRUN_PROBE_MLFLOW_TOKEN"] = tok
            elif usr and pwd:
                env["XRUN_PROBE_MLFLOW_USERNAME"] = usr
                env["XRUN_PROBE_MLFLOW_PASSWORD"] = pwd
            targets.append({
                "label": "mlflow",
                "vendor": "mlflow",
                "env": env or None,
                "args": ["--mlflow-url", url],
            })

    return targets


# ── Stepper bar ──────────────────────────────────────────────────────────────


def stepper_markup(step: int, n_steps: int) -> str:
    labels = ["Local", "Vendors", "Logging", "Done"]
    cells = []
    for i, lbl in enumerate(labels):
        if i < step:
            cells.append(f"[#9ece6a]✓ {lbl}[/]")
        elif i == step:
            cells.append(f"[bold #7aa2f7]● {i + 1}/{n_steps} {lbl}[/]")
        else:
            cells.append(f"[#414868]○ {lbl}[/]")
    return "  →  ".join(cells)
