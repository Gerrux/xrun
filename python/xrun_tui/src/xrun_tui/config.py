from __future__ import annotations

import json
import os
import sys
import tomllib
from pathlib import Path


def config_dir() -> Path:
    if env := os.environ.get("XRUN_CONFIG_DIR"):
        return Path(env)
    if sys.platform == "win32":
        # Match directories crate: %APPDATA%\xrun\config
        appdata = os.environ.get("APPDATA", str(Path.home()))
        return Path(appdata) / "xrun" / "config"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "xrun"
    return Path.home() / ".config" / "xrun"


# ── Credentials (credentials.toml, same format as xrun-core) ─────────────────

def read_credentials() -> dict:
    path = config_dir() / "credentials.toml"
    if not path.exists():
        return {}
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except Exception:
        return {}


def write_credentials(creds: dict) -> None:
    path = config_dir() / "credentials.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    lines: list[str] = []
    for section, values in creds.items():
        lines.append(f"[{section}]")
        for k, v in values.items():
            if v is not None:
                escaped = str(v).replace("\\", "\\\\").replace('"', '\\"')
                lines.append(f'{k} = "{escaped}"')
        lines.append("")
    path.write_text("\n".join(lines), encoding="utf-8")


def get_vast_api_key() -> str | None:
    key = read_credentials().get("vast", {}).get("api_key")
    if key:
        return key
    # Fallback: native vastai client stores key here
    native = Path.home() / ".config" / "vastai" / "vast_api_key"
    if native.exists():
        return native.read_text(encoding="utf-8").strip()
    return None


# ── TUI settings (xrun_tui_settings.json) ─────────────────────────────────────

_DEFAULTS: dict = {
    "runs_refresh_secs": 5,
    "instances_refresh_secs": 15,
    "default_vendor": "vast",
    "theme":            "tokyo-night",
}


def read_tui_settings() -> dict:
    path = config_dir() / "xrun_tui_settings.json"
    if not path.exists():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except Exception:
        return {}


def write_tui_settings(settings: dict) -> None:
    path = config_dir() / "xrun_tui_settings.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(settings, indent=2), encoding="utf-8")


def get_settings() -> dict:
    return {**_DEFAULTS, **read_tui_settings()}


def read_global_config() -> dict:
    """Parse `~/.config/xrun/config.toml` (xrun-core's GlobalConfig).

    Returns an empty dict if the file is missing or malformed; the caller then
    treats every flag as its default — including `ui.wizard_completed = false`,
    which is what triggers the first-run wizard.
    """
    path = config_dir() / "config.toml"
    if not path.exists():
        return {}
    try:
        with open(path, "rb") as f:
            return tomllib.load(f)
    except Exception:
        return {}


def wizard_pending() -> bool:
    """True when the first-run wizard should auto-launch."""
    cfg = read_global_config()
    return not bool(cfg.get("ui", {}).get("wizard_completed", False))
