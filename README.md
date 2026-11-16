# `lazyos-desktop`

Wayland desktop environment, multi-call Rust UI/CLI suite, theme engine, notifications, boot
greeter, and user manual for [LazyOS](https://github.com/Brunux/LazyOS).

## Contents

- `rust/` — Cargo workspace (`lazyos-cli`, `lazyos-ui`, `lazyos-theme`, `lazyos-mcp`,
  `lazyos-notify`, `spike-sixel`)
- `uis/sway/` — SwayFX configuration, Waybar panels, launcher shortcuts, desktop entries, icons,
  themes, and bundled user manual (`usr/share/doc/lazyos/`)
- `compositors/swayfx/` — Pinned SwayFX / SceneFX build versions and hashes
- `tools/` — Pinned `workstyle` and `opencode` versions and hashes
- `brand/` — SVG master artwork and generated brand assets
- `packages/tools.list` — Desktop userland CLI package manifest
- `scripts/` — Build, asset generation, staging (`stage-desktop.sh`), and `check-desktop.sh`

## Verification

```sh
# On ARM64 big cores (4-7):
taskset -c 4-7 cargo fmt --manifest-path rust/Cargo.toml --all -- --check
taskset -c 4-7 cargo clippy --manifest-path rust/Cargo.toml --workspace --all-targets -j 4 -- -D warnings
taskset -c 4-7 cargo test --manifest-path rust/Cargo.toml --workspace --all-targets -j 4
./scripts/check-desktop.sh
```
