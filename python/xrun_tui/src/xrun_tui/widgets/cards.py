"""Shared pieces of the "list of cards with a status pill" screens: Vendors,
Sinks and Notifications."""
from __future__ import annotations

# state → pill markup. One table for all three screens; a state name means the
# same label everywhere. Notifications' own "test in flight / delivered" pair
# is `testing` / `sent` (it used to reuse `checking` / `ok` with other labels).
_PILLS: dict[str, str] = {
    "empty":    "[#c0caf5 on #414868] EMPTY [/]",
    "checking": "[#1a1b26 on #e0af68] CHECK [/]",
    "ok":       "[#1a1b26 on #9ece6a] READY [/]",
    "error":    "[#c0caf5 on #f7768e] ERROR [/]",
    "paused":   "[#1a1b26 on #7aa2f7] PAUSED [/]",
    "disabled": "[#c0caf5 on #414868] v0.8 [/]",
    "on":       "[#1a1b26 on #9ece6a] ON [/]",
    "off":      "[#1a1b26 on #7aa2f7] OFF [/]",
    "testing":  "[#1a1b26 on #e0af68] TEST [/]",
    "sent":     "[#1a1b26 on #9ece6a] ✓ SENT [/]",
    "info":     "[#c0caf5 on #414868] · [/]",
}


def pill(state: str) -> str:
    """Status pill markup. state ∈ empty, checking, ok, error, paused,
    disabled, on, off, testing, sent, info; anything else renders as EMPTY."""
    return _PILLS.get(state, _PILLS["empty"])


class CardCursor:
    """j/k / up/down over a vertical list of cards, wrapping around, with the
    active card marked by a CSS class. Mix in before `Screen` and set:

      _CARD_PREFIX  card ids are `#<prefix>-<index>`
      _CARD_COUNT   how many cards
      _ACTIVE_CLASS class that marks the active card (stylesheets select on it)

    The screen keeps its own `__init__` (`self._cursor = 0`) and its
    `next` / `prev` bindings; these are the actions they point at.
    """

    _cursor = 0
    _CARD_PREFIX = ""
    _CARD_COUNT = 0
    _ACTIVE_CLASS = "vendor-row-active"

    def _highlight(self, idx: int) -> None:
        for i in range(self._CARD_COUNT):
            try:
                row = self.query_one(f"#{self._CARD_PREFIX}-{i}")  # type: ignore[attr-defined]
            except Exception:
                continue  # card not mounted (yet)
            row.set_class(i == idx, self._ACTIVE_CLASS)
            if i == idx:
                row.scroll_visible(animate=False)  # long lists scroll with the cursor

    def action_next(self) -> None:
        self._cursor = (self._cursor + 1) % self._CARD_COUNT
        self._highlight(self._cursor)

    def action_prev(self) -> None:
        self._cursor = (self._cursor - 1) % self._CARD_COUNT
        self._highlight(self._cursor)
