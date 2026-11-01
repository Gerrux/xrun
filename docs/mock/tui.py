"""Снимает настоящий TUI xrun на демо-базе для README и сайта.

База собирается из миграций `crates/xrun-core/src/store/migrations` и
заполняется демо-запусками: те же имена, что в `index.html`, никаких реальных
проектов, путей и хостов. TUI запускается без терминала (`App.run_test`), с
конфигом, данными и домашней папкой во временном каталоге, так что ни база,
ни настройки, ни ключи человека не трогаются. Textual отдаёт SVG, headless
Chrome переводит его в PNG.

    python docs/mock/tui.py            # → docs/screenshots/tui-*.png

Chrome ищется по стандартному пути; другой — через переменную CHROME.
"""
from __future__ import annotations

import asyncio
import json
import math
import os
import random
import re
import sqlite3
import subprocess
import sys
import tempfile
from datetime import datetime, timedelta, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
OUT = REPO / "docs" / "screenshots"
MIGRATIONS = REPO / "crates" / "xrun-core" / "src" / "store" / "migrations"
SIZE = (132, 30)
SCALE = 2
CHROME_PATHS = (
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
)

CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
rnd = random.Random(20261101)
NOW = datetime.now(timezone.utc).replace(microsecond=0)


def ulid(ts: datetime) -> str:
    t, time = int(ts.timestamp() * 1000), ""
    for _ in range(10):
        time = CROCKFORD[t % 32] + time
        t //= 32
    return time + "".join(rnd.choice(CROCKFORD) for _ in range(16))


def iso(ts: datetime) -> str:
    # Как пишет rusqlite из chrono: "2026-11-01 14:20:12.022679700+00:00".
    return ts.strftime("%Y-%m-%d %H:%M:%S.%f") + "000+00:00"


# name, vendor, status, started N minutes ago, minutes of work, gpu, $/hr, epochs done
RUNS = [
    ("resnet50-cifar", "vast", "running", 38, None, "RTX 4090", 0.41, 5),
    ("vit-small-lr3e4", "vast", "running", 71, None, "RTX 3090", 0.22, 9),
    ("bert-base-ft", "kaggle", "running", 112, None, "P100", None, 3),
    ("llama-lora-8b", "lightning", "provisioning", 2, None, "L4", None, 0),
    ("unet-seg-v2", "ssh", "failed", 13 * 60, 41, "A6000", None, 2),
    ("whisper-tiny-ft", "colab", "done", 16 * 60, 95, "T4", None, 6),
    ("resnet50-cifar", "vast", "done", 26 * 60, 84, "RTX 4090", 0.39, 8),
    ("gbm-baseline", "local", "done", 29 * 60, 12, None, None, 8),
    ("vit-small-lr1e3", "vast", "done", 31 * 60, 66, "RTX 3090", 0.21, 10),
    ("bert-base-ft", "kaggle", "failed", 33 * 60, 7, "P100", None, 0),
    ("resnet18-cifar", "ssh", "done", 44 * 60, 52, "A6000", None, 12),
    ("quickstart", "local", "done", 50 * 60, 1, None, None, 3),
]
STEPS_PER_EPOCH = 60


def curves(epochs: int, seed: float) -> tuple[list[tuple[int, float]], list[tuple[int, float]]]:
    """train_loss every step, val_f1 once per epoch (at its last step)."""
    n = epochs * STEPS_PER_EPOCH
    loss = [(s, 0.45 + 3.4 * math.exp(-s / (70 + 40 * seed)) + rnd.uniform(-0.05, 0.05)) for s in range(n)]
    f1 = [((e + 1) * STEPS_PER_EPOCH - 1, 0.86 - 0.5 * math.exp(-(e + 1) / (1.8 + seed)) + rnd.uniform(-0.008, 0.008))
          for e in range(epochs)]
    return loss, f1


def build_db(path: Path) -> str:
    db = sqlite3.connect(path)
    for sql in sorted(MIGRATIONS.glob("*.sql")):
        db.executescript(sql.read_text(encoding="utf-8"))
    hero = ""
    for i, (name, vendor, status, ago, took, gpu, dph, epochs) in enumerate(RUNS):
        created = NOW - timedelta(minutes=ago, seconds=rnd.randint(5, 50))
        started = None if status == "provisioning" else created + timedelta(seconds=rnd.randint(40, 90))
        ended = (started or created) + timedelta(minutes=took) if took else None
        rid = ulid(created)
        hero = hero or rid
        inst = f"{rnd.randint(20_000_000, 29_999_999)}" if vendor == "vast" else f"{vendor}-{rid}"
        hours = ((ended or NOW) - (started or created)).total_seconds() / 3600
        cost = round(dph * hours, 4) if dph else None
        db.execute(
            "INSERT INTO runs (id, name, manifest_hash, manifest_path, vendor, instance_id, status,"
            " created_at, started_at, ended_at, cost_usd, cost_usd_estimate, poller_pid, poller_heartbeat_at)"
            " VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            (rid, name, f"{rnd.getrandbits(64):016x}", f"exp/{name}.yaml", vendor, inst, status,
             iso(created), started and iso(started), ended and iso(ended),
             cost if ended else None, cost,
             # A live pid, or the TUI marks the run stale: this very process.
             os.getpid() if status in ("running", "provisioning") else None,
             iso(NOW - timedelta(seconds=3)) if status in ("running", "provisioning") else None),
        )
        db.execute(
            "INSERT INTO instances (id, vendor, run_id, gpu_type, price_per_hour, created_at, destroyed_at,"
            " state_json, max_cost_usd, accumulated_cost, last_active_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (inst, vendor, rid, gpu, dph, iso(created), ended and iso(ended + timedelta(seconds=20)),
             json.dumps({"id": inst, "vendor": vendor, "ssh_host": "203.0.113.24" if vendor in ("vast", "ssh") else None,
                         "ssh_port": 41022 if vendor == "vast" else 22, "ssh_user": "root"}),
             5.0 if vendor == "vast" else None, cost or 0.0, iso(NOW - timedelta(seconds=20))),
        )
        events = [("provision", "ok", None)]
        if started:
            events += [("upload", "start", None), ("upload", "ok", None), ("train_start", "ok", None),
                       ("setup", "start", None), ("setup", "ok", None), ("train", "start", None)]
        events += [("epoch", "ok", None)] * epochs
        if status == "failed":
            events += [("train", "fail", "CUDA out of memory. Tried to allocate 2.00 GiB")]
        if status == "done":
            events += [("train", "ok", None), ("done", "ok", None)]
        span = ((ended or NOW) - created).total_seconds()
        for n, (stage, st, msg) in enumerate(events):
            ts = created + timedelta(seconds=span * (n + 1) / (len(events) + 1))
            db.execute("INSERT INTO events (run_id, ts, stage, status, msg) VALUES (?,?,?,?,?)",
                       (rid, iso(ts), stage, st, msg))
        loss, f1 = curves(epochs, i * 0.15)
        total = max(epochs * STEPS_PER_EPOCH, 1)
        for key, series in (("train_loss", loss), ("val_f1", f1)):
            for step, value in series:
                ts = iso(created + timedelta(seconds=span * (step + 1) / (total + 1)))
                db.execute("INSERT INTO metrics VALUES (?,?,?,?,?)", (rid, step, key, round(value, 4), ts))
    db.commit()
    db.close()
    return hero


async def shoot(hero: str, svg_dir: Path) -> None:
    from xrun_tui.app import XrunApp
    from xrun_tui.screens.dashboard import DashboardScreen
    from xrun_tui.screens.runs import RunsScreen
    from xrun_tui.screens.run_detail import RunDetailScreen

    app = XrunApp()
    async with app.run_test(size=SIZE) as pilot:
        await pilot.pause(1.5)
        # The status bar shows the vast account from this cache: a demo one.
        app._vast_status_cache = {"vast_user": "demo", "vast_credit": 18.42,
                                  "username": "demo", "credit": 18.42}
        # name, screen, rows (fit the content), key to press after load
        shots = (("dashboard", DashboardScreen(), 30, None), ("runs", RunsScreen(), 23, None),
                 ("run-detail", RunDetailScreen(hero), 36, "4"))
        for name, screen, rows, key in shots:
            await pilot.resize_terminal(SIZE[0], rows)
            await app.switch_screen(screen)
            await pilot.pause(2.5)
            if key:
                await pilot.press(key)
                await pilot.pause(1.5)
            app.save_screenshot(f"tui-{name}.svg", str(svg_dir))


def to_png(svg: Path, png: Path) -> None:
    chrome = os.environ.get("CHROME") or next((p for p in CHROME_PATHS if Path(p).exists()), None)
    if not chrome:
        sys.exit("Chrome not found: set CHROME to its executable")
    _, _, w, h = (float(v) for v in re.search(r'viewBox="([^"]+)"', svg.read_text(encoding="utf-8")).group(1).split())
    subprocess.run(
        # Transparent page: the window's rounded corners would be white otherwise.
        [chrome, "--headless=new", "--hide-scrollbars", "--default-background-color=00000000",
         f"--force-device-scale-factor={SCALE}",
         f"--window-size={math.ceil(w)},{math.floor(h)}", f"--screenshot={png}", svg.as_uri()],
        check=True, capture_output=True, cwd=tempfile.gettempdir(),
    )


def main() -> None:
    sys.path.insert(0, str(REPO / "python" / "xrun_tui" / "src"))
    tmp = Path(tempfile.mkdtemp(prefix="xrun-tui-shots-"))
    # Home goes to the temp dir too: the TUI falls back to keys found there
    # (~/.config/vastai, ~/.kaggle, ~/.ssh) and would call the real APIs.
    os.environ.update(XRUN_DATA_DIR=str(tmp), XRUN_CONFIG_DIR=str(tmp), XRUN_TUI_NO_RESUME="1",
                      HOME=str(tmp), USERPROFILE=str(tmp), APPDATA=str(tmp))
    from xrun_tui import config, services
    config.get_vast_api_key = lambda: None

    # `xrun doctor` against the empty temp config fails on missing keys; the
    # picture shows a set-up machine.
    async def doctor():
        return True, [{"check": "demo", "category": "core", "status": "ok"}], ""
    services.doctor = doctor
    hero = build_db(tmp / "runs.db")
    asyncio.run(shoot(hero, tmp))
    for svg in sorted(tmp.glob("tui-*.svg")):
        to_png(svg, OUT / f"{svg.stem}.png")
        print(OUT / f"{svg.stem}.png")


if __name__ == "__main__":
    main()
