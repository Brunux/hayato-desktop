# editxr

The Markdown editor the Notes menu opens a quick note in.

editxr is not in Debian: it is a Swift program shipped as one statically linked
musl binary for Linux aarch64. `version` and `SHA256SUMS` pin the release
tarball; `scripts/build-rootfs.sh` fetches it (cached in `$SRC_DIR`, verified
against the pin, like the kernel and the compositor) and hands it to
`scripts/rootfs-setup.sh`, which extracts `/usr/local/bin/editxr`.

## Why editxr

A note is Markdown, and editxr renders it as it is typed: headings, lists,
tables and code are styled in place, while the line under the cursor stays raw.
That is the same promise the Notes screen makes with its page beside the list,
so the editor is a continuation of the window rather than a different view of
the file. `--vault ~/Documents/Notes` makes `[[wikilinks]]` and `Ctrl+K` work
across the folder; `Ctrl+F` finds inside the note; `Ctrl+E` exports it as HTML.
The theme can be changed from `Ctrl+P`; Solarized is built in.

### Themes and the session palette

editxr's themes are a fixed enum, not a palette file, so it cannot take our
`theme.toml` tokens directly. The Notes menu maps the active LazyOS slug to the
editxr theme with the same name (Solarized, Nord, Gruvbox, Tokyo Night,
Catppuccin, Dracula, One Dark, plus GitHub/Monokai/Mono/Clay for a user theme)
and writes only `theme` and `appearance` into `~/.config/editxr/config.json`,
leaving every other setting alone (a first-run file also gets the three keys
editxr's decoder requires). The light/dark choice comes from the session
palette's background, so `catppuccin-latte` opens the light Catppuccin. A theme
with no editxr twin falls back to editxr's neutral System palette. A real
per-token match would need an upstream custom-theme feature; until then the
built-in twin is the closest honest fit. `Ctrl+P` changes the theme for that
window, and the next note open follows the session again.

It has no line-number argument, so a search hit opens at the top of the note and
`Ctrl+F` finds the match. The fallbacks in `system/notes.rs` (`$EDITOR`, nvim,
vi) still take `+N` for a person who installs their own editor.

The AI assist is opt-in and local-first: the default provider is the offline
mock, and nothing is sent anywhere until a person configures a provider in
`Ctrl+P` → **LLM settings**. The image ships it signed out and with no config.

## Bumping it

```sh
ver=1.9.3
curl -fL -o "editxr-v$ver-linux-aarch64.tar.gz" \
	"https://github.com/pixdeo/editxr/releases/download/v$ver/editxr-v$ver-linux-aarch64.tar.gz"
sha256sum "editxr-v$ver-linux-aarch64.tar.gz"    # -> SHA256SUMS
printf '%s\n' "$ver" > tools/editxr/version
```

The tarball is one file, `editxr`; nothing else changes.
