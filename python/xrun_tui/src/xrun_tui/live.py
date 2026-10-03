"""Base screen for anything that loads or polls data.

Two rules the plain `Screen` does not enforce, and that every data screen
used to get wrong in its own way:

1. A refresh never runs on the screen's message pump. Awaiting a DB query, a
   vast.ai request or `xrun doctor` inside a timer callback (or a
   `call_after_refresh`) holds every key press queued behind it — on the
   Instances and Doctor screens that was seconds, long enough for a `g …`
   chord to expire unnoticed.
2. A screen that is not on top does not poll. Screens stay mounted while
   another one covers them; without this each visited screen kept hitting
   the DB / network on its own timer for the rest of the session.
"""
from __future__ import annotations

import inspect
from typing import Any, Callable

from textual.screen import Screen
from textual.timer import Timer
from textual.worker import Worker


class LiveScreen(Screen):
    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        self._live_timers: list[Timer] = []
        self._live_workers: dict[str, Worker] = {}

    def kick(self, fn: Callable[[], Any]) -> None:
        """Run async `fn` on a worker.

        A call made while the previous run of the same function is still in
        flight is dropped: the result on its way is as fresh as a second one
        would be, and a slow backend must not pile up requests.
        """
        name = getattr(fn, "__name__", repr(fn))
        prev = self._live_workers.get(name)
        if prev is not None and not prev.is_finished:
            return
        self._live_workers[name] = self.run_worker(
            fn(), group=f"live:{name}", exclusive=False,
        )

    def set_interval(  # type: ignore[override]
        self,
        interval: float,
        callback: Callable[[], Any] | None = None,
        **kwargs: Any,
    ) -> Timer:
        tick = callback
        if callback is not None and inspect.iscoroutinefunction(callback):
            target = callback
            tick = lambda: self.kick(target)  # noqa: E731

        timer = super().set_interval(interval, tick, **kwargs)
        # Stopped timers are dropped here rather than tracked explicitly —
        # pausing one is harmless, the list just must not grow per tab switch.
        self._live_timers = [
            t for t in self._live_timers if not _is_stopped(t)
        ] + [timer]
        return timer

    def on_screen_suspend(self) -> None:
        for timer in self._live_timers:
            timer.pause()

    def on_screen_resume(self) -> None:
        # A timer that missed a tick while paused fires as soon as it is
        # resumed, so a screen uncovered after a while refreshes at once and
        # one uncovered a second later does not refetch what it just showed.
        for timer in self._live_timers:
            timer.resume()


def _is_stopped(timer: Timer) -> bool:
    task = getattr(timer, "_task", None)
    return task is not None and task.done()
