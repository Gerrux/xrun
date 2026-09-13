"""Рисует знак xrun и картинки для README и сайта.

Знак — плитка с вырезанной кривой потерь: крутой спад, плато и отдельная точка
в конце — лучший чекпоинт, ради которого запуск и затевался. Кривая вырезана,
а не нарисована поверх: под ней обязан быть фон, иначе на тёмной подложке
знак превращается в белую закорючку на квадрате.

Геометрия задана в поле 100×100 и продублирована в `docs/index.html` (SVG
сайта). Правите одно — правьте и другое.

    python scripts/brand.py        # нужен Pillow
"""

from __future__ import annotations

from collections.abc import Sequence
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parent.parent
DOCS = ROOT / "docs"

# Tokyo Night — та же палитра, что у TUI (python/xrun_tui/src/xrun_tui/app.tcss).
BG = (0x1A, 0x1B, 0x26)
SURFACE = (0x24, 0x28, 0x3B)
TEXT = (0xC0, 0xCA, 0xF5)
MUTED = (0x56, 0x5F, 0x89)
BLUE = (0x7A, 0xA2, 0xF7)
MAGENTA = (0xBB, 0x9A, 0xF7)
# Цвета статусов — STATUS_DOT в python/xrun_tui/src/xrun_tui/utils.py.
STATES = [
    ((0xE0, 0xAF, 0x68), "provisioning"),
    ((0x9E, 0xCE, 0x6A), "running"),
    ((0x56, 0x5F, 0x89), "done"),
    ((0xF7, 0x76, 0x8E), "failed"),
]

# Геометрия знака в поле 100×100.
RADIUS = 22
CURVE = [(20, 24), (28, 62), (44, 70), (64, 70)]  # кубическая Безье
STROKE = 11
DOT = (80, 70, 7.5)


def _bezier(p: Sequence[tuple[float, float]], n: int = 256) -> list[tuple[float, float]]:
    (x0, y0), (x1, y1), (x2, y2), (x3, y3) = p
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append(
            (
                u**3 * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t**3 * x3,
                u**3 * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t**3 * y3,
            )
        )
    return out


def _gradient(size: int) -> Image.Image:
    """Диагональ BLUE → MAGENTA из левого верхнего угла в правый нижний."""
    img = Image.new("RGB", (size, size))
    px = img.load()
    assert px is not None
    for y in range(size):
        for x in range(size):
            t = (x + y) / (2 * (size - 1))
            px[x, y] = tuple(round(a + (b - a) * t) for a, b in zip(BLUE, MAGENTA))
    return img


def mark(size: int, fill: tuple[int, int, int] | None = None) -> Image.Image:
    """Знак на прозрачном фоне. fill=None — фирменная диагональ."""
    ss = 4  # суперсэмплинг, чтобы кромка кривой не шла лесенкой
    big = size * ss
    k = big / 100
    mask = Image.new("L", (big, big), 0)
    d = ImageDraw.Draw(mask)
    d.rounded_rectangle((0, 0, big - 1, big - 1), radius=RADIUS * k, fill=255)
    pts = [(x * k, y * k) for x, y in _bezier(CURVE)]
    w = STROKE * k
    # Штамп кругами вдоль кривой, а не line(): у line() на изгибе рвутся
    # стыки сегментов, и вырез идёт бахромой.
    for x, y in pts:
        d.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=0)
    cx, cy, r = DOT
    d.ellipse(((cx - r) * k, (cy - r) * k, (cx + r) * k, (cy + r) * k), fill=0)
    mask = mask.resize((size, size), Image.Resampling.LANCZOS)
    body = _gradient(size) if fill is None else Image.new("RGB", (size, size), fill)
    out = Image.new("RGBA", (size, size), (0, 0, 0, 0))
    out.paste(body, (0, 0), mask)
    return out


def _font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont | ImageFont.ImageFont:
    names = (
        ["bahnschrift.ttf", "segoeuib.ttf", "DejaVuSans-Bold.ttf"]
        if bold
        else ["segoeui.ttf", "DejaVuSans.ttf"]
    )
    for name in names:
        for base in ("C:/Windows/Fonts", "/usr/share/fonts/truetype/dejavu", ""):
            try:
                return ImageFont.truetype(str(Path(base) / name) if base else name, size)
            except OSError:
                continue
    return ImageFont.load_default()


def og() -> Image.Image:
    """Обложка для соцсетей 1200×630: локап слева, под ним строка про продукт."""
    img = Image.new("RGB", (1200, 630), BG)
    d = ImageDraw.Draw(img)
    m = mark(132)
    img.paste(m, (96, 150), m)
    d.text((260, 150), "xrun", font=_font(118, bold=True), fill=TEXT)
    d.rounded_rectangle((96, 340, 176, 344), radius=2, fill=BLUE)
    d.text((96, 372), "One YAML manifest — from GPU to checkpoint.", font=_font(40), fill=TEXT)
    d.text(
        (96, 430),
        "vast.ai · Kaggle · SSH · local  —  metrics in SQLite, budget guards, push",
        font=_font(28),
        fill=MUTED,
    )
    x = 96
    for color, label in STATES:
        d.ellipse((x, 530, x + 16, 546), fill=color)
        d.text((x + 26, 522), label, font=_font(26), fill=MUTED)
        x += 60 + round(d.textlength(label, font=_font(26)))
    return img


def sizes() -> Image.Image:
    """Размерный ряд на подложке TUI: 128, 64, 32, 16 и плоский цвет."""
    img = Image.new("RGB", (640, 200), BG)
    x = 32
    for s in (128, 64, 32, 16):
        m = mark(s)
        img.paste(m, (x, 100 - s // 2), m)
        x += s + 40
    for color in (TEXT, BLUE):
        m = mark(64, fill=color)
        img.paste(m, (x, 68), m)
        x += 104
    return img


def main() -> None:
    (DOCS / "brand").mkdir(parents=True, exist_ok=True)
    mark(256).save(DOCS / "brand" / "mark.png")
    mark(64).save(DOCS / "favicon.png")
    sizes().save(DOCS / "brand" / "sizes.png")
    og().save(DOCS / "og.png")
    print("docs/brand/mark.png docs/brand/sizes.png docs/favicon.png docs/og.png")


if __name__ == "__main__":
    main()
