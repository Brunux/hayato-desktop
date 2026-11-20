#!/usr/bin/env python3
"""The Hayato brand, generated from one grid.

`brand/mark-48.txt` is the mark: a golden eagle, wings raised, drawn on a 48x48
grid.  Every other form comes from it, so the icon, the wallpaper, the social
card and the terminal art can never drift apart:

  mark-48.txt        48x48 cells -> the 48 px icon (1x), the 24 px icon (2x2),
                      the 16 px icon (3x3) and the block art in a terminal
  hayato.svg         the mark on its tile, merged into horizontal runs
  wallpaper.svg      1366x912: the mark at 8x, the wordmark and the tagline
  social-preview.svg 1280x640: the same composition, wider

Run `scripts/make-brand.sh`, which calls this and then renders the PNGs.
"""
import subprocess
import sys

GRID = "mark-48.txt"
TILE = "#1a1b26"
EAGLE = "#7dcfff"
SNAKE = "#ff9e64"   # kept for the idle art only, never in the mark
TEXT = "#c0caf5"
DIM = "#565f89"

WORDMARK = "Hayato_"
TAGLINE = "light. fast. fun."


def read_grid(path):
    rows = [line.rstrip("\n") for line in open(path, encoding="utf-8") if line.strip()]
    width = len(rows[0])
    if any(len(r) != width for r in rows):
        raise SystemExit(f"{path}: rows are not all {width} cells wide")
    return rows


def runs(rows, x0=0, y0=0, scale=1):
    """Horizontal runs of filled cells, as SVG rects."""
    out = []
    for y, row in enumerate(rows):
        x = 0
        while x < len(row):
            if row[x] == "#":
                run = 1
                while x + run < len(row) and row[x + run] == "#":
                    run += 1
                out.append(
                    f'<rect x="{x0 + x * scale}" y="{y0 + y * scale}" '
                    f'width="{run * scale}" height="{scale}"/>'
                )
                x += run
            else:
                x += 1
    return out


def down(rows, factor):
    """Majority sample: factor x factor cells become one, so 16 px is exact."""
    size = len(rows) // factor
    out = []
    for y in range(size):
        line = []
        for x in range(size):
            hits = sum(
                1
                for dy in range(factor)
                for dx in range(factor)
                if rows[y * factor + dy][x * factor + dx] == "#"
            )
            line.append("#" if hits * 2 >= factor * factor else ".")
        out.append("".join(line))
    return out


def svg_mark(rows, size):
    body = "\n    ".join(runs(rows))
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="512" height="512" viewBox="0 0 {size} {size}" shape-rendering="crispEdges">
  <!-- The Hayato mark: a golden eagle, wings raised.  Generated from
       brand/mark-48.txt by scripts/make-brand.py; edit the grid, not this file. -->
  <rect width="{size}" height="{size}" rx="6" fill="{TILE}"/>
  <g fill="{EAGLE}">
    {body}
  </g>
</svg>
'''


def svg_wallpaper(rows):
    scale = 8
    side = 48 * scale
    x0, y0 = 120, 140
    body = "\n    ".join(runs(rows, x0, y0, scale))
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1366" height="912" viewBox="0 0 1366 912">
  <!-- The default Hayato wallpaper: the mark at 8x, the wordmark and the
       tagline.  Generated from brand/mark-48.txt by scripts/make-brand.py. -->
  <defs>
    <linearGradient id="bg" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0" stop-color="#1a1b26"/>
      <stop offset="1" stop-color="#16161e"/>
    </linearGradient>
  </defs>
  <rect width="1366" height="912" fill="url(#bg)"/>
  <g fill="{EAGLE}" shape-rendering="crispEdges">
    {body}
  </g>
  <text x="{x0}" y="{y0 + side + 96}" font-family="JetBrains Mono" font-weight="800"
        font-size="72" fill="{TEXT}">{WORDMARK[:-1]}<tspan fill="{SNAKE}">_</tspan></text>
  <text x="{x0}" y="{y0 + side + 148}" font-family="JetBrains Mono" font-size="26"
        fill="{DIM}">{TAGLINE}</text>
</svg>
'''


def svg_social(rows):
    scale = 5
    side = 48 * scale
    x0, y0 = 96, 110
    body = "\n    ".join(runs(rows, x0, y0, scale))
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1280" height="640" viewBox="0 0 1280 640">
  <!-- Hayato's social card.  Generated from brand/mark-48.txt. -->
  <rect width="1280" height="640" fill="{TILE}"/>
  <g fill="{EAGLE}" shape-rendering="crispEdges">
    {body}
  </g>
  <text x="{x0 + side + 80}" y="300" font-family="JetBrains Mono" font-weight="800"
        font-size="96" fill="{TEXT}">{WORDMARK[:-1]}<tspan fill="{SNAKE}">_</tspan></text>
  <text x="{x0 + side + 84}" y="360" font-family="JetBrains Mono" font-size="30"
        fill="{DIM}">Debian arm64 for Chromebooks and mainline ARM</text>
  <text x="{x0 + side + 84}" y="404" font-family="JetBrains Mono" font-size="30"
        fill="{EAGLE}">{TAGLINE}</text>
</svg>
'''


def terminal_art(rows):
    """The 24 px form as half blocks: 24 columns, 12 lines, square aspect.

    Every line is padded to the same width, never stripped: the idle screen
    centres the art line by line, so a short row would shear the picture.
    """
    small = down(rows, 2)
    out = []
    for y in range(0, len(small), 2):
        top = small[y]
        bottom = small[y + 1] if y + 1 < len(small) else "." * len(top)
        line = []
        for a, b in zip(top, bottom):
            line.append({"##": "\u2588", "#.": "\u2580", ".#": "\u2584"}.get(a + b, " "))
        out.append("".join(line))
    return out


def write(path, text):
    with open(path, "w", encoding="utf-8") as fh:
        fh.write(text)


def main():
    here = sys.argv[1] if len(sys.argv) > 1 else "."
    rows = read_grid(f"{here}/brand/{GRID}")
    write(f"{here}/brand/hayato.svg", svg_mark(rows, 48))
    write(f"{here}/brand/hayato-24.svg", svg_mark(down(rows, 2), 24))
    write(f"{here}/brand/hayato-16.svg", svg_mark(down(rows, 3), 16))
    write(f"{here}/brand/wallpaper.svg", svg_wallpaper(rows))
    write(f"{here}/brand/social-preview.svg", svg_social(rows))

    art = terminal_art(rows)
    conf = f"{here}/uis/sway/files/home/.config/hayato"
    write(
        f"{conf}/wordmark.txt",
        "\n".join(["$1" + line for line in art])
        + f"\n\n$3{WORDMARK[:-1]}$2_\n$1{TAGLINE}\n",
    )
    write(
        f"{conf}/screensaver.txt",
        "\n".join(art)
        + f"\n\n{WORDMARK}\n\n:: {TAGLINE} ::\n\u00bb press any key to play \u00ab\n",
    )
    print(f"mark {len(rows)}x{len(rows[0])}, art {len(art[0])}x{len(art)}")


if __name__ == "__main__":
    main()
