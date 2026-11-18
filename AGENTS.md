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
  - `lazyos-theme`: `theme.toml` palette compiler generating 13 surface configs idempotently.
  - `lazyos-mcp`: Read/propose-only MCP server (`never` add destructive apply tools).
  - `lazyos-notify`: Layer-shell desktop notification card daemon (`lazyos-notify`).
  - `spike-sixel`: Sixel image preview probe.

## Enforced Architecture & Design-System Invariants
Verified by workspace tests (`cargo test`) **and** `scripts/check-desktop.sh`:
1. **`Color::Rgb` only in `design.rs`**: `lazyos-ui/tests/colours.rs` fails on `Color::Rgb` or raw
   hex colors anywhere outside `lazyos-ui/src/design.rs`. Widgets consume `ColorRole`.
2. **Shell owns `Frame`**: `lazyos-cli` implements `Screen` rendering into `BodyCtx`. Mentioning
   `ratatui::Frame` or `&mut Frame` inside `lazyos-cli/src/` fails `tests/frame.rs`.
3. **Zero allocations per steady-state frame**: `lazyos-ui/tests/no_alloc.rs` asserts `0` frame
   allocations when redrawing cached rows. Pre-format widget state on events/state changes, not per tick.
4. **No resident daemons except `cliphist` and `workstyle`**: Never add polling daemons or background
   units in `uis/sway/`. Tool discovery runs async in-window on open; progress lives in `TaskView` or
   in-window `ActionRunner` banners (`lazyos-notify` is strictly for headless events).
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
   and `system/dictate.rs` (`DictationPhase`).

## Developer Profile Export
- `lazyos-profile export --include-home` streams the whole home through tar. The single
  `home_excludes` list in `bin/lazyos-profile` is used by both the `du` size estimate and the
  `tar` command, so the two never disagree.
- It must skip rootless container storage (`~/.local/share/containers`, `~/.local/share/docker`,
  `~/.local/share/buildah`): those layers are owned by subuids, unreadable to the user, and tar
  aborts the whole archive with `Cannot open: Permission denied` after copying the rest.
  `check-desktop.sh` reproduces that with a `000` directory and asserts `Documents` still travels.

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
