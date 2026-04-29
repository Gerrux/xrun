from __future__ import annotations

import asyncio
import json
import re
import urllib.request
from collections import Counter

# Vast.ai's `geolocation` is a free-form string. Observed shapes include
# "DE, Frankfurt", "Germany, DE", "US-CA, Santa Clara", or just "Germany".
# We scan for an explicit ISO-3166 alpha-2 token (a standalone 2-letter all-caps
# word) to extract the country code reliably. The 2-char prefix heuristic
# breaks on country-name-first entries and produces invalid widget ids.
_ISO_RE = re.compile(r"\b([A-Z]{2})\b")

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Button, Checkbox, Footer, Static

from xrun_tui import config


class CountryExcludeScreen(ModalScreen[list[str] | None]):
    """Modal that fetches available vast.ai offers, groups them by country,
    and lets the user toggle which countries to exclude from offer search.
    Returns the new exclude list on save, or None on cancel.
    """

    TITLE = "xrun — exclude countries"
    BINDINGS = [
        Binding("escape,q", "cancel", "Cancel"),
        Binding("ctrl+s",   "save",   "Save"),
        Binding("r,f5",     "reload", "Reload"),
    ]

    def __init__(self, current: list[str]) -> None:
        super().__init__()
        # Normalize: uppercase, strip, dedup, drop empties.
        seen: set[str] = set()
        normed: list[str] = []
        for c in current:
            cc = c.strip().upper()
            if cc and cc not in seen:
                seen.add(cc)
                normed.append(cc)
        self._initial_excluded: set[str] = set(normed)

    def compose(self) -> ComposeResult:
        with Vertical(id="cx-dialog"):
            yield Static(
                "[bold #c0caf5]Exclude countries from offer search[/]",
                classes="cx-title",
            )
            yield Static(
                "[#565f89]Vast.ai geolocation is unstructured "
                "(e.g. `DE, Frankfurt` or `Germany, DE`). "
                "Match is by ISO-3166 alpha-2 code anywhere in the string.[/]",
                classes="cx-hint",
            )
            yield Static("", id="cx-status", classes="cx-hint")
            with VerticalScroll(id="cx-scroll"):
                yield Vertical(id="cx-list")
            with Horizontal(id="cx-buttons"):
                yield Button("Save  [Ctrl+S]", id="btn-cx-save",
                             variant="primary")
                yield Button("Reload  [r]",   id="btn-cx-reload")
                yield Button("Cancel  [Esc]", id="btn-cx-cancel")
        yield Footer()

    def on_mount(self) -> None:
        self.run_worker(self._reload(), exclusive=True)

    # ── Loading ──────────────────────────────────────────────────────────────

    async def _reload(self) -> None:
        status = self.query_one("#cx-status", Static)
        api_key = config.get_vast_api_key()
        if not api_key:
            status.update(
                "[#f7768e]No vast.ai API key — configure it under "
                "Vendors first.[/]"
            )
            return

        status.update("[#e0af68]Fetching offers from vast.ai…[/]")
        try:
            offers = await _fetch_offers(api_key)
        except Exception as exc:
            status.update(f"[#f7768e]Failed to fetch offers: {exc}[/]")
            return

        counts: Counter[str] = Counter()
        for o in offers:
            cc = _extract_country_code(o)
            if cc:
                counts[cc] += 1

        # Include any pre-excluded codes that don't appear in the current
        # offer sample, so users can still see and unselect them.
        for cc in self._initial_excluded:
            counts.setdefault(cc, 0)

        if not counts:
            status.update(
                "[#414868]No offers returned — cannot enumerate countries. "
                "Type codes manually in Settings instead.[/]"
            )
            return

        sorted_codes = sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))
        total = sum(c for _, c in sorted_codes)
        excluded_known = sum(
            1 for cc, _ in sorted_codes if cc in self._initial_excluded
        )
        status.update(
            f"[#565f89]{len(sorted_codes)} countries, {total} offers — "
            f"{excluded_known} currently excluded[/]"
        )

        # Rebuild the checkbox list.
        list_box = self.query_one("#cx-list", Vertical)
        await list_box.remove_children()
        for cc, n in sorted_codes:
            label = f"{cc}  ({n} offers)" if n else f"{cc}  (not in current set)"
            await list_box.mount(
                Checkbox(label,
                         value=(cc in self._initial_excluded),
                         id=f"cx-{cc}"),
            )

    # ── Actions ──────────────────────────────────────────────────────────────

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-cx-save":   self.action_save()
            case "btn-cx-reload": self.run_worker(self._reload(), exclusive=True)
            case "btn-cx-cancel": self.action_cancel()

    def action_save(self) -> None:
        chosen: list[str] = []
        for cb in self.query(Checkbox):
            if cb.value and cb.id and cb.id.startswith("cx-"):
                chosen.append(cb.id.removeprefix("cx-"))
        chosen.sort()
        self.dismiss(chosen)

    def action_cancel(self) -> None:
        self.dismiss(None)

    def action_reload(self) -> None:
        self.run_worker(self._reload(), exclusive=True)


def _extract_country_code(offer: dict) -> str | None:
    """Best-effort ISO-3166 alpha-2 code from a vast.ai offer."""
    # Some payloads include an explicit field — prefer it.
    for key in ("country", "country_code", "geolocation_code"):
        v = offer.get(key)
        if isinstance(v, str):
            m = _ISO_RE.search(v.upper())
            if m:
                return m.group(1)
    geo = offer.get("geolocation")
    if isinstance(geo, str):
        m = _ISO_RE.search(geo.upper())
        if m:
            return m.group(1)
    return None


# ── Fetch helper ──────────────────────────────────────────────────────────────

async def _fetch_offers(api_key: str) -> list[dict]:
    """POST /bundles/ with a permissive query to enumerate available offers.
    Vast.ai's API requires at least the standard verified/rentable filters;
    GPU type is intentionally omitted to get a wide cross-section."""
    body = json.dumps({
        "verified":   {"eq": True},
        "external":   {"eq": False},
        "rentable":   {"eq": True},
        "rented":     {"eq": False},
        "type":       "on-demand",
        "order":      [["score", "desc"]],
        "allocated_storage": 5.0,
        # Default `/bundles/` page is ~64 offers, far below the real catalog
        # — bump it so the country histogram reflects the whole market.
        "limit":      1024,
    }).encode("utf-8")

    def _do() -> list[dict]:
        req = urllib.request.Request(
            "https://console.vast.ai/api/v0/bundles/",
            data=body,
            method="POST",
            headers={
                "Authorization": f"Bearer {api_key}",
                "Content-Type":  "application/json",
            },
        )
        with urllib.request.urlopen(req, timeout=20) as r:
            payload = json.loads(r.read())
        return payload.get("offers") or []

    return await asyncio.to_thread(_do)
