# `lazyos-desktop`

Wayland desktop environment, multi-call Rust UI/CLI suite, theme engine, notifications, boot
greeter, and user manual for [LazyOS](https://github.com/Brunux/LazyOS).

## Contents

- `rust/` — Cargo workspace (`lazyos-cli`, `lazyos-ui`, `lazyos-theme`, `lazyos-mcp`,
  `lazyos-notify`, `lazyos-games`, `spike-sixel`)
- `uis/sway/` — SwayFX configuration, Waybar panels, launcher shortcuts, desktop entries, icons,
  themes, and bundled user manual (`usr/share/doc/lazyos/`)
- `compositors/swayfx/` — Pinned SwayFX / SceneFX build versions and hashes
- `tools/` — Pinned `workstyle`, `opencode`, `retroarch` and `mupen64plus-next` versions and hashes
- `brand/` — SVG master artwork and generated brand assets
- `packages/tools.list` — Desktop userland CLI package manifest
- `packages/games.list` — Libretro cores, installed after the slim RetroArch package
- `lazyos gamepad` — the session's controller mapper (`lazyos-cli/src/gamepad.rs`): evdev → `wtype` arrows/Enter/Escape for the `lazyos-*` windows
- `scripts/` — Build, asset generation, staging (`stage-desktop.sh`), and `check-desktop.sh`

## Verification

```sh
# On ARM64 big cores (4-7):
taskset -c 4-7 cargo fmt --manifest-path rust/Cargo.toml --all -- --check
taskset -c 4-7 cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -j 4 -- -D warnings
taskset -c 4-7 cargo test --manifest-path rust/Cargo.toml --workspace --all-targets -j 4
./scripts/check-desktop.sh
```
