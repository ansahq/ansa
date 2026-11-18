# Run the Ansa performance harness for one profile.
#
# Examples:
#   pwsh scripts/bench.ps1 quick
#   pwsh scripts/bench.ps1 full
#   pwsh scripts/bench.ps1 soak
#   pwsh scripts/bench.ps1 cancel-flood
#   pwsh scripts/bench.ps1 delayed-provider
#   pwsh scripts/bench.ps1 corpus
#
# This builds release artifacts first. The launcher --perf-smoke-test marker is
# debug-only by design, so launch samples use target/debug/ansa-desktop.exe even
# when the harness itself is release. No software is installed and no external
# application is launched by the fixture.

[CmdletBinding()]
param(
    [ValidateSet('quick', 'full', 'soak', 'cancel-flood', 'delayed-provider', 'corpus')]
    [string]$Profile = 'quick',

    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Extra
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Invoke-Cargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

Push-Location $root
try {
    Write-Host "Building release harness and desktop (locked)..."
    Invoke-Cargo @('build', '--release', '--locked', '-p', 'ansa-bench', '-p', 'ansa-desktop')

    $bench = Join-Path $root 'target\release\ansa-bench.exe'
    if (-not (Test-Path $bench)) {
        throw "release harness not found at $bench"
    }

    $arguments = @($Profile)
    if ($Profile -ne 'corpus') {
        Write-Host "Building debug desktop for the debug-only first-frame marker..."
        Invoke-Cargo @('build', '--locked', '-p', 'ansa-desktop')
        $desktop = Join-Path $root 'target\debug\ansa-desktop.exe'
        if (-not (Test-Path $desktop)) {
            throw "debug desktop fixture not found at $desktop"
        }
        $arguments += "--desktop=$desktop"
    }
    if ($Extra) {
        $arguments += $Extra
    }

    if ($Profile -eq 'full' -or $Profile -eq 'soak') {
        Write-Host "Profile '$Profile' is long-running. Results are only evidence for the parameters printed in summary.md."
    }
    & $bench @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "ansa-bench $($arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
    Write-Host "Raw trace and summary are under docs\benchmarks\runs\ by default."
}
finally {
    Pop-Location
}
