# FBNeo, the Neo Geo core

Debian forky has no Neo Geo emulator and no arcade libretro core: no `fbneo`,
no `mame`, no `mame2003_plus`. This is the core the games library launches for
Neo Geo romsets, built here from the pinned **libretro** FBNeo repository
(`libretro/FBNeo`, the fork the libretro buildbot builds; upstream
`finalburnneo/FBNeo` no longer carries the libretro port).

The build is the **Neo Geo subset** (`SUBSET=neogeo`, `-DBUILD_NEOGEO`), so it
compiles the Neo Geo driver, the 68000/Z80 CPUs and the YM2610 sound instead of
every arcade board. The core is `fbneo_neogeo_libretro.so`; the library's row is
`neogeo`.

Neo Geo games stay **zipped**: FBNeo reads a MAME/FBNeo romset (`.zip`) as it
is, and needs the MVS BIOS `neogeo.zip` in `~/Games/bios/`. The scan claims a
`.zip` only inside a `neogeo` folder (`neogeo/`, `mvs/`, `aes/`, `fbneo/`), so a
zipped SNES ROM is not mistaken for an arcade game.

`version` is the pinned commit, `SHA256SUMS` the tarball. `check-desktop.sh`
checks the core the library lists like every other one.
