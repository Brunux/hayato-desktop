# Games

The **Games** row in the root menu opens the library fullscreen. It is one
window over two kinds of game:

- **retro games**: ROM files under `~/Games`, run by RetroArch with one libretro
  core per platform;
- **LazyOS games**: folders of HTML, CSS and JavaScript, run in the same
  WebKitGTK window as the library. The sample is **Snake**.

The library is data, not a launcher script: the page asks for an id, and the
process that starts something resolves that id against its own scan. No page
ever names a path, a command or an argument.

## Keys and gamepad

| Key | Does |
|---|---|
| arrows / left stick, D-pad | move in the grid |
| Enter / A | play the selected game |
| F | favourite / unfavourite |
| S | rescan `~/Games` |
| `/` | search by title or platform |
| Q / Escape | quit the library |
| F11 | fullscreen on or off |

A gamepad is optional. The library reads USB and Bluetooth pads **directly from
`/dev/input` (evdev)**, so the grid is navigable even when WebKit's Gamepad API
does not see a pad; the games themselves use the browser's Gamepad API, and
RetroArch reads the pad through udev and its own autoconfig.

| Pad | Does |
|---|---|
| D-pad / left stick | move in the grid |
| A / Start | play the selected game |
| B | back: clear the search, then All games |
| Y | favourite / unfavourite |
| X | open the game's folder in Files |
| L1 / R1 | previous / next platform |
| Select + Start | quit the library (or leave a game) |

## Retro games

Put a ROM in a platform folder, then press **S** (or reopen the library):

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
assets, no FFmpeg, no Vulkan, no online updater. The library passes
`/etc/lazyos/retroarch.cfg` with `--appendconfig`, so your own
`~/.config/retroarch/retroarch.cfg` is still read first and the menu still saves
there. **F1** opens the in-game menu (RGUI in Solarized Dark); **Escape** leaves
the game and the library reopens behind it.

Saves are the emulator's: battery-backed RAM in
`~/.local/share/lazyos/games/saves`, save states in
`~/.local/state/lazyos/games/states`. Auto-load and auto-save are on, so a game
continues where you left it. A row with a save shows a small **save** badge.

## LazyOS games

A LazyOS game is a directory with a `game.toml` and an `index.html`. Put yours in
`~/.local/share/lazyos/games/web/<name>/` (user) or, for something that ships
with the image, `/usr/share/lazyos/games/web/<name>/`.

```toml
id = "snake"              # lower-case slug; it becomes the page's origin
title = "Snake"
entry = "index.html"      # relative, inside this directory
art = "art.svg"           # optional; shown in the grid
author = "LazyOS"         # optional
description = "..."       # optional
```

The page is served from `lazyos-game://<id>/`, not `file://`. That gives it a
real origin, so `localStorage` and `IndexedDB` belong to that game alone, and a
`../` in a path is a 404. The host injects a small API at document start:

```js
LazyOS.play(id)          // only meaningful in the library
LazyOS.quit()            // leave the game (the library reopens)
LazyOS.rescan()          // library only
LazyOS.favorite(id, on)  // library only
LazyOS.reveal(id)        // open the game's folder in Files
LazyOS.exitCombo = false // a game where Select+Start is a move
```

`Ctrl+Q` and **Select+Start** (held 600 ms) leave a game; the library reopens
when the process exits. The window is fullscreen, so a game should use
`100vw`/`100vh`, `overscroll-behavior: none` and `touch-action: none`; the
sample `Snake` in `/usr/share/lazyos/games/web/snake/` is a complete template
(canvas, keyboard, gamepad, touch, a high score in `localStorage`).

The library and the games read the session palette from CSS variables in
`~/.config/lazyos/games.css`, written by `lazyos-theme`. A theme switch
repaints them on the next open.

## How the pieces fit

| Piece | Where |
|---|---|
| library and game host | `lazyos-games` (`desktop/rust/crates/lazyos-games/`) |
| library page and sample | `/usr/share/lazyos/games/` |
| user web games and saves | `~/.local/share/lazyos/games/` |
| library state (recents, favourites) | `~/.local/state/lazyos/games/state.json` |
| RetroArch (slim) | `desktop/tools/retroarch/`, `/etc/lazyos/retroarch.cfg` |
| N64 core | `desktop/tools/mupen64plus-next/` |
| other cores | `desktop/packages/games.list` (Debian packages) |

The library exits before a game starts and `exec`s itself again when the game
ends. On a 4 GB machine that is the difference between an idle WebView and a
full-speed emulator: the two are never alive together.

## When something does not work

- **"Core not installed"** — the platform's libretro core is missing. The image
  installs them from `desktop/packages/games.list`; on a checkout run
  `sudo apt install libretro-snes9x` (or the core named on the row).
- **"RetroArch is not installed"** — the slim package is missing. Build and
  install it with `scripts/build-retroarch.sh` and
  `core/scripts/install-retroarch.sh`.
- **A PS1 game asks for a BIOS** — put `scph5501.bin` (US) in `~/Games/bios/`.
- **An N64 game is slow** — it is the experimental platform. Lower the core's
  resolution in the F1 menu; some games will not reach full speed.
- **The window is blank or the GPU misbehaves** — start the library with
  `LAZYOS_GAMES_NO_ACCEL=1` to turn off WebKit's hardware acceleration. 2D
  canvas games still work; WebGL games may not.
- **No sound in a game** — RetroArch uses the ALSA default PCM, which is
  PipeWire through `pipewire-alsa`; check `pactl info` first.
- **A ROM does not appear** — check its extension, its size floor, and that it
  is under `~/Games` (or a root in `LAZYOS_GAMES_ROM_DIRS`), then press **S**.
