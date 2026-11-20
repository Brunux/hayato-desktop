# `hayato-desktop`

Wayland desktop environment, multi-call Rust UI/CLI suite, theme engine, notifications, boot
greeter, and user manual for [Hayato](https://github.com/Brunux/Hayato).

The brand is a golden eagle on a pixel grid, the default theme is Tokyo Night, and the promise is
**light. fast. fun.** `brand/mark-48.txt` is the only source for the mark: `scripts/make-brand.sh`
samples it into the 16/24/48 px icons, the wallpaper, the social card, the fastfetch wordmark and
the idle screen. `brand/README.md` has the rules.

## Contents

- `rust/` — Cargo workspace (`hayato-cli`, `hayato-ui`, `hayato-theme`, `hayato-mcp`,
  `hayato-notify`, `hayato-games-core`, `hayato-games`, `spike-sixel`)
- `uis/sway/` — SwayFX configuration, Waybar panels, launcher shortcuts, desktop entries, icons,
  themes, and bundled user manual (`usr/share/doc/hayato/`)
- `compositors/swayfx/` — Pinned SwayFX / SceneFX build versions and hashes
- `tools/` — Pinned `workstyle`, `opencode`, `retroarch` and `mupen64plus-next` versions and hashes
- `brand/` — SVG master artwork and generated brand assets
- `packages/tools.list` — Desktop userland CLI package manifest
- `packages/games.list` — Libretro cores, installed after the slim RetroArch package
- `hayato gamepad` — the session's controller mapper (`hayato-cli/src/gamepad.rs`): evdev → `wtype` arrows/Enter/Escape for the `hayato-*` windows, with the button table in `padmap.rs` and a Controller screen (`menu controller`) that edits it
- `scripts/` — Build, asset generation, staging (`stage-desktop.sh`), and `check-desktop.sh`

## Verification

```sh
# On ARM64 big cores (4-7):
taskset -c 4-7 cargo fmt --manifest-path rust/Cargo.toml --all -- --check
taskset -c 4-7 cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -j 4 -- -D warnings
taskset -c 4-7 cargo test --manifest-path rust/Cargo.toml --workspace --all-targets -j 4
./scripts/check-desktop.sh
```
