# Welcome to LazyOS

LazyOS is Debian (**forky**, arm64) with a keyboard-first `swayfx` desktop built
first for the **Google Hayato** Chromebook (ASUS Chromebook Detachable CM3,
MediaTek **MT8192** "Asurada" SoC, 4 GB RAM, 58 GB eMMC) and extended to generic
**ARM64 UEFI** (`arm64-uefi`) and **ARM64 U-Boot/SBC** (`arm64-extlinux`)
targets. On the Hayato, hardware support comes from our tuned `mediatek-mt81`
kernel (`7.2.7`, started from postmarketOS/Nura); on generic ARM64 targets,
Debian's packaged `linux-image-arm64` drives the hardware and Debian supplies
everything above the kernel.

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
  `rofi-learn` in a terminal.

## What is on the screen

| Piece | What it is |
|---|---|
| the bar (top) | waybar: menu, workspaces, clock, updates, tray, keyboard layout, Bluetooth, network, volume, brightness, battery. Left/right click and scroll do different things on each |
| compositor | **swayfx**, a fork of sway that draws rounded corners, shadows and a blurred backdrop under the translucent terminal, launcher and bar |
| the terminal | **foot**: solarized palette, no GPU renderer, about half the memory of an Alacritty window |
| the launcher | **lazyos**: applications (`lazyos menu apps`), windows (`lazyos menu window`), commands (`lazyos menu run`), and the 35 ported session menus rendered in native Ratatui TUI inside floating foot windows |
| notifications | **mako**, bottom of the top-right corner, under the bar |
| the session | `greetd` launches `lazyos-greeter` at boot and opens sway on login; swayidle runs the idle screen at 150 s and the lock at 300 s |

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

## The windows the framework draws

Some of the session's menus are already one program (`lazyos`, the same binary
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
**Symbols Nerd Font Mono** (`fonts-nerd-symbols`: Octicons, Codicons, Devicons,
Seti, Powerline, IEC) and **Font Awesome 6** (`Font Awesome 6 Free Solid` and
`Font Awesome 6 Brands Regular` from `fonts-font-awesome-legacy`), matching the
order named in `foot.ini` after JetBrains Mono. All 101 UI names — including
**Bluetooth**, **Wi-Fi**, the **battery**, the **keyboard**, the **touchpad**,
the **calculator**, a **QR code**, and **crop** — resolve to crisp codepoints
instead of tofu boxes or fallback tokens, while short ASCII tokens (`bt`, `wifi`,
`bat`, `kbd`, `pad`, `=`, `qr`, `crop`) remain compiled in as safety fallbacks if
a font is ever missing. `check-desktop.sh` (`lazyos --glyphs` /
`lazyos --glyph-font`) verifies every codepoint against the installed fonts.

A window fades in over a third of a second and nothing else animates;
`LAZYOS_REDUCED_MOTION=1` starts it at its final frame.

## The mark

The mark is a prompt: **z_**, a cyan `z` and an orange cursor, and the
wordmark is `lazyOS_`. The wallpaper is a keyboard in the dark with one lit
key carrying the mark - the key to press - and `lazyOS_`, `lazy by design` and
the arcade line in the corner. The colours are the session's own Solarized,
so nothing else had to be repainted. The launcher icon, the wallpaper and the
**About LazyOS** card are the same drawing - the card is **Alt+Space** >
**About**, and its last row opens `fastfetch` for the full picture. The lock screen keeps the mountain photograph
instead: the desktop is the surface the mark owns, and the photograph is what is
up when nobody is looking.

## The idle screen

150 s without input and the screen becomes **lazyOS_**: the z_ mark, the wordmark, `:: lazy by
design ::` and the arcade line `» press any key to play «`, drawn in cyan. One terminal-text
effect rebuilds the letters, then the next one does it differently, and so on while the machine waits. Any
key, any movement of the mouse or trackpad, or a tap on the touchscreen returns you to the
session, and the 150 s starts over.

At 300 s the screen locks instead: the wallpaper inside a base03 frame. The lock stops the animation
first and refuses to draw a second time over itself, and the suspend shortcuts lock the screen
*before* the machine sleeps, so nothing behind the lock is ever on screen.

The name is on purpose. **LazyOS** is the distribution, "lazy by design" is what it promises, and
the idea is the one the lazy tools share - lazygit, lazydocker: the tool does the boring part, and
the thing to press is on the screen.

## Where things live

| Path | Contents |
|---|---|
| `/etc/sway/` | the system half of the session: `config` plus `config.d/*.conf` drop-ins |
| `~/.config/sway/` | your half: it includes the system one and adds your own settings |
| `~/.config/bin/` | the commands the bar and the keys call (symlinks to `/usr/bin/lazyos` for ported surfaces, plus shell helpers) |
| `~/.config/{waybar,foot,rofi,mako}/` | the look of each piece |
| `/usr/share/doc/lazyos/` | these pages |
| `/etc/lazyos/` | device metadata (`deviceinfo`, `kernel-cmdline*.conf`, `installation`) read directly by the installer and boot helpers |
| `/usr/libexec/lazyos/` | installed runtime helpers (`install-boot.sh`, `backup-system.sh`, `restore-system.sh`, `install-kpart.sh`, fallback `install-emmc.sh`, and modular `check/*.sh`) |
| `~/Dev/LazyOS/` | optional **source checkout** (if present) organized as an umbrella over `core/`, `desktop/`, and `devices/` submodules |

Every running image works standalone without a source checkout: runtime helpers live
under `/usr/libexec/lazyos/` and read `/etc/lazyos/`. If you carry a checkout at
`~/Dev/LazyOS/` for development, rebuilds and validation use its `core/`,
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
