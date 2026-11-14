#!/bin/sh
# Builds a Windows executable through the Windows-side toolchain via PowerShell interop.

cd "$(dirname "$0")/.." || exit 1
exec powershell.exe -NoProfile -Command "
  \$ErrorActionPreference = 'Stop'
  \$env:CARGO_INCREMENTAL = '0'
  if (-not \$env:CARGO_TARGET_DIR) {
    \$env:CARGO_TARGET_DIR = Join-Path \$env:USERPROFILE '.yapzm-target'
  }
  cargo build --release $*
  if (\$LASTEXITCODE -ne 0) { exit \$LASTEXITCODE }
  Copy-Item (Join-Path \$env:CARGO_TARGET_DIR 'release\yapzm.exe') \$env:USERPROFILE
  Write-Host ('Built ' + (Join-Path \$env:USERPROFILE 'yapzm.exe'))
"
