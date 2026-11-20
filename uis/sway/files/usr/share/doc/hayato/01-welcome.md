# Welcome to Hayato

Hayato is Debian (**forky**, arm64) with a keyboard-first `swayfx` desktop built
first for the **Google Hayato** Chromebook (ASUS Chromebook Detachable CM3,
MediaTek **MT8192** "Asurada" SoC, 4 GB RAM, 58 GB eMMC) and extended to the
**Google Kukui** Chromebook family (`google-kukui`, MediaTek **MT8183**: Lenovo
IdeaPad Duet `krane`, `kodama`, `kakadu`, `katsu`, and `jacuzzi`) alongside generic
**ARM64 UEFI** (`arm64-uefi`) and **ARM64 U-Boot/SBC** (`arm64-extlinux`)
targets. On **Hayato** and **Kukui**, hardware support comes from our tuned
`mediatek-mt81` kernel (`7.2.7`, started from postmarketOS/Nura); on generic
ARM64 targets, Debian's packaged `linux-image-arm64` drives the hardware and Debian
supplies everything above the kernel.

## The first five minutes

* Log in with the user account the image was built with (`brunux` by default,
  password `debian`). Change the password now: `passwd`.
* `sudo` does not ask for a password on this machine. Anything that needs root
  goes through it, including the scripts that rewrite the boot partition.
* Open a terminal: **Alt+Return** (or the bar's menu → Terminal).
* Open the launcher: the **Search key** (the magnifier, left of Tab) or
  **Alt+D**. Type part of a name, Enter.
* See every key binding: **Alt+Ctrl+K**.
* This manual: the **Learn** entry in the bar menu (**Alt+Space**), or
  `hayato-learn` in a terminal.

## What is on the screen

| Piece | What it is |
|---|---|
| the bar (top) | waybar, drawn as the session's statusline: menu, workspaces, clock, updates, tray, keyboard layout, Bluetooth, network, volume, brightness, battery. Left/right click and scroll do different things on each |
| compositor | **swayfx**, a fork of sway that draws rounded corners, shadows and a blurred backdrop under the translucent terminal, launcher and bar |
| the terminal | **foot**: tokyo-night palette, no GPU renderer, about half the memory of an Alacritty window |
| the launcher | **hayato**: applications (`hayato menu apps`), windows (`hayato menu window`), commands (`hayato menu run`), and the 35 ported session menus rendered in native Ratatui TUI inside floating foot windows |
| notifications | **mako**, bottom of the top-right corner, under the bar |
| the session | `greetd` launches `hayato-greeter` at boot and opens sway on login; swayidle runs the idle screen at 150 s and the lock at 300 s |

**The workspace pills say what is in them.** Each pill is the number of the
workspace plus one glyph per *kind* of window open there - a terminal, a
browser, a file manager, a picture, a video, a document - and a workspace with
nothing in it keeps its bare number. The glyphs are the same alphabet the menus
draw with, so a terminal in the bar and a terminal in the power menu are the
same picture. It is the one thing on this system that keeps running in order to
do it (`workstyle`, about 1 MB), because "where did I leave that window" is a
question only something watching can answer. The table is
`~/.config/workstyle/config.toml`: one line per application, matched against its
title or its app_id, and the next window or workspace change picks it up - no
restart, and an app with no line of its own shows a plain window outline.

**The bar is the session's statusline.** It floats a few pixels below the top
and lines up with the windows on both sides, with rounded corners and no edge
drawn around it, so it reads as a panel over the desktop rather than a strip
welded to the edge, and it borrows the grammar of the windows under it. It opens with the mark - `z_`, a
prompt - the `z` in the reading colour and the cursor in the accent, and the
mark is also the menu button. The current workspace is a filled rounded chip,
the way the mockup draws it. The only filled things on the line are that chip,
work in progress (the spinner and the task it names, which is also the button
that stops it) and a mode (resize, move). Everything else stays in the reading
colour: a state is the picture, never a hue. Offline, muted, a pending
update and a screen someone else is watching all say what they are with the
glyph and the tooltip, so nothing on the right end turns red or amber, and no
word rides beside an icon either - the right end is glyphs only. A switch
changes its picture: the icon while the thing is on, and the same icon with a
slash through it while it is off - Bluetooth, wifi and the volume all use that
one pattern, drawn from the Material Design Icons font the image ships. The
keep-awake cup is the one switch with a single glyph and the one indicator that
takes a hue: dim while the screen may sleep and the accent (the current
workspace's colour) while it is held awake. Hover any of them and the tooltip
says which state it is in. The battery is the icon and nothing else: its charge
is in the tooltip, like wifi's signal, Bluetooth's state and the update count,
so the line stays glyphs.

## The windows the framework draws

Some of the session's menus are already one program (`hayato`, the same binary
under every old name), and every window it opens has the same three bands, so a
window you have never seen is readable the first time:

| Band | What it says |
|---|---|
| top | what this is, in cyan; a second line of context when there is one; and the one number worth showing while it runs, right-aligned over a rule |
| middle | the list or the body. The selected row has a cyan bar down its left edge and its whole width filled; everything else is dim |
| bottom | the keys that are not obvious, in brackets: `[Enter] open   [Esc] close` |

The keys are letters and Enter, never Alt chords: sway sees every Alt chord
before the window does, so an Alt hint in a window would be a hint the session
swallows. Type to filter the list - **upper and lower case are the same**, so
`readme` finds `README.md` and `año` finds `AÑO`, and the list narrows while you
type instead of waiting for Enter; `Up`/`Down` (or `k`/`j`) to move, `Enter` to
open, `Esc` to close with nothing done. A click on a row does what moving to it
and pressing `Enter` does; fold the keyboard back and the rows become two lines
tall so a finger can hit one, and a click on the blank line between two of them
does nothing at all.

The pictures in these windows are one-cell glyphs drawn in `foot` from
**Symbols Nerd Font Mono** (`fonts-nerd-symbols`: Octicons, Devicons,
Seti, Powerline, IEC — excluding Codicons stripped in Debian `dfsg2`) and **Font Awesome 6** (`Font Awesome 6 Free Solid` and
`Font Awesome 6 Brands Regular` from `fonts-font-awesome-legacy`), matching the
order named in `foot.ini` after JetBrains Mono. All 101 UI names — including
**Bluetooth**, **Wi-Fi**, the **battery**, the **keyboard**, the **touchpad**,
the **calculator**, a **QR code**, and **crop** — resolve to crisp codepoints
instead of tofu boxes or fallback tokens, while short ASCII tokens (`bt`, `wifi`,
`bat`, `kbd`, `pad`, `=`, `qr`, `crop`) remain compiled in as safety fallbacks if
a font is ever missing. `check-desktop.sh` (`hayato --glyphs` /
`hayato --glyph-font`) verifies every codepoint against the installed fonts.

A window fades in over a third of a second and nothing else animates;
`HAYATO_REDUCED_MOTION=1` starts it at its final frame.

## The mark

The mark is a golden eagle, wings raised: a cyan eagle on a dark tile, drawn on
a pixel grid, and the wordmark is `Hayato_` with an orange cursor. The default
wallpaper is the same eagle at eight times the size with the wordmark and the
tagline **light. fast. fun.** The colours are Tokyo Night, the default theme,
so the mark and the session already agree. The launcher icon, the wallpaper and
the **About Hayato** card are the same drawing, sampled from one grid - the card
is **Alt+Space** > **About**, and its last row opens `fastfetch` for the full
picture. The lock screen keeps the mountain photograph instead: the desktop is
the surface the mark owns, and the photograph is what is up when nobody is
looking.

## The idle screen

150 s without input and the screen becomes **Hayato_**: the eagle in block
characters, the wordmark, `:: light. fast. fun. ::` and the arcade line
`» press any key to play «`, drawn in the terminal's own colour. One terminal-text
effect rebuilds the letters, then the next one does it differently, and so on while the machine waits. Any
key, any movement of the mouse or trackpad, or a tap on the touchscreen returns you to the
session, and the 150 s starts over.

At 300 s the screen locks instead: the wallpaper inside a base03 frame. The lock stops the animation
first and refuses to draw a second time over itself, and the suspend shortcuts lock the screen
*before* the machine sleeps, so nothing behind the lock is ever on screen.

The name is on purpose. **Hayato** is the distribution - an old Japanese word for a falconer, the
person whose bird comes back to the hand - and "light. fast. fun." is what it promises. It is a
word, not a theme: no Japanese glyph or motif appears anywhere in the interface. The eagle is the
mascot, and the three words are the whole philosophy: stay light, be quick to the thing you want,
and make the machine a place worth being in.

## Where things live

| Path | Contents |
|---|---|
| `/etc/sway/` | the system half of the session: `config` plus `config.d/*.conf` drop-ins |
| `~/.config/sway/` | your half: it includes the system one and adds your own settings |
| `~/.config/bin/` | the commands the bar and the keys call (symlinks to `/usr/bin/hayato` for ported surfaces, plus shell helpers) |
| `~/.config/{waybar,foot,mako}/` | the look of each piece |
| `/usr/share/doc/hayato/` | these pages |
| `/etc/hayato/` | device metadata (`deviceinfo`, `kernel-cmdline*.conf`, `installation`) read directly by the installer and boot helpers |
| `/usr/libexec/hayato/` | installed runtime helpers (`install-boot.sh`, `backup-system.sh`, `restore-system.sh`, `install-kpart.sh`, fallback `install-emmc.sh`, and modular `check/*.sh`) |
| `~/Dev/Hayato/` | optional **source checkout** (if present) organized as an umbrella over `core/`, `desktop/`, and `devices/` submodules |

Every running image works standalone without a source checkout: runtime helpers live
under `/usr/libexec/hayato/` and read `/etc/hayato/`. If you carry a checkout at
`~/Dev/Hayato/` for development, rebuilds and validation use its `core/`,
`desktop/`, and `devices/` submodules.

## Is everything healthy?

```sh
check-system.sh
```

It prints one line per check (`OK`, `WARN`, `FAIL`, `SKIP`), and every check maps
to a bug this port has already hit - a missing firmware, a key that a remapper
swallowed, the mixer's boot sequence, the backlight permissions, the clock. Run it
after a boot that behaved strangely, and keep the output when you ask for help.

## Power

* **Lock key** (top row, right-most): locks the screen, and systemd-logind
  suspends the machine a moment later. That path needs no polkit and no session
  state, which is why it always works. **Alt+Ctrl+L** locks without suspending.
* **Alt+Escape** (or **Alt+Ctrl+P**) opens the power menu: lock, suspend,
  hibernate, log out, reboot and power off.
* **Alt+Shift+Escape**: lock, then suspend. **Alt+Ctrl+Escape**: suspend without
  locking.
* The bar's battery module opens the power panel (charge, draw, time left,
  charger, uptime, memory and swap), and its **root menu** (**Alt+Space**) has log
  out, reboot and power off.
* Hibernate is offered only when a disk swap and `resume=` are both present; see
  **The system** page.
