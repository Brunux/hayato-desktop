# Games

The **Games** row in the root menu opens the library. The library is a
**Ratatui screen**, the same framework as every other LazyOS menu, so it looks
and navigates the same in its window and fullscreen. It lists two kinds of game:

- **retro games**: ROM files under `~/Games`, run by RetroArch with one libretro
  core per platform;
- **LazyOS games**: folders of HTML, CSS and JavaScript, played fullscreen in a
  **GTK + WebKitGTK** host. The sample is **Snake**.

The GTK/WebKit window exists only for playing an HTML game. The library itself
never loads a web page; its scan, ids and launch rules live in
`lazyos-games-core`, shared by the TUI and the host.

## Keys and gamepad

| Keys | Does |
|---|---|
| ← / → | previous / next tab: All games, Recent, Favourites, each console, LazyOS games |
| ↑ / ↓ | move through the games in the tab |
| Enter | play the selected game |
| type | filter the current tab by title or platform |
| f | favourite / unfavourite |
| s | rescan `~/Games` |
| Esc | clear the filter, then close the library |
| Alt+F | fullscreen on or off (the compositor) |

A gamepad is optional. The framework's own windows, including this screen, are
navigated by **`lazyos gamepad`**: it reads a USB or Bluetooth pad from
`/dev/input` and injects arrows, Enter and Escape with `wtype` into the focused
`lazyos-*` window. D-pad/left stick are the arrows, A/Start is Enter, B is
Escape. The same pad therefore moves the root menu, Settings, Files and this
library without a separate mode.

The Games library does **not** use the browser's Gamepad API; the HTML game
does. RetroArch reads the pad through udev and its own autoconfig, so a pad
never moves two things at once.

## Retro games

Put a ROM in a platform folder, then press **s** (or reopen the library):

```
~/Games/snes/Super Mario World.sfc
~/Games/nes/Metroid.nes
~/Games/gb/Tetris.gb
~/Games/psx/Final Fantasy VII (Disc 1).cue
```

The folder name does not matter; the extension selects the platform. A file is
ignored when it is smaller than the smallest real ROM for that platform, which
is what keeps a `README.md` out of the Genesis shelf (`.md` is also a Mega Drive
extension).

| Platform | Extensions | Core | Firmware |
|---|---|---|---|
| NES / Famicom | `nes`, `fds` | nestopia | `disksys.rom` for FDS |
| Super Nintendo | `sfc`, `smc`, `swc`, `fig` | snes9x | — |
| Game Boy / Color | `gb`, `gbc` | gambatte | — |
| Game Boy Advance | `gba` | mgba | optional `gba_bios.bin` |
| Genesis / Mega Drive | `md`, `gen`, `smd`, `32x` | genesis_plus_gx | — |
| Master System / Game Gear | `sms`, `gg` | genesis_plus_gx | — |
| PlayStation | `cue`, `chd`, `pbp`, `m3u`, `toc`, `ccd` | mednafen_psx | `scph5501.bin` (US), `scph5502.bin` (EU), `scph5500.bin` (JP) |
| PC Engine / TurboGrafx-16 | `pce`, `sgx` | mednafen_pce_fast | `syscard3.pce` for CD |
| Virtual Boy | `vb` | mednafen_vb | — |
| WonderSwan | `ws`, `wsc` | mednafen_wswan | — |
| Nintendo 64 | `n64`, `z64`, `v64` | mupen64plus_next | — |

Firmware goes in `~/Games/bios/`. It is not shipped: a BIOS is copyrighted, and
a PS1 game without one simply does not start. A zipped ROM is not unpacked by
the library; unzip it first.

**Nintendo 64 is experimental.** Debian has no N64 core, so the image builds
Mupen64Plus-Next with the AArch64 dynarec and GLideN64 on GLES3 for the
Mali-G57. Many games run at 640×480; some do not reach full speed. The library
marks an N64 row whose core is missing instead of pretending it can start.

RetroArch is the **slim** build in `tools/retroarch/`: Wayland, GLES, no Qt, no
assets, no FFmpeg, no Vulkan, no online updater. The host passes
`/etc/lazyos/retroarch.cfg` with `--appendconfig`, so your own
`~/.config/retroarch/retroarch.cfg` is still read first and the menu still saves
there. **F1** opens the in-game menu (RGUI in Solarized Dark); **Escape** leaves
the game and the TUI library reopens in a new window.

Saves are the emulator's: battery-backed RAM in
`~/.local/share/lazyos/games/saves`, save states in
`~/.local/state/lazyos/games/states`. Auto-load and auto-save are on, so a game
continues where you left it. A row with a save shows **save** in its detail.

## LazyOS games

A LazyOS game is a directory with a `game.toml` and an `index.html`. Put yours in
`~/.local/share/lazyos/games/web/<name>/` (user) or, for something that ships
with the image, `/usr/share/lazyos/games/web/<name>/`.

```toml
id = "snake"              # lower-case slug; it becomes the page's origin
title = "Snake"
entry = "index.html"      # relative, inside this directory
art = "art.svg"           # optional
author = "LazyOS"         # optional
description = "..."       # optional
```

The page is served from `lazyos-game://<id>/` by the GTK host, not `file://`.
That gives it a real origin, so `localStorage` and `IndexedDB` belong to that
game alone, and a `../` in a path is a 404. The host injects a small API at
document start:

```js
LazyOS.quit()            // leave the game (the TUI library reopens)
LazyOS.back()            // the same
LazyOS.exitCombo = false // a game where Select+Start is a move
```

`Ctrl+Q` and **Select+Start** (held 600 ms) leave a game; the TUI library
reopens when the host exits. The window is fullscreen, so a game should use
`100vw`/`100vh`, `overscroll-behavior: none` and `touch-action: none`; the
sample `Snake` in `/usr/share/lazyos/games/web/snake/` is a complete template
(canvas, keyboard, gamepad, touch, a high score in `localStorage`).

The game pages read the session palette from CSS variables in
`~/.config/lazyos/games.css`, written by `lazyos-theme` (surface `games`). A
theme switch repaints them on the next open. The TUI library uses the same
palette through the framework's design tokens.

## How the pieces fit

| Piece | Where |
|---|---|
| library screen | `lazyos menu games` (`lazyos-cli/src/system/games.rs`) |
| scan, ids, launch rules | `lazyos-games-core` |
| GTK + WebKit game host | `lazyos-games` (`desktop/rust/crates/lazyos-games/`) |
| sample game and theme fallback | `/usr/share/lazyos/games/` |
| user web games and saves | `~/.local/share/lazyos/games/` |
| library state (tab, position, favourites) | `~/.local/state/lazyos/games/state.json` |
| RetroArch (slim) | `desktop/tools/retroarch/`, `/etc/lazyos/retroarch.cfg` |
| N64 core | `desktop/tools/mupen64plus-next/` |
| other cores | `desktop/packages/games.list` (Debian packages) |
| controller | `lazyos gamepad` (`lazyos-cli/src/gamepad.rs`), `wtype` |

The library **exits before a game starts**: it starts `lazyos-games --play
<id>` (or the host starts RetroArch) and closes. When the game ends, the host
opens `lazyos menu games` in a new window with the saved tab and position. The
TUI and the game window are never alive together on a 4 GB machine.

## When something does not work

- **"Core not installed"** — the platform's libretro core is missing. The image
  installs them from `desktop/packages/games.list`; on a checkout run
  `sudo apt install libretro-snes9x` (or the core named in the row).
- **"RetroArch is not installed"** — the slim package is missing. Build and
  install it with `scripts/build-retroarch.sh` and
  `core/scripts/install-retroarch.sh`.
- **A PS1 game asks for a BIOS** — put `scph5501.bin` (US) in `~/Games/bios/`.
- **An N64 game is slow** — it is the experimental platform. Lower the core's
  resolution in the F1 menu; some games will not reach full speed.
- **The game window is blank or the GPU misbehaves** — start it with
  `LAZYOS_GAMES_NO_ACCEL=1` to turn off WebKit's hardware acceleration. 2D
  canvas games still work; WebGL games may not.
- **No sound in a game** — RetroArch uses the ALSA default PCM, which is
  PipeWire through `pipewire-alsa`; check `pactl info` first.
- **A ROM does not appear** — check its extension, its size floor, and that it
  is under `~/Games` (or a root in `LAZYOS_GAMES_ROM_DIRS`), then press **s**.
- **The controller does nothing** — see `06-troubleshooting.md`; the TUI uses
  `lazyos gamepad`, the HTML game uses WebKit's Gamepad API, and RetroArch uses
  udev.
