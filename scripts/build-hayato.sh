#!/bin/bash
# Build the session framework: the `hayato` multi-call binary.
#
# The Rust workspace in rust/ is the source; this script only builds it the way
# it ships (release profile) and stages the binary where install-hayato.sh
# expects it:
#
#   $HAYATO_STAGE/hayato   /usr/bin/hayato
#
# There is no tarball and no pin: the framework is this repository's own code,
# which is also why the binary reports the committed VERSION (compiled in by
# rust/crates/hayato-cli/src/version.rs) instead of a cargo version.
#
# Usage: build-hayato.sh
set -euo pipefail
_here="$(cd "$(dirname "$0")" && pwd)"
DESKTOP_DIR="${DESKTOP_DIR:-$(cd "$_here/.." && pwd)}"
if [ -n "${HAYATO_CORE_DIR:-}" ] && [ -f "$HAYATO_CORE_DIR/scripts/lib.sh" ]; then
	. "$HAYATO_CORE_DIR/scripts/lib.sh"
elif [ -f "$_here/../../core/scripts/lib.sh" ]; then
	. "$_here/../../core/scripts/lib.sh"
elif [ -f "$_here/../../hayato-core/scripts/lib.sh" ]; then
	. "$_here/../../hayato-core/scripts/lib.sh"
else
	. "$_here/lib-desktop.sh"
fi

command -v cargo >/dev/null 2>&1 || die "cargo is not installed (run scripts/00-deps.sh)"

log "Building the session framework (hayato)"
cargo build --release --locked --manifest-path "$DESKTOP_DIR/rust/Cargo.toml"

BIN="$DESKTOP_DIR/rust/target/release/hayato"
[ -x "$BIN" ] || die "cargo finished but $BIN is missing"
NOTIFY_BIN="$DESKTOP_DIR/rust/target/release/hayato-notify"
[ -x "$NOTIFY_BIN" ] || die "cargo finished but $NOTIFY_BIN is missing"
GAMES_BIN="$DESKTOP_DIR/rust/target/release/hayato-games"
[ -x "$GAMES_BIN" ] || die "cargo finished but $GAMES_BIN is missing"
# A binary that cannot report its version is not the one the image should
# carry: the check is one process and it catches a broken include_str!.
VERSION="$(cat "$DESKTOP_DIR/VERSION")"
REPORTED="$("$BIN" --version)"
[ "$REPORTED" = "$VERSION" ] ||
	die "hayato reports '$REPORTED' but VERSION is '$VERSION'"
GAMES_REPORTED="$("$GAMES_BIN" --version)"
[ "$GAMES_REPORTED" = "$VERSION" ] ||
	die "hayato-games reports '$GAMES_REPORTED' but VERSION is '$VERSION'"

mkdir -p "$HAYATO_STAGE"
install -m 0755 "$BIN" "$HAYATO_STAGE/hayato"
install -m 0755 "$NOTIFY_BIN" "$HAYATO_STAGE/hayato-notify"
install -m 0755 "$GAMES_BIN" "$HAYATO_STAGE/hayato-games"
# The library page and the sample game are data, not a binary: they travel
# beside the executable so install-hayato.sh can copy one directory.
install -d "$HAYATO_STAGE/share/hayato/games"
cp -a "$DESKTOP_DIR/rust/crates/hayato-games/data/." "$HAYATO_STAGE/share/hayato/games/"
log "hayato $REPORTED -> $HAYATO_STAGE/hayato"
log "hayato-notify -> $HAYATO_STAGE/hayato-notify"
log "hayato-games $GAMES_REPORTED -> $HAYATO_STAGE/hayato-games"
log "games data -> $HAYATO_STAGE/share/hayato/games"
