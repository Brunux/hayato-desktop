#!/usr/bin/env python3
"""Generate PNG sprite assets for the Hayato Plymouth boot splash theme.

Outputs into desktop/uis/sway/files/usr/share/plymouth/themes/hayato/:
  - letter-0.png .. letter-5.png : H, a, y, a, t, o in #c0caf5 (JetBrains Mono Bold)
  - cursor.png                   : #ff9e64 Hayato orange underscore bar
  - bar.png                      : #7dcfff cyan soft-ended gliding accent bar

Import-safe: generation only runs under main().
"""

from __future__ import annotations

import math
import shutil
import struct
import subprocess
import zlib
from pathlib import Path

WORD = "Hayato"
# Tokyo Night / Hayato brand palette (matches desktop/AGENTS.md rule 22)
BG_HEX = "#1a1b26"
FG_HEX = "#c0caf5"
CURSOR_RGB = (0xFF, 0x9E, 0x64)  # #ff9e64 signature orange
BAR_RGB = (0x7D, 0xCF, 0xFF)     # #7dcfff eagle cyan

LETTER_W = 54
LETTER_H = 92
LETTER_FONT_SIZE = 72
LETTER_BASELINE_Y = 70

CURSOR_W = 38
CURSOR_H = 9

BAR_W = 112
BAR_H = 4

FONT_CANDIDATES = (
    "/usr/share/fonts/truetype/jetbrains-mono/JetBrainsMono-Bold.ttf",
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
)

THEME_DIR = (
    Path(__file__).resolve().parent.parent
    / "uis"
    / "sway"
    / "files"
    / "usr"
    / "share"
    / "plymouth"
    / "themes"
    / "hayato"
)


def _png_chunk(tag: bytes, data: bytes) -> bytes:
    crc = zlib.crc32(tag + data) & 0xFFFFFFFF
    return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", crc)


def encode_rgba_png(width: int, height: int, rows: list[bytes]) -> bytes:
    """Encode raw RGBA scanlines (each len == width * 4) into a valid PNG."""
    if len(rows) != height:
        raise ValueError(f"expected {height} rows, got {len(rows)}")
    raw = bytearray()
    for row in rows:
        if len(row) != width * 4:
            raise ValueError(f"expected row of {width * 4} bytes, got {len(row)}")
        raw.append(0)  # filter type 0 (None)
        raw.extend(row)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw), level=9)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", idat)
        + _png_chunk(b"IEND", b"")
    )


def render_cursor_png(
    width: int = CURSOR_W,
    height: int = CURSOR_H,
    rgb: tuple[int, int, int] = CURSOR_RGB,
) -> bytes:
    """Render a crisp rounded-pill underscore cursor in RGBA PNG format."""
    r, g, b = rgb
    radius = (height - 1) / 2.0
    rows: list[bytes] = []
    for y in range(height):
        row = bytearray()
        dy = y - radius
        for x in range(width):
            if x < radius:
                dx = radius - x
            elif x > width - 1 - radius:
                dx = x - (width - 1 - radius)
            else:
                dx = 0.0
            dist = math.hypot(dx, dy) - radius
            # 1px anti-aliased edge
            coverage = max(0.0, min(1.0, 0.5 - dist))
            alpha = int(round(coverage * 255))
            row.extend((r, g, b, alpha))
        rows.append(bytes(row))
    return encode_rgba_png(width, height, rows)


def render_bar_png(
    width: int = BAR_W,
    height: int = BAR_H,
    rgb: tuple[int, int, int] = BAR_RGB,
) -> bytes:
    """Render a soft-ended cyan gliding accent bar in RGBA PNG format."""
    r, g, b = rgb
    mid_x = (width - 1) / 2.0
    mid_y = (height - 1) / 2.0
    rows: list[bytes] = []
    for y in range(height):
        row = bytearray()
        # Subtle vertical softness on top/bottom edges
        vy = 1.0 - 0.25 * (abs(y - mid_y) / max(mid_y, 1.0))
        for x in range(width):
            # Smooth raised-cosine taper at both ends
            nx = abs(x - mid_x) / max(mid_x, 1.0)
            hx = 0.5 * (1.0 + math.cos(math.pi * min(1.0, nx)))
            alpha = int(round(max(0.0, min(1.0, hx * vy)) * 230))
            row.extend((r, g, b, alpha))
        rows.append(bytes(row))
    return encode_rgba_png(width, height, rows)


def find_font() -> str:
    for candidate in FONT_CANDIDATES:
        if Path(candidate).is_file():
            return candidate
    raise FileNotFoundError(
        f"no monospace bold font found in {', '.join(FONT_CANDIDATES)}"
    )


def render_letter_png(letter: str, out_path: Path, font_path: str) -> None:
    """Render a single baseline-aligned glyph onto a transparent RGBA PNG via ffmpeg."""
    ffmpeg = shutil.which("ffmpeg")
    if not ffmpeg:
        raise FileNotFoundError("ffmpeg is required to render Plymouth letter sprites")
    # Escape drawtext special chars if ever extended
    safe_text = letter.replace("\\", "\\\\").replace(":", "\\:").replace("'", "\\'")
    vf = (
        f"format=rgba,"
        f"drawtext=fontfile={font_path}:"
        f"text='{safe_text}':"
        f"fontcolor={FG_HEX}:"
        f"fontsize={LETTER_FONT_SIZE}:"
        f"x=(w-text_w)/2:"
        f"y={LETTER_BASELINE_Y}-ascent"
    )
    cmd = [
        ffmpeg,
        "-y",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        f"color=c=black@0.0:s={LETTER_W}x{LETTER_H}:d=1",
        "-vf",
        vf,
        "-frames:v",
        "1",
        "-pix_fmt",
        "rgba",
        str(out_path),
    ]
    subprocess.run(cmd, check=True)


def generate_theme_assets(out_dir: Path = THEME_DIR) -> list[Path]:
    """Generate all PNG sprite assets for the Hayato Plymouth theme."""
    out_dir.mkdir(parents=True, exist_ok=True)
    font_path = find_font()
    written: list[Path] = []
    for idx, ch in enumerate(WORD):
        p = out_dir / f"letter-{idx}.png"
        render_letter_png(ch, p, font_path)
        written.append(p)
    cursor_path = out_dir / "cursor.png"
    cursor_path.write_bytes(render_cursor_png())
    written.append(cursor_path)

    bar_path = out_dir / "bar.png"
    bar_path.write_bytes(render_bar_png())
    written.append(bar_path)
    return written


def main() -> None:
    files = generate_theme_assets()
    for p in files:
        print(f"wrote {p.relative_to(THEME_DIR.parent.parent.parent.parent.parent.parent)} ({p.stat().st_size} B)")


if __name__ == "__main__":
    main()
