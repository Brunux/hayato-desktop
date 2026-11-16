#!/bin/bash
# Standalone fallback helpers when lazyos-desktop scripts run outside lazyos-core.

[ -n "${_LIB_DESKTOP_LOADED:-}" ] && return 0
_LIB_DESKTOP_LOADED=1

DESKTOP_DIR="${DESKTOP_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"
UI="${UI:-sway}"
ARCH="${ARCH:-$(uname -m)}"
BUILD_ROOT="${BUILD_ROOT:-/var/tmp/lazyos-build}"
WORK_DIR="${WORK_DIR:-$BUILD_ROOT/work}"
SRC_DIR="${SRC_DIR:-$BUILD_ROOT/src}"
LAZYOS_STAGE="${LAZYOS_STAGE:-$WORK_DIR/lazyos-stage}"

log()  { printf '\033[1;34m[lazyos]\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[1;31m[fail]\033[0m %s\n' "$*" >&2; exit 1; }

COMPOSITOR="${COMPOSITOR:-swayfx}"
COMPOSITOR_DIR="$DESKTOP_DIR/compositors/$COMPOSITOR"
require_compositor() {
	[ -d "$COMPOSITOR_DIR" ] || die "compositor '$COMPOSITOR' not found in $DESKTOP_DIR/compositors"
}
require_compositor
COMPOSITOR_VERSION="$(tr -d ' \t\r\n' <"$COMPOSITOR_DIR/version")"
SCENEFX_VERSION="$(tr -d ' \t\r\n' <"$COMPOSITOR_DIR/scenefx-version")"
COMPOSITOR_STAGE="${COMPOSITOR_STAGE:-$WORK_DIR/${COMPOSITOR}-stage}"
COMPOSITOR_SRC="$SRC_DIR/$COMPOSITOR-$COMPOSITOR_VERSION"
SCENEFX_SRC="$SRC_DIR/scenefx-$SCENEFX_VERSION"
COMPOSITOR_TARBALL="$COMPOSITOR-$COMPOSITOR_VERSION.tar.gz"
SCENEFX_TARBALL="scenefx-$SCENEFX_VERSION.tar.gz"
COMPOSITOR_URL="https://github.com/WillPower3309/$COMPOSITOR/archive/refs/tags/$COMPOSITOR_VERSION.tar.gz"
SCENEFX_URL="https://github.com/wlrfx/scenefx/archive/refs/tags/$SCENEFX_VERSION.tar.gz"

WORKSTYLE="workstyle"
WORKSTYLE_DIR="$DESKTOP_DIR/tools/$WORKSTYLE"
WORKSTYLE_VERSION="$(tr -d ' \t\r\n' <"$WORKSTYLE_DIR/version")"
WORKSTYLE_STAGE="${WORKSTYLE_STAGE:-$WORK_DIR/$WORKSTYLE-stage}"
WORKSTYLE_SRC="$SRC_DIR/$WORKSTYLE-$WORKSTYLE_VERSION"
WORKSTYLE_TARBALL="$WORKSTYLE-$WORKSTYLE_VERSION.crate"
WORKSTYLE_URL="https://static.crates.io/crates/$WORKSTYLE/$WORKSTYLE_TARBALL"

OPENCODE="opencode"
OPENCODE_DIR="$DESKTOP_DIR/tools/$OPENCODE"
OPENCODE_VERSION="$(tr -d ' \t\r\n' <"$OPENCODE_DIR/version")"
OPENCODE_TARBALL="$OPENCODE-cli-linux-arm64-$OPENCODE_VERSION.tgz"
OPENCODE_URL="https://registry.npmjs.org/@opencode/cli-linux-arm64/-/cli-linux-arm64-$OPENCODE_VERSION.tgz"

deb_depends() {
	local deps="" f so pkg ver
	declare -A seen=()
	for f in "$@"; do
		[ -e "$f" ] || continue
		while read -r _ _ so _; do
			[ -n "${so:-}" ] && [ -e "$so" ] || continue
			pkg="$(dpkg-query -S "$so" 2>/dev/null | head -n1 | cut -d: -f1 || true)"
			pkg="${pkg%%:*}"
			[ -n "$pkg" ] && [ -z "${seen[$pkg]:-}" ] || continue
			seen[$pkg]=1
			ver="$(dpkg-query -W -f='${Version}' "$pkg" 2>/dev/null || true)"
			[ -n "$ver" ] && deps="${deps:+$deps, }$pkg (>= ${ver%%-*})" || deps="${deps:+$deps, }$pkg"
		done < <(ldd "$f" 2>/dev/null | awk '/=> \// { print $1, $2, $3, $4 }')
	done
	printf '%s\n' "$deps"
}

build_deb() {
	local out="$1" root="$2" name="$3" ver="$4" deps="$5" desc="$6" extra="${7:-}"
	local deb_arch
	case "$ARCH" in
		aarch64|arm64) deb_arch="arm64" ;;
		x86_64|amd64)  deb_arch="amd64" ;;
		*)             deb_arch="$(dpkg --print-architecture 2>/dev/null || echo "$ARCH")" ;;
	esac
	mkdir -p "$out" "$root/DEBIAN"
	{
		printf 'Package: %s\n' "$name"
		printf 'Version: %s\n' "$ver"
		printf 'Architecture: %s\n' "$deb_arch"
		printf 'Maintainer: LazyOS <maintainers@lazyos.org>\n'
		[ -n "$deps" ] && printf 'Depends: %s\n' "$deps"
		[ -n "$extra" ] && printf '%s\n' "$extra"
		printf 'Section: x11\n'
		printf 'Priority: optional\n'
		printf 'Description: %s\n' "$desc"
	} >"$root/DEBIAN/control"
	dpkg-deb --build --root-owner-group "$root" "$out/${name}_${ver}_${deb_arch}.deb"
}
