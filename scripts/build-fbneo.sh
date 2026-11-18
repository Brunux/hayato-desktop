#!/bin/bash
# Build the Neo Geo libretro core from the pinned libretro FBNeo repository.
#
# Debian forky ships no Neo Geo or arcade core: not fbneo, not mame. This is the
# core the games library launches for Neo Geo romsets (a `.zip` inside a
# `neogeo` folder). The build is the Neo Geo subset (`SUBSET=neogeo`), so it
# compiles the Neo Geo driver instead of every arcade board; the core reads a
# MAME/FBNeo romset as it is and needs the MVS BIOS `neogeo.zip` in the system
# directory.
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

log "Building FBNeo $CORE_REV (Neo Geo subset)"
# REGEN_HEADERS=1 is what makes the subset real: the tarball ships a
# `dep/generated/driverlist.h` with every arcade driver, and burn.cpp links that
# table. Regenerating it from the subset's SOURCES_CXX leaves only the Neo Geo
# drivers; without it the link fails on every other board's BurnDrv symbol.
make -C "$CORE_SRC/src/burner/libretro" platform=unix SUBSET=neogeo REGEN_HEADERS=1 \
	-j"${JOBS:-$(nproc)}"

PKG="$WORK_DIR/fbneo-pkg"
rm -rf "$PKG"
install -Dm755 "$CORE_SRC/src/burner/libretro/fbneo_neogeo_libretro.so" \
	"$PKG/usr/lib/$MULTIARCH/libretro/fbneo_neogeo_libretro.so"
strip --strip-unneeded "$PKG/usr/lib/$MULTIARCH/libretro/fbneo_neogeo_libretro.so" 2>/dev/null || true

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
SHLIBS="$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O "$PKG/usr/lib/$MULTIARCH/libretro/fbneo_neogeo_libretro.so" 2>/dev/null |
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
Description: FBNeo libretro core (Neo Geo subset)
 The Neo Geo core the LazyOS games library launches for .zip romsets. Built
 from the libretro FBNeo fork with SUBSET=neogeo; it reads a MAME/FBNeo
 romset as it is and needs the MVS BIOS neogeo.zip in the system directory.
EOF

mkdir -p "$LAZYOS_STAGE"
DEB="$LAZYOS_STAGE/lazyos-libretro-fbneo_${PKG_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
log "FBNeo $CORE_REV (neogeo) -> $DEB"
