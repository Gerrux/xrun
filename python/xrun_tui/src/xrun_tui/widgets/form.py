"""Shared behaviour of the forms that save config through `xrun config`:
one save at a time, and a "discard unsaved changes?" prompt on Esc."""
from __future__ import annotations

import functools
from typing import Any

from textual.widgets import Input, RadioSet, Select


def single_save(fn):
    """Decorator for a form's `async def action_save`: a second call while one
    is running (double Ctrl+S, or button + Ctrl+S) is refused. Two runs would
    interleave their `xrun config` read-modify-write cycles on the same file."""

    @functools.wraps(fn)
    async def wrapper(self, *args, **kwargs):
        if self._saving:
            self.notify("Save already in progress", severity="warning")
            return None
        self._saving = True
        try:
            return await fn(self, *args, **kwargs)
        finally:
            self._saving = False

    return wrapper


class FormGuard:
    """Mix in before `Screen` on a config form.

    Esc / Back (`action_go_back`) leaves at once when no Input / Select /
    RadioSet differs from the baseline, else asks first. The baseline is not
    taken automatically: call `snapshot_form()` once the fields hold their
    prefilled values, and again after a successful save. Until then nothing is
    dirty. Secret Inputs start blank by design, so blank is their baseline.

    `_leave()` is how the form goes away (default: pop the screen); forms that
    hand a result back override it. `_FORM_IGNORE` lists widget ids that are
    not part of what Save writes (a scratch field, a "clean up" box).
    """

    _saving = False
    _baseline: dict[str, Any] | None = None
    _FORM_IGNORE: frozenset[str] = frozenset()

    def _form_values(self) -> dict[str, Any]:
        out: dict[str, Any] = {}
        for w in self.query(Input):  # type: ignore[attr-defined]
            if w.id and w.id not in self._FORM_IGNORE:
                out[w.id] = w.value
        for w in self.query(Select):  # type: ignore[attr-defined]
            if w.id and w.id not in self._FORM_IGNORE:
                out[w.id] = w.value
        for w in self.query(RadioSet):  # type: ignore[attr-defined]
            if w.id and w.id not in self._FORM_IGNORE:
                out[w.id] = w.pressed_index
        return out

    def snapshot_form(self, only: set[str] | None = None) -> None:
        """Remember the current values as "unchanged". With `only` (widget
        ids), refresh just those entries — for fields filled in later, such as
        an asynchronous prefill, without forgetting edits made elsewhere."""
        if not self.is_attached:  # type: ignore[attr-defined]
            return
        values = self._form_values()
        if only is None or self._baseline is None:
            self._baseline = values
        else:
            self._baseline.update({k: v for k, v in values.items() if k in only})

    def form_dirty(self) -> bool:
        return self._baseline is not None and self._form_values() != self._baseline

    def _leave(self) -> None:
        self.app.pop_screen()  # type: ignore[attr-defined]

    def action_go_back(self) -> None:
        if self._saving:
            # Leaving mid-save is refused. A save that finishes under the
            # discard prompt would close the prompt instead of the form
            # (`dismiss` / `pop_screen` take the top screen), and leaving
            # half-way cut the SSH form's writes short (host without user).
            self.notify("Save in progress — wait for it to finish",  # type: ignore[attr-defined]
                        severity="warning")
            return
        if not self.form_dirty():
            self._leave()
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        def _after(discard: bool | None) -> None:
            if discard:
                self._leave()

        self.app.push_screen(  # type: ignore[attr-defined]
            ConfirmScreen("Discard unsaved changes?", default_no=True), _after
        )
