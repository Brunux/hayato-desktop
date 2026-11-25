#!/bin/bash
# Build the N64 libretro core from the pinned libretro repository.
#
# Debian has no N64 emulator at all, so this is the one core the image cannot
# take from apt. The build is the AArch64 dynarec + GLideN64 on GLES3, which is
# what Panfrost provides; Parallel-RDP is not used because the image's PanVK is
# a compute driver, not a Vulkan graphics implementation.
#
# Usage: build-mupen64plus-next.sh
# Output: $HAYATO_STAGE/hayato-libretro-mupen64plus-next_<rev>+hayato1_<arch>.deb
set -euo pipefail
umask 022
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

CORE_DIR="$DESKTOP_DIR/tools/mupen64plus-next"
CORE_REV="$(tr -d ' \t\r\n' <"$CORE_DIR/version")"
CORE_SHORT="$(printf '%.12s' "$CORE_REV")"
HAYATO_VERSION="$(tr -d ' \t\r\n' <"$DESKTOP_DIR/VERSION")"
CORE_TARBALL="mupen64plus-next-$CORE_REV.tar.gz"
CORE_URL="https://github.com/libretro/mupen64plus-libretro-nx/archive/$CORE_REV.tar.gz"
CORE_SRC="$SRC_DIR/mupen64plus-libretro-nx-$CORE_REV"
PACKAGING_REVISION="hayato1"
PKG_FILE_VERSION="${HAYATO_VERSION}+git.${CORE_SHORT}+${PACKAGING_REVISION}"
PKG_VERSION="1:${PKG_FILE_VERSION}"
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
	log "Extracting mupen64plus-next $CORE_REV"
	tar -xf "$SRC_DIR/$CORE_TARBALL" -C "$SRC_DIR"
fi

log "Building mupen64plus-next $CORE_REV (AArch64 dynarec, GLideN64 on GLES3)"
make -C "$CORE_SRC" platform=unix FORCE_GLES3=1 ARCH=aarch64 \
	CPUFLAGS="-march=armv8-a -mtune=cortex-a76.cortex-a55" GIT_VERSION="$CORE_REV" -j"${JOBS:-$(nproc)}"

PKG="$WORK_DIR/mupen64plus-next-pkg"
rm -rf "$PKG"
install -Dm755 "$CORE_SRC/mupen64plus_next_libretro.so" \
	"$PKG/usr/lib/$MULTIARCH/libretro/mupen64plus_next_libretro.so"
strip --strip-unneeded "$PKG/usr/lib/$MULTIARCH/libretro/mupen64plus_next_libretro.so" 2>/dev/null || true

SHLIBS_DIR="$WORK_DIR/mupen64plus-next-shlibs"
rm -rf "$SHLIBS_DIR"
mkdir -p "$SHLIBS_DIR/debian"
cat >"$SHLIBS_DIR/debian/control" <<'EOF'
Source: hayato-libretro-mupen64plus-next
Section: games
Priority: optional
Maintainer: Hayato <hayato@localhost>
Standards-Version: 4.6.0
Package: hayato-libretro-mupen64plus-next
Architecture: any
EOF
SHLIBS="$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O "$PKG/usr/lib/$MULTIARCH/libretro/mupen64plus_next_libretro.so" 2>/dev/null |
	sed -n 's/^shlibs:Depends=//p')"
[ -n "$SHLIBS" ] || SHLIBS="libc6, libegl1, libgles2, libstdc++6"
INSTALLED_SIZE="$(du -sk "$PKG/usr" | cut -f1)"

mkdir -p "$PKG/DEBIAN"
cat >"$PKG/DEBIAN/control" <<EOF
Package: hayato-libretro-mupen64plus-next
Version: $PKG_VERSION
Architecture: $DEB_ARCH
Maintainer: Hayato <hayato@localhost>
Installed-Size: $INSTALLED_SIZE
Depends: $SHLIBS
Provides: libretro-core
Section: games
Priority: optional
Homepage: https://github.com/libretro/mupen64plus-libretro-nx
Description: Mupen64Plus-Next libretro core (AArch64, GLES3)
 The N64 core the Hayato games library launches. Built with the AArch64
 dynarec and GLideN64 on GLES3 for the Mali-G57 through Panfrost; N64 is the
 experimental platform in the library.
EOF

mkdir -p "$HAYATO_STAGE"
rm -f "$HAYATO_STAGE"/hayato-libretro-mupen64plus-next_*.deb
DEB="$HAYATO_STAGE/hayato-libretro-mupen64plus-next_${PKG_FILE_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
log "mupen64plus-next $CORE_REV -> $DEB"
