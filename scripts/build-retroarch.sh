#!/bin/bash
# Build the kiosk RetroArch from the pinned upstream tarball and package it.
#
# Debian's retroarch is not used: its assets package hard-depends on
# fonts-noto-extra (334 MB) and it links Qt5. This build is the same emulator
# without Qt, assets, FFmpeg, Vulkan or the online updater; RGUI is the only
# menu, and the LazyOS library is the UI that launches games.
#
# The package installs /usr/bin/retroarch and /etc/lazyos/retroarch.cfg (the
# --appendconfig overlay). The libretro cores are Debian packages from
# uis/sway/packages.list; N64 is tools/mupen64plus-next/.
#
# Usage: build-retroarch.sh
# Output: $LAZYOS_STAGE/retroarch_<version>+lazyos1_<arch>.deb
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

RETROARCH_DIR="$DESKTOP_DIR/tools/retroarch"
RETROARCH_VERSION="$(tr -d ' \t\r\n' <"$RETROARCH_DIR/version")"
RETROARCH_TARBALL="retroarch-$RETROARCH_VERSION.tar.gz"
RETROARCH_URL="https://github.com/libretro/RetroArch/archive/refs/tags/v$RETROARCH_VERSION.tar.gz"
RETROARCH_SRC="$SRC_DIR/RetroArch-$RETROARCH_VERSION"
PACKAGING_REVISION="lazyos1"
PKG_VERSION="$RETROARCH_VERSION+$PACKAGING_REVISION"
DEB_ARCH="$(dpkg --print-architecture)"

[ -f "$RETROARCH_DIR/SHA256SUMS" ] || die "no SHA256SUMS in $RETROARCH_DIR"

fetch() {
	if [ ! -f "$SRC_DIR/$RETROARCH_TARBALL" ]; then
		log "Downloading $RETROARCH_TARBALL"
		mkdir -p "$SRC_DIR"
		curl -fL --retry 3 --progress-bar -o "$SRC_DIR/$RETROARCH_TARBALL.part" "$RETROARCH_URL"
		mv "$SRC_DIR/$RETROARCH_TARBALL.part" "$SRC_DIR/$RETROARCH_TARBALL"
	fi
	( cd "$SRC_DIR" && sha256sum -c --ignore-missing "$RETROARCH_DIR/SHA256SUMS" ) ||
		die "checksum mismatch for $RETROARCH_TARBALL (delete $SRC_DIR/$RETROARCH_TARBALL and retry)"
}

fetch
if [ ! -d "$RETROARCH_SRC" ]; then
	log "Extracting RetroArch $RETROARCH_VERSION"
	tar -xf "$SRC_DIR/$RETROARCH_TARBALL" -C "$SRC_DIR"
fi

if [ ! -f "$RETROARCH_SRC/config.mk" ]; then
	log "Configuring RetroArch $RETROARCH_VERSION"
	(
		cd "$RETROARCH_SRC"
		./configure --prefix=/usr \
			--disable-qt --disable-x11 --enable-wayland \
			--disable-ffmpeg --disable-discord --disable-vulkan --disable-online_updater \
			--disable-xmb --disable-ozone --disable-materialui --enable-rgui \
			--enable-opengles --enable-opengles3 --enable-opengles3_1 \
			--disable-sdl2 --disable-jack --disable-oss --disable-cg --disable-cheevos \
			--disable-kms --disable-v4l2 --enable-egl --enable-udev --enable-pulse --enable-alsa
	)
fi

log "Building RetroArch $RETROARCH_VERSION"
make -C "$RETROARCH_SRC" -j"${JOBS:-$(nproc)}"

PKG="$WORK_DIR/retroarch-pkg"
rm -rf "$PKG"
log "Staging RetroArch $PKG_VERSION"
make -C "$RETROARCH_SRC" install DESTDIR="$PKG" >/dev/null
# The upstream default config is not this image's contract: the library passes
# /etc/lazyos/retroarch.cfg as an overlay instead.
rm -f "$PKG/etc/retroarch.cfg" "$PKG/usr/bin/retroarch-cg2glsl"
install -Dm644 "$RETROARCH_DIR/retroarch.cfg" "$PKG/etc/lazyos/retroarch.cfg"
strip --strip-unneeded "$PKG/usr/bin/retroarch" 2>/dev/null || true

# Debian's package name is `retroarch`; ours is the same name with a version
# that sorts above 1.22.2+dfsg-3, so installing this package upgrades it rather
# than fighting it.
SHLIBS_DIR="$WORK_DIR/retroarch-shlibs"
rm -rf "$SHLIBS_DIR"
mkdir -p "$SHLIBS_DIR/debian"
cat >"$SHLIBS_DIR/debian/control" <<'EOF'
Source: retroarch
Section: games
Priority: optional
Maintainer: LazyOS <lazyos@localhost>
Standards-Version: 4.6.0
Package: retroarch
Architecture: any
EOF
SHLIBS="$(cd "$SHLIBS_DIR" && dpkg-shlibdeps -O "$PKG/usr/bin/retroarch" 2>/dev/null |
	sed -n 's/^shlibs:Depends=//p')"
[ -n "$SHLIBS" ] || SHLIBS="libc6, libasound2t64, libegl1, libgles2, libpulse0, libudev1, libwayland-client0"
INSTALLED_SIZE="$(du -sk "$PKG/usr" | cut -f1)"

mkdir -p "$PKG/DEBIAN"
cat >"$PKG/DEBIAN/control" <<EOF
Package: retroarch
Version: $PKG_VERSION
Architecture: $DEB_ARCH
Maintainer: LazyOS <lazyos@localhost>
Installed-Size: $INSTALLED_SIZE
Depends: $SHLIBS
Provides: libretro-frontend
Section: games
Priority: optional
Homepage: https://www.retroarch.com/
Description: RetroArch for the LazyOS games library (slim build)
 A Wayland/GLES build without Qt, assets, FFmpeg, Vulkan or the online
 updater. The LazyOS library passes one libretro core and one ROM; the
 in-game menu is RGUI in the session palette.
EOF

mkdir -p "$LAZYOS_STAGE"
DEB="$LAZYOS_STAGE/retroarch_${PKG_VERSION}_${DEB_ARCH}.deb"
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
log "RetroArch $PKG_VERSION -> $DEB"
