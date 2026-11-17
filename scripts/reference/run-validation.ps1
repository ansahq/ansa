# Cross-platform wrapper for the F01 reference checks (PowerShell 7+ / Windows PowerShell).
# No installs, no configuration changes and no reference application launches.
[CmdletBinding()]
param(
    [string]$Python = $env:PYTHON,
    [string]$Root = "",
    [switch]$SkipTests,
    [switch]$Json
)

$ErrorActionPreference = 'Stop'

function Resolve-Python {
    param([string]$Explicit)
    if ($Explicit) { return @($Explicit) }
    if ($env:PYTHON) { return @($env:PYTHON) }
    foreach ($name in @('python3', 'python')) {
        $command = Get-Command $name -ErrorAction SilentlyContinue
        if ($command) { return @($command.Source) }
    }
    $launcher = Get-Command 'py' -ErrorAction SilentlyContinue
    if ($launcher) { return @($launcher.Source, '-3') }
    throw "Python 3 was not found. Pass -Python <path> or set PYTHON."
}

if (-not $Root) {
    $Root = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
}

$runtime = @(Resolve-Python -Explicit $Python)
$executable = $runtime[0]
$prefix = @()
if ($runtime.Count -gt 1) { $prefix = $runtime[1..($runtime.Count - 1)] }

$validator = Join-Path $PSScriptRoot 'validate_reference.py'
$tests = Join-Path $PSScriptRoot 'test_validate_reference.py'

$validatorArguments = @($validator, '--root', $Root)
if ($Json) { $validatorArguments += @('--format', 'json') }

Write-Host "Validating F01 reference artifacts under $Root"
& $executable @prefix @validatorArguments
if ($LASTEXITCODE -ne 0) {
    throw "reference validation failed with exit code $LASTEXITCODE"
}

if (-not $SkipTests) {
    Write-Host "Running reference validator negative tests"
    & $executable @prefix $tests --root $Root
    if ($LASTEXITCODE -ne 0) {
        throw "reference negative tests failed with exit code $LASTEXITCODE"
    }
}

Write-Host "Reference checks passed. This validates the specification, not a reference runtime."
