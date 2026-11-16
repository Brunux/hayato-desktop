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
   - Nerd Font codepoints: edit `scripts/make-glyphs.sh`, never hand-edit `glyphs/table.rs`.
   - PNG app/notification icons: edit `scripts/make-icons.sh` tables and regenerate PNGs.
   - Brand SVGs -> PNGs: `scripts/make-brand.sh` (outputs `brand/apple-touch-icon.png`, wallpapers, wordmark).
   - Theme templates: edit `rust/crates/lazyos-theme/templates/` + `scripts/make-theme.sh`.
7. **Image codecs**: `image` crate features stay minimal (`png`, `jpeg`) to protect build time & binary size.
8. **Version invariant**: `VERSION` at the repo root is compiled into `lazyos --version` via
   `include_str!`. Keep it valid semver `x.y.z` matching releases.

## Cross-Repo Boundaries & Pure-Rust Installer (No Hardcoded Paths!)
- Never put device-specific keymaps, udev rules, IRQ tuning, or board hacks into `uis/sway/files/`.
- Pure-Rust installer (`lazyos installer` / `lazyos installer apply` under `src/installer/`) reads
  `/etc/lazyos/deviceinfo` & `/etc/lazyos/kernel-cmdline*.conf` directly (`BootType::{Depthcharge, Uefi, Extlinux}`),
  spawns detached root work via `sudo -n setsid systemd-inhibit ... apply --stdin --state-dir /run/lazyos-install`
  with secrets passed solely over stdin, enforces the `first_write` cancel gate, and writes `/etc/lazyos/installation`
  last. `core/scripts/install-emmc.sh` stays intact only as a Hayato manual/debug fallback.
- When desktop binaries invoke `lazyos-core` system scripts (`install-boot.sh`, `backup-system.sh`),
  resolve `/usr/libexec/lazyos/<script>` first, then `$LAZYOS_CORE_DIR/scripts/`, then umbrella/sibling fallbacks.
- Guard optional hardware resume hooks in Sway configs (e.g., `[ -x /usr/local/bin/lazyos-dp-resume ]`).

## Feature Witnesses
Every user-facing change ships with:
1. Automated test(s) in `rust/crates/*/tests/` and/or a probe in `scripts/check-desktop.sh`.
2. User documentation in `uis/sway/files/usr/share/doc/lazyos/` (`01`–`07`).
3. A note in this file when adding or altering an architectural contract.
