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

command -v cargo >/dev/null 2>&1 || die "cargo is not installed (run scripts/00-deps.sh)"

log "Building the session framework (hayato)"
BIN="$DESKTOP_DIR/rust/target/release/hayato"
NOTIFY_BIN="$DESKTOP_DIR/rust/target/release/hayato-notify"
GAMES_BIN="$DESKTOP_DIR/rust/target/release/hayato-games"
if [ "${SKIP_CARGO_BUILD:-no}" != "yes" ] || [ ! -x "$BIN" ] || [ ! -x "$NOTIFY_BIN" ] || [ ! -x "$GAMES_BIN" ]; then
	cargo build --release --locked --manifest-path "$DESKTOP_DIR/rust/Cargo.toml"
fi

[ -x "$BIN" ] || die "cargo finished but $BIN is missing"
[ -x "$NOTIFY_BIN" ] || die "cargo finished but $NOTIFY_BIN is missing"
[ -x "$GAMES_BIN" ] || die "cargo finished but $GAMES_BIN is missing"
# A binary that cannot report its version is not the one the image should
# carry: the check is one process and it catches a broken include_str!.
VERSION="$(tr -d ' \t\r\n' <"$DESKTOP_DIR/VERSION")"
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

# Package hayato, hayato-notify, hayato-games, the desktop system overlay, and
# the user skeleton into a Debian package for in-place apt upgrades.
PACKAGING_REVISION="1hayato1"
PKG_VERSION="${VERSION}-${PACKAGING_REVISION}"
DEB_ARCH="$(dpkg --print-architecture)"
PKG="$WORK_DIR/hayato-pkg"
rm -rf "$PKG"
install -d -m 0755 "$PKG/usr/bin" "$PKG/usr/share/hayato/games" \
	"$PKG/usr/share/hayato/etc/sway" "$PKG/usr/share/hayato/skel" \
	"$PKG/usr/libexec/hayato/check"

install -m 0755 "$BIN" "$PKG/usr/bin/hayato"
install -m 0755 "$NOTIFY_BIN" "$PKG/usr/bin/hayato-notify"
install -m 0755 "$GAMES_BIN" "$PKG/usr/bin/hayato-games"
cp -a "$DESKTOP_DIR/rust/crates/hayato-games/data/." "$PKG/usr/share/hayato/games/"

while IFS='	' read -r _name _path; do
	[ -n "$_name" ] || continue
	ln -sfn hayato "$PKG/usr/bin/$_name"
done < <("$BIN" --symlinks)

if [ -f "$DESKTOP_DIR/scripts/check-desktop.sh" ]; then
	install -m 0755 "$DESKTOP_DIR/scripts/check-desktop.sh" "$PKG/usr/libexec/hayato/check/30-desktop.sh"
fi

UI_FILES="$DESKTOP_DIR/uis/${UI:-sway}/files"
if [ -d "$UI_FILES/usr" ]; then
	cp -a "$UI_FILES/usr/." "$PKG/usr/"
fi
if [ -d "$UI_FILES/etc" ]; then
	install -d -m 0755 "$PKG/etc"
	cp -a "$UI_FILES/etc/." "$PKG/etc/"
	# /etc/sway/config is owned by sway/swayfx, /etc/yt-dlp.conf by yt-dlp, and
	# /etc/environment by libpam-modules; ship them under /usr/share/hayato/etc/
	# and install them in postinst so dpkg never reports a file conflict during
	# apt upgrades.
	if [ -f "$PKG/etc/sway/config" ]; then
		mv "$PKG/etc/sway/config" "$PKG/usr/share/hayato/etc/sway/config"
	fi
	if [ -f "$PKG/etc/yt-dlp.conf" ]; then
		mv "$PKG/etc/yt-dlp.conf" "$PKG/usr/share/hayato/etc/yt-dlp.conf"
	fi
	if [ -f "$PKG/etc/environment" ]; then
		mv "$PKG/etc/environment" "$PKG/usr/share/hayato/etc/environment"
	fi
fi
if [ -d "$UI_FILES/home" ]; then
	cp -a "$UI_FILES/home/." "$PKG/usr/share/hayato/skel/"
	if [ -d "$PKG/usr/share/hayato/skel/.config/bin" ]; then
		find "$PKG/usr/share/hayato/skel/.config/bin" -maxdepth 1 -type f -exec chmod 0755 {} +
	fi
fi
chmod -R go-w "$PKG"
if [ -f "$DESKTOP_DIR/tools/favnyr/SHA256SUMS" ] && [ -f "$PKG/usr/bin/favnyr" ]; then
	( cd "$DESKTOP_DIR" && sha256sum -c "$DESKTOP_DIR/tools/favnyr/SHA256SUMS" >/dev/null ) ||
		die "favnyr checksum mismatch against tools/favnyr/SHA256SUMS"
fi

EXTRA_BINS=()
[ -f "$PKG/usr/bin/favnyr" ] && EXTRA_BINS+=("$PKG/usr/bin/favnyr")
DEPS="$(deb_depends "$BIN" "$NOTIFY_BIN" "$GAMES_BIN" "${EXTRA_BINS[@]}")"
INSTALLED_SIZE="$(du -sk --exclude=DEBIAN "$PKG" | cut -f1)"
mkdir -p "$PKG/DEBIAN"
cat >"$PKG/DEBIAN/control" <<EOF
Package: hayato
Version: $PKG_VERSION
Architecture: $DEB_ARCH
Maintainer: Hayato <maintainers@hayato.org>
Installed-Size: $INSTALLED_SIZE
${DEPS:+Depends: $DEPS}
Section: x11
Priority: optional
Homepage: https://github.com/Brunux/Hayato
Description: Hayato Wayland desktop framework, greeter, notifications, and games library
 The multi-call hayato session binary, hayato-notify layer-shell notification
 daemon, hayato-games WebKitGTK host, Sway configuration, manuals, and desktop
 helpers for Hayato.
EOF

cat >"$PKG/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e

if [ -f /usr/share/hayato/etc/sway/config ]; then
	install -d -m 0755 /etc/sway
	install -m 0644 /usr/share/hayato/etc/sway/config /etc/sway/config
fi
if [ -f /usr/share/hayato/etc/yt-dlp.conf ]; then
	install -m 0644 /usr/share/hayato/etc/yt-dlp.conf /etc/yt-dlp.conf
fi
if [ -f /usr/share/hayato/etc/environment ]; then
	if [ ! -f /etc/environment ]; then
		install -m 0644 /usr/share/hayato/etc/environment /etc/environment
	else
		while IFS= read -r _line; do
			case "$_line" in
				'' | '#'*) continue ;;
			esac
			_key="${_line%%=*}"
			grep -qE "^${_key}=" /etc/environment 2>/dev/null || printf '%s\n' "$_line" >>/etc/environment
		done </usr/share/hayato/etc/environment
	fi
fi

# Preserve non-default desktop user home paths across apt upgrades: shipped
# Sway configs, skeleton configs, and .desktop entries reference /home/brunux by default.
_user="$(awk -F: '$3 == 1000 { print $1; exit }' /etc/passwd 2>/dev/null || true)"
[ -n "$_user" ] || _user="$(awk -F: '$3 >= 1000 && $3 < 60000 { print $1; exit }' /etc/passwd 2>/dev/null || true)"
_desktop_home="$(awk -F: -v u="$_user" '$1 == u && $6 != "" { print $6; exit }' /etc/passwd 2>/dev/null || true)"
if [ -n "$_desktop_home" ] && [ "$_desktop_home" != "/home/brunux" ]; then
	for _f in /etc/sway/config /etc/sway/config.d/*.conf /usr/share/applications/org.hayato.*.desktop \
		/usr/share/hayato/skel/.config/sway/config /usr/share/hayato/skel/.config/sway/config.d/*.conf \
		/usr/share/hayato/skel/.config/glow/glow.yml /usr/share/hayato/skel/.config/swaylock/config \
		"$_desktop_home/.config/sway/config" "$_desktop_home/.config/sway/config.d/"*.conf \
		"$_desktop_home/.config/glow/glow.yml" "$_desktop_home/.config/swaylock/config"; do
		[ -f "$_f" ] || continue
		if grep -q '/home/brunux' "$_f" 2>/dev/null; then
			sed -i "s|/home/brunux|${_desktop_home}|g" "$_f"
		fi
	done
fi

if [ -x /usr/bin/hayato ]; then
	/usr/bin/hayato --symlinks 2>/dev/null | while IFS='	' read -r _name _path; do
		[ -n "$_name" ] || continue
		ln -sfn hayato "/usr/bin/$_name"
	done
fi

rm -f /etc/fonts/conf.d/10-nerd-font-symbols.conf
if command -v fc-cache >/dev/null 2>&1; then
	fc-cache -f -s >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1 && [ -d /usr/share/icons/Hayato ]; then
	gtk-update-icon-cache -q -t -f /usr/share/icons/Hayato 2>/dev/null || true
fi
if command -v plymouth-set-default-theme >/dev/null 2>&1 &&
	[ -f /usr/share/plymouth/themes/hayato/hayato.plymouth ]; then
	plymouth-set-default-theme hayato >/dev/null 2>&1 || true
fi

_home="${_desktop_home:-/home/$_user}"
if [ -n "$_user" ] && [ -d "$_home" ] && [ -d /usr/share/hayato/skel ]; then
	if [ -d /usr/share/hayato/skel/.config/bin ]; then
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/.config/bin"
		for _f in /usr/share/hayato/skel/.config/bin/*; do
			[ -e "$_f" ] || [ -L "$_f" ] || continue
			_dst="$_home/.config/bin/$(basename "$_f")"
			if [ -L "$_f" ]; then
				rm -f "$_dst"
				cp -Pf "$_f" "$_dst"
				chown -h "$_user:$_user" "$_dst" 2>/dev/null || true
			elif [ -f "$_f" ]; then
				rm -f "$_dst"
				install -m 0755 -o "$_user" -g "$_user" "$_f" "$_dst"
			fi
		done
	fi
	if [ -d /usr/share/hayato/skel/.config/hayato ]; then
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/.config/hayato"
		for _sv in /usr/share/hayato/skel/.config/hayato/screensaver*.txt; do
			[ -f "$_sv" ] || continue
			install -m 0644 -o "$_user" -g "$_user" "$_sv" "$_home/.config/hayato/$(basename "$_sv")"
		done
	fi
	if [ -f /usr/share/hayato/skel/.config/foot/screensaver.ini ]; then
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/.config/foot"
		install -m 0644 -o "$_user" -g "$_user" \
			/usr/share/hayato/skel/.config/foot/screensaver.ini \
			"$_home/.config/foot/screensaver.ini"
	fi
	if [ -d /usr/share/hayato/skel/.config/opencode ]; then
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/.config/opencode"
		for _oc in opencode.json AGENTS.md; do
			if [ -f "/usr/share/hayato/skel/.config/opencode/$_oc" ]; then
				install -m 0644 -o "$_user" -g "$_user" \
					"/usr/share/hayato/skel/.config/opencode/$_oc" \
					"$_home/.config/opencode/$_oc"
			fi
		done
	fi
	if [ -d /usr/share/hayato/skel/.config/retroarch/autoconfig/udev ]; then
		install -d -m 0755 -o "$_user" -g "$_user" \
			"$_home/.config/retroarch" \
			"$_home/.config/retroarch/autoconfig" \
			"$_home/.config/retroarch/autoconfig/udev"
		for _ra in /usr/share/hayato/skel/.config/retroarch/autoconfig/udev/*; do
			[ -f "$_ra" ] || continue
			install -m 0644 -o "$_user" -g "$_user" "$_ra" \
				"$_home/.config/retroarch/autoconfig/udev/$(basename "$_ra")"
		done
	fi
	if [ -d /usr/share/hayato/skel/Pictures/Wallpapers ]; then
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/Pictures" "$_home/Pictures/Wallpapers"
		for _wp in /usr/share/hayato/skel/Pictures/Wallpapers/*; do
			[ -f "$_wp" ] || continue
			install -m 0644 -o "$_user" -g "$_user" "$_wp" \
				"$_home/Pictures/Wallpapers/$(basename "$_wp")"
		done
	fi
	for _d in Documents Downloads Music Videos; do
		install -d -m 0755 -o "$_user" -g "$_user" "$_home/$_d"
	done
	if [ -x /usr/bin/hayato-games ]; then
		su -s /bin/sh "$_user" -c 'HOME="$1" /usr/bin/hayato-games --ensure-layout' -- "$_home" >/dev/null 2>&1 || true
	fi
	if [ -x /usr/bin/hayato ] && [ -f "$_home/.local/state/hayato/theme" ]; then
		_slug="$(tr -d ' \t\r\n' <"$_home/.local/state/hayato/theme" 2>/dev/null || true)"
		case "$_slug" in
			'' | *[!a-z0-9-]*) ;;
			*)
				su -s /bin/sh "$_user" -c 'HOME="$1" /usr/bin/hayato theme set "$2" --headless --force' -- "$_home" "$_slug" >/dev/null 2>&1 || true
				;;
		esac
	fi
fi
exit 0
EOF
chmod 0755 "$PKG/DEBIAN/postinst"

DEB="$HAYATO_STAGE/hayato_${PKG_VERSION}_${DEB_ARCH}.deb"
rm -f "$HAYATO_STAGE"/hayato_*.deb
dpkg-deb --build --root-owner-group "$PKG" "$DEB" >/dev/null
log "hayato package -> $DEB"
