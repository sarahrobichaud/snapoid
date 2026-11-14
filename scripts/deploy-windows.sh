#!/bin/sh
# Deploy the project to the Windows filesystem (C:\src\snapoid) so it can
# be built and tested with the native Windows toolchain instead of through
# the slow 9p path.
#
# Usage:
#   scripts/deploy-windows.sh           copy the project only
#   scripts/deploy-windows.sh --build   copy, then cargo build --release
#                                       on the Windows side
#
# - target/ and .git/ are skipped: the Windows build keeps its artifacts
#   in its own target dir (see check-windows.sh), and git history is not
#   needed for testing.
# - rsync --delete mirrors the tree, so files removed from the project
#   disappear on the Windows side too.
set -eu

PROJECT_DIR=$(cd "$(dirname "$0")/.." && pwd)
DEST=/mnt/c/src/snapoid

mkdir -p "$(dirname "$DEST")"
rsync -rlpt --delete \
    --exclude target/ \
    --exclude .git/ \
    "$PROJECT_DIR/" "$DEST/"
echo "deployed to $DEST"

if [ "${1:-}" = "--build" ]; then
    cd "$DEST"
    # Same env handling as check-windows.sh: incremental compilation
    # cannot lock files on 9p, and artifacts stay on the native Windows
    # filesystem.
    exec powershell.exe -NoProfile -Command "Set-Item Env:CARGO_INCREMENTAL -Value '0'; Set-Item Env:CARGO_TARGET_DIR -Value 'C:\Users\zyriel\snapoid-target'; cargo build --release"
fi
