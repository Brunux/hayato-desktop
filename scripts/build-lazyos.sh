#!/bin/bash
# Build the session framework: the `lazyos` multi-call binary.
#
# The Rust workspace in rust/ is the source; this script only builds it the way
# it ships (release profile) and stages the binary where install-lazyos.sh
# expects it:
#
#   $LAZYOS_STAGE/lazyos   /usr/bin/lazyos
#
# There is no tarball and no pin: the framework is this repository's own code,
# which is also why the binary reports the committed VERSION (compiled in by
# rust/crates/lazyos-cli/src/version.rs) instead of a cargo version.
#
# Usage: build-lazyos.sh
set -euo pipefail
_here="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="${DESKTOP_DIR:-$(cd "$_here/.." && pwd)}"
if [ -n "${LAZYOS_CORE_DIR:-}" ] && [ -f "$LAZYOS_CORE_DIR/scripts/lib.sh" ]; then
	. "$LAZYOS_CORE_DIR/scripts/lib.sh"
elif [ -f "$_here/../../core/scripts/lib.sh" ]; then
	. "$_here/../../core/scripts/lib.sh"
elif [ -f "$_here/../../lazyos-core/scripts/lib.sh" ]; then
	. "$_here/../../lazyos-core/scripts/lib.sh"
else
	. "$_here/lib-desktop.sh"
fi

command -v cargo >/dev/null 2>&1 || die "cargo is not installed (run scripts/00-deps.sh)"

log "Building the session framework (lazyos)"
cargo build --release --locked --manifest-path "$DESKTOP_DIR/rust/Cargo.toml"

BIN="$DESKTOP_DIR/rust/target/release/lazyos"
[ -x "$BIN" ] || die "cargo finished but $BIN is missing"
NOTIFY_BIN="$DESKTOP_DIR/rust/target/release/lazyos-notify"
[ -x "$NOTIFY_BIN" ] || die "cargo finished but $NOTIFY_BIN is missing"
GAMES_BIN="$DESKTOP_DIR/rust/target/release/lazyos-games"
[ -x "$GAMES_BIN" ] || die "cargo finished but $GAMES_BIN is missing"
# A binary that cannot report its version is not the one the image should
# carry: the check is one process and it catches a broken include_str!.
VERSION="$(cat "$DESKTOP_DIR/VERSION")"
REPORTED="$("$BIN" --version)"
[ "$REPORTED" = "$VERSION" ] ||
	die "lazyos reports '$REPORTED' but VERSION is '$VERSION'"
GAMES_REPORTED="$("$GAMES_BIN" --version)"
[ "$GAMES_REPORTED" = "$VERSION" ] ||
	die "lazyos-games reports '$GAMES_REPORTED' but VERSION is '$VERSION'"

mkdir -p "$LAZYOS_STAGE"
install -m 0755 "$BIN" "$LAZYOS_STAGE/lazyos"
install -m 0755 "$NOTIFY_BIN" "$LAZYOS_STAGE/lazyos-notify"
install -m 0755 "$GAMES_BIN" "$LAZYOS_STAGE/lazyos-games"
# The library page and the sample game are data, not a binary: they travel
# beside the executable so install-lazyos.sh can copy one directory.
install -d "$LAZYOS_STAGE/share/lazyos/games"
cp -a "$DESKTOP_DIR/rust/crates/lazyos-games/data/." "$LAZYOS_STAGE/share/lazyos/games/"
log "lazyos $REPORTED -> $LAZYOS_STAGE/lazyos"
log "lazyos-notify -> $LAZYOS_STAGE/lazyos-notify"
log "lazyos-games $GAMES_REPORTED -> $LAZYOS_STAGE/lazyos-games"
log "games data -> $LAZYOS_STAGE/share/lazyos/games"
