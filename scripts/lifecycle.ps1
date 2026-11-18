#!/usr/bin/env pwsh
# Run the debug-only native lifecycle smoke test.
#
# The launcher window is real; no global hotkey is registered and no external
# application is launched. The final line must be ANSA_LIFECYCLE_OK.
param(
    [ValidateRange(1, 2000)]
    [int]$Cycles = 1000
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo run -p ansa-desktop -- --lifecycle-smoke-test "--lifecycle-cycles=$Cycles"
    if ($LASTEXITCODE -ne 0) {
        throw "lifecycle smoke failed with exit code $LASTEXITCODE"
    }
} finally {
    Pop-Location
}
