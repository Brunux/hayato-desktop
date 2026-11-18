#!/bin/bash
# Build and stage all desktop artifacts (swayfx, workstyle, lazyos binaries, RetroArch, N64 core).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
"$HERE/build-swayfx.sh"
"$HERE/build-workstyle.sh"
"$HERE/build-lazyos.sh"
"$HERE/build-retroarch.sh"
"$HERE/build-mupen64plus-next.sh"
