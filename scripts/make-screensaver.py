#!/usr/bin/env python3
"""Generate pixel-hacker Golden Eagle screensaver artworks for Hayato.

Produces three ultra-minimal half-block pixel-art screensavers in
desktop/uis/sway/files/home/.config/hayato/:
  - screensaver.txt       : Heraldic spread-wings Golden Eagle crest + Hayato _
  - screensaver-dive.txt  : Diving / swooping Golden Eagle + Hayato _
  - screensaver-head.txt  : High-detail Golden Eagle head profile + Hayato _

Each artwork uses uniform-width lines (required by hayato-screensaver's
line-by-line centering and tte's canvas measurement) and pairs the eagle
with the exact same 5x8 pixel `Hayato _` wordmark used by Plymouth and
hayato-greeter.

Pure Python and import-safe: generation only runs under main().
"""

from __future__ import annotations

from pathlib import Path

OUT_DIR = (
    Path(__file__).resolve().parent.parent
    / "uis"
    / "sway"
    / "files"
    / "home"
    / ".config"
    / "hayato"
)

# Exact 5x8 pixel wordmark bitmaps shared with Plymouth and hayato-greeter
WORD = "Hayato"
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
CURSOR_BITMAP: tuple[str, ...] = (
    "....",
    "....",
    "....",
    "....",
    "....",
    "....",
    "####",
    "....",
)

# 1. Heraldic Spread-Wings Golden Eagle Crest (48 x 26 px -> 48 x 13 rows)
EAGLE_CREST: tuple[str, ...] = (
    ".....................######.....................",
    "....................########....................",
    "...................###.#######..................",
    ".####..............###########.............####.",
    ".#######...........########..#..........#######.",
    "..#########.........######...........#########..",
    "....###########.....######.......###########....",
    ".##################.######.###################..",
    "..###########################################...",
    "....#######################################.....",
    "..###########.##################.###########....",
    "....#########..################..#########......",
    "......#######...##############...#######........",
    "........#####....############....#####..........",
    "..........###.....##########.....###............",
    "...................########.....................",
    "..................###.##.###....................",
    ".................##..####..##...................",
    ".....................####.......................",
    "....................######......................",
    "...................########.....................",
    "..................##########....................",
    ".................###.####.###...................",
    "................##...####...##..................",
    ".....................####.......................",
    "................................................",
)

# 2. Diving / Swooping Golden Eagle (48 x 24 px -> 48 x 12 rows)
EAGLE_DIVE: tuple[str, ...] = (
    ".........###..####..............................",
    ".......#####.######.............................",
    "......##############............................",
    ".....################...........................",
    "....##################..........................",
    "....###################.........................",
    ".....###################........................",
    ".....###################.....######.............",
    "....#####################...########............",
    "....######################.####.######..........",
    ".....##################################.........",
    "......##################################........",
    ".......#############################..##........",
    "........########################................",
    "....############################................",
    "..##############################................",
    ".############....############...................",
    "....######........##########....................",
    "..................####..####....................",
    ".................###.....###....................",
    "................###.......####..................",
    "...............##.#.......##.##.................",
    "..............#...#........#..#.................",
    "................................................",
)

# 3. Cyber-Raptor Golden Eagle Head Profile (40 x 26 px -> 40 x 13 rows)
EAGLE_HEAD: tuple[str, ...] = (
    ".............############...............",
    "..........###################...........",
    "........#######################.........",
    "......###########################.......",
    ".....#############################......",
    "....##############################......",
    "...###########........#############.....",
    "..###########..#####...#############....",
    "..##########..###.###...#############...",
    ".###########...#####.....#############..",
    ".###############........###############.",
    ".######################################.",
    "..######################........#######.",
    "...####################...........#####.",
    "....###################............####.",
    "...###################..............###.",
    "..####################...............#..",
    ".#####################..................",
    "####.################...................",
    "###..###############....................",
    "....###.###########.....................",
    "...##...##########......................",
    "........###.#####.......................",
    ".......##...####........................",
    "............###.........................",
    "........................................",
)


def to_half_blocks(bitmap: tuple[str, ...]) -> list[str]:
    """Convert a '#' / '.' pixel grid into uniform-width Unicode half-block rows."""
    width = len(bitmap[0])
    if any(len(row) != width for row in bitmap):
        raise ValueError("all bitmap rows must have the same width")
    padded = list(bitmap)
    if len(padded) % 2 == 1:
        padded.append("." * width)
    out: list[str] = []
    for y in range(0, len(padded), 2):
        top = padded[y]
        bot = padded[y + 1]
        chars = []
        for a, b in zip(top, bot):
            chars.append({"##": "\u2588", "#.": "\u2580", ".#": "\u2584"}.get(a + b, " "))
        out.append("".join(chars))
    return out


def wordmark_half_blocks() -> list[str]:
    """Render the 40x8 pixel `Hayato _` wordmark into 4 half-block rows."""
    rows = [""] * 8
    for idx, ch in enumerate(WORD):
        bm = LETTER_BITMAPS[ch]
        for r in range(8):
            rows[r] += bm[r] + "."
    for r in range(8):
        rows[r] += CURSOR_BITMAP[r]
    return to_half_blocks(tuple(rows))


def compose_screensaver(eagle_bitmap: tuple[str, ...]) -> str:
    """Compose centered eagle + blank separator + pixel `Hayato _` wordmark."""
    eagle = to_half_blocks(eagle_bitmap)
    wordmark = wordmark_half_blocks()
    width = max(len(eagle[0]), len(wordmark[0]))

    def center_line(line: str) -> str:
        pad = width - len(line)
        left = pad // 2
        right = pad - left
        return (" " * left) + line + (" " * right)

    lines = [center_line(line) for line in eagle]
    lines.append(" " * width)
    lines.extend(center_line(line) for line in wordmark)
    return "\n".join(lines) + "\n"


VARIANTS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("screensaver.txt", EAGLE_CREST),
    ("screensaver-dive.txt", EAGLE_DIVE),
    ("screensaver-head.txt", EAGLE_HEAD),
)


def generate_all(out_dir: Path = OUT_DIR) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    written: list[Path] = []
    for filename, bitmap in VARIANTS:
        p = out_dir / filename
        p.write_text(compose_screensaver(bitmap), encoding="utf-8")
        written.append(p)
    return written


def main() -> None:
    for p in generate_all():
        print(f"=== {p.name} ===")
        print(p.read_text(encoding="utf-8"))


if __name__ == "__main__":
    main()
