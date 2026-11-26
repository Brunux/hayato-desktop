# AGENTS.md — `hayato-desktop`

## Scope
Wayland desktop environment (`uis/sway/`), multi-call Rust UI/CLI suite (`rust/`), SwayFX/Workstyle
packaging (`compositors/`, `tools/`), brand artwork (`brand/`), CLI packages (`packages/tools.list`),
and session verification (`scripts/check-desktop.sh` -> `/usr/libexec/hayato/check/30-desktop.sh`).

Hardware board definitions, kernels, keymaps, and ChromeOS quirks belong in `hayato-devices`.
Rootfs assembly, bootloaders, image creation, and eMMC installers belong in `hayato-core`.

## Rust Workspace & Performance Rules
- **Always pin builds/tests on 8-core ARM64 hosts (MT8192)**: use
  `taskset -c 4-7 cargo <build|clippy|test> --manifest-path rust/Cargo.toml -j 4` so efficiency cores
  0-3 stay responsive for Wayland. (Do **not** hardcode `taskset` in CI scripts running on 4 cores.)
- Workspace crates under `rust/crates/`:
  - `hayato-cli`: `hayato` multi-call binary (`argv[0]` symlink dispatch + subcommands), `hayato greeter`
    (pure-Rust `greetd` Unix socket client + Sway/inline fallback).
  - `hayato-ui`: Ratatui shell (`App` + `Screen` + `BodyCtx`), design tokens (`design.rs`), Nerd Font
    `Glyph` table (`glyphs/`), `ActionStrip` / `TaskView` / `ActionRunner`, `Card` notification &
    in-window panels, terminal raw-mode cleanup (`Drop` + panic hook + signal handlers).
  - `hayato-theme`: `theme.toml` palette compiler generating 14 surface configs idempotently.
  - `hayato-mcp`: Read/propose-only MCP server (`never` add destructive apply tools).
  - `hayato-notify`: Layer-shell desktop notification card daemon (`hayato-notify`).
  - `hayato-games-core`: the game scan, ids and launch rules, with no UI toolkit; shared by the
    Ratatui library and the GTK host.
  - `hayato-games`: the GTK4 + WebKitGTK host that plays one HTML game fullscreen (`hayato-games`).
  - `hayato-music-core`: pure-Rust audio engine (`symphonia` + `ffmpeg` Opus fallback + `cpal` ALSA/PipeWire),
    lock-free `rtrb` PCM tap + `realfft` 32-band spectrum & beat detector, `lofty` library scanner, `.lrc`
    lyrics, `zbus` MPRIS2 server (`org.mpris.MediaPlayer2.hayato_cassette`), and Spotify OAuth PKCE/Web API.
  - `hayato-cassette`: terminal-native Cyberdeck music player (`hayato-cassette`, symlink `hayato-music`,
    `Alt+M` Sway scratchpad toggle via direct `$SWAYSOCK` `i3-ipc`, Waybar `custom/cassette` pill, `SIGUSR1`
    live theme reload, `Design::ramp` zero-alloc beat-pulse/spectrum LUTs, and `hayato_ui::pictures` Sixel art).
  - `spike-sixel`: Sixel image preview probe.

## Enforced Architecture & Design-System Invariants
Verified by workspace tests (`cargo test`) **and** `scripts/check-desktop.sh`:
1. **`Color::Rgb` only in `design.rs`**: `hayato-ui/tests/colours.rs` fails on `Color::Rgb` or raw
   hex colors anywhere outside `hayato-ui/src/design.rs`. Widgets consume `ColorRole`.
2. **Shell owns `Frame`**: `hayato-cli` implements `Screen` rendering into `BodyCtx`. Mentioning
   `ratatui::Frame` or `&mut Frame` inside `hayato-cli/src/` fails `tests/frame.rs`.
3. **Zero allocations per steady-state frame**: `hayato-ui/tests/no_alloc.rs` asserts `0` frame
   allocations when redrawing cached rows. Pre-format widget state on events/state changes, not per tick.
4. **No resident daemons except `cliphist`, `workstyle` and `hayato gamepad`**: Never add polling
   daemons or background units in `uis/sway/`. Tool discovery runs async in-window on open; progress
   lives in `TaskView` or in-window `ActionRunner` banners (`hayato-notify` is strictly for headless
   events). `hayato gamepad` is the one input helper: it reads `/dev/input` and injects
   arrows/Enter/Escape into the focused `hayato-*` window, because a USB or Bluetooth pad must
   navigate every framework menu, not only the Games library; its pid lock keeps it single.
5. **Dual density support**: `Screen::key` and `Screen::hit` receive `&Design` (`$XDG_RUNTIME_DIR/hayato-tablet`)
   rather than caching hit boxes across normal vs tablet row spacing.
6. **Generated glyph & icon tables**:
   - Glyph codepoints (`Symbols Nerd Font Mono` + `Font Awesome 6 Free Solid` + `Font Awesome 6 Brands Regular`, avoiding Codicons `ea60..ec1e` stripped in Debian `fonts-nerd-symbols 3.4.0+dfsg2-1`): edit `scripts/make-glyphs.sh`, never hand-edit `glyphs/table.rs`.
   - `hayato-notify` (`FontRenderer`) loads both `SymbolsNerdFontMono` and `Font Awesome 6` (`Solid`/`Brands`) and resolves UI glyph codepoints via `hayato_ui::Glyph`.
   - Greeter console power buttons (`^Z`/`^R`/`^P`) invoke `systemctl <verb> -i` with stdio redirected (never inheriting raw TTY over Ratatui), authorized for local active `_greetd` via `/etc/polkit-1/rules.d/49-hayato-greeter-power.rules`. `GreeterScreen` uses a minimalist layout: `Header::default()` (no top band/rule), themed pixel-hacker `Hayato _` wordmark (4-row half-block pixel art already formed on entry with a blinking pixel underscore cursor and subtle periodic randomized single-letter glitch pulse on `>= 44x26`, compact 1-line `Hayato _` on smaller grids, sharing the Plymouth boot splash bitmaps and honouring `reduced_motion`), a tight border-only login card with empty placeholders and no idle filler text, an optional centered icon-only action strip below the card (`^T`, `^Z`, `^R`, `^P`, no words, no `^G`, hidden as a unit when `show_power_actions = false`), an optional quiet bottom-right date & time (`show_clock`), centered power confirmation dialogs, and no bottom keybar in any mode. `hayato-greeter --session` uses a solid theme background (`solid_color`, opaque `foot` `alpha=1.0`, no per-boot PNG wallpaper generation) for faster startup and a flicker-free handoff. `GreeterSettingsScreen` places its action strip at the top and exposes only the five core options (`Follow user theme`, `Default theme`, `User pills`, `Date & time`, `Action buttons`). `Input::render_field` blinks the active cursor cleanly without an underline (`_`) fallback.
   - PNG app/notification icons: edit `scripts/make-icons.sh` tables and regenerate PNGs.
   - Brand SVGs -> PNGs: `scripts/make-brand.sh` (outputs `brand/apple-touch-icon.png`, wallpapers, wordmark).
   - Pixel-hacker Golden Eagle screensavers: `scripts/make-screensaver.py` generates three uniform-width half-block pixel artworks (`screensaver.txt` Heraldic Crest, `screensaver-dive.txt` Diving Raptor, `screensaver-head.txt` Cyber-Raptor Head) paired with the shared 5×8 pixel `Hayato _` wordmark; `hayato-screensaver` cycles through all three with pixel-hacker `tte` effects (`decrypt`, `beams`, `binarypath`, `rain`, `sweep`, `wipe`).
   - Plymouth boot splash: `scripts/make-plymouth.py` generates pure-Python pixel-art RGBA sprites (`letter-0.png`..`letter-5.png`, cyan decode `glitch-0.png`..`glitch-5.png`, and orange pixel underscore `cursor.png`) under `uis/sway/files/usr/share/plymouth/themes/hayato/` alongside `hayato.plymouth` and `hayato.script` (solid `#1a1b26` background for a flicker-free handoff into `hayato-greeter`); `build-hayato.sh` `postinst` and `core/scripts/rootfs-setup.sh` activate `plymouth-set-default-theme hayato`.
   - Theme templates: edit `rust/crates/hayato-theme/templates/` + `scripts/make-theme.sh` (Waybar outer end spacing lives in `.modules-left` / `.modules-right` to match `config.jsonc` `spacing: 4`, keeping edge pills inset cleanly even when terminal modules are hidden).
7. **Image codecs**: `image` crate features stay minimal (`png`, `jpeg`) to protect build time & binary size.
8. **Version invariant**: `VERSION` at the repo root is compiled into `hayato --version` via
   `include_str!`. Keep it valid semver `x.y.z` matching releases.
9. **Multi-input cursor & contrast contract**: Screens with multiple text inputs (`InstallerScreen`,
   `GreeterScreen`, `GreeterSettingsScreen`, `ExportSetupScreen`) must render via `Input::render_field(..., focused, blink_on, bg_role, ...)`
   so only the active input shows the accent bar and blinking 500 ms cursor (solid when
   `reduced_motion`), and inputs inside `ColorRole::Surface` cards use `ColorRole::Bg` to preserve
   palette contrast across themes where the two are close (they were identical in Solarized Dark,
   which is why the rule exists) and across light themes.
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
10. **A recording is a file of the screen (`hayato-studio` & `hayato-capture`)**: `hayato menu studio`
    (`hayato-studio`, `Alt+Ctrl+R`, with `~/.config/bin/hayato-camera` wrapping `--mode photo`) unifies
    Screen Record (`wf-recorder`), Webcam Video (`ffmpeg`), and Photo Snap (`mpv.sock` zero-flicker
    viewfinder + `ffmpeg` fallback) using dynamic `/sys/class/video4linux` `uvcvideo` discovery (never
    hardcoding `/dev/video0`). Live PipeWire audio switching (`Off`/`Mic`/`System`/`Both`) uses an
    ephemeral `hayato_rec_mix` null-sink + dynamic `module-loopback` (never looping `hayato_rec_mix.monitor`
    back into itself). Waybar displays a minimal pulsing `custom/rec` icon next to `privacy` (0px when idle)
    whose left-click opens `LiveHudScreen` (`hayato menu studio --hud`) for hot audio switching and 60 FPS
    cubic-eased Sway IPC `hayato-webcam` PiP gliding (`sticky enable`, scratchpad Hide/Show, fullscreen warning).
    While `hayato_ui::recording::is_recording()` reports a live session (either `hayato-studio/session.json`
    or legacy `hayato-capture/recording.pid`), `hayato-notify` holds back non-critical cards so nothing
    contaminates the video, and the "saved" card fires only after the recorder exits.
10b. **System Share Sheet (`hayato share` / `hayato-share`)**: `system/share/` provides the OS-wide
    Share Sheet invoked from `hayato-files` (`Ctrl+S`), `file_actions`, `hayato-lens` (`s`), `hayato-clip`
    (`Alt+S`), `hayato-notes` (`Ctrl+S`), `hayato-capture` (`Ctrl+S`), `hayato-studio` (`Ctrl+S`), and CLI
    stdin/paths (`system/drop.rs` is a thin compatibility facade). Its ephemeral LAN HTTP server uses a
    128-bit `/dev/urandom` token, constant-time comparison, active-theme CSS injection, HTML escaping,
    multi-file/folder ZIP streaming, two-way phone upload/clipboard push, and 5-min auto-expiry, alongside
    zero-daemon on-demand LocalSend v2 UDP multicast (`224.0.0.167:53317`) peer discovery.
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
   (`PkgStage::Installing(TaskView)`), `hayato-ui/src/recording.rs` (`Recorder`),
   `system/theme_picker.rs` (`PreviewPhase`), `system/dev.rs` (`ExportFocus`),
   `files/find.rs` (`Stage`), `hayato-ui/src/task.rs` (`StopPhase`),
   `system/media.rs` (`PlaybackState`), `hayato-ui/src/widgets.rs` (`RunnerState`),
   `system/dictate.rs` (`DictationPhase`), `installer/worker.rs` (`WorkerPhase`),
   `installer/ui.rs` (`LocFocus` / `UserFocus` / `DiskFocus`), and
   `greeter/screen.rs` (`UserChoice` / `AuthPrompt`). Screens expose the phase
   through `Screen::phase()`; `HAYATO_DEBUG_STATE=1` appends transitions to
   `$XDG_STATE_HOME/hayato/state.log` (never stderr, which would paint over the
   frame).
13. **A file card opens its folder**: a notification about a written file carries the
   `x-hayato-open-path` hint (`Card::open_path`, `hayato notify --open`, or `BUSY_OPEN` for a
   busy result). A left click on the card body runs `hayato-files <parent>`; grimshot predates the
   hint, so its absolute `app_icon` (the saved picture) is the fallback. The daemon never opens
   the file itself: Files is where the person decides what to do with it.
14. **The library is a TUI screen; GTK is only the game host**: the Games row runs
   `hayato menu games`, a Ratatui screen in `hayato-cli` (`system/games.rs`); the GTK + WebKit
   binary `hayato-games` plays one HTML game and has no library mode. The scan, ids and launch
   rules live in `hayato-games-core`, so neither UI owns them. The host gets a game id from the
   library's scan, never a path or a command; the library spawns it by name, so the first
   `hayato-games` in `PATH` is the one that runs — a stale copy in `/usr/local/bin` shadows
   `/usr/bin/hayato-games`, and `check-desktop.sh` fails when it does. The host's stderr goes to
   `~/.local/state/hayato/games/host.log` (overwritten per launch) because the library closes as
   soon as the host is spawned: without that file a host that dies says nothing at all.
   `hayato-game://` serves only inside that game's directory (canonicalise + prefix), and no ROM,
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
   `HAYATO-GAMES.txt`, so a new person only drops files in. The image build
   creates the shelf (`install-hayato.sh` runs `hayato-games --ensure-layout`,
   the same `library::ensure_layout` the library runs when it opens), so a fresh
   host has it before Games is ever opened. The library
   **exits before a game starts**; the host returns to a new `hayato menu games` window, so the
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
15. **A controller is the session's input helper**: `hayato gamepad` reads `/dev/input` through
   evdev and injects arrows, Enter and Escape with `wtype` into the focused `hayato-*` window
   (the root menu, Settings, Files, the Games TUI). The library is one of those windows; its tabs
   are Left/Right and its list Up/Down. Its own surfaces get three buttons — X options, Y manual,
   L1 the folder in Files — sent as **Alt+O / Alt+M / Alt+R**, never bare letters, because a bare
   letter would type into another screen's filter; the Alt combinations are unbound elsewhere.
   The table lives in `crate::padmap`, not in the mapper: the Controller screen (`menu controller`,
   Settings → Input, and the library's `o` row) edits it and writes
   `~/.config/hayato/pad.json`, and the mapper reloads the file when its mtime changes, so a remap
   needs no restart and works with no pad connected. The defaults are the console layout and cover
   a DualShock 4 (Cross confirm, Circle cancel), which pairs over Bluetooth (PS + Share) like any
   other pad (`bluetooth.rs` enables `pairable on` via an RAII guard, clears stale unbonded
   `Paired: yes`/`Trusted: yes` entries, and keeps `--agent NoInputNoOutput` active across
   `pair`/`connect` so BlueZ 5.8x writes a `[LinkKey]` without weakening `ClassicBondedOnly`); a
   DualShock 3 is the USB exception (BlueZ's sixaxis plugin writes the host address),
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
   `retroarch`, so `install-retroarch.sh` installs the Hayato package **first** and the cores
   after it; installing the session list first would pull Debian's retroarch, its 547 MB assets
   and Qt5.
16. **The game stack is built, not installed from Debian**: `tools/retroarch/` builds the slim
   emulator (no Qt, assets, FFmpeg, Vulkan or updater) and `tools/mupen64plus-next/` builds the
   N64 core Debian does not have. The cores in `packages/games.list` Depends on `retroarch`, so
   `install-retroarch.sh` installs the Hayato package **first** and the cores after it; installing
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
   `hayato menu pkg --search <package>` in a new window; the key bar drops the hints that cannot
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

21. **The bar is the session's statusline**: `~/.config/waybar/style.css` is generated from
   `templates/waybar-style.css.tpl`, and the shape is the contract. The bar floats 6px from
   the top and 12px from both sides (`config.jsonc` margins - the side inset matches a window's,
   gaps outer 4 + inner 8) with a 3px radius in `style.css` and a 1px border in the bar's own
   background colour, so the panel reads as one solid object with no edge drawn around it; the
   4px inset in `.modules-left` / `.modules-right` keeps the ends off the rounded corners. A
   module is a flat segment with no hover fill: pointing at an indicator must not light it up.
   Three things are filled and nothing else: the current workspace chip (solid `accent` fill,
   `background` label - the same readable pair as the busy chip), the work-in-progress chip
   (solid `warn_text` fill) and the mode chip. The brand at the left end is the wordmark's
   `H_` with no pill - `H` in `text`, the `_` cursor in `accent` (the same accent as the current
   chip), split across two modules only so the cursor can carry a colour - and both halves are
   the menu button. No indicator changes hue except the keep-awake cup: **a switch changes its
   glyph, not its colour** - the icon while
   the thing is on, the same icon with a slash through it while it is off, all three from the
   Material Design Icons font the image ships (`f00af`/`f00b2` Bluetooth, `f05a9`/`f05aa` wifi,
   `f057e`/`f0581` volume). Those glyphs live in plane 15, so `config.jsonc` writes them as a
   surrogate pair with the codepoint named in the comment; the Rust scripts write `\u{f00b2}`
   directly. The tooltip is the word. The cup is the one switch with a single glyph, so it
   states its two sides in colour instead - `dim` while the screen may sleep, `accent` while
   it is held awake - and its tooltip says the same in words. A module that
   polls (`custom/bluetooth` every 30 s, `custom/vnc` every 10 s, the built-in `network` every
   10 s) is signalled by the window that changed it - `pkill -RTMIN+8 waybar`, the `signal: 8`
   in `config.jsonc` - so a toggle made in Hayato reaches the bar at once instead of on the next
   tick; a change made outside Hayato waits for the poll. Offline,
   muted, a pending update and a served screen all stay in `text`; they say what they are with
   the glyph and the tooltip, never with a hue and never with a word - the right end is glyphs
   only, and the name lives in the tooltip. **The bar prints no number**: the battery's charge,
   wifi's signal, Bluetooth's state and the update count all live in the tooltip, so the line
   stays glyphs.
   The line is weight 500; emphasis is 700, and only on the current chip, the busy chip and the
   mode. An urgent tab that is not current takes `danger_text`; the current chip keeps its fill,
   because the fill is where you are. A status drawn as a word uses `ok_text`/`warn_text`/`danger_text` (the
   palette's colour lifted toward the theme's text until it clears 4.5:1 on the background,
   derived in `hayato-theme` and proven by `a_status_drawn_as_text_clears_the_body_text_floor`);
   the raw `ok`/`warn`/`danger` stay for icons, which only need 3:1. `text`, never `strong`, is
   the bar's reading colour: `strong` is the bright end of the palette, and on Catppuccin Latte
   it is the colour of the background. A new state is a word plus a colour, or a 2px bar - never
   a colour alone.

22. **The brand is one grid, and the mark does not follow the theme**: `brand/mark-48.txt` is the
   only source; `scripts/make-brand.py` samples it 1x1, 2x2 and 3x3 into the 48/24/16 px icons and
   into the fastfetch wordmark and the idle art, so no surface resamples another and the screensaver
   cannot drift from the launcher. `brand/hayato.svg`, `hayato-24.svg`, `hayato-16.svg`,
   `wallpaper.svg` and `social-preview.svg` are generated - edit the grid, never them. The mark is
   fixed to Tokyo Night (`#7dcfff` eagle on `#1a1b26`, `#ff9e64` cursor) because a logo does not
   change with the wallpaper; the wallpaper does follow the theme. No kanji or kana appear in the
   product: *hayato* is a word whose meaning is the philosophy, and the philosophy is stated in
   prose only. The serpent belongs to the idle screen and nowhere else. Every built-in theme cycles
   exactly three wallpapers, and `rust/crates/hayato-theme/tests/wallpapers.rs` fails a theme that
   declares another number, a file that is not in the tree, or a name carrying the old brand.

23. **The session PATH carries the sbin directories, and the agent config is V2**: greetd sources
    `/etc/profile`, whose non-root PATH omits `/usr/sbin` and `/sbin`, and every window inherits it.
    `uis/sway/files/etc/profile.d/90-cli-tools.sh` puts them back because `parted`, `mkfs.exfat`,
    `mkfs.ext2/4`, `mkswap`, `swapon`, `swapoff`, `blkid`, `blockdev`, `e2fsck`, `findfs` and
    `i2cset` live there; the disk menu, the installer pre-flight, the auto-mount helper and the
    Type-C mux resume hook call them by name, and without the block they answer "missing" while
    being installed. `~/.config/opencode/opencode.json` is OpenCode V2's shape -
    `mcp.servers.<name>`, with `disabled` rather than `enabled` - and the shipped file and
    `check-desktop.sh` move together. The release sanitizer strips `service.json` and `auth.json`
    from that directory, never the directory, so the registration reaches the image.
 24. **The Files window is one surface with one state file per concern**: `hayato-filer` is the GTK4
    browser (`Alt+Ctrl+Shift+F`), the windowed companion of the `hayato menu files` TUI; the two
    share neither code nor state. Its UI choices (view mode, sort field and order, hidden files,
    compact sidebar, window size) persist in `~/.config/hayato/filer-state.json` (`prefs.rs`, read on
    launch, written on close) and pinned folders in `~/.config/hayato/filer-bookmarks.json`; a
    missing or corrupt file falls back to the defaults, never a second copy of the choice. The open
    folder is watched with `gio::FileMonitor` (inotify) and reloads itself, so **F5** is a fallback
    and not the mechanism. Undo is a session stack of `ops::UndoAction` (rename, move, trash, copy,
    new folder) applied by `ops::undo`; a permanent delete is deliberately not undoable. Archive work
    shells out to the system `tar`/`zip` (`fs/archive.rs`) and reuses the copy progress channel - it
    links nothing in. A drag that may navigate (spring-loaded tabs and folders) holds the source
    widget in `FilerWindow::drag_hold`, so a canvas rebuild cannot cancel the drag the person is
    still holding, and a spring-load timer clears its own stored `SourceId` when it fires, because
    removing an already-finished glib source panics and aborts the window. A drop moves when it
    carries its local `GdkDrag` (GDK reports every local drop as `COPY`, so the drag object, not
    `actions()`, is the signal) and copies when the source is another application or another
    filesystem; the shelf prunes a reference whose path is gone. `hayato-filer --self-test` proves
    the CSS and the pure state/archive logic headlessly, and `check-desktop.sh` runs it.

## Developer Profile Export
- `hayato-profile export --include-home` streams the whole home through tar. The single
  `home_excludes` list in `bin/hayato-profile` is used by both the `du` size estimate and the
  `tar` command, so the two never disagree.
- It must skip rootless container storage (`~/.local/share/containers`, `~/.local/share/docker`,
  `~/.local/share/buildah`): those layers are owned by subuids, unreadable to the user, and tar
  aborts the whole archive with `Cannot open: Permission denied` after copying the rest.
  `check-desktop.sh` reproduces that with a `000` directory and asserts `Documents` still travels.

## Shell State Protocols
The session helpers keep runtime state in `$XDG_RUNTIME_DIR` (the welcome marker in
`$XDG_STATE_HOME`) and user choices in `~/.config/hayato` or
`~/.config/sway/config.d`. The menu, the bar and the login line read the same file. Treat
that file as the contract; never add a second one.

| Helper | State | Values |
|---|---|---|
| `night-light` | `$XDG_RUNTIME_DIR/hayato-night-light` + `~/.config/hayato/night-light.conf` | runtime `schedule`, `warm`; absent = off. Config `mode=auto\|warm\|off`, `warmth=2000..6500`. `apply` reads the config. |
| `hayato-idle` | `~/.config/hayato/idle.conf` | `screen_off`, `lock`, `dpms` in seconds; `0` = never. `start`/`restart` replaces the system swayidle chain. |
| `touchpad` | `~/.config/sway/config.d/36-touchpad.conf` | `tap`, `natural_scroll`, `dwt`, `pointer_accel`; Settings applies the same values with `swaymsg input`. |
| `workspaces` | `~/.config/sway/config.d/35-workspaces.conf` | `workspace N output NAME` for 1–10; Settings applies the same commands with `swaymsg workspace N output NAME`. Absent = Sway's focused-monitor default. |
| `reduced motion` | `~/.config/hayato/reduced-motion` | `1`/`yes`/`true` = on. `Design::for_session` reads it alongside `HAYATO_REDUCED_MOTION=1`. |
| `appearance` | `~/.config/hayato/appearance.conf` + `~/.config/sway/config.d/41-appearance.conf` + `~/.config/sway/config.d/36-custom-wallpaper.conf` | `corners=sharp\|rounded\|soft`, `shadows=on\|off`, `blur=off\|frosted\|xray`, `animations=off\|calm\|normal`, `wallpaper=theme\|/abs/path`; the 41 drop-in is the generated compositor copy, and the 36 drop-in is the custom image override (copied into `~/.local/share/hayato/wallpapers/`). Bar position/density/modules live in `~/.config/waybar/config.jsonc`, not here. |
| `hayato-notify` | `~/.config/hayato/notify.conf` + `$XDG_RUNTIME_DIR/hayato-notify.sock` | `default_timeout` (ms, 0 = never), `max_visible`, `card_width`, `history_limit`, `group_by_sender`, `collapse_repeats`. Read once at startup; Settings restarts the daemon after a save. The 0600 socket takes `act-newest`, `dismiss-newest`, `dismiss-all` from the keyboard keys and dies with the session. |
| `hayato-clipboard` | `~/.config/hayato/clipboard.conf` + `$XDG_STATE_HOME/hayato/clipboard.log` | `enabled=yes\|no`; the helper starts/stops `wl-paste --watch cliphist store`, logs the watcher's own errors and retries once (a session can start before the compositor socket answers). |
| `hayato-capture` | `~/.config/hayato/record.conf` + `$XDG_RUNTIME_DIR/hayato-capture/camera.pid` | `audio=off\|mic\|system`, `webcam=on\|off`; the camera pid is the mpv overlay, signalled with the recorder. |
| `hayato-vnc` | `$XDG_RUNTIME_DIR/hayato-vnc/{pid,wayvnc.log}` | the mode is the process, not a file: `status` and the bar read `/proc/<pid>/cmdline`, where `--disable-input` means read-only and its absence means remote control. Both bind 127.0.0.1 and are reached through the ssh tunnel; remote input is the explicit second start. |
| `notes` | `~/Documents/Notes` + `~/Documents/Signed` | `*.md` quick notes edited in `$EDITOR` (nvim), `*.xopp` ink notes opened in xournalpp, `README.md` generated once and skipped by the list; exported PDFs in `Signed`. No state file: the folder is the list. EPUB is Files/`open` → foliate, never the Notes menu. |
| `tablet-mode` | `$XDG_RUNTIME_DIR/hayato-tablet/` | `kbd.pid`, `poller.pid`, `out` while on; absent while off. `off` repairs a missing directory. |
| `busy` / `bar-busy` | `$XDG_RUNTIME_DIR/hayato-busy/<tag>.{label,id,icon,stop}` | `.stop` is the line the bar runs; `busy cancelled` says whether the card is gone. |
| `hayato-profile` | target archive + `$XDG_CACHE_HOME/hayato` | export writes `.partial` then renames; import backs up then extracts; `report` is one file. The archive list matches `*-profile-*.tar.zst*`, so a backup exported before the rebrand is still offered. |
| `auxiliary swap` | `/etc/fstab` marked block + `LABEL=hayato_swap` | the label is the contract: an existing swap area is adopted by relabelling it, `configured()` reads the block or a hand-written `LABEL=hayato_swap` line, and `pri=10` keeps it below zram's 100. |
| `sway-lock` | no file | lock/unlock belongs to the compositor; readiness is the fifo handshake. |
| `quake` console | no file | the window is the state: it waits in Sway's scratchpad while hidden, and `hayato quake --status` reads `swaymsg -t get_tree` for `absent`/`hidden`/`visible`. `34-agent-console.conf` owns its geometry (full width, 60% tall, top, no border) and `Alt+grave` toggles it. |
| `hayato-screensaver` | no file | one `foot` window; idempotent by `pgrep`. |
| `hayato-welcome` | `$XDG_STATE_HOME/hayato/welcome` | the machine id last shown; absent = show once. |

`check-desktop.sh` exercises the transitions that are invisible until they fail:
night-light status, tablet stale repair, busy stop/cancelled, and the profile export.
Add a probe when a new state is introduced.

## Settings Structure
- `system/settings.rs` has seven sections: Network, Display & Sound, Appearance, Input,
  Power & Idle, System, Accessibility. `rows()` is the flat canonical table for
  `check-system.sh`; `sections()` / `--sections` is the UI structure. Every row belongs to
  exactly one section (`every_settings_row_belongs_to_one_section`).
- A new setting writes a user config file and delegates to a helper (`hayato-idle`,
  `night-light`, `touchpad`, `reduced-motion`), rather than adding a second state file or a
  second copy of the command line.
- A section row's detail comes from the same reader the submenu uses; a top-level section
  stays cheap (no nmcli/systemctl probe storm) and the live status is one level down.
- System identity uses the system tools: `localectl` for language (next login),
  `hostnamectl` plus `/etc/hosts` for the name, `xdg-mime`/`gio` for default apps, and
  `chpasswd` with the password on stdin for the account password.
- Notification and clipboard settings are user config files read by their helpers.
  `hayato-notify` reads its file once at startup; saving a notification setting restarts
  the daemon and clears visible cards. `hayato-clipboard` starts/stops the `cliphist`
  watcher from `clipboard.conf`.
- Captive portal sign-in is a read-only probe in `system::portal`: NetworkManager's
  `CONNECTIVITY` says whether a page is needed, and the URL comes from the connectivity
  endpoint's redirect. It opens only `http`/`https`, writes no state, and is reachable as
  `hayato menu net --portal` for checks.

## Cross-Repo Boundaries & Pure-Rust Installer (No Hardcoded Paths!)
- Never put device-specific keymaps, udev rules, IRQ tuning, or board hacks into `uis/sway/files/`.
- Pure-Rust installer (`hayato installer` / `hayato installer apply` under `src/installer/`) reads
  `/etc/hayato/deviceinfo` & `/etc/hayato/kernel-cmdline*.conf` directly (`BootType::{Depthcharge, Uefi, Extlinux}`),
  evaluates power via sysfs `type`/`scope` (recognizing ChromeOS `USB` / `Mains` PD chargers and
  ignoring `Device`-scoped stylus batteries), enforces `check_machine_safety` before writes in both TUI
  pre-flight and headless `apply`, spawns detached root work via `sudo -n setsid systemd-inhibit ... apply --stdin --state-dir /run/hayato-install`
  with secrets passed solely over stdin, enforces the `first_write` cancel gate, writes `/etc/hayato/installation`
  last, and triggers `[ Reboot System Now ]` on completion via `menu::run_session(SessionAction::Reboot)` (detached `setsid` `systemctl reboot` with `sudo -n systemctl reboot` fallback, never a child tied to the exiting `foot` PTY). `core/scripts/install-emmc.sh` stays intact only as a Hayato manual/debug fallback.
- When desktop binaries invoke `hayato-core` system scripts (`install-boot.sh`, `backup-system.sh`),
  resolve `/usr/libexec/hayato/<script>` first, then `$HAYATO_CORE_DIR/scripts/`, then umbrella/sibling fallbacks.
- The installer's `rsync` excludes `/var/cache/*` but includes `/var/cache/fontconfig/**` first
  (rsync filters are first-match): the image builds the font cache once, and dropping it made the
  first window walk every font directory on the installed disk. `install-emmc.sh` carries the same
  pair.
- **The browser is one indirection.** `~/.config/bin/hayato-browser` is the only reader of
  `xdg-settings get default-web-browser` (what Settings writes); the `$mod+Shift+Return` key, the
  `/usr/local/bin/xdg-open` shim, `system::portal` and Files' browser fallback all go through it.
  Never bind a key or spawn a browser by name again: `firefox-esr` is the image's last resort, not
  the choice, and a literal there silently ignores Settings.
- Guard optional hardware resume hooks in Sway configs (e.g., `[ -x /usr/local/bin/hayato-dp-resume ]`),
  and detect both `eDP-*` and `DSI-*` internal panel outputs dynamically in `system::display`.
- **In-place `.deb` Packaging (`scripts/build-hayato.sh`, `scripts/build-fbneo.sh`, `scripts/build-mupen64plus-next.sh`)**:
  `build-hayato.sh` packages `hayato_<ver>-1hayato1_<arch>.deb` containing `/usr/bin/{hayato,hayato-notify,hayato-games}`,
  all `hayato --symlinks`, `/usr/share/hayato/games/`, `/usr/libexec/hayato/check/30-desktop.sh`,
  `uis/sway/files/{usr,etc}` (staging `/etc/sway/config` and `/etc/yt-dlp.conf` under
  `/usr/share/hayato/etc/` and installing them in `postinst` so `dpkg` never conflicts with `swayfx` or
  `yt-dlp`), and `uis/sway/files/home/` under `/usr/share/hayato/skel/` (whose `postinst` syncs
  `~/.config/bin/*`, `~/.config/opencode/{opencode.json,AGENTS.md}`, wallpapers, and DualShock 4 profiles
  while preserving `cli.json`/`service.json` and re-rendering the active theme headlessly). Git-pinned
  libretro core packages (`build-fbneo.sh`, `build-mupen64plus-next.sh`) prefix `Version:` with
  `1:<hayato_ver>+git.<shortrev>+hayato1` (and filename `<hayato_ver>+git.<shortrev>+hayato1`) so
  `dpkg` version comparisons remain strictly monotonic across pin bumps.

## Feature Witnesses
Every user-facing change ships with:
1. Automated test(s) in `rust/crates/*/tests/` and/or a probe in `scripts/check-desktop.sh`.
2. User documentation in `uis/sway/files/usr/share/doc/hayato/` (`01`–`07`).
3. A note in this file when adding or altering an architectural contract.

## Tests
- Rust unit tests live beside the module (`#[cfg(test)]`) when the logic is pure: task ids, notify
  argument parsing, built-in theme lookup and wallpaper gradient interpolation, plus the existing
  workspace suites.
- The procedural wallpaper scripts are imported by `scripts/tests/test_*.py`; keep generation behind
  `main()` so importing them does not write files or run `ffmpeg`. Run them with
  `python3 -m unittest discover -s scripts/tests -p 'test_*.py'`.
- `.github/workflows/rust.yml` runs both `cargo test --workspace --all-targets` and the Python suite.
