# Games

The **Games** row in the root menu opens the library. The library is a
**Ratatui screen**, the same framework as every other LazyOS menu, so it looks
and navigates the same in its window and fullscreen. It lists two kinds of game:

- **retro games**: ROM files under `~/Games`, run by RetroArch with one libretro
  core per platform;
- **LazyOS games**: folders of HTML, CSS and JavaScript, played fullscreen in a
  **GTK + WebKitGTK** host. The sample is **Snake**.

It is a **shelf**: a rail of platforms at the top, a card per game down the left
(the cartridge, the title, the core and the size, the path) and a **hero** on the
right with the selected game big — its generated monogram, platform, core, size,
folder, when you last played it, and a **PLAY** button. The window is wide
because the hero is the point: **1200×780** for the shelf, **1000×640** for the
dense list, and it re-centres itself when the layout changes. Under about 100
columns the hero steps aside and the shelf keeps the whole window.

The cartridge and the monogram are **drawn from the game's own title**, not
loaded from a file: a library with no cover art still looks like a shelf, and a
theme switch repaints it with everything else.

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
| **o** | **options**: sort, layout, covers, hide missing cores |
| **m** | **this manual**, inside the library: scroll it, Esc goes back |
| **r** | open the game's folder in Files |
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

## Options

**`o`** opens four settings, and only four — the rest are keys, not
preferences:

| Option | Values | Does |
|---|---|---|
| **Sort** | Title, Recent, Platform | the order of the shelf |
| **Layout** | Shelf, List | rich cards, or the dense one-line list |
| **Covers** | On, Off | the cartridge and the hero's monogram |
| **Hide missing cores** | Off, On | hide a game whose core is not installed |

↑/↓ pick a row, ←/→ (or Enter/Space) change it, **Esc** goes back. Every change
is saved in `~/.local/state/lazyos/games/state.json` with the favourites and the
position, so the library opens the way you left it, and the window resizes when
you switch between Shelf and List.

## The manual, inside the library

**`m`** opens a manual over the library: where a ROM goes, the platform table
with its extensions and core, the firmware files, the steps to play, the keys,
the save locations and the fixes for the common failures. ↑/↓ scroll a line,
**PgUp/PgDn** a page, **Home/End** the ends, and **Esc** (or `m`) goes back to
the list. It is the same information as this page, short enough to read without
leaving the window.

The table is not a copy: it is built from `platform::PLATFORMS`, the table the
scan itself uses, and each row asks the library whether that core is installed
— so a row that says **core missing** here is the same answer Enter gives.
Adding a platform to that table adds it to the manual, the scan and the launch
path at once.

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

**The folders are already there.** The first time the library opens it creates
one folder per platform and `bios/`, plus `~/Games/LAZYOS-GAMES.txt` with the
same table as below, so all you have to do is drop files in. Arcade romsets are
the exception to the extension rule: they stay zipped, and the **folder** names
the board (`neogeo/`, `cps1/`, `cps2/`, `cps3/`, `pgm/`, `arcade/`); a `.zip`
anywhere else is not a game.

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
| Neo Geo | `zip` in `neogeo/`, `mvs/`, `aes/` | fbneo | `neogeo.zip` (MVS BIOS) |
| Capcom CPS1 | `zip` in `cps1/` | fbneo | — |
| Capcom CPS2 | `zip` in `cps2/` | fbneo | `qsound.zip` |
| Capcom CPS3 | `zip` in `cps3/` | fbneo | `cps3.zip` |
| IGS PGM | `zip` in `pgm/` | fbneo | `pgm.zip` |
| Arcade (everything else) | `zip` in `arcade/`, `mame/`, `fbneo/` | fbneo | some boards |

Firmware goes in `~/Games/bios/`. It is not shipped: a BIOS is copyrighted, and
a PS1 game without one simply does not start. A zipped ROM is not unpacked by
the library; unzip it first — except a Neo Geo romset, which the core reads as
it is.

**Nintendo 64 is experimental.** Debian has no N64 core, so the image builds
Mupen64Plus-Next with the AArch64 dynarec and GLideN64 on GLES3 for the
Mali-G57. Many games run at 640×480; some do not reach full speed. The library
marks an N64 row whose core is missing instead of pretending it can start.

**Arcade is its own kind of game.** Debian has no arcade core either, so the
image builds **FBNeo** (`tools/fbneo/`), one core for every board it knows. A
game is the usual MAME/FBNeo romset — `mslug.zip`, `sf2.zip`, `kof98.zip`,
`dino.zip` — and the library lists the set's own short name as the title:

```
~/Games/neogeo/mslug.zip          # Neo Geo MVS
~/Games/cps1/dino.zip             # Capcom CPS1
~/Games/cps2/sf2.zip              # Capcom CPS2
~/Games/bios/neogeo.zip           # the MVS BIOS set, not the game
```

The folder names the board, so a set dropped in the wrong folder is listed under
the wrong platform (and the core will still refuse it). The BIOS zip must come
from the same MAME set and live in `~/Games/bios/`: `neogeo.zip` for MVS/AES,
`qsound.zip` for CPS2, `cps3.zip` for CPS3, `pgm.zip` for PGM. The romset must
match the core's driver names — a set renamed to `Metal Slug.zip` is not found —
so keep the short name. Saves are the core's NVRAM and states, in the same two
folders as every other platform.

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
