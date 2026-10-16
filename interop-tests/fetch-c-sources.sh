#!/bin/sh
# SPDX-License-Identifier: BSD-3-Clause
# Fetches the C libmetal and open-amp sources the interop tests build against.
# Pinned so CI results are reproducible; bump deliberately.
set -eu
cd "$(dirname "$0")"
LIBMETAL_REV=205b5750415ce0e8f4c4f21e033b41814fac9f26   # v1.10.0+ (main, 2026-10-08)
OPENAMP_REV=a0ec4fc2c764dd8927cafbd751637a29eb6bd761    # v1.10.0+ (main, 2026-09-28)
mkdir -p c-src
fetch() {
    dir=c-src/$1
    if [ ! -d "$dir/.git" ]; then
        git clone --quiet "https://github.com/OpenAMP/$1.git" "$dir"
    fi
    git -C "$dir" fetch --quiet --depth 1 origin "$2" 2>/dev/null || git -C "$dir" fetch --quiet origin
    git -C "$dir" -c advice.detachedHead=false checkout --quiet "$2"
}
fetch libmetal "$LIBMETAL_REV"
fetch open-amp "$OPENAMP_REV"
echo "C sources ready in $(pwd)/c-src"
