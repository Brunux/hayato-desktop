#!/bin/bash
# Build the arcade libretro core from the pinned libretro FBNeo repository.
#
# Debian forky ships no arcade core: not fbneo, not mame. This is the core the
# games library launches for arcade romsets (a `.zip` inside `neogeo/`, `cps1/`,
# `cps2/`, `cps3/`, `pgm/` or `arcade/`). The full build covers every board
# FBNeo supports; the core reads a MAME/FBNeo romset as it is and needs the
# board's BIOS zip (`neogeo.zip`, `qsound.zip`, `cps3.zip`, `pgm.zip`) in the
# system directory.
#
# Usage: build-fbneo.sh
# Output: $LAZYOS_STAGE/lazyos-libretro-fbneo_<rev>+lazyos1_<arch>.deb
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

CORE_DIR="$DESKTOP_DIR/tools/fbneo"
CORE_REV="$(tr -d ' \t\r\n' <"$CORE_DIR/version")"
CORE_TARBALL="FBNeo-$CORE_REV.tar.gz"
CORE_URL="https://github.com/libretro/FBNeo/archive/$CORE_REV.tar.gz"
CORE_SRC="$SRC_DIR/FBNeo-$CORE_REV"
PACKAGING_REVISION="lazyos1"
PKG_VERSION="$CORE_REV+$PACKAGING_REVISION"
DEB_ARCH="$(dpkg --print-architecture)"
MULTIARCH="$(dpkg-architecture -qDEB_HOST_MULTIARCH)"

[ -f "$CORE_DIR/SHA256SUMS" ] || die "no SHA256SUMS in $CORE_DIR"

if [ ! -f "$SRC_DIR/$CORE_TARBALL" ]; then
	log "Downloading $CORE_TARBALL"
	mkdir -p "$SRC_DIR"
	curl -fL --retry 3 --progress-bar -o "$SRC_DIR/$CORE_TARBALL.part" "$CORE_URL"
	mv "$SRC_DIR/$CORE_TARBALL.part" "$SRC_DIR/$CORE_TARBALL"
fi
( cd "$SRC_DIR" && sha256sum -c --ignore-missing "$CORE_DIR/SHA256SUMS" ) ||
	die "checksum mismatch for $CORE_TARBALL (delete $SRC_DIR/$CORE_TARBALL and retry)"

if [ ! -d "$CORE_SRC" ]; then
	log "Extracting FBNeo $CORE_REV"
	tar -xf "$SRC_DIR/$CORE_TARBALL" -C "$SRC_DIR"
fi

log "Building FBNeo $CORE_REV (all boards)"
# REGEN_HEADERS=1 regenerates the driver list and the generated CPU tables from
# this checkout's sources, so a stale `dep/generated/` in the tarball cannot
# link a different set of drivers than the objects beside it.
make -C "$CORE_SRC/src/burner/libretro" platform=unix REGEN_HEADERS=1 \
	-j"${JOBS:-$(nproc)}"

PKG="$WORK_DIR/fbneo-pkg"
rm -rf "$PKG"
install -Dm755 "$CORE_SRC/src/burner/libretro/fbneo_libretro.so" \
	"$PKG/usr/lib/$MULTIARCH/libretro/fbneo_libretro.so"
strip --strip-unneeded "$PKG/usr/lib/$MULTIARCH/libretro/fbneo_libretro.so" 2>/dev/null || true

SHLIBS_DIR="$WORK_DIR/fbneo-shlibs"
rm -rf "$SHLIBS_DIR"
mkdir -p "$SHLIBS_DIR/debian"
cat >"$SHLIBS_DIR/debian/control" <<'EOF'
Source: lazyos-libretro-fbneo
Section: games
Priority: optional
Maintainer: LazyOS <lazyos@localhost>
Standards-Version: 4.6.0
Package: lazyos-libretro-fbneo
Architecture: any
EOF
SHLIBS="$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O "$PKG/usr/lib/$MULTIARCH/libretro/fbneo_libretro.so" 2>/dev/null |
	sed -n 's/^shlibs:Depends=//p')"
[ -n "$SHLIBS" ] || SHLIBS="libc6, libstdc++6"
INSTALLED_SIZE="$(du -sk "$PKG/usr" | cut -f1)"

mkdir -p "$PKG/DEBIAN"
cat >"$PKG/DEBIAN/control" <<EOF
Package: lazyos-libretro-fbneo
Version: $PKG_VERSION
Architecture: $DEB_ARCH
Maintainer: LazyOS <lazyos@localhost>
Installed-Size: $INSTALLED_SIZE
Depends: $SHLIBS
Provides: libretro-core
Section: games
Priority: optional
Homepage: https://github.com/libretro/FBNeo
Description: FBNeo libretro core (all arcade boards)
 The arcade core the LazyOS games library launches for .zip romsets. Built
 from the libretro FBNeo fork; it reads a MAME/FBNeo romset as it is and
 needs the board's BIOS zip in the system directory.
EOF

mkdir -p "$LAZYOS_STAGE"
DEB="$LAZYOS_STAGE/lazyos-libretro-fbneo_${PKG_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
log "FBNeo $CORE_REV -> $DEB"
