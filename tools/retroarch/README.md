# RetroArch, built slim for Hayato

The library runs RetroArch as its emulator host: it passes one libretro core and
one ROM, fullscreen, with `/etc/hayato/retroarch.cfg` appended to whatever the
user has in `~/.config/retroarch/retroarch.cfg`.

Debian's `retroarch` package is not used. It hard-depends on
`retroarch-assets`, which hard-depends on `fonts-noto-extra` (334 MB), and it
links Qt5 for the WIMP menu: about 547 MB installed before a single core. This
build has no Qt, no assets, no FFmpeg, no Vulkan, no online updater and no XMB,
Ozone, GLUI or MaterialUI. What remains is the binary (about 16 MB) and RGUI,
which needs no assets and is only the in-game menu; the library is the UI.

`version` is the pinned upstream tag, `SHA256SUMS` the tarball it was built
from. `scripts/build-retroarch.sh` fetches, configures, builds and packages it as
`retroarch_<version>-hayato1_<arch>.deb`; `core/scripts/install-retroarch.sh`
installs that package. The Debian packages for the cores (and their `.info`
files) come from `uis/sway/packages.list`; the N64 core is a separate build in
`tools/mupen64plus-next/`.
