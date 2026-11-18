# Mupen64Plus-Next, the N64 core

Debian forky has no N64 emulator and no N64 libretro core: not mupen64plus, not
mupen64plus-next, not parallel-n64. This is the core the games library launches
for `.n64`/`.z64`/`.v64` files, built here from the pinned libretro repository.

The build is the AArch64 dynarec with GLideN64 on GLES3 (`FORCE_GLES3=1`), which
is what the Mali-G57 through Panfrost provides. Parallel-RDP is not used: the
image's PanVK is a compute driver, not a Vulkan graphics implementation. The
core is about 4.4 MB.

N64 is the one **experimental** platform in the library: the CPU is a Cortex-A76
and the GPU a Mali-G57, so many games run at 640×480 and some do not reach full
speed. `version` is the pinned commit; `SHA256SUMS` the tarball; the library's
row is marked by the core's presence, and `check-desktop.sh` warns (does not
fail) when it is missing.
