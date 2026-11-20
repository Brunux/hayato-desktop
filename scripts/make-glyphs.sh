#!/bin/sh
# Regenerate the TUI alphabet: the codepoint behind every `Glyph` name.
#
# The session's Rust framework (`rust/crates/hayato-ui`) draws its rows in
# `foot`, using `Symbols Nerd Font Mono` (`fonts-nerd-symbols`, Debian's
# DFSG-stripped build: Octicons, Devicons, Seti, Powerline, IEC — note Debian
# `3.4.0+dfsg2-1` removed Codicons `ea60..ec1e`) plus the two Font Awesome
# faces from `fonts-font-awesome-legacy` (`Font Awesome 6 Free Solid` and `Font
# Awesome 6 Brands Regular`) listed in `foot.ini`.  `Glyph` is the handle a
# command names (`Glyph::Bluetooth`, not `'\u{f293}'`); this script is where a
# name becomes a codepoint.
#
# The table is filtered against the resolved fonts' character maps: any
# candidate missing from all of them falls back to `draw: None` (`Glyph::token`,
# short ASCII text) instead of rendering a tofu box.
#
#   scripts/make-glyphs.sh                 rewrite the table and the pin
#   scripts/make-glyphs.sh --check         exit 1 when either file is stale
#   scripts/make-glyphs.sh --font F --charset C --out D   (the tests' form)
#
# The primary pin (`FONT_FAMILY`/`FONT_SHA256`) tracks Nerd Fonts v3.5.1
# (`Symbols Nerd Font Mono`), and `EXTRA_FAMILIES` lists the Font Awesome faces
# verified alongside it (`bluetooth`, `wifi`, `battery`, `keyboard`, `mouse`,
# `calculator`, `qr`, `crop`).
#
# Regenerating needs fontconfig (`fc-match`, `fc-query`), `sha256sum`,
# `fonts-nerd-symbols`, and `fonts-font-awesome-legacy` on the host.  None of
# that is a build dependency of the image: the generated files are committed,
# and `check-desktop.sh` compares the pin with the fonts installed on the
# machine.

set -eu

# The release the candidate codepoints were read from.  Bumping it means
# re-checking the list above against upstream's `glyphnames.json`, which is the
# point of writing it down.
nerd_version=3.5.1

here=$(cd "$(dirname "$0")/.." && pwd)
out="$here/rust/crates/hayato-ui/src/glyphs/table.rs"
sidecar="$here/rust/crates/hayato-ui/src/glyphs/font.pin"
font=
charset_file=
check=no
custom_font=no

die() {
	echo "make-glyphs.sh: $*" >&2
	exit 1
}

usage() {
	cat <<'EOF'
usage: scripts/make-glyphs.sh [--font FILE] [--charset FILE] [--out FILE]
                              [--sidecar FILE] [--check]

  --font FILE      the symbol font to filter against (default: fontconfig's
                   match for 'Symbols Nerd Font Mono')
  --charset FILE   read the character map from FILE instead of fc-query
  --out FILE       where to write table.rs (default: the crate's src/glyphs/)
  --sidecar FILE   where to write font.pin (default: the crate's src/glyphs/)
  --check          write nothing; exit 1 when the committed files are stale
EOF
}

while [ $# -gt 0 ]; do
	case $1 in
	--font)
		[ $# -ge 2 ] || die "--font needs a file"
		font=$2
		custom_font=yes
		shift 2
		;;
	--charset)
		[ $# -ge 2 ] || die "--charset needs a file"
		charset_file=$2
		shift 2
		;;
	--out)
		[ $# -ge 2 ] || die "--out needs a file"
		out=$2
		shift 2
		;;
	--sidecar)
		[ $# -ge 2 ] || die "--sidecar needs a file"
		sidecar=$2
		shift 2
		;;
	--check)
		check=yes
		shift
		;;
	-h | --help)
		usage
		exit 0
		;;
	*)
		usage >&2
		die "unknown argument '$1'"
		;;
	esac
done

command -v awk >/dev/null 2>&1 || die "awk is needed to build the table"
command -v sha256sum >/dev/null 2>&1 ||
	die "sha256sum is needed to pin the font (coreutils)"

# Resolve the font and its character map.  `--charset` exists for the crate's
# integration test: it runs this script against a two-line fixture and must not
# depend on which fonts the machine has.
if [ -z "$font" ]; then
	command -v fc-match >/dev/null 2>&1 ||
		die "fontconfig is needed (fc-match): install fontconfig, and fonts-nerd-symbols for the font it resolves"
	command -v fc-query >/dev/null 2>&1 ||
		die "fontconfig is needed (fc-query): install fontconfig"
	font=$(fc-match -f '%{file}' 'Symbols Nerd Font Mono' 2>/dev/null) || font=
	[ -n "$font" ] && [ -r "$font" ] ||
		die "fontconfig resolves no readable 'Symbols Nerd Font Mono': install fonts-nerd-symbols on this host"
fi
[ -r "$font" ] || die "cannot read the font '$font'"

if [ -n "$charset_file" ]; then
	[ -r "$charset_file" ] || die "cannot read the character map '$charset_file'"
	charset=$(cat "$charset_file")
	family=${MAKE_GLYPHS_FAMILY:-fixture}
else
	command -v fc-query >/dev/null 2>&1 ||
		die "fontconfig is needed (fc-query): install fontconfig"
	charset=$(fc-query --format='%{charset}' "$font" 2>/dev/null) || charset=
	[ -n "$charset" ] ||
		die "fc-query returned no character map for '$font' (fontconfig cannot read the font)"
	family=$(fc-query --format='%{family[0]}' "$font" 2>/dev/null) || family=
fi

# A fontconfig match for a family that is not installed answers another family,
# and a table filtered against the wrong font is a table of holes: refuse.
if [ -z "$charset_file" ] && [ "${family#*Symbols Nerd Font}" = "$family" ]; then
	die "'$font' is ${family:-no family}, not a Symbols Nerd Font: install fonts-nerd-symbols"
fi

extra_families=
if [ -z "$charset_file" ] && [ "$custom_font" = no ]; then
	for ef in "Font Awesome 6 Free Solid" "Font Awesome 6 Brands Regular"; do
		ep=$(fc-match -f '%{file}' "$ef" 2>/dev/null) || ep=
		[ -n "$ep" ] && [ -r "$ep" ] ||
			die "fontconfig resolves no readable '$ef': install fonts-font-awesome-legacy on this host"
		efam=$(fc-query --format='%{family[0]}' "$ep" 2>/dev/null) || efam=
		[ "$efam" = "$ef" ] ||
			die "'$ep' is ${efam:-no family}, not '$ef': install fonts-font-awesome-legacy"
		ecs=$(fc-query --format='%{charset}' "$ep" 2>/dev/null) || ecs=
		[ -n "$ecs" ] ||
			die "fc-query returned no character map for '$ep'"
		charset="$charset $ecs"
		extra_families=${extra_families:+$extra_families, }$ef
	done
fi

sum=$(sha256sum "$font")
sum=${sum%% *}
base=$(basename "$font")

tmp=$(mktemp -d "${TMPDIR:-/tmp}/make-glyphs.XXXXXX")
trap 'rm -rf "$tmp"' EXIT INT TERM HUP

# ---------------------------------------------------------------------------
# The candidates: one `Variant codepoint` pair per line, in the order the enum
# lists them.  A `-` is a name the stripped symbol set has no codepoint for at
# all; the row shows its text token (see `Glyph::token`).
#
# Nerd Fonts v3.5.1 names are in the comment, so the next person can re-check a
# choice against upstream's table instead of guessing what a glyph is.
# ---------------------------------------------------------------------------
cat >"$tmp/candidates" <<'EOF'
# files and places
Folder f413			# oct-file_directory
File f4a5			# oct-file
Document f4f6			# oct-note
Pdf f1c1			# fa-file-pdf (Font Awesome 6 Free Solid)
Html f40d			# oct-file_code
Archive f410			# oct-file_zip
Executable f471			# oct-file_binary
Hidden f4c5			# oct-eye_closed
BrokenLink f529			# oct-unlink
Image f4e5			# oct-image
Video f52c			# oct-video
Music f001			# fa-music (Font Awesome 6 Free Solid)
Library f02d			# fa-book (Font Awesome 6 Free Solid)
Home f46d			# oct-home
Desktop f4a9			# oct-device_desktop
Terminal f489			# oct-terminal
# the machine
Window f488			# oct-browser
Disk f472			# oct-database (the closest thing to a drive this set has)
Gear f423			# oct-gear
Book f405			# oct-book
People f4fd			# oct-people
Globe f484			# oct-globe
Link f44c			# oct-link
Volume f485			# oct-unmute
Bell f49a			# oct-bell
BellOff f478			# oct-bell_slash
Clock f43a			# oct-clock
Calendar f455			# oct-calendar
History f464			# oct-history
Trash f48e			# oct-trash
Bug f46f			# oct-bug
Mic f130			# fa-microphone (Font Awesome 6 Free Solid)
Sliders f462			# oct-sliders
Lock f456			# oct-lock
Unlock f52a			# oct-unlock
Eye f441			# oct-eye
Moon f4ee			# oct-moon
Brightness f522			# oct-sun
Keyboard f11c			# fa-keyboard (Font Awesome 6 Free Solid)
Wifi f1eb			# fa-wifi (Font Awesome 6 Free Solid)
Bluetooth f293			# fa-bluetooth-b (Font Awesome 6 Brands Regular)
Battery f240			# fa-battery-full (Font Awesome 6 Free Solid)
Mouse f245			# fa-arrow-pointer (Font Awesome 6 Free Solid)
Network f43c			# oct-broadcast
Server f473			# oct-server
Database f1c0			# fa-database (Font Awesome 6 Free Solid)
Chip f4bc			# oct-cpu
Display e163			# fa-display (Font Awesome 6 Free Solid)
Power 23fb			# iec-power
Logout f426			# oct-sign_out
Reboot f021			# fa-arrows-rotate (Font Awesome 6 Free Solid)
Rotate f2f9			# fa-rotate-right (Font Awesome 6 Free Solid)
# actions
Search f422			# oct-search
Warning f421			# oct-alert
Info f449			# oct-info
Check f42e			# oct-check
Undo f2ea			# fa-rotate-left (Font Awesome 6 Free Solid)
Save f0c7			# fa-floppy-disk (Font Awesome 6 Free Solid)
Sync f46a			# oct-sync
Download f409			# oct-download
Capture f50c			# oct-screen_full
Camera f446			# oct-device_camera
Crop f125			# fa-crop (Font Awesome 6 Free Solid)
Qr f029				# fa-qrcode (Font Awesome 6 Free Solid)
Clipboard f429			# oct-paste
Smile f4a2			# oct-smiley
Filter f4d7			# oct-filter
Tag f412			# oct-tag
Key f43d			# oct-key
Map f279			# fa-map (Font Awesome 6 Free Solid)
Theme f53f			# fa-palette (Font Awesome 6 Free Solid)
Brush f48f			# oct-paintbrush
Box f487			# oct-package
Calculator f1ec			# fa-calculator (Font Awesome 6 Free Solid)
Play f500			# oct-play
Pause f04c			# fa-pause (Font Awesome 6 Free Solid)
Stop f46e			# oct-stop
Next f460			# oct-chevron_right
Prev f47d			# oct-chevron_left
ArrowUp f431			# oct-arrow_up
ArrowDown f433			# oct-arrow_down
ArrowLeft f434			# oct-arrow_left
ArrowRight f432			# oct-arrow_right
Target f140			# fa-bullseye (Font Awesome 6 Free Solid)
Star f41e			# oct-star
Heart f4e1			# oct-heart_fill
Pin f435			# oct-pin
Lightbulb f400			# oct-light_bulb
Cloud f4ac			# oct-cloud
# the tools and the agents
Code f44f			# oct-code
Agent f477			# oct-hubot
Sparkle f51b			# oct-sparkle_fill
Git f418			# oct-git_branch
Github f408			# oct-mark_github
Docker e7b0			# dev-docker
Rust e68b			# seti-rust
Python e606			# seti-python
Node e719			# dev-nodejs
Linux e712			# dev-linux
Debian e77d			# dev-debian
Shield f49c			# oct-shield
EOF

CHARSET=$charset awk -v stats="$tmp/stats" '
function dec(h,   i, v) {
	v = 0
	for (i = 1; i <= length(h); i++)
		v = v * 16 + index("0123456789abcdef", tolower(substr(h, i, 1))) - 1
	return v
}
BEGIN {
	n = split(ENVIRON["CHARSET"], token, /[ \t\n]+/)
	count = 0
	for (i = 1; i <= n; i++) {
		if (token[i] == "") continue
		if (index(token[i], "-")) {
			split(token[i], range, "-")
			lo[count] = dec(range[1])
			hi[count] = dec(range[2])
		} else {
			lo[count] = dec(token[i])
			hi[count] = dec(token[i])
		}
		count++
	}
	if (count == 0) {
		print "make-glyphs.sh: the character map is empty" > "/dev/stderr"
		exit 1
	}
	drawn = 0
	fallback = 0
}
$1 == "" || $1 == "#" { next }
{
	variant = $1
	cp = $2
	if (cp == "-") {
		printf "    Entry {\n        glyph: Glyph::%s,\n        draw: None,\n    },\n", variant
		fallback++
		next
	}
	value = dec(cp)
	covered = 0
	for (i = 0; i < count; i++)
		if (value >= lo[i] && value <= hi[i]) {
			covered = 1
			break
		}
	if (!covered) {
		printf "    Entry {\n        glyph: Glyph::%s,\n        draw: None,\n    },\n", variant
		fallback++
		next
	}
	printf "    Entry {\n        glyph: Glyph::%s,\n        draw: Some(\"\\u{%s}\"),\n    },\n", variant, tolower(cp)
	drawn++
}
END {
	print drawn, fallback > stats
}
' "$tmp/candidates" >"$tmp/entries"

read -r drawn fallback <"$tmp/stats"
[ -n "${drawn:-}" ] || die "the generator produced no counts (awk failed)"
[ $((drawn + fallback)) -gt 0 ] || die "the candidate table is empty"

# ---------------------------------------------------------------------------
# table.rs: the data the enum is looked up in.
# ---------------------------------------------------------------------------
{
	cat <<EOF
//! The alphabet table: which codepoint draws which name.
//!
//! **Generated by \`scripts/make-glyphs.sh\`; do not edit.**  The names are the
//! variants of \`Glyph\` in \`glyphs.rs\` and the codepoints are Nerd Fonts
//! v$nerd_version values filtered against the character map of
//! \`FONT_FAMILY\`; a name that map does not carry is \`draw: None\` and its row
//! shows text instead (\`Glyph::token\`).  The comments in the generator say
//! which upstream glyph each codepoint is.
//!
//! The filter is what keeps a row from being an empty box: a codepoint no
//! installed font carries draws nothing at all and logs nothing, which is how
//! the old alphabet's tofu shipped once already.

use super::Glyph;

/// The font the alphabet was filtered against.  \`hayato --glyph-font\` prints
/// these values and \`scripts/check-system.sh\` compares them with the font
/// installed on the machine: a font upgrade that drops a codepoint has to fail
/// the report, not turn a row into a hole.
pub const FONT_FAMILY: &str = "$family";
pub const EXTRA_FAMILIES: &str = "$extra_families";
pub const FONT_FILE: &str = "$base";
pub const FONT_SHA256: &str = "$sum";

/// The Nerd Fonts release the candidate codepoints come from.
pub const NERD_VERSION: &str = "$nerd_version";

/// How many names the font draws, and how many fall back to text.
pub const DRAWN: usize = $drawn;
pub const FALLBACK: usize = $fallback;

pub(crate) struct Entry {
    pub glyph: Glyph,
    /// The codepoint as an escaped Rust string (the source stays ASCII: a
    /// literal private-use character does not survive every editor, which this
    /// repository has already shipped once).
    pub draw: Option<&'static str>,
}

pub(crate) const TABLE: &[Entry] = &[
EOF
	cat "$tmp/entries"
	printf '];\n'
} >"$tmp/table.rs"

cat >"$tmp/font.pin" <<EOF
# The font the TUI alphabet is drawn from (generated by scripts/make-glyphs.sh).
# \`hayato --glyph-font\` prints the same values, so scripts/check-system.sh can
# compare them with the font installed on the machine: a font upgrade that drops
# a codepoint turns that row into an empty box, and nothing logs it.
family=$family
extra=$extra_families
file=$base
sha256=$sum
nerd=$nerd_version
drawn=$drawn
fallback=$fallback
EOF

if [ "$check" = yes ]; then
	stale=no
	if [ ! -f "$out" ] || ! cmp -s "$tmp/table.rs" "$out"; then
		echo "make-glyphs.sh: $out is stale" >&2
		diff -u "$out" "$tmp/table.rs" 2>/dev/null | sed -n '1,40p' >&2 || true
		stale=yes
	fi
	if [ ! -f "$sidecar" ] || ! cmp -s "$tmp/font.pin" "$sidecar"; then
		echo "make-glyphs.sh: $sidecar is stale" >&2
		diff -u "$sidecar" "$tmp/font.pin" 2>/dev/null | sed -n '1,40p' >&2 || true
		stale=yes
	fi
	[ "$stale" = no ] || exit 1
	echo "make-glyphs.sh: $out and $sidecar are current ($drawn drawn, $fallback text)"
	exit 0
fi

mkdir -p "$(dirname "$out")" "$(dirname "$sidecar")"
cp "$tmp/table.rs" "$out"
cp "$tmp/font.pin" "$sidecar"

echo "make-glyphs.sh: $drawn codepoints, $fallback text fallbacks from $family -> $out"
echo "make-glyphs.sh: pinned $(basename "$font") ($sum) -> $sidecar"
