"""LiveScreen: refreshes stay off the message pump and stop while the screen
is covered. Both regressed silently before — the symptom was a TUI that
answered keys seconds late after a few `g …` hops.
"""
from __future__ import annotations

import asyncio

from textual.app import App
from textual.screen import Screen

from xrun_tui.live import LiveScreen


class _Probe(LiveScreen):
    def __init__(self) -> None:
        super().__init__()
        self.started = 0
        self.finished = 0
        self.gate = asyncio.Event()

    def on_mount(self) -> None:
        self.set_interval(0.05, self._slow_refresh)

    async def _slow_refresh(self) -> None:
        self.started += 1
        await self.gate.wait()
        self.finished += 1


class _Host(App):
    pass


def test_interval_refresh_does_not_hold_the_pump_or_pile_up() -> None:
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            probe = _Probe()
            await app.push_screen(probe)
            await asyncio.sleep(0.3)  # several ticks, refresh still blocked
            # One run in flight, later ticks dropped rather than queued.
            assert probe.started == 1
            # The pump is free: it serves a queued callback (as it would a
            # key press) while the refresh hangs.
            handled = asyncio.Event()
            probe.call_later(handled.set)
            await asyncio.wait_for(handled.wait(), 1)
            probe.gate.set()
            await pilot.pause()
            await asyncio.sleep(0.15)
            assert probe.finished >= 1
            assert probe.started >= 2  # ticking again once the first is done

    asyncio.run(scenario())


def test_covered_screen_stops_polling_and_catches_up_on_return() -> None:
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            probe = _Probe()
            probe.gate.set()
            await app.push_screen(probe)
            await asyncio.sleep(0.2)
            assert probe.started >= 1

            await app.push_screen(Screen())
            await pilot.pause()
            await asyncio.sleep(0.1)  # let an in-flight tick settle
            covered = probe.started
            await asyncio.sleep(0.3)
            assert probe.started == covered

            await app.pop_screen()
            await pilot.pause()
            await asyncio.sleep(0.2)
            assert probe.started > covered

    asyncio.run(scenario())
