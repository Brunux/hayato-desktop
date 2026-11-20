#!/bin/bash
# Build and stage all desktop artifacts (swayfx, workstyle, hayato binaries,
# RetroArch, the N64 core, the Neo Geo core).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
"$HERE/build-swayfx.sh"
"$HERE/build-workstyle.sh"
"$HERE/build-hayato.sh"
"$HERE/build-retroarch.sh"
"$HERE/build-mupen64plus-next.sh"
"$HERE/build-fbneo.sh"
