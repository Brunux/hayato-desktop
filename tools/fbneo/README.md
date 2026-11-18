# FBNeo, the arcade core

Debian forky has no arcade emulator and no arcade libretro core: no `fbneo`,
no `mame`, no `mame2003_plus`. This is the core the games library launches for
every arcade romset, built here from the pinned **libretro** FBNeo repository
(`libretro/FBNeo`, the fork the libretro buildbot builds; upstream
`finalburnneo/FBNeo` no longer carries the libretro port).

The build is the full FBNeo (`fbneo_libretro.so`), so one core covers the
boards the library knows:

| Board | Folder | BIOS |
|---|---|---|
| Neo Geo (MVS/AES) | `neogeo/`, `mvs/`, `aes/` | `neogeo.zip` |
| Capcom CPS1 | `cps1/` | — |
| Capcom CPS2 | `cps2/` | `qsound.zip` |
| Capcom CPS3 | `cps3/` | `cps3.zip` |
| IGS PGM | `pgm/` | `pgm.zip` |
| everything else | `arcade/`, `mame/`, `fbneo/` | some boards |

Arcade games stay **zipped**: FBNeo reads a MAME/FBNeo romset as it is. The
scan claims a `.zip` only inside one of those folders, so a zipped SNES ROM is
not mistaken for an arcade game. The BIOS zips go in `~/Games/bios/`.

`version` is the pinned commit, `SHA256SUMS` the tarball. `check-desktop.sh`
checks the core the library lists like every other one.
