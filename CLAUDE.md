# CLAUDE.md — `lazyos-desktop`

@AGENTS.md

## Working in this repo
- Always pin Cargo builds, Clippy, and workspace tests on 8-core MT8192 devices to big cores:
  `taskset -c 4-7 cargo <cmd> -j 4` (omit `taskset` on 4-core CI hosts).
- Never embed `~/Dev/LazyOS/` or board hardware workarounds inside desktop scripts or Rust code.
