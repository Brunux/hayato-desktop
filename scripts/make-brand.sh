#!/bin/bash
# The Hayato brand, generated from one grid and rendered into the files the
# image and the repository ship.  brand/README.md is the brand itself: the
# mark, the wordmark, the palette and the rules.
#
# Usage: scripts/make-brand.sh
#
#   brand/mark-48.txt                                       the mark (source of truth)
#   brand/hayato.svg                                        the mark, generated
#   brand/wallpaper.svg                                     the wallpaper, generated
#   brand/social-preview.svg                                GitHub's social card, generated
#
#   .../icons/Hayato/48x48/apps/hayato.png                  the launcher, the About row
#   .../icons/Hayato/scalable/apps/hayato.svg               GTK at larger sizes
#   .../home/Pictures/Wallpapers/hayato.png                 1366x912, the default background
#   .../home/.config/hayato/wordmark.txt                    fastfetch's logo
#   .../home/.config/hayato/screensaver.txt                 the idle screen's art
#   brand/social-preview.png                                1280x640, the repository's card
#   site/apple-touch-icon.png                               180x180, the landing page's touch icon
#
# The outputs are committed, because build-rootfs.sh stages `git archive HEAD`
# and a build host without rsvg-convert must still be able to build an image.
# Only an edit to brand/ needs this script to run.  The landing page (site/)
# takes its pictures straight from brand/ when the Pages workflow assembles it.
#
# Host tool, like make-icons.sh: rsvg-convert is `librsvg2-bin` on Debian and
# `librsvg` on Alpine (scripts/00-deps.sh installs it); the text is drawn with
# JetBrains Mono, which is `fonts-jetbrains-mono` on Debian.
set -eu

here=$(cd "$(dirname "$0")/.." && pwd)
brand="$here/brand"
theme="$here/uis/sway/files/usr/share/icons/Hayato"
wall="$here/uis/sway/files/home/Pictures/Wallpapers/hayato.png"

die() {
	echo "make-brand.sh: $*" >&2
	exit 1
}

command -v rsvg-convert >/dev/null 2>&1 ||
	die "rsvg-convert is not installed (Debian: librsvg2-bin, Alpine: librsvg)"

# fc-match always answers, with a fallback when the family is missing; ask it
# for the family it resolved and refuse a substitute, because a wallpaper
# typeset in DejaVu is not the brand.
family=$(fc-match -f '%{family}' 'JetBrains Mono' 2>/dev/null || true)
case "$family" in
"JetBrains Mono" | "JetBrains Mono,"*) ;;
*) die "JetBrains Mono is not installed (Debian: fonts-jetbrains-mono)" ;;
esac

[ -r "$brand/mark-48.txt" ] || die "missing brand/mark-48.txt"

# One drawing, every surface: the grid is rasterised into the three SVGs and
# into the terminal art, so the icon, the wallpaper and the idle screen are
# the same eagle at different sizes and can never drift apart.
echo "  generate    brand/*.svg, .config/hayato/{wordmark,screensaver}.txt"
python3 "$here/scripts/make-brand.py" "$here"

mkdir -p "$theme/48x48/apps" "$theme/24x24/apps" "$theme/16x16/apps" \
	"$theme/scalable/apps" "$(dirname "$wall")"

# The 16 and 24 px icons are sampled from the grid (3x3 and 2x2 cells per
# pixel), never resampled from the 48, so each is exact at the size the bar,
# the launcher and the file manager actually ask for.
echo "  mark 48x48  $(basename "$theme")/48x48/apps/hayato.png"
rsvg-convert -w 48 -h 48 -o "$theme/48x48/apps/hayato.png" "$brand/hayato.svg"

echo "  mark 24x24  $(basename "$theme")/24x24/apps/hayato.png"
rsvg-convert -w 24 -h 24 -o "$theme/24x24/apps/hayato.png" "$brand/hayato-24.svg"

echo "  mark 16x16  $(basename "$theme")/16x16/apps/hayato.png"
rsvg-convert -w 16 -h 16 -o "$theme/16x16/apps/hayato.png" "$brand/hayato-16.svg"

echo "  mark svg    $(basename "$theme")/scalable/apps/hayato.svg"
install -m 0644 "$brand/hayato.svg" "$theme/scalable/apps/hayato.svg"

# The panel's own mode (docs/optimization.md): generating at the size it is
# shown at avoids a resample, and `fill` in the sway config crops rather than
# stretches if the output ever differs.  The size lives in the SVG.
echo "  wallpaper   1366x912  Pictures/Wallpapers/hayato.png"
rsvg-convert -w 1366 -h 912 -b '#1a1b26' -o "$wall" "$brand/wallpaper.svg"

echo "  social      1280x640  brand/social-preview.png"
rsvg-convert -w 1280 -h 640 -b '#1a1b26' -o "$brand/social-preview.png" "$brand/social-preview.svg"

# iOS masks the icon itself and shows transparency as black, so the touch icon
# is the mark on an opaque background square.
echo "  touch icon  180x180   brand/apple-touch-icon.png"
rsvg-convert -w 180 -h 180 -b '#1a1b26' -o "$brand/apple-touch-icon.png" "$brand/hayato.svg"

echo "Done."
