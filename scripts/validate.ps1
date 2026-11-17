# Local foundation checks. No installers, configuration changes, or external app launches.
[CmdletBinding()]
param(
    [switch]$NativeSmoke,
    [switch]$Offline
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent

function Invoke-CargoCheck {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

Push-Location $root
try {
    $resolution = @('--locked')
    if ($Offline) { $resolution += '--offline' }
    Invoke-CargoCheck -CargoArguments @('fmt', '--all', '--', '--check')
    Invoke-CargoCheck -CargoArguments (@('test', '--workspace') + $resolution)
    Invoke-CargoCheck -CargoArguments (@('clippy', '--workspace', '--all-targets') + $resolution + @('--', '-D', 'warnings'))
    Invoke-CargoCheck -CargoArguments (@('check', '-p', 'ansa-desktop') + $resolution)
    if ($NativeSmoke) {
        Write-Host 'Opening temporary native windows for synthetic tests; these do not validate screen readers, physical IME input, or focus restoration.'
        Invoke-CargoCheck -CargoArguments (@('run', '-p', 'ansa-desktop') + $resolution + @('--', '--smoke-test'))
        Invoke-CargoCheck -CargoArguments (@('run', '-p', 'ansa-desktop') + $resolution + @('--', '--interaction-smoke-test'))
    }
    Write-Host 'Requested local checks passed. Native accessibility and physical-input release gates remain open.'
}
finally {
    Pop-Location
}
