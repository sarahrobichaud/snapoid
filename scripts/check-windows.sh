#!/bin/sh
# Type-check the Windows build from WSL using the Windows-side Rust
# toolchain via PowerShell interop.
#
# - CARGO_INCREMENTAL=0: incremental compilation cannot lock files on the
#   9p filesystem that exposes WSL paths to Windows.
# - CARGO_TARGET_DIR: keep Windows artifacts on the native Windows
#   filesystem (faster than 9p, and avoids clobbering Linux artifacts in
#   the shared target/ directory).
exec powershell.exe -NoProfile -Command "Set-Item Env:CARGO_INCREMENTAL -Value '0'; Set-Item Env:CARGO_TARGET_DIR -Value 'C:\Users\zyriel\yapzm-target'; cargo check"
