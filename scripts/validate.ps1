# Local F03/foundation checks. No installers, configuration changes, or external
# app launches. All Cargo resolution uses --locked; use -Offline after fetching.
[CmdletBinding()]
param(
    [switch]$NativeSmoke,
    [switch]$Offline,
    [string]$Toolchain,
    [string]$PythonExecutable,
    [switch]$SkipDependencyInventory,
    [switch]$SkipReferenceValidation,
    [switch]$SkipReuseToolingTests,
    [switch]$SkipDesktopBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
$cargoPrefix = @()
if ($Toolchain) {
    $cargoPrefix = @("+$Toolchain")
}

function Invoke-CargoCheck {
    param(
        [string[]]$CargoArguments,
        [string[]]$CargoPrefix = @()
    )
    & cargo @CargoPrefix @CargoArguments
    if ($LASTEXITCODE -ne 0) {
        throw "cargo $($CargoPrefix -join ' ') $($CargoArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

function Invoke-PythonCheck {
    param([string]$Executable, [string[]]$PythonArguments)
    & $Executable @PythonArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Python check $($PythonArguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

Push-Location $root
$previousRustupToolchain = $env:RUSTUP_TOOLCHAIN
try {
    if ($Toolchain) {
        $env:RUSTUP_TOOLCHAIN = $Toolchain
        Write-Host "Explicit toolchain override '$Toolchain': this run does not establish acceptance of the checked-in toolchain selection."
    }
    $python = if ($PythonExecutable) { $PythonExecutable }
        elseif ($env:PYTHON) { $env:PYTHON }
        else { 'python' }
    $resolution = @('--locked')
    if ($Offline) { $resolution += '--offline' }

    Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments @('fmt', '--all', '--', '--check')
    Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('test', '--workspace', '--lib', '--bins', '--tests') + $resolution)
    Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('test', '--workspace', '--doc') + $resolution)
    Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('clippy', '--workspace', '--all-targets') + $resolution + @('--', '-D', 'warnings'))
    if (-not $SkipDesktopBuild) {
        Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('build', '-p', 'ansa-desktop') + $resolution)
    }

    Invoke-PythonCheck -Executable $python -PythonArguments @('-m', 'unittest', 'discover', '-s', 'scripts/build/tests', '-v')

    if (-not $SkipDependencyInventory) {
        Invoke-PythonCheck -Executable $python -PythonArguments @('-m', 'unittest', 'discover', '-s', 'scripts/dependencies/tests', '-v')
        $inventoryArguments = @('scripts/dependencies/inventory.py', '--check')
        if ($Toolchain) { $inventoryArguments += @('--toolchain', $Toolchain) }
        Invoke-PythonCheck -Executable $python -PythonArguments $inventoryArguments
    }

    if (-not $SkipReferenceValidation) {
        Invoke-PythonCheck -Executable $python -PythonArguments @('scripts/reference/validate_reference.py', '--root', '.')
        Invoke-PythonCheck -Executable $python -PythonArguments @('scripts/reference/test_validate_reference.py')
    }

    if (-not $SkipReuseToolingTests) {
        if ($env:OS -eq 'Windows_NT') {
            & pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1
            if ($LASTEXITCODE -ne 0) { throw 'Reuse probe safety tests failed' }
        }
        else {
            Write-Host 'Reuse probe safety tests are Windows-only; not exercised on this host.'
        }
    }

    if ($NativeSmoke) {
        Write-Host 'Opening temporary native windows for synthetic tests; these do not validate screen readers, physical IME input, or focus restoration.'
        Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('run', '-p', 'ansa-desktop') + $resolution + @('--', '--smoke-test'))
        Invoke-CargoCheck -CargoPrefix $cargoPrefix -CargoArguments (@('run', '-p', 'ansa-desktop') + $resolution + @('--', '--interaction-smoke-test'))
    }

    Write-Host 'Requested local checks passed. The dependency inventory is a lockfile artifact, not a legal review. Native accessibility and physical-input release gates remain open.'
}
finally {
    $env:RUSTUP_TOOLCHAIN = $previousRustupToolchain
    Pop-Location
}
