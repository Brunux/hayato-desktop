#!/usr/bin/env python3
"""Generate crisp pixel-art PNG sprites for the Hayato Plymouth boot splash.

Outputs into desktop/uis/sway/files/usr/share/plymouth/themes/hayato/:
  - letter-0.png .. letter-5.png : pixel-art H, a, y, a, t, o in #c0caf5
  - glitch-0.png .. glitch-5.png : pixel-art decode glyphs in #7dcfff
  - cursor.png                   : #ff9e64 Hayato orange solid pixel block cursor
  - dot-dim.png, dot-lit.png     : 8x8 pixel scanner dots (#292e42 / #7dcfff)

Pure Python (no ffmpeg/PIL required) and import-safe: generation only runs under main().
"""

from __future__ import annotations

import struct
import zlib
from pathlib import Path

WORD = "Hayato"
# Tokyo Night / Hayato brand palette (matches desktop/AGENTS.md rule 22)
BG_HEX = "#1a1b26"
FG_RGB = (0xC0, 0xCA, 0xF5)      # #c0caf5 locked pixel letters
GLITCH_RGB = (0x7D, 0xCF, 0xFF)  # #7dcfff cyan decode/glitch glyphs
CURSOR_RGB = (0xFF, 0x9E, 0x64)  # #ff9e64 signature orange block cursor
DOT_DIM_RGB = (0x29, 0x2E, 0x42) # #292e42 dim pixel track
DOT_LIT_RGB = (0x7D, 0xCF, 0xFF) # #7dcfff active pixel scanner

PIXEL_SCALE = 8
GRID_W = 5
GRID_H = 8
LETTER_W = GRID_W * PIXEL_SCALE  # 40 px
LETTER_H = GRID_H * PIXEL_SCALE  # 64 px

CURSOR_W = 4 * PIXEL_SCALE       # 32 px
CURSOR_H = 7 * PIXEL_SCALE       # 56 px

DOT_SIZE = PIXEL_SCALE           # 8 px

# 5x8 pixel bitmaps (row 0..6 main body, row 7 descender)
LETTER_BITMAPS: dict[str, tuple[str, ...]] = {
    "H": (
        "#...#",
        "#...#",
        "#...#",
        "#####",
        "#...#",
        "#...#",
        "#...#",
        ".....",
    ),
    "a": (
        ".....",
        ".....",
        ".###.",
        "....#",
        ".####",
        "#...#",
        ".####",
        ".....",
    ),
    "y": (
        ".....",
        ".....",
        "#...#",
        "#...#",
        "#...#",
        ".####",
        "....#",
        ".###.",
    ),
    "t": (
        ".....",
        "..#..",
        "#####",
        "..#..",
        "..#..",
        "..#..",
        "...##",
        ".....",
    ),
    "o": (
        ".....",
        ".....",
        ".###.",
        "#...#",
        "#...#",
        "#...#",
        ".###.",
        ".....",
    ),
}

# 5x8 pixel decode/scramble glyphs (0, 1, #, /, =, +)
GLITCH_BITMAPS: tuple[tuple[str, ...], ...] = (
    (
        ".....",
        ".###.",
        "#..##",
        "#.#.#",
        "##..#",
        ".###.",
        ".....",
        ".....",
    ),
    (
        ".....",
        "..#..",
        ".##..",
        "..#..",
        "..#..",
        ".###.",
        ".....",
        ".....",
    ),
    (
        ".....",
        ".#.#.",
        "#####",
        ".#.#.",
        "#####",
        ".#.#.",
        ".....",
        ".....",
    ),
    (
        ".....",
        "....#",
        "...#.",
        "..#..",
        ".#...",
        "#....",
        ".....",
        ".....",
    ),
    (
        ".....",
        ".....",
        "#####",
        ".....",
        "#####",
        ".....",
        ".....",
        ".....",
    ),
    (
        ".....",
        "..#..",
        "..#..",
        "#####",
        "..#..",
        "..#..",
        ".....",
        ".....",
    ),
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
        raw.append(0)
        raw.extend(row)
    ihdr = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    idat = zlib.compress(bytes(raw), level=9)
    return (
        b"\x89PNG\r\n\x1a\n"
        + _png_chunk(b"IHDR", ihdr)
        + _png_chunk(b"IDAT", idat)
        + _png_chunk(b"IEND", b"")
    )


def render_bitmap_png(
    bitmap: tuple[str, ...],
    rgb: tuple[int, int, int],
    scale: int = PIXEL_SCALE,
    gap: int = 1,
) -> bytes:
    """Render a crisp pixel-art bitmap into an RGBA PNG with optional 1px pixel-grid gap."""
    r, g, b = rgb
    grid_h = len(bitmap)
    grid_w = len(bitmap[0])
    width = grid_w * scale
    height = grid_h * scale
    on_px = bytes((r, g, b, 255))
    off_px = b"\x00\x00\x00\x00"
    rows: list[bytes] = []
    for gy, line in enumerate(bitmap):
        if len(line) != grid_w:
            raise ValueError(f"bitmap row {gy} has width {len(line)}, expected {grid_w}")
        for sy in range(scale):
            row = bytearray()
            is_gap_y = gap > 0 and sy >= scale - gap
            for ch in line:
                lit = ch == "#" and not is_gap_y
                for sx in range(scale):
                    is_gap_x = gap > 0 and sx >= scale - gap
                    row.extend(on_px if (lit and not is_gap_x) else off_px)
            rows.append(bytes(row))
    return encode_rgba_png(width, height, rows)


def render_solid_block_png(
    width: int,
    height: int,
    rgb: tuple[int, int, int],
    alpha: int = 255,
) -> bytes:
    """Render a razor-sharp solid pixel rectangle in RGBA PNG format."""
    r, g, b = rgb
    row = bytes((r, g, b, alpha)) * width
    return encode_rgba_png(width, height, [row] * height)


def generate_theme_assets(out_dir: Path = THEME_DIR) -> list[Path]:
    """Generate all pixel-art PNG sprite assets for the Hayato Plymouth theme."""
    out_dir.mkdir(parents=True, exist_ok=True)
    # Remove legacy bar.png if present from earlier smooth theme
    legacy_bar = out_dir / "bar.png"
    if legacy_bar.exists():
        legacy_bar.unlink()

    written: list[Path] = []
    for idx, ch in enumerate(WORD):
        p = out_dir / f"letter-{idx}.png"
        p.write_bytes(render_bitmap_png(LETTER_BITMAPS[ch], FG_RGB))
        written.append(p)

    for idx, bm in enumerate(GLITCH_BITMAPS):
        p = out_dir / f"glitch-{idx}.png"
        p.write_bytes(render_bitmap_png(bm, GLITCH_RGB))
        written.append(p)

    cursor_path = out_dir / "cursor.png"
    cursor_path.write_bytes(render_solid_block_png(CURSOR_W, CURSOR_H, CURSOR_RGB))
    written.append(cursor_path)

    dot_dim_path = out_dir / "dot-dim.png"
    dot_dim_path.write_bytes(render_solid_block_png(DOT_SIZE, DOT_SIZE, DOT_DIM_RGB))
    written.append(dot_dim_path)

    dot_lit_path = out_dir / "dot-lit.png"
    dot_lit_path.write_bytes(render_solid_block_png(DOT_SIZE, DOT_SIZE, DOT_LIT_RGB))
    written.append(dot_lit_path)

    return written


def main() -> None:
    files = generate_theme_assets()
    for p in files:
        print(f"wrote {p.name} ({p.stat().st_size} B)")


if __name__ == "__main__":
    main()
