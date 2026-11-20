#!/usr/bin/env python3
"""Fetch and install the three wallpapers each built-in theme cycles.

Every theme ships exactly three, all landscape, all at least 2560 px wide, so
`Alt+Shift+W` always has somewhere to go and the picker never shows a gap.

Sources are the omarchy `quattro` tree (MIT); the pictures themselves carry
their own provenance, which `brand/wallpapers.md` records per file.  A picture
is rejected when it carries a wordmark, a logo, a trademark or a brand name in
the artwork - "omarchy" above all, but also the Audi/Castrol car, the LUMON
card and the OMARCHY poster, which are named here so nobody re-adds them.

Usage:
    scripts/fetch-wallpapers.py            # fetch what is missing, convert, install
    scripts/fetch-wallpapers.py --check    # verify every theme has three
"""
import os
import subprocess
import sys

BASE = "https://raw.githubusercontent.com/omacom/omarchy/quattro/themes"
REPO = os.path.abspath("uis/sway/files/home/Pictures/Wallpapers")

# theme -> [(source, dest name)]; a source of None means the file already ships
# in this repository (a picture from before the omarchy import).
WALLPAPERS = {
    "tokyo-night": [
        ("tokyo-night/backgrounds/4-sunset-lake.webp", "tokyo-night-sunset-lake.jpg"),
        ("tokyo-night/backgrounds/3-swirl-buck.webp", "tokyo-night-swirl-buck.jpg"),
        ("tokyo-night/backgrounds/1-winding-road.webp", "tokyo-night-winding-road.jpg"),
    ],
    "solarized": [
        (None, "solarized-japan.jpg"),
        (None, "solarized-nebula.jpg"),
        ("retro-82/backgrounds/5-zen-boat.webp", "solarized-zen-boat.jpg"),
    ],
    "catppuccin": [
        ("catppuccin/backgrounds/2-waves.webp", "catppuccin-waves.jpg"),
        ("catppuccin/backgrounds/3-blue-eye.webp", "catppuccin-blue-eye.jpg"),
        ("lupine/backgrounds/03-pastel-clouds.webp", "catppuccin-pastel-clouds.jpg"),
    ],
    "rose-pine": [
        ("rose-pine/backgrounds/1-funky-shapes.webp", "rose-pine-funky-shapes.jpg"),
        ("rose-pine/backgrounds/2-dot-map.webp", "rose-pine-dot-map.jpg"),
        ("miasma/backgrounds/01-nature-of-fear.webp", "rose-pine-nature-of-fear.jpg"),
    ],
    "nord": [
        ("nord/backgrounds/0-black-moon.jpg", "nord-black-moon.jpg"),
        ("nord/backgrounds/1-city-view.webp", "nord-city-view.jpg"),
        ("nord/backgrounds/2-night-hawks.webp", "nord-night-hawks.jpg"),
    ],
    "gruvbox": [
        ("gruvbox/backgrounds/1-the-backwater.jpg", "gruvbox-the-backwater.jpg"),
        ("gruvbox/backgrounds/3-village-square.jpg", "gruvbox-village-square.jpg"),
        ("gruvbox/backgrounds/5-leaves.jpg", "gruvbox-leaves.jpg"),
    ],
    "everforest": [
        ("everforest/backgrounds/1-tree-tops.webp", "everforest-tree-tops.jpg"),
        ("jade-horse/backgrounds/01-mountain-horses.webp", "everforest-mountain-horses.jpg"),
        ("jade-horse/backgrounds/03-carved-jade-horse.webp", "everforest-carved-jade-horse.jpg"),
    ],
    "kanagawa": [
        ("kanagawa/backgrounds/1-kanagawa.jpg", "kanagawa-wave.jpg"),
        ("solitude/backgrounds/4-ether.webp", "kanagawa-ether.jpg"),
        ("last-horizon/backgrounds/4-new-horizons.jpg", "kanagawa-new-horizons.jpg"),
    ],
    "dracula": [
        (None, "dracula-night.jpg"),
        ("miasma/backgrounds/02-crowned.webp", "dracula-crowned.jpg"),
        ("vantablack/backgrounds/1-twisted-stairs.webp", "dracula-twisted-stairs.jpg"),
    ],
    "one-dark": [
        (None, "one-dark-space.jpg"),
        ("solitude/backgrounds/1-on-pole.webp", "one-dark-on-pole.jpg"),
        ("solitude/backgrounds/5-eyed.jpg", "one-dark-eyed.jpg"),
    ],
    "hackerman": [
        (None, "hackerman-matrix.jpg"),
        ("hackerman/backgrounds/1-synth-scape.jpg", "hackerman-synth-scape.jpg"),
        ("hackerman/backgrounds/2-geometric.webp", "hackerman-geometric.jpg"),
    ],
    "lumon": [
        (None, "lumon-severance.jpg"),
        ("vantablack/backgrounds/2-layers-deep.webp", "lumon-layers-deep.jpg"),
        ("vantablack/backgrounds/3-layers-stacked.webp", "lumon-layers-stacked.jpg"),
    ],
    "matte-black": [
        ("matte-black/backgrounds/0-ship-at-sea.jpg", "matte-black-ship-at-sea.jpg"),
        ("matte-black/backgrounds/1-dark-waters.webp", "matte-black-dark-waters.jpg"),
        ("matte-black/backgrounds/2-dot-hands.webp", "matte-black-dot-hands.jpg"),
    ],
    "omacon": [
        (None, "omacon-cosmic.jpg"),
        ("ethereal/backgrounds/1-cosmic.webp", "omacon-nebula.jpg"),
        ("retro-82/backgrounds/8-glitter-glass.webp", "omacon-glitter-glass.jpg"),
    ],
    "catppuccin-latte": [
        ("catppuccin-latte/backgrounds/1-color-fade.webp", "catppuccin-latte-color-fade.jpg"),
        ("lupine/backgrounds/02-cherry-blossom-white.webp", "catppuccin-latte-cherry-blossom.jpg"),
        ("lupine/backgrounds/05-abstract-wave.webp", "catppuccin-latte-abstract-wave.jpg"),
    ],
    "flexoki-light": [
        ("flexoki-light/backgrounds/1-orb.webp", "flexoki-light-orb.jpg"),
        ("white/backgrounds/1-white.webp", "flexoki-light-white.jpg"),
        ("lupine/backgrounds/04-elegant-blue-wave.webp", "flexoki-light-blue-wave.jpg"),
    ],
}

# Rejected on sight, with the reason.  Kept here so the review is auditable and
# a future fetch does not quietly bring them back.
REJECTED = {
    "tokyo-night/backgrounds/2-quattro.webp": "Audi Quattro with Audi, Castrol and Michelin marks",
    "tokyo-night/backgrounds/omarchy.webp": "carries the omarchy wordmark",
    "tokyo-night/backgrounds/5-omakub.webp": "carries the omakub wordmark",
    "tokyo-night/backgrounds/7-oma.webp": "brand poster",
    "tokyo-night/backgrounds/6-oma-cityscape.jpg": "brand-named source, synthwave city",
    "lumon/backgrounds/02-opinions-equally.webp": "carries the OMARCHY wordmark",
    "lumon/backgrounds/omarchy.webp": "carries the omarchy wordmark",
    "rose-pine/backgrounds/3-omarchy-plants.webp": "carries the omarchy wordmark",
    "catppuccin/backgrounds/1-totoro.webp": "third-party character art",
    "flexoki-light/backgrounds/2-omarchy.webp": "carries the omarchy wordmark",
}


def convert(src, dest):
    subprocess.run(
        ["convert", src, "-resize", "2560x", "-quality", "88", dest], check=True)


def fetch(source, dest):
    url = f"{BASE}/{source}"
    tmp = f"/tmp/opencode/wallpool/{source.replace('/', '__')}"
    if not os.path.exists(tmp):
        subprocess.run(["curl", "-sfL", url, "-o", tmp], check=True)
    convert(tmp, dest)


def main():
    check = "--check" in sys.argv
    os.makedirs(REPO, exist_ok=True)
    missing = []
    for theme, entries in WALLPAPERS.items():
        if len(entries) != 3:
            raise SystemExit(f"{theme}: {len(entries)} wallpapers, want 3")
        for source, name in entries:
            dest = os.path.join(REPO, name)
            if not os.path.exists(dest):
                if check:
                    missing.append(f"{theme}: {name}")
                    continue
                if source is None:
                    raise SystemExit(f"{name} should ship in the repo but is missing")
                print(f"  {theme:18} {name}")
                fetch(source, dest)
    if check:
        if missing:
            print("missing wallpapers:")
            for line in missing:
                print(" ", line)
            return 1
        print(f"{len(WALLPAPERS)} themes, 3 wallpapers each")
    return 0


if __name__ == "__main__":
    sys.exit(main())
