# Software

Debian **forky** with a normal `apt`. Standard Debian packages come from
`deb.debian.org/debian forky main contrib non-free non-free-firmware`, and
Hayato's own components (`hayato`, `hayato-core`, `hayato-device-<board>`,
`linux-image-hayato-<flavor>`, `swayfx`, `workstyle`, `retroarch`,
`hayato-libretro-fbneo`, `hayato-libretro-mupen64plus-next`) are distributed as
`.deb` packages through the Hayato Cloudsmith APT repository
(`/etc/apt/sources.list.d/hayato.sources`, signed by
`/usr/share/keyrings/hayato-archive-keyring.gpg`) and archived on GitHub
Releases. `sudo` does not ask for a password.

```sh
sudo hayato-apt enable                  # enable the Cloudsmith APT repository and refresh keys
sudo apt update                         # refresh the package lists
sudo apt install --no-install-recommends <package>  # add something without Recommends
sudo apt remove <package>               # remove it (add --purge for its configuration)
apt search <word>                       # find packages
apt show <package>                      # details, size, dependencies
sudo apt full-upgrade --no-install-recommends       # upgrade Debian and Hayato in place
sudo hayato-apt upgrade-from-github     # fallback: install the latest release .debs directly from GitHub
```

> **Kernel & boot safety:** `hayato menu software`, `hayato menu pkg`, and Setup bundles always pass `--no-install-recommends` and simulate every transaction (`apt-get -s`) first, blocking any UI operation that would pull in or remove `linux-image-*`, bootloaders, or core session packages. Note that running raw `apt` in a terminal bypasses that UI guard.

Nothing about this machine depends on a package manager other than apt: no
Flatpak, no Snap, no AUR. Every Hayato release publishes both full disk images
and `.deb` packages so an installed machine upgrades in place with `apt`.

## Hayato Software (`hayato menu software`)

**Hayato Software** is the session's intent-first graphical/TUI front end over Debian's native APT ecosystem: open it from the root menu (**Alt+Space** → *Software*), **Settings → System → Software catalog**, or by left-clicking the Waybar updates pill (right-clicking the pill still opens `hayato menu pkg` directly).

- **Discover** (*What do you want to do?*): 12 curated tasks (*Edit video*, *Program in Python*, *Write docs & spreadsheets*, *Draw & illustrate*, *Play games*, *Make or play music*, *Record screen & stream*, *Browse & stay private*, *System & disk tools*, *Web & communication*, *Edit photos*, *Code & git*) instead of hunting for Debian package names.
- **Search & All-APT toggle**: filter curated intents and apps as you type; press **Enter** to search Debian package names via `apt-cache search --names-only`, or press **Ctrl+A** (`All APT`) to include raw Debian matches alongside curated picks.
- **App Detail & Trust/Access**: shows version, candidate, download and disk delta from `apt-get -s`, APT origin (*Debian official*, *Contrib*, *Non-free*, or *Hayato Cloudsmith*), and derived capability hints (Network, Camera/Mic, Removable storage, GUI/Terminal), plus **Open**, **Install**, **Update**, or **Remove**.
- **Updates & Cleanup**: reviews upgradable packages (flagging security updates and held kernel packages — run `hayato update` for kernel upgrades) and measures reclaimable APT cache, `autoremove` orphans, `rc` residual configs, and user trash/thumbnails using `pkg_guard` safeguards.
- **Headless queries**: `hayato menu software --status`, `--updates`, `--rows`, `--search <query>`, and `--cleanup-dry`.

## What is installed, and where the lists are

The image is built from four package lists across the Hayato submodules
(`core/`, `desktop/`, and `devices/`):

| List | Holds |
|---|---|
| `core/packages/base.list` | what the system needs: systemd, NetworkManager, ssh, filesystems, audio plumbing, zram, the clock |
| `desktop/packages/tools.list` | the terminal tools (below) |
| `desktop/uis/sway/packages.list` | the session: sway, waybar, foot, mako, swayfx's dependencies, fonts |
| `devices/devices/<codename>/packages.list` | target hardware: firmware (`firmware-mediatek`, plus `firmware-atheros` on `google-kukui`), bootloader tools (`cgpt`, `vboot`, `depthcharge-tools` on `google-asurada`/`google-kukui`), and `linux-image-arm64` on `debian-arm64` targets |

Editing a list does not change the running machine; it changes the next image
that is built. For the machine in front of you, `apt install` is the answer.

## Games

The **Games** row opens the library (see `08-games.md`). It is one window over
two backends: RetroArch for ROMs in `~/Games`, and WebKitGTK for the HTML games
that ship with the image or that you write. The session packages are
`libwebkitgtk-6.0-4` (124 MB installed, six new packages, because GTK4 and
GStreamer are already here) and the libretro cores in
`desktop/packages/games.list` (about 43 MB). RetroArch itself is **not** Debian's
package: that one is 547 MB installed because `retroarch-assets` hard-depends on
`fonts-noto-extra` and it links Qt5. `desktop/tools/retroarch/` builds the same
emulator without Qt, assets, FFmpeg, Vulkan or the online updater, and
`desktop/tools/mupen64plus-next/` builds the N64 core Debian does not have.
Nothing in the image is a ROM or a BIOS.

The library itself is the Ratatui screen `hayato menu games`; the GTK +
WebKitGTK binary `hayato-games` is only the host that plays one HTML game
fullscreen. A USB or Bluetooth gamepad is handled by `hayato gamepad`, a
session helper started at login: it reads `/dev/input` and uses `wtype` (already
in `packages/tools.list` for dictation) to type arrows, Enter and Escape into
the framework's `hayato-*` windows, the Games library included. RetroArch and
the HTML games keep their own input. See `03-shortcuts.md`.

## The terminal

**foot**, themed in the active session palette, JetBrains Mono with a Nerd Font
fallback for glyphs. Selecting text with the mouse automatically copies it to
both the PRIMARY selection and the system CLIPBOARD (`selection-target=both` in
`~/.config/foot/foot.ini`), so you can immediately paste in any application with
`Ctrl+V` or `Ctrl+Shift+V` without pressing `Ctrl+Shift+C` first.

| Keys | Does |
|---|---|
| Mouse drag / select | auto-copy selection to clipboard and primary |
| Ctrl+Shift+C / Ctrl+Shift+V | copy / paste |
| Ctrl+Shift+N | new window |
| Ctrl+Plus / Ctrl+Minus | text size |
| Ctrl+Shift+PageUp / PageDown | scrollback |
| Shift+PageUp | scrollback too |

### kitty, if you install it

`sudo apt install kitty` gives a second terminal that arrives already in this
session's palette: `~/.config/kitty/kitty.conf` ships with the image and carries
the same tokyo-night colours as foot, JetBrains Mono 11 with the Nerd Font
fallback, 6 px of padding, 1000 lines of scrollback and 0.87 alpha, so the
compositor's blur shows through it exactly as it does through foot. Nothing here
starts it - `Alt+Return` and every menu open foot - so the file exists for one
reason: installing a terminal should not put a second, unthemed one beside the
first.

| Keys | Does |
|---|---|
| Ctrl+Shift+C / Ctrl+Shift+V | copy / paste |
| Ctrl+Shift+T | new tab (the tab bar appears with the second one) |
| Ctrl+Shift+N | new window |
| Ctrl+Shift+Z / Ctrl+Shift+X | previous / next shell prompt |
| Ctrl+Shift+Plus / Minus | text size (`Ctrl+Shift+Backspace` back to 11) |
| Ctrl+Shift+Up / Down | scrollback, one line |
| Alt+Q | close the window, which is sway's key and not kitty's |

Alt is the session's modifier and sway sees every Alt chord first, which is why
kitty keeps its own shortcuts on Ctrl+Shift. The one respect in which it differs
from foot: kitty draws JetBrains Mono's ligatures (`->`, `!=`), the same bytes
in a narrower column.

### The prompt

**starship**, in the shape of a powerline: one coloured block per question, left
to right - who you are, where you are, which branch and what the tree looks like,
which languages this directory has, which container, what time - then a second
line that is nothing but the character: cyan `❯` when the last command worked,
red `✗` when it did not. The blocks are tokyo-night (base3 on orange for the user,
base02/base01 for time and containers, cyan for git, blue for the languages) and
the separators are the triangles `U+E0B0`/`U+E0B2`, which is the form that looks
the same in foot and in kitty.

Two things are worth knowing before editing `~/.config/starship.toml`:

- Most of what you see in the prompt is a *private-use* character, and a
  character no installed font has is a box - silently, in some terminals and not
  in others. This image ships the DFSG-stripped Nerd Fonts set, which has no
  Nerd Fonts v3 `nf-md` glyphs at all, so the OS logo the upstream prompt carried
  is not here (and neither are the distro logos). Every glyph the shipped prompt
  uses comes from JetBrains Mono, Symbols Nerd Font Mono, Font Awesome or
  Material Design Icons, all named in `~/.config/foot/foot.ini`.
- `check-system.sh` reads the prompt, asks fontconfig which font draws each
  codepoint and FAILs when the answer is "none", or when the only family that has
  it is not one foot is given (that one renders small, from fontconfig's
  fallback). Add a glyph, run the report: it names the codepoint.

`starship prompt` prints what the prompt would look like, and `starship preset
nerd-font-symbols` is a starting point if you would rather build another one.

The shell is bash with a set of aliases and tools that are worth knowing:

| Instead of | Use | Note |
|---|---|---|
| `ls` | **eza** | `ls`, `lsa`, `lt` (tree), `lta` |
| `cat` | **bat** | `cat file`, with colours; `bat file` pages it |
| `find` | **fd** | `fd name` |
| `grep` | **rg** | not aliased: `rg pattern` |
| `cd` | **zoxide** | `cd` still works for paths; `z name` jumps by frecency |
| `Ctrl+R` | **fzf** history | `Ctrl+T` files, `Alt+C` directories |
| `ff` / `eff` | fzf + bat | pick a file with a preview / edit the pick |
| `t` | tmux | attach to the `main` session or start it |
| the prompt | **starship** | user, directory, git, language, container, time |

Also there: `btop`, `lazygit`, `tmux`, `gum`, `tldr` (cached cheat sheets),
`fastfetch`, `inxi`, `tmux`, `yt-dlp` (with `ffmpeg` to merge streams), `node`
and `npm`, `tesseract` (OCR), `zbarimg` (QR/barcode), `qrencode`, `socat`,
`inotifywait`, `whois`, `unzip`, `jq`, `less`, `man` (pages go through bat).
`swappy` annotates a screenshot and `wf-recorder` records the screen, both
through the capture menu (Alt+Ctrl+C).

**`cd -` is bash's, not zoxide's.** `cd` is the wrapper `zd`, which keeps real
paths working and jumps by frecency otherwise; a first argument of `-` goes
straight to bash's own `builtin cd -`, so it returns to the previous directory
and prints it instead of asking zoxide for a match. The shell initialises zoxide
at the very end of `~/.bashrc`, after starship and after `~/.bash_aliases`:
starship replaces `PROMPT_COMMAND`, and zoxide's directory-tracking hook has to
be appended to it last - initialised earlier, the hook is dropped and the first
`cd -` prints "zoxide: detected a possible configuration issue".

`man <command>` and `tldr <command>` are the two fastest ways to learn a tool;
`tldr` needs one fetch the first time (`tldr --update`).

## Editors

**nano** is the editor that is always there (`nano file`).

**editxr** is the note editor: **Alt+Space → Notes** opens a quick note in
editxr, which renders Markdown as it is typed and uses the notes folder as its
vault, so `[[wikilinks]]` and `Ctrl+K` work across the folder. `Ctrl+S` saves,
`Ctrl+Q` quits, `Ctrl+F` finds, `Ctrl+E` exports HTML, and `Ctrl+P` opens the
palette for themes (Tokyo Night is built in) and settings. It starts on the editxr
theme that matches the session — Tokyo Night, Nord, Gruvbox, Tokyo Night,
Catppuccin, Dracula or One Dark — and the Notes menu writes only those two keys
in `~/.config/editxr/config.json` before it opens the editor, leaving the
person's other settings alone; the Hayato themes with no editxr twin
(Everforest, Kanagawa, Rose Pine, …) fall back to
editxr's neutral System palette, because editxr's themes are a fixed set and it
has no custom palette file. `Ctrl+P` changes the theme for that window; the next
note open follows the session again. It is not `$EDITOR`, so
the shell keeps nvim for `eff`, `git commit` and `crontab -e`; a person who
exports `EDITOR` still gets their editor when editxr is not installed. editxr has
no line-number argument, so a search hit opens at the top of the note. Its AI
assist is opt-in and local-first: the image ships it signed out, and nothing is
sent anywhere until a provider is configured.

**nvim** is what `$EDITOR` names, so `eff`, `git commit` and `crontab -e` open
Neovim: `nvim file` for one file, `nvim` for the empty buffer. It brings its own
Lua runtime and tree-sitter, and it starts with no configuration - plugins, a
language server and a theme live in `~/.config/nvim`, which the image does not
write. **vim** is the other one (`vim file`), and it is what makes `/usr/bin/vi`
exist: `vi file` is the same editor, reading `~/.vimrc`.

The variable comes from `/etc/profile.d/90-cli-tools.sh` (a login shell and this
terminal read it) and only when nothing has set it already, so an `EDITOR` you
export yourself wins.

## Notes and documents

**Alt+Space → Notes** (also the launcher entry **Notes**, and `hayato-notes`)
is one list for the three document shapes this image has, each opened by the
tool that owns it. The selected note is read on the page beside the list; on a
narrow window the first line and the date sit in a two-line strip below it.
Typing filters the list (Escape clears the filter before it closes the window).

| Row | What it does |
|---|---|
| a note (`.md`, `.txt`) | Enter opens it in **editxr** in a foot window; quitting the editor returns to the list. The page shows its Markdown, **Ctrl+P** opens the whole note in `glow`, **Del** moves it to the trash |
| **New quick note** | asks for a title, writes `~/Documents/Notes/<date>-<title>.md`, and opens it |
| **Today's note** | opens `~/Documents/Notes/<date>.md`, creating it with the date as its heading |
| a `.xopp` ink note | Enter opens it in **xournalpp** |
| **New ink note** | a blank xournalpp document; save it in this folder (Ctrl+S) and it joins the list |
| **Sign or annotate a PDF** | picks a PDF from Documents or Downloads and opens it in xournalpp; save the `.xopp`, then export the signed PDF |
| **Export ink to PDF** | `xournalpp --create-pdf` writes `<name>.pdf` into `~/Documents/Signed` |
| **Search the notes** | ripgrep over the folder; Enter opens the note in editxr (`Ctrl+F` finds the match) |
| **Open folder** | the notes folder in Files |

The folder is the state: there is no index and no daemon. The first open creates
`~/Documents/Notes` with a `README.md` and `~/Documents/Signed` for exported
PDFs; deleting a note elsewhere simply removes it from the list.

**Reading is not note-taking.** `.epub` books open in **foliate** from Files or
`hayato open` (papers does not read EPUB), and `.pdf` still
opens in papers for reading. Files' **Tab** actions add the document rows for
the file under the cursor: **Annotate or sign (xournalpp)** for a PDF, **Read
(foliate)** for an EPUB, **Open in xournalpp** and **Export to PDF** for a
`.xopp`, and **Edit (nvim)** for text.

Both tools start with GTK's dark theme (`GTK_THEME=Adwaita:dark`), so their
chrome matches the session while a document page stays white.

`xournalpp` (8.8 MB, GTK3) and `foliate` (12.4 MB, GTK4/WebKit) are in
`uis/sway/packages.list`. The rootfs installs with `--no-install-recommends`, so
xournalpp's TeX Live recommends stay out: handwriting, PDF annotation and PDF
export work without the LaTeX tool.

## The browser

The image ships **Chromium** (`chromium`), bound to **Alt+Shift+Return**
and selectable from the launcher (`Alt+D`).

On Hayato's 1366×912 panel and 4 GB RAM budget, Chromium ships tuned for native
Wayland, GPU acceleration, low memory usage, and **Native Vertical Tabs** via
three conflict-free files:

- **`/etc/chromium.d/hayato`**: enables native Wayland + IME
  (`--ozone-platform-hint=auto --enable-wayland-ime`, exposing `app_id="chromium"`
  for idle-inhibit, touchpad tab gestures, and workspace glyphs), GPU zero-copy
  compositing (`--ignore-gpu-blocklist --enable-zero-copy`), a bounded
  renderer pool (`--renderer-process-limit=6`) and 64 MB disk cache, and
  enables `--enable-features=VerticalTabs,VerticalTabsLaunch` (disabling `LensOverlay`).
- **`/etc/chromium/policies/recommended/hayato.json`**: non-locking recommended
  defaults enabling Memory Saver (`HighEfficiencyModeEnabled`), Battery Saver
  on battery (`BatterySaverModeAvailability`), hardware acceleration, disabling
  persistent background processes when closed (`BackgroundModeEnabled: false`),
  and turning off speculative page preloading and telemetry.
- **`~/.config/chromium/Default/Preferences`**: seeds new profiles with
  `vertical_tabs.enabled: true` (reclaiming horizontal tab-strip height on the
  3:2 screen), balanced timer-based Memory Saver (`state: 2`, `aggressiveness: 1`,
  `time_before_discard_in_minutes: 10`), and tab freezing.

Because everything uses drop-in flags, **recommended** (not locked) policies,
and skeleton preferences, you can toggle vertical tabs or tune memory settings
freely in `chrome://settings` (or switch browsers anytime in **Settings →
System → Default apps**).

- **Fullscreen**: pressing **F11** or Sway's **Alt+F** gives 100% of the
  panel to web content and inhibits screen lock (`32-idle-inhibit.conf`).


## Emoji

**Alt+Ctrl+E** opens the emoji picker (the root menu and the launcher carry the
same row): type a name, Enter inserts the glyph at the cursor, **Ctrl+Y** only copies
it. The list is `~/.config/emojis.tsv`, and **every row is a glyph the shipped
font draws**: on the build host the table is filtered against the character map
of `fonts-noto-color-emoji`, so a symbol the font does not carry is never offered
instead of being inserted as an empty box. `~/.config/emojis.font` next to it
pins the SHA256 and the family of the font the filter used, which is how
`check-system.sh` notices a font upgrade; after one, re-running
`desktop/scripts/make-emojis.sh` on a host with `fontconfig` and the font rebuilds the
table. Nothing of this runs on the machine: the picker is one foot window and the
table is a file.


## Capture

The **Capture** menu (Alt+Ctrl+C, and the root menu) offers screenshots
(screen, area, window), annotation (swappy), OCR (tesseract), QR code reading
(zbarimg), and recording (wf-recorder). Each action ends with a card showing
the path or result. Recordings start/stop from the same menu and write to
`~/Videos/`; while one runs the bar carries the spinner and no card is drawn
into the file (normal notifications wait, critical ones still show).

**Recording audio** cycles no audio → microphone → system audio (line out), and
**Webcam overlay** toggles an mpv camera preview in the bottom-right corner.
Both are saved in `~/.config/hayato/record.conf`; the webcam is a normal
floating window, so `wf-recorder` captures it as part of the screen. For an area
recording the overlay is placed inside the selected area.

A notification that a file was written (a screenshot, an annotation, a saved
recording) is clickable: a left click opens the file's folder in Files, where
the file is one row away. The card never opens the file itself.




## QR Codes

The **QR** menu (**Share** or root menu → *QR*) generates Wi-Fi QR codes
or encodes arbitrary text. The Wi-Fi payload includes the SSID and password
(escaped for ZXing), rendered in-window via `qrencode -t ANSIUTF8`.



## Reminders

The **Reminders** menu (root menu / `hayato menu reminders`) creates transient
systemd user timers. The flow is: title → optional note → when (delays, date,
repeats). Every expression is validated by `systemd-analyze calendar`. The list
shows active reminders with cancel actions.



## Calculator

The **Calculator** menu (`Alt+C`, and the root menu) evaluates arithmetic
expressions with a Pratt parser (operators + - * / % ^, parentheses, constants
pi/e/tau, functions sin/cos/tan/sqrt/log/ln/abs/floor/ceil/round). History is
kept and can be copied.


## An external monitor

**Both USB-C ports carry DisplayPort, and the display is the DTB plus one
udev one-shot.** This port's device tree described no external-display pipeline
at all: `dpi@14016000` was disabled and the second ANX7625 bridge - the one on
`i2c@11d00000`, not the internal panel's - was missing, so the kernel created a
single connector and there was nowhere for a monitor to go. The patch
`kernels/mediatek-mt81/patches/mt8192-asurada-external-display.patch` describes
them (the DPI output with its pins, the bridge with its rails, its power and
reset GPIOs and its DPI input), and that is what makes `card1-DP-1` exist.

**The chip has one DisplayPort transmitter and two Type-C ports**, and which one
it reaches is a crosspoint (`TCPC_SWITCH_0/1`, 7-bit address `0x2c` on
`i2c-7`): `0x21` is the left port, `0x12` the right one, and the chip powers up
pointing at the left one. ChromeOS sets those registers from a typec-switch mux;
mainline's driver has no such code, so a monitor on the right port reports HPD,
answers EDID and gets a mode - and shows its no-signal test pattern, because the
lanes never reach it. The fix is not in the session: `/usr/local/sbin/hayato-dp-mux`,
run by `/etc/udev/rules.d/90-hayato-dp-mux.rules` when a port enters DisplayPort
alt mode, writes the two registers with `i2cset` (package `i2c-tools`). It has to
happen before the compositor enables the output, which is why it is a udev rule
and not a login hook: the Type-C event arrives about a second before the
connector's hotplug. Nothing is resident - one process while the plug is handled.

**A monitor can be plugged in at any time and into either port.** That is sway's
own output handling (`90-hayato-dp-mux.rules` sets the crosspoint, and
`91-hayato-display-hotplug.rules` runs `/usr/local/sbin/hayato-display-hotplug`
once and exits): a first-time monitor pops a quiet card offering **Arrange…**
(opening **Display Quick Setup** with `1`–`4` presets and workspace split) or
one-click **Mirror**, while previously configured monitors silently apply your
saved `40-display.conf` layout and resume wakes stay quiet. The new connector
gets a workspace and windows go there with `Alt+Shift+<arrow>`; unplugging
takes its workspaces back to the internal panel. The chip drives one
DisplayPort stream, so two monitors are one monitor followed by the other, not
two at once.

**Where it sits is Settings → Display arrangement** (`Alt+O`,
`~/.config/bin/hayato-display`): monitor on the right, on the left, above, below,
mirrored, plus the monitor's resolution and scale, and **Open wdisplays** for
what those rows do not cover (a custom position, a rotation) - that window opens
on a workspace of its own, because its two rectangles need the width. The
Settings row's detail is the menu's own `--status`, so the two cannot disagree.

**A position given at runtime is not remembered, and the menu is what remembers
it.** sway keeps `output position` until the session ends, so each arrangement is
also written to `~/.config/sway/config.d/40-display.conf`, which the session's
`~/.config/sway/config` includes: the monitor's resolution, scale and position,
and nothing else. The panel is never moved - the monitor is placed relative to
it, which is why "on the left" is a negative x (`swaymsg -- output DP-1 position
-1920 0`; the `--` is not optional, or swaymsg reads `-1920` as one of its own
flags). Only the monitor is written because it is the output that can be absent,
so a boot without it is untouched. **Only the monitor** and **Only the internal
panel** are deliberately not saved - a config that disables a screen would boot a
card with nothing on it - so they last until the session ends and the menu says
how to undo them. Delete the file to go back to sway's default (on the right).

**If the panel is the screen that is off, there are two ways back.**
"Only the monitor" turns the internal panel off for the session, so if the cable
then comes out - or the monitor loses its link - there is no picture at all.
**`Alt+Ctrl+Shift+D`** turns the panel back on and needs no picture to do it: it
runs `~/.config/bin/hayato-display --panel-on`, which is the reason the key exists,
because a menu cannot be drawn when every output is off - and sway allows
turning the last one off. When a screen *can* still draw (the monitor is present
but disabled, or the panel is off and the monitor is on), `Alt+O` →
**Display arrangement** carries the same row, **Bring the internal panel back**,
and the menu offers it even with no monitor plugged in. Over ssh,
`swaymsg output eDP-1 enable` (or `DSI-1` on MT8183 tablets) is the same command, and a reboot clears it too,
because the choice is never written to the saved arrangement.

**Workspaces per monitor** assigns the ten workspaces to the outputs you have.
**Split 1–5 / 6–10** puts the first five on the internal panel and the next five
on the external monitor; **All workspaces on …** puts all ten on one output. The
choice is written to `~/.config/sway/config.d/35-workspaces.conf` and applied
live with `swaymsg workspace N output NAME`, so existing windows move with their
workspace. **Clear assignments** removes the file and returns the ten
workspaces to the focused monitor.

**The night light does not reach it.** `wlsunset` asks the compositor for a
gamma table per output, and this kernel has one only for the internal panel's
CRTC: `modetest -M mediatek -p` shows `GAMMA_LUT` on the 1366x912 CRTC and
nothing on the 1920x1080 one, because MT8192's second display path
(OVL1 → RDMA1 → DPI) has no gamma block bound to it in mainline, and the SoC's
one gamma block sits in the first path. `wlsunset -t 3000` says so itself:
*gamma control of output ... failed*. Use the monitor's own colour setting; a
warm external display would need the driver to carry a gamma block on that CRTC.

**A suspend resets the bridge's crosspoint, and the session puts it back.** The
bridge powers down with the rest of the display pipeline and re-initialises on
the way back, so the crosspoint returns to its power-on default (the left port)
- measured on this machine: `0x12` before the sleep, `0x00` and then `0x21`
after it - while the kernel's Type-C stack only re-detects the partner. The
connector still says `connected` with its EDID and sway still has the mode, so
the picture goes to the empty port and the panel shows its no-signal pattern
until the cable is replugged. `~/.config/bin/hayato-dp-resume` runs from the
idle chain's `after-resume` and does the two things that fix it: it writes the
crosspoint for the port that has DisplayPort alt mode, then forces one modeset
(1280x720 and back), because a crosspoint write alone changes nothing - the link
was already trained when it ran. The mode change is deliberate: disabling the
output would move that output's workspaces to the internal panel, and a resume
hook should not rearrange the desktop. Nothing is resident, and
`journalctl -t hayato-dp-resume` says what it did.

## Files in a window

**Alt+Shift+F** opens **Hayato Files** (`hayato-filer`): the files
**Alt+Search** and **Alt+Ctrl+F** list, with a mouse. It is a second entry and
not a replacement, because the two answer different questions - a list is one
keystroke and no window, a window is what drag and drop, several files at once,
renaming in place and the trash need. The launcher (**Alt+D**) has both rows,
**Files** and **Files (window)**, the root menu (**Alt+Space**) lists them the
same way, and the window is a normal one: tile it, move it to another workspace,
close it with **Alt+Q**. `10-files.md` is its manual - the keys, tabs, the
staging shelf, archives, undo and Quick Properties.

* **A double click opens what the Files menu opens.** The window hands the path
  to `~/.config/bin/hayato-view` (`hayato open`), so text goes to `bat`, a
  picture to `imv`, a video to `mpv`, a PDF to `papers`, and an EPUB to
  `foliate`. Unhandled files—including **`.deb` packages**, **archives**
  (`.zip`, `.tar.*`, `.7z`, `.rar`), **disk images** (`.iso`/`.img`), files whose
  app isn't installed yet (like `.docx`/`.xlsx`), and Windows/Mac installers
  (`.exe`/`.dmg`)—open the **File Intent Router** (`hayato menu open`) instead of
  falling through to the browser, offering guarded installation, safe extraction,
  or curated Software alternatives. `/etc/xdg/mimeapps.list` stays the table for
  anything that asks `gio`.
* **It is one GTK4 binary, and the palette comes from the theme.** `hayato theme
  set` renders `~/.config/hayato/filer.css` from the same palette as every other
  surface (background window, cyan selection, JetBrains Mono) and signals the
  open window, which re-reads it in place; the icons are the session's Font
  Awesome alphabet rather than a PNG set. The window remembers the grid or list,
  the sort, hidden files, the sidebar and its size in
  `~/.config/hayato/filer-state.json`.
* **The terminal it opens is foot** (**Ctrl+Shift+T**, "Open Terminal Here"),
  started with the current folder as its working directory.
* **The trash is the FreeDesktop one**, written directly
  (`~/.local/share/Trash/{files,info}`) instead of through `gvfs`, so a deleted
  file can be restored with **Ctrl+Z** or from the Trash place. SMB and phone
  mounts are still not installed, so the Network places are off.
* **A disk it cannot unmount says why.** Mount, unmount and eject go through
  `udisksctl`, and when udisks refuses because something still has the mount
  open, the window asks `fuser` which process it is and names it (*In use by:
  ...*). Both come from packages the image installs (`udisks2`, `psmisc`), so
  the message arrives with the answer instead of only with the refusal.
* **The document reader gets the same palette, from a different pair of files.**
  `papers` is GTK4/libadwaita, which does not read `gtk-3.0/` at all: it asks the
  *settings portal* for the desktop's colour scheme, and this image runs no
  Settings portal (the only backend is the wlr one, for screenshots and screen
  casting). It therefore opened in libadwaita's light default, a white rectangle
  in a dark session. `~/.config/gtk-4.0/settings.ini` states the dark
  preference and `~/.config/gtk-4.0/gtk.css` paints the libadwaita named colours
  in the same tokyo-night values as the GTK3 file next door. Any other GTK4 or
  libadwaita application you install inherits it, and no portal daemon is
  installed for one boolean.
* **A document can be read, searched, zoomed and annotated; printing needs a
  print service.** "Print" in `papers` answers that it cannot find one, because
  the image ships no `cups` daemon (it is a system service, not a viewer). If you
  print, install it (`sudo apt install cups`) and configure a queue once.

## USB sticks and SD cards

A USB stick **mounts itself when you plug it in**: a clickable notification says
*USB drive ready — Click to browse files* (clicking the card or pressing `Alt+,`
opens the stick straight in Files), and it is also listed in **Places**
(`Alt+Space`) and in Hayato Files' **DEVICES** sidebar. **Ctrl+E** (or **Delete**) on a mounted
disk in Places unmounts it; eject before pulling the stick out. An SD card in the
internal slot waits as a **Mount** row instead.

* **Auto-mount is one udev event, not a daemon.** Plugging in a stick makes udev
  start `hayato-automount@<partition>.service`, which runs
  `/usr/local/sbin/hayato-automount` once and exits. There is no `udiskie`: that
  is a resident Python daemon (and a tray icon this bar has no host for), and an
  on-demand `udisksctl` is the same result without it. Nothing runs between two
  plugs. A stick that was **already inserted when the machine booted** waits in
  Places instead: the event fires before anyone is logged in, and the helper has
  no account to mount for (the same rule that keeps it out of the root
  filesystem), so unplug it and plug it back in - or pick the Mount row.
* **It mounts as you, not as root**, because udisks2 writes the *caller's* uid
  and gid into the mount options of a vfat or exFAT filesystem: a root mount is
  `/run/media/root/<label>` with `uid=0`, a stick you cannot write to. The helper
  drops to your account with `setpriv` (no PAM, no logind session, no second
  login), and a polkit rule
  (`/etc/polkit-1/rules.d/49-hayato-automount.rules`) authorizes that
  session-less caller through the `plugdev` group - which is why the account is
  created with it.
* **Only USB disks.** The rule matches the USB transport, so the eMMC and the
  internal microSD slot are never touched: they are the two disks this machine
  boots and rescues from, and `mmcblk1` is where the rescue card lives. A card in
  that slot is a **Mount** row in Places, one click - also the honest amount of
  ceremony for a card you may be about to repartition.
* **Settings → Auto-mount USB drives** turns it off: one line, `enabled=no`, in
  `~/.config/hayato/automount.conf`, and a missing file means on. With it off a
  stick waits as a **Mount** row, which is what an image without this feature
  did.
* A dirty vfat or exFAT stick mounts read-only and the notification says
  `(read-only)`: udisks does that rather than refusing it, and a stick nobody can
  write to looks broken until you know why. Unplug it, or run `sudo fsck.vfat -a
  /dev/sdX1` (with nothing mounted from it) and plug it back in.
* Mounting is **udisks2**'s job, because an ordinary session cannot mount
  anything itself (that is a root operation). It is D-Bus activated: nothing of
  it runs until a mount is asked for, and nothing stays afterwards.
* The same disks are in Hayato Files' sidebar (**Alt+Shift+F**), and a stick shows
  up there as soon as it is mounted.
* **Unmounting from the window goes through `udisksctl`, like the Places menu.**
  When udisks refuses because something still has the disk open, the window runs
  `fuser -m` to find out what and says so (*In use by: python3, mousepad*) - so
  `psmisc` is in `uis/sway/packages.list` and the refusal arrives with a reason.
  `eject` is not involved: it was pcmanfm's dependency and no program in this
  image execs it any more.
* `vfat`, `exFAT` and `NTFS` all work - the kernel has those drivers - so a
  stick from a camera or from Windows needs no extra package.
* **Auxiliary swap** is the row above it: it takes a card that is not a system card
  and turns the whole thing - or 2, 4 or 8 GiB of it - into a swap partition
  labelled `hayato_swap`, adds the `/etc/fstab` line that activates it at every
  boot, and turns it on now. "Not a system card" is the list: nothing mounted on
  it, no bootable kernel partition, and not the disk this system runs from. The
  card that installed the eMMC has one of those kernel partitions, so with it
  attached the row answers *Nothing to offer* - that is the guard, not a fault,
  and the same sentence names the reason. Repurposing such a card is a deliberate
  root action (`sudo wipefs -a /dev/mmcblk1`, which erases the rescue system too),
  never a menu row. The priority is 10 against zram's 100, so the
  compressed RAM is used first and the card is the spillover; `nofail` plus a 15 s
  device timeout keep a boot without the card from waiting or failing. It is a
  dedicated swap card, not a filesystem, so it never appears as a Mount row.
  Pulling it while it is active loses whatever pages live there: open the same row
  and choose **Turn it off** first (`swapoff`, then the fstab entry is removed; the
  partition stays, and **Format a disk** erases it). `resume=LABEL=hayato_swap` is
  the extra piece hibernate needs - see **The system** page.
* **Format a disk** is the last row of Places: it offers only disks with nothing
  mounted on them, no ChromeOS kernel partition, and that are neither built into
  the machine nor the disk this system is running from - so the eMMC and the card
  this system boots from are never candidates, even after a partition table is
  wiped - and it asks twice with Cancel first. The filesystem is exFAT (`HAYATO`,
  for a camera, a TV or another computer: Windows, macOS and Linux all read it,
  and unlike FAT32 it has no 4 GB file limit) or ext4 (for a Linux machine), and
  the whole disk becomes one partition. Internal system installation uses `hayato installer`
  (with `/usr/libexec/hayato/install-emmc.sh` preserved as a manual fallback); this menu is for sticks and cards. It is not
  stoppable: a killed `mkfs` leaves a disk that looks formatted and is not.
* A failed mount says why in the notification, in udisks' own words: "not
  authorized" is a policy problem, "already mounted" means it is already in the
  list above.



## Quick settings

The **Settings** menu (`Alt+O`, and the root menu) is grouped into seven sections:

- **Network** - Wi-Fi radio and networks (with a sign-in row when the network asks for one), Bluetooth radio and devices, DNS, Remote login (SSH), Share this screen.
- **Display & Sound** - Night light, Display arrangement, Workspaces per monitor, Rotation, Audio output, Volume per application (`hayato menu audio`), Media.
- **Appearance** - Theme, Wallpaper, Bar position, Bar density, Bar modules, Window corners, Window shadows, Window blur, Animations.
- **Input** - Touchpad, Keyboard layout, Tablet mode (with Calibrate rotation when on).
- **Power & Idle** - Battery, the power menu, and **Idle & lock** (screen-off, lock and panel-off timeouts, with a "Never" choice).
- **System** - Date & time (time zone and network time), Language & region, Hostname, Default apps, Change password, Clipboard history, Notifications, Do not disturb, System updates, Free space, Auto-mount USB drives, Login screen (Greeter), Translate.
- **Accessibility** - Reduced motion, saved to `~/.config/hayato/reduced-motion` and honored by new windows (the same file the design reads alongside `HAYATO_REDUCED_MOTION=1`), and the on-screen keyboard (`wvkbd-mobintl`) for typing without the hardware keyboard.

Each row shows the real state from the same probe as today and delegates to the existing pickers/panels (bar-net, bar-bt, bar-audio, bar-power, hayato-display, hayato-vnc, hayato-dns, hayato-ssh, hayato-tz, hayato-pkg, hayato-clean) instead of duplicating them. **←**/**→** (or **Tab**) move between the section rail and the item list, **[** / **]** step sections, **Ctrl+1..7** jump directly, typing searches globally across every section, and **Esc** unwinds (clear filter → rail → close while preserving the last selected section and item).

**Appearance → Theme** opens the existing theme picker. **Wallpaper** opens the Files list at `~/Pictures` so you can choose any PNG, JPEG, WebP, GIF, BMP or TIFF; the image is copied to `~/.local/share/hayato/wallpapers/` and a `36-custom-wallpaper.conf` drop-in points Sway at it. **Next preset** cycles the active theme's bundled wallpapers (and clears a custom one), while **Use theme wallpaper** appears when a custom image is set and returns to the preset. **Bar position** and **Bar density** edit `~/.config/waybar/config.jsonc` and signal Waybar, so the bar moves or tightens without a logout. **Bar modules** cycles Full → Compact → Minimal, rewriting `modules-right` (tray, updates, layout, Bluetooth, VNC, drives, VPN, Connect, and Cassette `custom/cassette` beside `custom/rec`: `Full` and `Compact` show Cassette; `Minimal` hides it) while keeping the date/time `clock` dead center. Live CPU (cores, load, thermal), RAM, active swap, disk (`/` and `~`), and uptime telemetry lives directly on the **`H_`** brand at the left end of the bar (`hayato sysmon --bar`): only the **`H`** shifts to warning/critical pressure colours while **`_`** keeps the accent colour, with **left-click** opening the root menu, **right-click** opening a terminal, and **middle-click** launching `btop`. **Window corners**, **Window shadows**, **Window blur** and **Animations** write `~/.config/hayato/appearance.conf`, render it into `~/.config/sway/config.d/41-appearance.conf` for the next login, and apply the same SwayFX commands through `swaymsg` now. **Accessibility → Reduced motion** stays the separate switch for the TUI windows; the compositor's animation duration is its own setting. **On-screen keyboard** starts `wvkbd-mobintl` at the bottom of the screen, or says it is already up.

**Input → Touchpad** writes `~/.config/sway/config.d/36-touchpad.conf` and applies the same values with `swaymsg input type:touchpad` immediately. It covers tap-to-click, natural scrolling, disable-while-typing and pointer speed; the file is the user override read after `/etc/sway/config`, so it wins over the system tap/natural-scroll defaults at the next login.

**System → Language & region** uses `localectl` and takes effect at the next login. **Hostname** uses `hostnamectl` and keeps the `127.0.1.1` line in `/etc/hosts` in step. **Default apps** lists what `gio mime` registers for the browser, file manager, text editor, image/video/music player and PDF viewer, and writes the choice with `xdg-mime` (plus `xdg-settings` for the browser). The browser choice is the one every path reads: the `$mod+Shift+Return` key, `xdg-open`, the captive-portal sign-in and Files' browser fallback all go through `hayato-browser`, which launches the desktop file `xdg-settings` names - so choosing another browser opens it instead of the image's `chromium` fallback. **Change password** uses `chpasswd` with the password on stdin, never on a command line.

**System → Notifications** sets the default timeout, how many cards are visible, the card width and the history limit in `~/.config/hayato/notify.conf`. The daemon reads that file once at startup, so saving a change restarts `hayato-notify` and clears the cards currently on screen; explicit app timeouts and critical cards are untouched. Cards from one sender group together and identical repeats collapse into one with a **×N** badge. **Alt+Comma** acts on the newest card, **Alt+Shift+Comma** dismisses it, and **Alt+Ctrl+Comma** clears the stack. **Clipboard history** pauses or clears the `cliphist` watcher through `~/.config/hayato/clipboard.conf`.

The VNC row shows the current sharing state. The display row shows the arrangement from `hayato-display --status`. The tablet mode row adds a "Calibrate rotation" sub-row when the mode is on.

The `--status` subcommand returns a one-line summary of all switches.

## Your profile, on a stick

**Alt+Ctrl+D** - also the root menu's **Developer** row and the launcher entry -
opens three actions: export your profile to a disk, restore an exported profile,
and save a diagnostics bundle. They are about the *account*, not the machine: the
keys, the dotfiles and the configuration the tools in this image read at login,
in one archive you can carry to another computer.

**Export my profile** asks for a disk (a mounted one, a partition it can mount
with udisks - the same mechanism as Places - or `~/Downloads` when no stick is
in), then opens **Passphrase & options** with masked passphrase confirmation
and an optional **`[ ] Include user home folder`** check (`Space`/`Enter`/click).
When unchecked (the default), it writes only the curated keys, dotfiles, configs,
agents, and Chromium profile; when checked (`--include-home`), it exports all files
and directories inside `$HOME` (skipping `~/.cache`, Trash, runtime sockets,
rootless container storage under `~/.local/share/containers`,
`~/.local/share/docker` and `~/.local/share/buildah`, and older profile archives
under the target folder so exports to `~/Downloads` do not nest themselves). The
container layers are owned by subuids and cannot be read by the user, so
including them would abort the archive with `Permission denied`. It writes
`hayato-profile-<host>-<date>.tar.zst.gpg` and a `.txt` manifest beside it
(`scope: profile` or `scope: home`).

* **It streams directly to the target drive**: `tar | zstd | gpg` writes one
  encrypted file straight onto the destination (`hayato-profile-...tar.zst.gpg`),
  so exporting even a full home folder uses no multi-gigabyte staging copy on local
  storage and never writes plaintext tar data onto the stick. A trap removes any
  partial file on cancel or `Ctrl-C`. The compression is deliberately light
  (`zstd -1`) and `gpg` is told not to compress again: a profile is mostly
  pictures, models and wine prefixes, where a heavier setting spends minutes to
  save nothing, while the text half still shrinks by about half.
* **It is a tar, not a copy of the files**, because the disks this menu is
  pointed at are usually exFAT (that is what **Format a disk** writes): exFAT
  has no permission bits and no symlinks, so a plain copy would leave
  `~/.ssh/id_ed25519` with no `0600`, and ssh refuses a key like that. The modes
  travel inside the tar, where the filesystem underneath cannot drop them.
* **What is inside**: `~/.ssh`, `~/.gnupg`, the shell files (`.bashrc`,
  `.bash_aliases`, `.profile`, `.bash_logout`), `.gitconfig`, the configuration
  of the tools this image ships (`foot`, `kitty`, `nvim`, `vim`,
  `starship.toml`, `glow`, `sway` drop-ins, `mako`, `gtk-3.0`,
  `gtk-4.0`, `hayato`, `favnyr`, `mpv`, `yt-dlp`, `go`, `pip`, `cargo`,
  `rustup`, `mimeapps.list`, `user-dirs.dirs`), `~/.local/bin`, the user's own
  applications, and the **agents**: the account directories of the Agents menu
  (`~/.local/share/hayato/agents`, sign-ins included), OpenCode's configuration
  (`~/.config/opencode`) and the CLIs' own homes - `~/.claude`, `~/.codex`,
  `~/.gemini`, `~/.copilot` and `~/.config/github-copilot` - with their
  credentials and settings; and the **Chromium profile** (`~/.config/chromium`):
  bookmarks, preferences, extensions and saved site logins.
* **What is deliberately not**: caches, Chromium *Service Worker* / *IndexedDB*
  / `ShaderCache` / `Code Cache*` / `GPUCache*` directories, `Singleton*`
  locks (close Chromium first so databases flush cleanly before archiving) - `node_modules`, the
  code in `~/Dev` (git remotes are its backup), the conversation transcripts and
  caches inside the agents' homes (`~/.claude/projects`, `~/.codex/sessions`,
  `~/.copilot/session-state`, `plugins/repos`…: history, not configuration), and
  `~/.local/share/opencode/opencode.db` - OpenCode keeps its sign-in there and on
  this machine it is 49 MB of session history, so sign in again on the machine
  that restored the profile; `~/.local/share/opencode/auth.json` travels when a
  layout keeps the credentials outside the database.
* The **manifest** (`...txt`, plaintext, no secrets) names the date, the host,
  the image revision, the paths, `apt-mark showmanual` and `npm ls -g`, so a
  stick can be read without the passphrase.
* From a terminal - including over ssh, which this machine answers - the same
  work is one command:
  `~/.config/bin/hayato-profile export /run/media/$USER/STICK` (add
  `--include-home` to carry the full home folder). It asks for the
  passphrase on the terminal, and `--passphrase-file FILE` makes it scriptable.

**Restore a profile** lists every archive it can see (mounted disks and
`~/Downloads`, showing `full home` when the manifest carries `scope: home`),
asks for the passphrase, and says how many files it is about to
overwrite - **Cancel is the first row**. Before extracting anything it backs up
the current profile paths and any existing files in `$HOME` that the archive
would overwrite into
`~/.cache/hayato/backups/profile-before-restore-<date>.tar.zst`, so a restore
that turns out to be the wrong archive is itself undoable. It then extracts into
your home with the modes from the archive (restoring both profile config paths
and any exported user home files) and says to log out and back in: the
shell, the session and the agents read their configuration at login. It cannot
be stopped once it starts - a killed extraction is a half-restored home.
By hand the archive is ordinary gpg and tar:

```sh
gpg -d hayato-profile-20261023-1200.tar.zst.gpg | tar --zstd -x -C ~
```

**Save a diagnostics bundle** writes `hayato-report-<host>-<date>.tar.zst` with
the output of `check-system.sh`, the last 3000 lines of this boot's journal, the
kernel ring buffer and one page of hardware and package facts. It is not
encrypted because it holds no keys; the journal does name Wi-Fi networks, paths
and errors, which is why the README inside says to read it before posting it. It
is the file to attach to a bug report - the same report `docs/test-checklist.md`
asks for after a fresh card.

## The night light

The session starts `wlsunset` at login: in **Sunset** mode it warms the screen
after dark and leaves it alone by day. **Settings → Display & Sound → Night
light** offers **Off**, **Sunset** and **Warm now**, plus 3500/4000/4500 K
warmth. The choice is written to `~/.config/hayato/night-light.conf`;
`~/.config/bin/night-light` (`apply`, `login`, `start`, `stop`, `resume`,
`status`, `config`) reads it, so the login line and the Settings row cannot
disagree. The remembered mode is one line in
`$XDG_RUNTIME_DIR/hayato-night-light`, so it dies with the session.

Suspend and resume are the part that used to lose it: the panel comes back with
its own gamma table, so a running `wlsunset` no longer has any effect and the
screen wakes up at daylight colour while the switch still says on. The idle
drop-in's `after-resume` hook asks the helper to restart the mode that was
running (`night-light resume`), and it does nothing when the night light was off
before the sleep. `night-light status` prints `on schedule`, `on warm` or `off`
from a terminal.

**An external monitor stays at its own colour temperature**: the kernel has a
gamma table only for the internal panel's CRTC, so the warm ramp cannot reach
it - see § *An external monitor*.

## Themes

The colours of this session come from **one file**, not from a dozen edited
configs: `theme.toml` holds the 16 terminal colours and the roles a surface asks
for (background, text, accent, danger…), and every other file is *generated*
from it — foot, waybar (including the uniform 4 px bar end insets matching widget spacing), mako, btop, lazygit, nvim, fzf, the lock screen,
the prompt (starship), the greeter, the window frame sway uses, and the wallpaper.

```
hayato menu theme           # interactive visual theme selector (Alt+Shift+T / hayato-theme-picker)
hayato theme list           # every theme, the active one marked *
hayato theme set gruvbox    # regenerate everything and reload the session
hayato theme next           # the next one in the cycle      (Alt+Ctrl+Shift+T)
hayato theme wallpaper next # the next wallpaper of a theme  (Alt+Shift+W)
hayato theme validate       # parse every built-in and check its contrast
hayato theme diff           # is every file still what the theme says?
```

Tokyo Night Dark is the default and the brand; the others are Catppuccin Mocha,
Tokyo Night, Rose Pine, Nord, Gruvbox, Everforest Dark and Kanagawa.

**The Visual Theme Selector (`hayato menu theme` / `Alt+Shift+T` / `hayato-theme-picker`).**
An interactive, dual-pane Ratatui surface for exploring themes with zero friction:
* **Left pane:** lists all installed themes with search-as-you-type filtering, active session markers (`●`/`○`), and accent badges.
* **Right pane:** real-time eye-candy preview canvas showing the 16 ANSI truecolor terminal swatches, semantic roles (surface, accent, ok, warn, danger), simulated Hayato desktop mockups (mini Waybar status strip, floating notification card, Starship shell prompt), WCAG 2.1 contrast ratio verification, and wallpaper info.
* **Zero friction:** type at any time to filter, press **Enter** to apply, **Ctrl+W** to cycle wallpapers, and **Ctrl+P** to toggle **live desktop peek** (temporarily previews the theme across all your real windows and workspace, clearing the filter or closing cleanly on **Esc** / committing on **Enter**).

**Every theme carries its own wallpaper.** The wallpaper is a gradient
composition generated from the theme's palette with the Hayato wordmark
centered, cached at `~/.cache/hayato/wallpapers/`. The first wallpaper is what
`theme set` installs as the desktop background and the lock screen image.

**The greeter and lock screen are themed.** At boot, `greetd` launches
`hayato-greeter` (`/usr/bin/hayato-greeter --session`), a native Ratatui login
surface running fullscreen in `foot` over a minimal `sway` compositor with a
solid theme background (no per-boot wallpaper generation or handoff flash, with
automatic VT7 console fallback if Wayland cannot start). It
centers a minimalist `HH:MM` clock, date, and single `Hayato` wordmark above a
tight, border-only login card (user pills + password box with no placeholder
hints or idle filler text) and a single quiet action row underneath (**Ctrl+T**
/ **F4** for themes, plus Sleep / Reboot / Shut down with Cancel-first
confirmation). To preview it inside an open session without logging out, run
`hayato greeter --preview` (press **Esc** to close); to revert to raw VT7
fallback in `/etc/greetd/config.toml`, add `--tty` to
`/usr/bin/hayato-greeter --session --tty`.

Defaults for the boot greeter live in `/etc/hayato/greeter.toml` and can be
configured interactively from **Settings → Login screen (Greeter)**, the root
menu (`Greeter settings`), `hayato menu greeter`, or `hayato greeter settings`:
the action buttons (`Preview`, `Reset defaults`, `Done`) sit at the top above
the defaults summary and five core settings (`Follow user theme`, `Default theme`,
`User pills`, `Clock format`, and `Power buttons`). The lock screen
(`swaylock`) uses the theme's colours for its indicator ring and the theme's
wallpaper as its background image.

**A generated file says so on its first line.** If you edit one of them, the
next switch will *report* the difference and leave your edit alone instead of
silently replacing it: `hayato theme set NAME` prints the differing lines and
exits 1, and `--force` replaces them. To keep a change, put it in the template
(`/usr/share/hayato/themed/` or `~/.config/hayato/themed/`) — that is what the
generator reads.

Two things a switch does not do. **A terminal that is already open keeps its
palette**, because foot reads its configuration when a window is created: open a
new window and it has the new colours. And a theme's own wallpaper art arrives
with the release that ships it; until then every theme names the same picture.

`hayato theme set NAME --headless` writes the files and asks nothing to reload,
which is what to use over ssh. The last generated body is kept in
`~/.local/state/hayato/theme/`, which is how "the template moved on" is told from
"a person edited this file".

### Frictionless in-window UX

Every interactive `hayato` window is self-contained and anticipates your primary
intent on open:

- **Immediate discovery without blocking first paint**: opening Wi-Fi (`menu net`)
  or Bluetooth (`menu bluetooth`) renders cached state immediately and triggers a
  live background scan automatically; opening Free Space (`menu clean`) measures
  reclaimable caches asynchronously; opening the Calculator (`menu calc`) evaluates
  expressions live as you type. Toggling a setting or rescanning updates the list
  in-place instead of closing the window.
- **In-window progress instead of external toasts**: short operations show an
  in-window braille spinner and status banner, while heavy multi-step operations
  (package install/upgrade, disk format, swap setup, profile backup/restore, media
  download, agent install) transition in-window into `TaskView` with step
  checklists, progress bars, and a persistent final card. External desktop
  notifications (`hayato-notify`) are reserved strictly for headless events with no
  active TUI window (hardware volume/brightness OSD, post-close screenshot or
  recording confirmation, in-place translation/dictation, and masked clipboard
  secret copy).
- **Consistent foundation and adaptive geometry**: menus share `draw_card` spacing,
  single-advance Nerd Font icons, case-folded filtering (`draw_filter_bar`), and
  scrollbars (`draw_scrollbar`), while resizing the floating window live via Sway
  IPC when switching modes (for example displaying an inline half-block Wi-Fi QR
  code in `menu net` or `menu qr`, or expanding `menu about` / `menu learn` into a
  wide reader).

## Translate

**Alt+Ctrl+T** translates **the text you selected with the mouse** - in the
terminal, in the browser, in any window - or, when nothing is selected, what is
on the clipboard. Nothing is opened first: the selection *is* the question, and
the key answers it in place.

The source language is always detected; everything goes into the language
**Settings → Translate** sets. Until that is set it is **English**, so an image
that was never configured answers the common case with one key. The Settings row
also says which language is in use, and the menu it opens lists the twenty-six
languages both engines take with the how-to on the same screen
(`~/.config/bin/translate config` from a terminal). The setting is one line in
`~/.config/hayato/translate.conf`:

    target=es

The translation is **copied to the clipboard** and shown in a card that names
the direction it went (`Spanish → English`). When it is longer than a few lines
the card is only the summary: a read window opens with the whole translation and
the selection underneath it, paged with `bat`, and Alt+Q closes it. A selection
cut at 3000 characters says so on the card. English into English is the one
direction that answers something already known, so that case says **Already
English** instead of echoing the text and points at Settings.

**The network** decides the rest. Google's JSON endpoint is asked first
(`clients5` with `client=dict-chrome-ex` - the same answer that detects the
language), and MyMemory without a key is the fallback, in 450-character pieces
because 500 is its ceiling. When neither answers, the card says so - *No answer
from the translator: is the network up?* - instead of showing half a
translation. The request is one `curl`; a `busy` card and the bar's spinner say
it is running, and **a click on the spinner stops it**.

Nothing is stored by this feature: no history file, no cache. The result is on
the clipboard, which means it is in the clipboard history (`Alt+Ctrl+V`) like
anything else copied, and the card is in mako's history - so a secret translated
is a secret in both, and the clipboard menu's last row clears that history.

`translate-shell` is deliberately not installed. It would have been one package
(277 kB), but its engine is Google's *other* endpoint, which answers this
machine with a captcha page; the package would be a command that never works
here. `check-system.sh` reports the tools this script does need (`curl`, `jq`,
`wl-paste`) and SKIPs the network probe when the machine is offline.

## A local model

**Alt+Ctrl+M** opens a model that runs on this machine: **Qwen3.5-0.8B**, four
cores, **no network and no account**. The window is an ordinary chat - type a
question, Enter sends it, Ctrl+C stops a long answer - and **Alt+Q** closes it.
It is also the root menu's **Local model** row, the launcher entry of the same
name, and `hayato-model` in a terminal (with several models it asks which one
with `fzf`).

The weights are **not in the image**: they are 517 MB (or 1.2 GB for the legacy
1.7B), and shipping them would bloat the rootfs. `hayato-model --fetch` downloads
the recommended one (Qwen3.5-0.8B weight-space programming, Q4_K_M) and checks
it against a pinned SHA256 before keeping it
(`~/.config/hayato/models.sha256`), the way `dictate-model` fetches whisper's;
`--verify` re-checks the files that are already there. They land in
`~/.local/share/hayato/models/`, so a rebuilt image keeps them. From the launcher
the same command runs in the model window, with the progress bar on screen.

Three measured numbers are the whole configuration, and the wrapper passes them
so no invocation has to remember them (`hayato-model --flags` prints them,
`--bench` re-measures `-t 2/4/8` on the machine; the full table is
`~/.local/share/hayato/models/BENCH.md`):

- **four threads, not eight.** llama.cpp asks the kernel for the core count and
  uses all of it: that measured **7.71 tokens/s** on 8 threads, because the four
  A55 cores are slower and drag every barrier. The A76 cluster alone measured
  **17.30 tokens/s** (+124% throughput over 8 threads). `taskset` keeps the desktop
  off those four cores too, which is why the window that asks the question stays responsive.
- **Q4_K_M quant with prompt pre-fill at 92.1 tokens/s.** At 516.8 MiB, the 0.8B
  model responds swiftly without the memory footprint or thinking delays of larger models.
- **a q8_0 KV cache behind flash attention**: 4k of context peaks at **1227 MiB**
  RSS (compared to 2356 MiB on 1.7B), leaving more than 1.3 GB free RAM beside the
  session and browser without swap pressure.

The upstream Qwen3.5-0.8B architecture features MIT CSAIL **weight-space programming**
(Rikiya Takehi et al.), which surgically cures pathological infinite thinking loops in
small reasoning models. By default, **reasoning is off** (`-rea off`) for instant answers;
setting `REASONING=on` in `~/.config/bin/hayato-model` enables chain-of-thought without
risk of repetitive loops.

What this machine will not do, so the window does not pretend: the Mali-G57 has
no compute path here (Panfrost is OpenGL 3.1, PanVK answers *Mali-G57 not
supported*) and there is no NPU, so nothing is offloaded to the GPU; and with no
`i8mm` the IQ-quants are out. A 3B model does not fit in 4 GB beside a browser.
Over 17 tokens a second is responsive dialogue - fast enough for real-time local assistance.

## Text in other languages


`fonts-noto-core` is installed, so Arabic, Hebrew, Devanagari, Thai, Armenian,
Georgian and the rest of Noto's non-CJK families have a face: a file name, a web
page or an EPUB in one of those scripts is text instead of a row of boxes, in the
terminal and in the browser alike. Nothing has to be configured - fontconfig's
fallback finds them.

Chinese, Japanese and Korean are **not** installed. `fonts-noto-cjk` is 91 MB
installed, the largest single item left after the browser, and a card that only
needs Latin should not pay for it. If you read those scripts:

```sh
sudo apt install fonts-noto-cjk
```

No logout is needed; fontconfig picks the new faces up on the next lookup, and
`fc-cache -f` forces it if an application cached the miss. `fc-list :lang=ja`
answers whether a Japanese face is there, and `check-system.sh` reports the
non-Latin case with `fc-list :lang=ar`.

## What is deliberately not installed

A GUI editor, an office suite and any Electron-style application. The image is
2.9 GB and every one of those would add hundreds of megabytes and a resident
process to a machine with 4 GB of RAM. If you want one, `apt search` it - the
session is a normal Wayland desktop and any GTK/Qt application works. The file
manager above is the one GUI exception, because it does two things a list
cannot; it costs 4.9 MB and starts no daemon.
