"""Confirm-and-run flows for a run (stop / rerun / pull / sync), shared by the
Runs list, Run detail and the error dialog so wording, confirmation policy and
result reporting cannot drift between screens.

Every function takes the calling screen and a run dict (`id`, optional `name`).
`after(ok)` is the screen's own follow-up (refresh, dismiss); it is skipped
when the screen went away while the CLI was working.
"""
from __future__ import annotations

import inspect
from collections.abc import Awaitable, Callable
from typing import Any

from xrun_tui import services
from xrun_tui.screens.confirm import ConfirmScreen

Work = Callable[[], Awaitable[tuple[bool, str]]]
After = Callable[[bool], Any]

_ERR_CLIP = 300
_PROMPT_NAMES = 5

# (verb, run id) pairs whose CLI call has not returned yet. The screen's data
# is stale until then, so pressing the key again would offer the same action a
# second time — for rerun that is a second billed run.
_busy: set[tuple[str, str]] = set()


def run_label(run: dict) -> str:
    """`my_experiment (01HXYZ12)`, or just the short id when unnamed."""
    short = (run.get("id") or "")[:8]
    name = run.get("name")
    return f"{name} ({short})" if name else short


def _clip(msg: str) -> str:
    msg = (msg or "").strip()
    return msg if len(msg) <= _ERR_CLIP else msg[:_ERR_CLIP] + "…"


def _spawn(screen, job) -> None:
    """Run the CLI call as an app worker. Awaiting it inside the confirm
    callback held the message pump for the whole call — a pull can take
    minutes, and no key was processed meanwhile. The worker belongs to the
    app, not the screen, so the result is still reported if the screen closes.
    """
    screen.app.run_worker(job, group="run-actions", exclusive=False)


async def _call_after(screen, after: After | None, ok: bool) -> None:
    if after is None or not screen.is_attached:
        return
    res = after(ok)
    if inspect.isawaitable(res):
        await res


async def run_and_report(
    screen,
    work: Work,
    *,
    ok_msg: str | Callable[[str], str],
    fail_prefix: str,
    start_msg: str | None = None,
    after: After | None = None,
) -> bool:
    """Run `work`, notify the outcome, then the screen's `after`."""
    # Notify through the app: the screen may be closed by the time we finish,
    # and a stop/pull result must still reach the user.
    app = screen.app
    if start_msg:
        app.notify(start_msg, severity="information")
    ok, msg = await work()
    if ok:
        text = ok_msg(msg) if callable(ok_msg) else ok_msg
        app.notify(text, severity="information")
    else:
        app.notify(f"{fail_prefix}: {_clip(msg)}", severity="error", timeout=8)
    await _call_after(screen, after, ok)
    return ok


async def confirm_and_run(
    screen,
    prompt: str,
    work: Work,
    *,
    ok_msg: str | Callable[[str], str],
    fail_prefix: str,
    default_no: bool = False,
    start_msg: str | None = None,
    after: After | None = None,
    busy_key: tuple[str, str] | None = None,
) -> None:
    def _is_busy() -> bool:
        if busy_key in _busy:
            screen.app.notify(
                f"{busy_key[0].capitalize()} already in progress for this run",
                severity="warning",
            )
            return True
        return False

    if busy_key and _is_busy():
        return

    async def _on_answer(confirmed: bool | None) -> None:
        # Checked again: two prompts can be answered one after the other.
        if not confirmed or (busy_key and _is_busy()):
            return
        if busy_key:
            _busy.add(busy_key)

        async def _job() -> None:
            try:
                await run_and_report(
                    screen, work, ok_msg=ok_msg, fail_prefix=fail_prefix,
                    start_msg=start_msg, after=after,
                )
            finally:
                if busy_key:
                    _busy.discard(busy_key)

        _spawn(screen, _job())

    await screen.app.push_screen(
        ConfirmScreen(prompt, default_no=default_no), _on_answer
    )


async def stop(screen, run: dict, *, after: After | None = None) -> None:
    # Destroys a paid instance: focus No so a stray Enter cannot confirm.
    await confirm_and_run(
        screen, f"Stop {run_label(run)}?",
        lambda: services.stop_run(run["id"]),
        ok_msg=f"Stopped {run_label(run)}", fail_prefix="Stop failed",
        default_no=True, after=after, busy_key=("stop", run["id"]),
    )


async def rerun(screen, run: dict, *, after: After | None = None) -> None:
    # A rerun starts a new billed run, so it defaults to No as well.
    await confirm_and_run(
        screen, f"Rerun {run_label(run)}?",
        lambda: services.rerun_run(run["id"]),
        ok_msg="Rerun launched", fail_prefix="Rerun failed",
        default_no=True, after=after, busy_key=("rerun", run["id"]),
    )


async def pull(screen, run: dict, *, after: After | None = None) -> None:
    await confirm_and_run(
        screen, f"Pull latest checkpoint of {run_label(run)}?",
        lambda: services.pull(run["id"], ckpt="latest"),
        ok_msg="Pull complete", fail_prefix="Pull failed",
        start_msg="Pulling latest checkpoint…", after=after,
    )


async def sync(
    screen, run: dict | None = None, *, after: After | None = None,
) -> None:
    """`xrun fix-status` for one run, or every running one when `run` is None.
    Harmless and read-mostly, so no confirmation."""
    run_id = run["id"] if run else None
    _spawn(screen, run_and_report(
        screen, lambda: services.fix_status(run_id),
        ok_msg=lambda msg: f"Sync ok: {msg.splitlines()[-1] if msg else 'no change'}",
        fail_prefix="Sync failed",
        start_msg=f"Reconciling {run_label(run)}…" if run else "Reconciling stale runs…",
        after=after,
    ))


async def _bulk(
    screen,
    runs: list[dict],
    prompt: str,
    work_one: Callable[[dict], Awaitable[tuple[bool, str]]],
    *,
    verb: str,
    default_no: bool,
    after: After | None,
) -> None:
    def _on_answer(confirmed: bool | None) -> None:
        if confirmed:
            _spawn(screen, _job())

    async def _job() -> None:
        app = screen.app
        failed = 0
        first_err = ""
        for run in runs:
            ok, msg = await work_one(run)
            if not ok:
                failed += 1
                first_err = first_err or f"{run_label(run)}: {_clip(msg)}"
        done = len(runs) - failed
        if failed:
            app.notify(
                f"{verb}: {done} ok, {failed} failed. First error: {first_err}",
                severity="error", timeout=8,
            )
        else:
            app.notify(f"{verb}: {done} ok", severity="information")
        await _call_after(screen, after, failed == 0)

    await screen.app.push_screen(
        ConfirmScreen(prompt, default_no=default_no), _on_answer
    )


def _bulk_prompt(question: str, runs: list[dict]) -> str:
    """A count alone hides which runs are meant; the list on screen may be
    filtered or on another tab, so the prompt names them."""
    names = [run_label(r) for r in runs[:_PROMPT_NAMES]]
    if len(runs) > _PROMPT_NAMES:
        names.append(f"… and {len(runs) - _PROMPT_NAMES} more")
    return question + "\n" + "\n".join(names)


async def bulk_stop(screen, runs: list[dict], *, after: After | None = None) -> None:
    await _bulk(
        screen, runs, _bulk_prompt(f"Stop {len(runs)} runs?", runs),
        lambda r: services.stop_run(r["id"]),
        verb="Stop", default_no=True, after=after,
    )


async def bulk_pull(screen, runs: list[dict], *, after: After | None = None) -> None:
    await _bulk(
        screen, runs,
        _bulk_prompt(f"Pull latest checkpoint of {len(runs)} runs?", runs),
        lambda r: services.pull(r["id"], ckpt="latest"),
        verb="Pull", default_no=False, after=after,
    )
