#!/bin/bash
# Build and stage all desktop artifacts (swayfx, workstyle, lazyos binaries).
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
"$HERE/build-swayfx.sh"
"$HERE/build-workstyle.sh"
"$HERE/build-lazyos.sh"
