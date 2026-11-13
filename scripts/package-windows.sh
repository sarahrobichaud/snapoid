#!/bin/sh
# Build a distributable Windows zip of yapzm from WSL.
#
# Usage:
#   scripts/package-windows.sh              # version taken from Cargo.toml
#   scripts/package-windows.sh v0.1.0-proto # override the version in the name
#
# Steps:
#   1. mirror the project to C:\src\yapzm and build it there with the
#      native Windows toolchain (scripts/deploy-windows.sh --build)
#   2. zip the exe as yapzm-<version>-x86_64-windows.zip
#   3. copy the zip and the bare exe (same name, .exe suffix) back into
#      the project's dist/ directory and print the zip's sha256 (handy
#      for release notes)
#
# Attach either dist/ artifact to a GitHub release: the zip is the
# conventional choice, the bare exe is fine for a quick prototype.
set -eu

PROJECT_DIR=$(cd "$(dirname "$0")/.." && pwd)
WIN_PROJECT='C:/src/yapzm'
WIN_TARGET='C:/Users/zyriel/yapzm-target'
PS=/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe

VERSION=${1:-}
if [ -z "$VERSION" ]; then
    VERSION=v$(sed -n 's/^version = "\(.*\)"/\1/p' "$PROJECT_DIR/Cargo.toml" | head -n1)
fi
ZIP_NAME="yapzm-$VERSION-x86_64-windows.zip"

echo "==> deploy + build (Windows toolchain)"
sh "$PROJECT_DIR/scripts/deploy-windows.sh" --build

echo "==> packaging $ZIP_NAME"
"$PS" -NoProfile -Command "
    New-Item -ItemType Directory -Force -Path '$WIN_PROJECT/dist' | Out-Null
    Compress-Archive -Path '$WIN_TARGET/release/yapzm.exe' \
        -DestinationPath '$WIN_PROJECT/dist/$ZIP_NAME' -Force
"

DIST="$PROJECT_DIR/dist"
mkdir -p "$DIST"
cp "/mnt/c/src/yapzm/dist/$ZIP_NAME" "$DIST/"
cp "/mnt/c/Users/zyriel/yapzm-target/release/yapzm.exe" "$DIST/yapzm-$VERSION-x86_64-windows.exe"

echo "==> done"
echo "    artifact: dist/$ZIP_NAME"
echo "    bare exe: dist/yapzm-$VERSION-x86_64-windows.exe"
echo "    sha256:   $(sha256sum "$DIST/$ZIP_NAME" | cut -d' ' -f1)"
