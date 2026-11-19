# AGENTS.md — `lazyos-desktop`

## Scope
Wayland desktop environment (`uis/sway/`), multi-call Rust UI/CLI suite (`rust/`), SwayFX/Workstyle
packaging (`compositors/`, `tools/`), brand artwork (`brand/`), CLI packages (`packages/tools.list`),
and session verification (`scripts/check-desktop.sh` -> `/usr/libexec/lazyos/check/30-desktop.sh`).

Hardware board definitions, kernels, keymaps, and ChromeOS quirks belong in `lazyos-devices`.
Rootfs assembly, bootloaders, image creation, and eMMC installers belong in `lazyos-core`.

## Rust Workspace & Performance Rules
- **Always pin builds/tests on 8-core ARM64 hosts (MT8192)**: use
  `taskset -c 4-7 cargo <build|clippy|test> --manifest-path rust/Cargo.toml -j 4` so efficiency cores
  0-3 stay responsive for Wayland. (Do **not** hardcode `taskset` in CI scripts running on 4 cores.)
- Workspace crates under `rust/crates/`:
  - `lazyos-cli`: `lazyos` multi-call binary (`argv[0]` symlink dispatch + subcommands), `lazyos greeter`
    (pure-Rust `greetd` Unix socket client + Sway/inline fallback).
  - `lazyos-ui`: Ratatui shell (`App` + `Screen` + `BodyCtx`), design tokens (`design.rs`), Nerd Font
    `Glyph` table (`glyphs/`), `ActionStrip` / `TaskView` / `ActionRunner`, `Card` notification &
    in-window panels, terminal raw-mode cleanup (`Drop` + panic hook + signal handlers).
  - `lazyos-theme`: `theme.toml` palette compiler generating 14 surface configs idempotently.
  - `lazyos-mcp`: Read/propose-only MCP server (`never` add destructive apply tools).
  - `lazyos-notify`: Layer-shell desktop notification card daemon (`lazyos-notify`).
  - `lazyos-games-core`: the game scan, ids and launch rules, with no UI toolkit; shared by the
    Ratatui library and the GTK host.
  - `lazyos-games`: the GTK4 + WebKitGTK host that plays one HTML game fullscreen (`lazyos-games`).
  - `spike-sixel`: Sixel image preview probe.

## Enforced Architecture & Design-System Invariants
Verified by workspace tests (`cargo test`) **and** `scripts/check-desktop.sh`:
1. **`Color::Rgb` only in `design.rs`**: `lazyos-ui/tests/colours.rs` fails on `Color::Rgb` or raw
   hex colors anywhere outside `lazyos-ui/src/design.rs`. Widgets consume `ColorRole`.
2. **Shell owns `Frame`**: `lazyos-cli` implements `Screen` rendering into `BodyCtx`. Mentioning
   `ratatui::Frame` or `&mut Frame` inside `lazyos-cli/src/` fails `tests/frame.rs`.
3. **Zero allocations per steady-state frame**: `lazyos-ui/tests/no_alloc.rs` asserts `0` frame
   allocations when redrawing cached rows. Pre-format widget state on events/state changes, not per tick.
4. **No resident daemons except `cliphist`, `workstyle` and `lazyos gamepad`**: Never add polling
   daemons or background units in `uis/sway/`. Tool discovery runs async in-window on open; progress
   lives in `TaskView` or in-window `ActionRunner` banners (`lazyos-notify` is strictly for headless
   events). `lazyos gamepad` is the one input helper: it reads `/dev/input` and injects
   arrows/Enter/Escape into the focused `lazyos-*` window, because a USB or Bluetooth pad must
   navigate every framework menu, not only the Games library; its pid lock keeps it single.
5. **Dual density support**: `Screen::key` and `Screen::hit` receive `&Design` (`$XDG_RUNTIME_DIR/lazyos-tablet`)
   rather than caching hit boxes across normal vs tablet row spacing.
6. **Generated glyph & icon tables**:
   - Glyph codepoints (`Symbols Nerd Font Mono` + `Font Awesome 6 Free Solid` + `Font Awesome 6 Brands Regular`, avoiding Codicons `ea60..ec1e` stripped in Debian `fonts-nerd-symbols 3.4.0+dfsg2-1`): edit `scripts/make-glyphs.sh`, never hand-edit `glyphs/table.rs`.
   - `lazyos-notify` (`FontRenderer`) loads both `SymbolsNerdFontMono` and `Font Awesome 6` (`Solid`/`Brands`) and resolves UI glyph codepoints via `lazyos_ui::Glyph`.
   - Greeter console power buttons (`^Z`/`^R`/`^P`) invoke `systemctl <verb> -i` with stdio redirected (never inheriting raw TTY over Ratatui), authorized for local active `_greetd` via `/etc/polkit-1/rules.d/49-lazyos-greeter-power.rules`.
   - PNG app/notification icons: edit `scripts/make-icons.sh` tables and regenerate PNGs.
   - Brand SVGs -> PNGs: `scripts/make-brand.sh` (outputs `brand/apple-touch-icon.png`, wallpapers, wordmark).
   - Theme templates: edit `rust/crates/lazyos-theme/templates/` + `scripts/make-theme.sh` (Waybar outer end spacing lives in `.modules-left` / `.modules-right` to match `config.jsonc` `spacing: 4`, keeping edge pills inset cleanly even when terminal modules are hidden).
7. **Image codecs**: `image` crate features stay minimal (`png`, `jpeg`) to protect build time & binary size.
8. **Version invariant**: `VERSION` at the repo root is compiled into `lazyos --version` via
   `include_str!`. Keep it valid semver `x.y.z` matching releases.
9. **Multi-input cursor & contrast contract**: Screens with multiple text inputs (`InstallerScreen`,
   `GreeterScreen`, `GreeterSettingsScreen`, `ExportSetupScreen`) must render via `Input::render_field(..., focused, blink_on, bg_role, ...)`
   so only the active input shows the accent bar and blinking 500 ms cursor (solid when
   `reduced_motion`), and inputs inside `ColorRole::Surface` cards use `ColorRole::Bg` to preserve
   palette contrast across Solarized Dark (`Surface == Selection`) and light themes.
   Widgets never draw the raw `Dim`: `TextStyle::Dim` is `ColorRole::Muted` (4.5:1 on `Bg`), a
   card's border is `ColorRole::Border` (3:1), a selected row uses `SelectionText`/`SelectionDim`
   (4.5:1 on `Selection`), and the three status roles `Ok`/`Warn`/`Danger` are lifted to 4.5:1 on
   `Bg` because a status is text (“Done”, “Stopped”, an error summary); a tinted fill stays the
   palette's `danger_bg`, which is not derived.
   `Design` derives them once from the palette, never per frame, so a theme
   keeps writing `dim`/`selection` while the window draws what is readable on it;
   `every_builtin_theme_clears_the_derived_floors` fails a palette that cannot. The key bar draws a
   hint whole or not at all: it drops the middle to `…` and keeps the last hint (the way out) on
   the right.
10. **A recording is a file of the screen**: the capture menu starts `wf-recorder` as its own argv
   (`lazyos menu capture --start`, a detached re-exec; never a shell line, so the saved pid is the
   recorder), verifies `/proc/<pid>/comm`, and signals that exact pid (INT, then TERM). While
   `lazyos_ui::recording` reports a live recorder, `lazyos-notify` holds back non-critical cards
   (history still records them) because anything drawn is in the video; critical cards still show,
   and the "saved" card is sent only after the recorder is gone.
11. **A modal replaces the body; it never floats over a live list**: a `Confirm` or a
   `centered` card is drawn on its own screen (like the Ask strip and the kill screen), or the
   screen first clears the card area. At 80×24 a 56-wide card cuts list rows in half
   (`Hiber╭`, `Log o│t`); `tests/menu.rs` asserts the Power confirm contains no list text and
   the snapshots are regenerated at 80×24 and 152×45.
12. **One phase enum per surface**: a screen whose parts are mutually exclusive keeps one enum
   (`Mode` / `Stage` / `Phase`) that owns the data for that state. Do not encode "which mode" as a
   bool, a sentinel index, or a status string, and do not keep an `Option` beside a flag that says
   whether it is `Some`. Derived questions are methods (`is_recording`, `can_cancel`), transitions
   go through one `load` / `set_phase`, and IO stays at the edge. Current examples:
   `system/pick.rs` (`Current`), `menu/screen.rs` (`AnswerBody`), `system/pkg.rs`
   (`PkgStage::Installing(TaskView)`), `lazyos-ui/src/recording.rs` (`Recorder`),
   `system/theme_picker.rs` (`PreviewPhase`), `system/dev.rs` (`ExportFocus`),
   `files/find.rs` (`Stage`), `lazyos-ui/src/task.rs` (`StopPhase`),
   `system/media.rs` (`PlaybackState`), `lazyos-ui/src/widgets.rs` (`RunnerState`),
   `system/dictate.rs` (`DictationPhase`), `installer/worker.rs` (`WorkerPhase`),
   `installer/ui.rs` (`LocFocus` / `UserFocus` / `DiskFocus`), and
   `greeter/screen.rs` (`UserChoice` / `AuthPrompt`). Screens expose the phase
   through `Screen::phase()`; `LAZYOS_DEBUG_STATE=1` appends transitions to
   `$XDG_STATE_HOME/lazyos/state.log` (never stderr, which would paint over the
   frame).
13. **A file card opens its folder**: a notification about a written file carries the
   `x-lazyos-open-path` hint (`Card::open_path`, `lazyos notify --open`, or `BUSY_OPEN` for a
   busy result). A left click on the card body runs `lazyos-files <parent>`; grimshot predates the
   hint, so its absolute `app_icon` (the saved picture) is the fallback. The daemon never opens
   the file itself: Files is where the person decides what to do with it.
14. **The library is a TUI screen; GTK is only the game host**: the Games row runs
   `lazyos menu games`, a Ratatui screen in `lazyos-cli` (`system/games.rs`); the GTK + WebKit
   binary `lazyos-games` plays one HTML game and has no library mode. The scan, ids and launch
   rules live in `lazyos-games-core`, so neither UI owns them. The host gets a game id from the
   library's scan, never a path or a command; the library spawns it by name, so the first
   `lazyos-games` in `PATH` is the one that runs — a stale copy in `/usr/local/bin` shadows
   `/usr/bin/lazyos-games`, and `check-desktop.sh` fails when it does. The host's stderr goes to
   `~/.local/state/lazyos/games/host.log` (overwritten per launch) because the library closes as
   soon as the host is spawned: without that file a host that dies says nothing at all.
   `lazyos-game://` serves only inside that game's directory (canonicalise + prefix), and no ROM,
   BIOS or copyrighted art ships. The platform table is the only place a platform is declared:
   the scan (`platform::detect`), the manual's table and the launch core all read
   `platform::PLATFORMS`. A row with `folders` is claimed only
   inside those folder names — the six arcade boards are the case, because a `.zip` is a romset in
   `neogeo/`, `cps1/`, `cps2/`, `cps3/`, `pgm/` or `arcade/`. A row with `archive` is claimed by a
   `.zip` **only inside its own folder** (`nes/Mario.zip`): a zip's extension names no platform, so
   the folder schema is the whole rule, and RetroArch opens the archive and plays the first ROM
   inside it — hence one game per zip, said in the manual. A disc row is never claimed by a zip,
   because a `.cue` inside an archive cannot find the `.bin` files beside it. The first time the
   library opens it creates one folder per platform plus `bios/` and a generated
   `LAZYOS-GAMES.txt`, so a new person only drops files in. The library
   **exits before a game starts**; the host returns to a new `lazyos menu games` window, so the
   TUI and the game window are never alive together on a 4 GB machine. Web games are trusted
   local data; the sandbox and per-origin storage are the boundary, not a second shell.
   **`m` opens the library's manual in the same screen** (`Mode::Manual`): where a ROM goes, the
   firmware names, the keys, the saves and the common failures, wrapped to the window and
   scrollable. Its platform table is generated from `platform::PLATFORMS` and asks
   `Library::core_present` per row, so the manual cannot drift from the scan or from Enter;
   never write a second copy of that table by hand. The prose is a short form of
   `08-games.md`, which stays the long reference.
   The shelf is a master-detail surface: the rail is the platform, the shelf (or the dense list)
   is the game, the hero is the selected one. `o` opens the four options — sort, layout, covers,
   hide missing cores — stored in `State::options` so they survive the next game; `r` opens the
   game's folder in Files. The manual is markdown rendered by **glow** through the shared
   `system::manual` module (Learn uses the same renderer and ANSI parser), so its tables are real
   tables; it re-renders at the window width. The cartridge and the hero's monogram are generated
   from the title (`system/games/art.rs`), never an asset, so a library with no covers still looks
   like a shelf.
15. **A controller is the session's input helper**: `lazyos gamepad` reads `/dev/input` through
   evdev and injects arrows, Enter and Escape with `wtype` into the focused `lazyos-*` window
   (the root menu, Settings, Files, the Games TUI). The library is one of those windows; its tabs
   are Left/Right and its list Up/Down. Its own surfaces get three buttons — X options, Y manual,
   L1 the folder in Files — sent as **Alt+O / Alt+M / Alt+R**, never bare letters, because a bare
   letter would type into another screen's filter; the Alt combinations are unbound elsewhere.
   The table lives in `crate::padmap`, not in the mapper: the Controller screen (`menu controller`,
   Settings → Input, and the library's `o` row) edits it and writes
   `~/.config/lazyos/pad.json`, and the mapper reloads the file when its mtime changes, so a remap
   needs no restart and works with no pad connected. The defaults are the console layout and cover
   a DualShock 4 (Cross confirm, Circle cancel), which pairs over Bluetooth (PS + Share) like any
   other pad; a DualShock 3 is the USB exception (BlueZ's sixaxis plugin writes the host address),
   never a scan. The mapper never touches RetroArch or the HTML host: they read the pad themselves
   (udev and the browser Gamepad API), so a press cannot move two things. RetroArch's own mapping
   ships as a udev autoconfig profile in the home tree
   (`~/.config/retroarch/autoconfig/udev/`), which is the directory `joypad_autoconfig_dir`
   already names; without it RetroArch logs `Autoconfig: … not configured` and falls back to the
   kernel's button order. The screen shows the charge `hid-playstation` publishes
   (`ps-controller-battery-<address>`) and says to charge a pad below 15 %, because a flat DualShock
   4 connects, lights up and hangs up within seconds and otherwise reads as a pairing failure. It is
   the one resident input helper; a pid lock keeps it single. The game stack itself is built, not
   installed from Debian: `tools/retroarch/` builds the slim emulator (no Qt, assets, FFmpeg, Vulkan or updater)
   and `tools/mupen64plus-next/` builds the N64 core Debian does not have; `tools/fbneo/`
   builds **FBNeo** (`REGEN_HEADERS=1`, the libretro fork) for every arcade board, because Debian
   has no arcade core either. The cores in `packages/games.list` Depends on
   `retroarch`, so `install-retroarch.sh` installs the LazyOS package **first** and the cores
   after it; installing the session list first would pull Debian's retroarch, its 547 MB assets
   and Qt5.
16. **The game stack is built, not installed from Debian**: `tools/retroarch/` builds the slim
   emulator (no Qt, assets, FFmpeg, Vulkan or updater) and `tools/mupen64plus-next/` builds the
   N64 core Debian does not have. The cores in `packages/games.list` Depends on `retroarch`, so
   `install-retroarch.sh` installs the LazyOS package **first** and the cores after it; installing
   the session list first would pull Debian's retroarch, its 547 MB assets and Qt5.
17. **Notes is a shelf beside a page, and the folder is still the state**: `system/notes.rs` reads
   the selected note once when the selection changes (`refresh_preview`) and caches the wrapped
   Markdown (`wrap`), so the draw loop copies strings and does no IO or allocation per frame. The
   page sits beside the shelf at 100 columns and wider and becomes a two-line strip on the 80×24
   window; the title prompt owns the shelf and swaps the key bar, so a key never means two things.
   Typing filters the list and the counter shows `visible / total`; Escape clears the filter before
   the shell sees it. Quick notes open in **editxr** (`tools/editxr/` pins the static aarch64
   release; core's rootfs setup installs it), with the notes folder as its vault; a search hit opens
   at the top because editxr has no line argument. `tests/notes.rs` snapshots both grids and is the
   layout's witness.

18. **One header grammar and no duplicate titles**: the header is `<product> · <surface>` on the
   left, the status on the right, and the card title is the context (the question, or what the
   card holds) - never the surface's own name. A picker whose question repeats its name draws no
   card title. The menu draws one heading per group (`Group::heading`) from a `Row::heading`: it
   is a label with a rule, never a cursor stop or a click target, and it leaves the list as soon
   as a filter is typed. `ListState` skips headings on every move, and `tests/menu.rs` snapshots
   the result at 80×24 and 152×45.

19. **A dead end offers the next step**: a finished task draws its outcome as a card in the
   outcome's colour (green Done, red Failed, amber Stopped), and a failure offers `c` (copy the
   error) beside `l` (focus the log). The log pane carries a scrollbar and `↑↓`/`End` in the key
   bar. A search whose tool is missing (`fd`, `ripgrep`) offers `Ctrl+I`, which opens
   `lazyos menu pkg --search <package>` in a new window; the key bar drops the hints that cannot
   work (`open`, `copy path`) while the tool is missing. `tests/files.rs` proves the hand-off and
   the task snapshots prove the card.

20. **One legend per input device, and no state by colour alone**: an `ActionStrip` is the
   pointer/touch path and a key bar is the keyboard legend; a screen may carry both when each is a
   different path (About, Learn), but a screen with no pointer path draws no strip. Neither cuts a
   label — the strip drops a whole pill that does not fit, the key bar drops a whole hint. A state
   is always also a word (`Done`, `Failed`, `Stopped`, `connected`) or a number (`42%`); the
   selected row is an accent bar plus a bold label, never the fill alone. Motion that carries state
   (the progress spinner) is exempt from reduced motion because it is essential progress; the text
   cursor and every decorative animation honour it. The mechanical half of the rule is the derived
   floors: a status colour that cannot be read fails `cargo test`. A picker whose rows come from
   the machine builds them in a `questions(...)` function that takes that data as arguments
   (`places::questions(&places, note)`, `agents::questions(&installed, &accounts, default, note)`),
   so `tests/pickers.rs` draws the real screen at both grids without a mounted disk, a clipboard
   or an agent on `PATH`; `menu()` calls the same function and nothing else builds rows. A screen
   that writes to the terminal while being constructed gets a silent constructor for the net
   (`ThemePickerScreen::with_design_silent` skips the OSC preview), and one that reads the host
   takes its data as an argument (`InstallerScreen::from_preflight_isolated(preflight, true)`,
   `GreeterSetup` plus `set_clock_for_test`) instead of going untested. **Every screen is in
   `tests/screens.rs` or `tests/pickers.rs`; a new one lands with a render.**

## Developer Profile Export
- `lazyos-profile export --include-home` streams the whole home through tar. The single
  `home_excludes` list in `bin/lazyos-profile` is used by both the `du` size estimate and the
  `tar` command, so the two never disagree.
- It must skip rootless container storage (`~/.local/share/containers`, `~/.local/share/docker`,
  `~/.local/share/buildah`): those layers are owned by subuids, unreadable to the user, and tar
  aborts the whole archive with `Cannot open: Permission denied` after copying the rest.
  `check-desktop.sh` reproduces that with a `000` directory and asserts `Documents` still travels.

## Shell State Protocols
The session helpers keep runtime state in `$XDG_RUNTIME_DIR` (the welcome marker in
`$XDG_STATE_HOME`) and user choices in `~/.config/lazyos` or
`~/.config/sway/config.d`. The menu, the bar and the login line read the same file. Treat
that file as the contract; never add a second one.

| Helper | State | Values |
|---|---|---|
| `night-light` | `$XDG_RUNTIME_DIR/lazyos-night-light` + `~/.config/lazyos/night-light.conf` | runtime `schedule`, `warm`; absent = off. Config `mode=auto\|warm\|off`, `warmth=2000..6500`. `apply` reads the config. |
| `lazyos-idle` | `~/.config/lazyos/idle.conf` | `screen_off`, `lock`, `dpms` in seconds; `0` = never. `start`/`restart` replaces the system swayidle chain. |
| `touchpad` | `~/.config/sway/config.d/36-touchpad.conf` | `tap`, `natural_scroll`, `dwt`, `pointer_accel`; Settings applies the same values with `swaymsg input`. |
| `workspaces` | `~/.config/sway/config.d/35-workspaces.conf` | `workspace N output NAME` for 1–10; Settings applies the same commands with `swaymsg workspace N output NAME`. Absent = Sway's focused-monitor default. |
| `reduced motion` | `~/.config/lazyos/reduced-motion` | `1`/`yes`/`true` = on. `Design::for_session` reads it alongside `LAZYOS_REDUCED_MOTION=1`. |
| `appearance` | `~/.config/lazyos/appearance.conf` + `~/.config/sway/config.d/41-appearance.conf` + `~/.config/sway/config.d/36-custom-wallpaper.conf` | `corners=sharp\|rounded\|soft`, `shadows=on\|off`, `blur=off\|frosted\|xray`, `animations=off\|calm\|normal`, `wallpaper=theme\|/abs/path`; the 41 drop-in is the generated compositor copy, and the 36 drop-in is the custom image override (copied into `~/.local/share/lazyos/wallpapers/`). Bar position/density/modules live in `~/.config/waybar/config.jsonc`, not here. |
| `lazyos-notify` | `~/.config/lazyos/notify.conf` + `$XDG_RUNTIME_DIR/lazyos-notify.sock` | `default_timeout` (ms, 0 = never), `max_visible`, `card_width`, `history_limit`, `group_by_sender`, `collapse_repeats`. Read once at startup; Settings restarts the daemon after a save. The 0600 socket takes `act-newest`, `dismiss-newest`, `dismiss-all` from the keyboard keys and dies with the session. |
| `lazyos-clipboard` | `~/.config/lazyos/clipboard.conf` | `enabled=yes\|no`; the helper starts/stops `wl-paste --watch cliphist store`. |
| `lazyos-capture` | `~/.config/lazyos/record.conf` + `$XDG_RUNTIME_DIR/lazyos-capture/camera.pid` | `audio=off\|mic\|system`, `webcam=on\|off`; the camera pid is the mpv overlay, signalled with the recorder. |
| `lazyos-vnc` | `$XDG_RUNTIME_DIR/lazyos-vnc/{pid,wayvnc.log}` | the mode is the process, not a file: `status` and the bar read `/proc/<pid>/cmdline`, where `--disable-input` means read-only and its absence means remote control. Both bind 127.0.0.1 and are reached through the ssh tunnel; remote input is the explicit second start. |
| `notes` | `~/Documents/Notes` + `~/Documents/Signed` | `*.md` quick notes edited in `$EDITOR` (nvim), `*.xopp` ink notes opened in xournalpp, `README.md` generated once and skipped by the list; exported PDFs in `Signed`. No state file: the folder is the list. EPUB is Files/`open` → foliate, never the Notes menu. |
| `tablet-mode` | `$XDG_RUNTIME_DIR/lazyos-tablet/` | `kbd.pid`, `poller.pid`, `out` while on; absent while off. `off` repairs a missing directory. |
| `busy` / `bar-busy` | `$XDG_RUNTIME_DIR/lazyos-busy/<tag>.{label,id,icon,stop}` | `.stop` is the line the bar runs; `busy cancelled` says whether the card is gone. |
| `lazyos-profile` | target archive + `$XDG_CACHE_HOME/lazyos` | export writes `.partial` then renames; import backs up then extracts; `report` is one file. |
| `sway-lock` | no file | lock/unlock belongs to the compositor; readiness is the fifo handshake. |
| `quake` console | no file | the window is the state: it waits in Sway's scratchpad while hidden, and `lazyos quake --status` reads `swaymsg -t get_tree` for `absent`/`hidden`/`visible`. `34-agent-console.conf` owns its geometry (full width, 60% tall, top, no border) and `Alt+grave` toggles it. |
| `lazyos-screensaver` | no file | one `foot` window; idempotent by `pgrep`. |
| `lazyos-welcome` | `$XDG_STATE_HOME/lazyos/welcome` | the machine id last shown; absent = show once. |

`check-desktop.sh` exercises the transitions that are invisible until they fail:
night-light status, tablet stale repair, busy stop/cancelled, and the profile export.
Add a probe when a new state is introduced.

## Settings Structure
- `system/settings.rs` has seven sections: Network, Display & Sound, Appearance, Input,
  Power & Idle, System, Accessibility. `rows()` is the flat canonical table for
  `check-system.sh`; `sections()` / `--sections` is the UI structure. Every row belongs to
  exactly one section (`every_settings_row_belongs_to_one_section`).
- A new setting writes a user config file and delegates to a helper (`lazyos-idle`,
  `night-light`, `touchpad`, `reduced-motion`), rather than adding a second state file or a
  second copy of the command line.
- A section row's detail comes from the same reader the submenu uses; a top-level section
  stays cheap (no nmcli/systemctl probe storm) and the live status is one level down.
- System identity uses the system tools: `localectl` for language (next login),
  `hostnamectl` plus `/etc/hosts` for the name, `xdg-mime`/`gio` for default apps, and
  `chpasswd` with the password on stdin for the account password.
- Notification and clipboard settings are user config files read by their helpers.
  `lazyos-notify` reads its file once at startup; saving a notification setting restarts
  the daemon and clears visible cards. `lazyos-clipboard` starts/stops the `cliphist`
  watcher from `clipboard.conf`.
- Captive portal sign-in is a read-only probe in `system::portal`: NetworkManager's
  `CONNECTIVITY` says whether a page is needed, and the URL comes from the connectivity
  endpoint's redirect. It opens only `http`/`https`, writes no state, and is reachable as
  `lazyos menu net --portal` for checks.

## Cross-Repo Boundaries & Pure-Rust Installer (No Hardcoded Paths!)
- Never put device-specific keymaps, udev rules, IRQ tuning, or board hacks into `uis/sway/files/`.
- Pure-Rust installer (`lazyos installer` / `lazyos installer apply` under `src/installer/`) reads
  `/etc/lazyos/deviceinfo` & `/etc/lazyos/kernel-cmdline*.conf` directly (`BootType::{Depthcharge, Uefi, Extlinux}`),
  evaluates power via sysfs `type`/`scope` (recognizing ChromeOS `USB` / `Mains` PD chargers and
  ignoring `Device`-scoped stylus batteries), enforces `check_machine_safety` before writes in both TUI
  pre-flight and headless `apply`, spawns detached root work via `sudo -n setsid systemd-inhibit ... apply --stdin --state-dir /run/lazyos-install`
  with secrets passed solely over stdin, enforces the `first_write` cancel gate, and writes `/etc/lazyos/installation`
  last. `core/scripts/install-emmc.sh` stays intact only as a Hayato manual/debug fallback.
- When desktop binaries invoke `lazyos-core` system scripts (`install-boot.sh`, `backup-system.sh`),
  resolve `/usr/libexec/lazyos/<script>` first, then `$LAZYOS_CORE_DIR/scripts/`, then umbrella/sibling fallbacks.
- Guard optional hardware resume hooks in Sway configs (e.g., `[ -x /usr/local/bin/lazyos-dp-resume ]`),
  and detect both `eDP-*` and `DSI-*` internal panel outputs dynamically in `system::display`.

## Feature Witnesses
Every user-facing change ships with:
1. Automated test(s) in `rust/crates/*/tests/` and/or a probe in `scripts/check-desktop.sh`.
2. User documentation in `uis/sway/files/usr/share/doc/lazyos/` (`01`–`07`).
3. A note in this file when adding or altering an architectural contract.

## Tests
- Rust unit tests live beside the module (`#[cfg(test)]`) when the logic is pure: task ids, notify
  argument parsing, built-in theme lookup and wallpaper gradient interpolation, plus the existing
  workspace suites.
- The procedural wallpaper scripts are imported by `scripts/tests/test_*.py`; keep generation behind
  `main()` so importing them does not write files or run `ffmpeg`. Run them with
  `python3 -m unittest discover -s scripts/tests -p 'test_*.py'`.
- `.github/workflows/rust.yml` runs both `cargo test --workspace --all-targets` and the Python suite.
