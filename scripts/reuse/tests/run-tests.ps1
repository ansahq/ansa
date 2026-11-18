# Isolated tests for scripts/reuse.
#
# These tests use temporary miniature git repositories and a fake cargo
# executable. They never touch the pinned candidate checkouts and never run a
# candidate build. Run with:
#   pwsh -NoProfile -File scripts/reuse/tests/run-tests.ps1
[CmdletBinding()]
param(
    [switch]$KeepTemp
)

$ErrorActionPreference = 'Stop'

$scriptsDir = Split-Path $PSScriptRoot -Parent
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $scriptsDir '..\..'))
$boundedBuild = Join-Path $scriptsDir 'bounded-build.ps1'
$fetchCandidates = Join-Path $scriptsDir 'fetch-candidates.ps1'
$scanDependencies = Join-Path $scriptsDir 'scan-dependencies.ps1'
$pwsh = (Get-Process -Id $PID).Path
$git = (Get-Command git -ErrorAction Stop).Source
$root = Join-Path $env:TEMP ('opencode\reuse-script-tests-' + [guid]::NewGuid().ToString('N'))

$script:Passed = 0
$script:Failed = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) { throw $Message }
}

function Invoke-ChildScript {
    param([string]$ScriptPath, [string[]]$Arguments)

    $output = & $script:Pwsh -NoProfile -File $ScriptPath @Arguments 2>&1
    [pscustomobject]@{
        ExitCode = $LASTEXITCODE
        Output   = (@($output | ForEach-Object { "$_" }) -join "`n")
    }
}

function Test-Case {
    param([string]$Name, [scriptblock]$Body)

    try {
        & $Body
        $script:Passed++
        Write-Host "PASS $Name"
    }
    catch {
        $script:Failed++
        Write-Host "FAIL $Name :: $($_.Exception.Message)"
    }
    finally {
        foreach ($variable in @('FAKE_CARGO_MARKER', 'FAKE_CARGO_RECORD', 'FAKE_CARGO_EXIT', 'FAKE_CARGO_SLEEP_MS', 'CARGO_TARGET_DIR')) {
            Remove-Item "Env:\$variable" -ErrorAction SilentlyContinue
        }
    }
}

function Invoke-GitReal {
    param([string]$Path, [string[]]$Arguments)

    $output = & $script:Git -C $Path @Arguments 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') failed in '$Path': $($output -join ' ')"
    }
    return @($output | ForEach-Object { "$_" })
}

function Write-PinFile {
    param([string]$Path, [object[]]$Entries)

    $json = ConvertTo-Json -InputObject @($Entries) -Depth 5
    Set-Content -LiteralPath $Path -Value $json -Encoding utf8
}

function New-Checkout {
    param([string]$Destination)

    $parent = Split-Path $Destination -Parent
    New-Item -ItemType Directory -Force -Path $parent | Out-Null
    $output = & $script:Git clone --quiet $script:Seed $Destination 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "git clone failed: $($output -join ' ')"
    }
}

function Read-Records {
    param([string]$Destination)

    $path = Join-Path $Destination 'results.jsonl'
    if (-not (Test-Path -LiteralPath $path)) { return @() }
    return @(Get-Content -LiteralPath $path |
            Where-Object { $_.Trim() -ne '' } |
            ForEach-Object { $_ | ConvertFrom-Json })
}

function Get-CargoJsonArguments {
    param([string]$FakeCargoPath)
    ConvertTo-Json -InputObject @('-NoProfile', '-File', $FakeCargoPath) -Compress
}

function Get-GitJsonArguments {
    param([string]$FakeGitPath)
    ConvertTo-Json -InputObject @('-NoProfile', '-File', $FakeGitPath) -Compress
}

function Invoke-Bounded {
    param(
        [string]$Destination,
        [string]$PinFile,
        [string[]]$Candidates = @('gauntlet'),
        [int]$TimeoutSeconds = 600
    )

    $arguments = @(
        '-Destination', $Destination,
        '-PinFile', $PinFile,
        '-Candidate', ($Candidates -join ','),
        '-CargoPath', $script:Pwsh,
        '-CargoPathArgumentsJson', (Get-CargoJsonArguments $script:FakeCargo),
        '-TimeoutSeconds', [string]$TimeoutSeconds
    )
    Invoke-ChildScript -ScriptPath $script:BoundedBuild -Arguments $arguments
}

function Invoke-Fetch {
    param([string]$Destination, [string]$PinFile)

    Invoke-ChildScript -ScriptPath $script:FetchCandidates -Arguments @(
        '-Destination', $Destination,
        '-PinFile', $PinFile
    )
}

function Invoke-Scan {
    param(
        [string]$Destination,
        [string]$PinFile,
        [string]$Candidate = 'gauntlet',
        [string]$OutputPath
    )

    $arguments = @(
        '-Destination', $Destination,
        '-PinFile', $PinFile,
        '-Candidate', $Candidate
    )
    if ($OutputPath) { $arguments += @('-OutputPath', $OutputPath) }
    Invoke-ChildScript -ScriptPath $script:ScanDependencies -Arguments $arguments
}

try {
    New-Item -ItemType Directory -Force -Path $root | Out-Null
    $script:Seed = Join-Path $root 'seed'
    $script:BoundedBuild = $boundedBuild
    $script:FetchCandidates = $fetchCandidates
    $script:ScanDependencies = $scanDependencies
    $script:Pwsh = $pwsh
    $script:Git = $git
    $script:RepositoryRoot = $repositoryRoot
    New-Item -ItemType Directory -Force -Path $script:Seed | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $root 'markers') | Out-Null

    Invoke-GitReal -Path $script:Seed -Arguments @('init', '--quiet')
    Set-Content -LiteralPath (Join-Path $script:Seed 'README.md') -Value 'seed content'
    New-Item -ItemType Directory -Force -Path (Join-Path $script:Seed 'sub') | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $script:Seed 'src') | Out-Null
    Set-Content -LiteralPath (Join-Path $script:Seed 'Cargo.lock') -Encoding utf8 -Value @'
version = 4

[[package]]
name = "fixture-root"
version = "0.1.0"

[[package]]
name = "fixture-registry-a"
version = "1.0.0"
source = "registry+https://github.com/rust-lang/crates.io-index"
checksum = "0000000000000000000000000000000000000000000000000000000000000000"
'@
    Set-Content -LiteralPath (Join-Path $script:Seed 'Cargo.toml') -Encoding utf8 -Value @'
[package]
name = "fixture-root"
version = "0.1.0"
license = "MIT"
'@
    Set-Content -LiteralPath (Join-Path $script:Seed 'sub\Cargo.toml') -Encoding utf8 -Value @'
[package]
name = "fixture-sub"
version = "0.1.0"
'@
    Set-Content -LiteralPath (Join-Path $script:Seed 'package.json') -Encoding utf8 -Value '{"name":"fixture","license":"ISC"}'
    Set-Content -LiteralPath (Join-Path $script:Seed 'package-lock.json') -Encoding utf8 -Value '{"name":"fixture","lockfileVersion":3,"packages":{"":{"name":"fixture","license":"ISC"},"node_modules/a":{"version":"1.0.0","license":"MIT"},"node_modules/b":{"version":"1.0.0"}}}'
    Set-Content -LiteralPath (Join-Path $script:Seed 'CMakeLists.txt') -Encoding utf8 -Value @'
cmake_minimum_required(VERSION 3.20)
project(fixture LANGUAGES CXX)
include(FetchContent)
FetchContent_Declare(fixture_dep GIT_REPOSITORY https://example.invalid/fixture.git GIT_TAG v1.2.3)
find_package(Qt6 6.9 REQUIRED COMPONENTS Core)
'@
    Set-Content -LiteralPath (Join-Path $script:Seed 'LICENSE') -Encoding utf8 -Value 'Mozilla Public License Version 2.0'
    Set-Content -LiteralPath (Join-Path $script:Seed 'src\lib.rs') -Encoding utf8 -Value '// SPDX-License-Identifier: MIT'
    [void](Invoke-GitReal -Path $script:Seed -Arguments @('add', '.'))
    [void](Invoke-GitReal -Path $script:Seed -Arguments @('-c', 'user.name=Reuse Script Test', '-c', 'user.email=test@example.invalid', 'commit', '-m', 'seed', '--quiet'))
    $script:Pin = ((Invoke-GitReal -Path $script:Seed -Arguments @('rev-parse', 'HEAD')) -join '').Trim()

    $script:FakeCargo = Join-Path $root 'fake-cargo.ps1'
    Set-Content -LiteralPath $script:FakeCargo -Encoding utf8 -Value @'
Write-Output "fake-cargo-args: $($args -join ' ')"
[Console]::Error.WriteLine('fake-cargo-stderr')
$record = $env:FAKE_CARGO_RECORD
if ($record) {
    "TARGET=$env:CARGO_TARGET_DIR" | Add-Content -LiteralPath $record
    "ARGS=$($args -join '|')" | Add-Content -LiteralPath $record
}
if ($env:FAKE_CARGO_MARKER) {
    Set-Content -LiteralPath $env:FAKE_CARGO_MARKER -Value 'invoked'
}
if ($env:FAKE_CARGO_SLEEP_MS) {
    Start-Sleep -Milliseconds ([int]$env:FAKE_CARGO_SLEEP_MS)
}
if ($env:FAKE_CARGO_EXIT) {
    exit [int]$env:FAKE_CARGO_EXIT
}
exit 0
'@

    $script:FakeGit = Join-Path $root 'fake-git-fail-remote.ps1'
    Set-Content -LiteralPath $script:FakeGit -Encoding utf8 -Value @'
if ($args -contains 'remote') {
    [Console]::Error.WriteLine('simulated git failure')
    exit 7
}
exit 0
'@

    $script:FakeGitRevParse = Join-Path $root 'fake-git-fail-rev-parse.ps1'
    Set-Content -LiteralPath $script:FakeGitRevParse -Encoding utf8 -Value @'
if ($args -contains 'rev-parse') {
    [Console]::Error.WriteLine('simulated rev-parse failure')
    exit 3
}
[Console]::Out.WriteLine('')
exit 0
'@

    Test-Case 'bounded-build rejects a wrong pin before running cargo' {
        $destination = Join-Path $root 'wrong-pin'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-wrong.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = $script:Seed; commit = ('f' * 40) })
        $marker = Join-Path $root 'markers\wrong-pin.txt'
        New-Item -ItemType Directory -Force -Path (Split-Path $marker -Parent) | Out-Null
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) "expected refusal exit code, got 0. Output: $($result.Output)"
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for a wrong pin'
        $records = Read-Records -Destination $destination
        Assert-True ($records.Count -eq 1 -and $records[0].kind -eq 'refusal') 'expected one refusal record'
        Assert-True ($records[0].message -match 'pin mismatch') 'refusal must name the pin mismatch'
    }

    Test-Case 'bounded-build rejects a tracked modification before running cargo' {
        $destination = Join-Path $root 'dirty-tracked'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        Set-Content -LiteralPath (Join-Path $destination 'gauntlet\README.md') -Value 'local tracked change'
        $pinFile = Join-Path $root 'pins-ok.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = $script:Seed; commit = $script:Pin })
        $marker = Join-Path $root 'markers\dirty-tracked.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected refusal exit code'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for a dirty checkout'
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].kind -eq 'refusal' -and $records[0].message -match 'dirty') 'refusal must name the dirty checkout'
        Assert-True ((Get-Content -LiteralPath (Join-Path $destination 'gauntlet\README.md') -Raw).Trim() -eq 'local tracked change') 'local change must be preserved'
    }

    Test-Case 'bounded-build rejects an untracked file before running cargo' {
        $destination = Join-Path $root 'dirty-untracked'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        Set-Content -LiteralPath (Join-Path $destination 'gauntlet\untracked.txt') -Value 'untracked'
        $pinFile = Join-Path $root 'pins-ok.json'
        $marker = Join-Path $root 'markers\dirty-untracked.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected refusal exit code'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for an untracked file'
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].kind -eq 'refusal' -and $records[0].message -match 'dirty') 'refusal must name the dirty checkout'
    }

    Test-Case 'bounded-build verifies all selected checkouts before any cargo run' {
        $destination = Join-Path $root 'multi'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        New-Checkout -Destination (Join-Path $destination 'pop-launcher')
        Set-Content -LiteralPath (Join-Path $destination 'pop-launcher\README.md') -Value 'dirty second candidate'
        $pinFile = Join-Path $root 'pins-multi.json'
        Write-PinFile -Path $pinFile -Entries @(
            [ordered]@{ name = 'gauntlet'; repository = $script:Seed; commit = $script:Pin },
            [ordered]@{ name = 'pop-launcher'; repository = $script:Seed; commit = $script:Pin }
        )
        $marker = Join-Path $root 'markers\multi.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile -Candidates @('gauntlet', 'pop-launcher')
        Assert-True ($result.ExitCode -ne 0) 'expected refusal exit code'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'no candidate may run when a later selected checkout is dirty'
        Assert-True ($result.Output -match 'pop-launcher') 'refusal must identify the dirty later candidate'
    }

    Test-Case 'bounded-build restores a set CARGO_TARGET_DIR and records the probe' {
        $destination = Join-Path $root 'env-set'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $recordFile = Join-Path $root 'env-set-cargo.txt'
        $callerValue = 'caller-target-' + [guid]::NewGuid().ToString('N')
        $env:CARGO_TARGET_DIR = $callerValue
        $env:FAKE_CARGO_RECORD = $recordFile
        $env:FAKE_CARGO_EXIT = '0'

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -eq 0) "expected collector exit 0, got $($result.ExitCode): $($result.Output)"
        Assert-True ($env:CARGO_TARGET_DIR -eq $callerValue) 'CARGO_TARGET_DIR was not restored'
        $records = Read-Records -Destination $destination
        Assert-True ($records.Count -eq 1 -and $records[0].kind -eq 'probe') 'expected one probe record'
        Assert-True ($records[0].exitCode -eq 0) 'probe exit code must be recorded'
        Assert-True ($records[0].actualCommit -eq $script:Pin) 'probe record must carry the verified commit'
        Assert-True ($records[0].command -contains '--locked') 'probe record must carry the command'
        $cargoRecord = Get-Content -LiteralPath $recordFile
        Assert-True (($cargoRecord -join "`n") -notmatch [regex]::Escape($callerValue)) 'cargo must receive the probe target dir, not the caller value'
    }

    Test-Case 'bounded-build leaves an unset CARGO_TARGET_DIR unset' {
        $destination = Join-Path $root 'env-unset'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
        $env:FAKE_CARGO_EXIT = '0'

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -eq 0) "expected collector exit 0, got $($result.ExitCode)"
        Assert-True (-not (Test-Path Env:\CARGO_TARGET_DIR)) 'unset CARGO_TARGET_DIR must remain unset'
    }

    Test-Case 'bounded-build rejects zero, negative and overflowing timeouts before cargo' {
        $destination = Join-Path $root 'timeout-args'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $marker = Join-Path $root 'markers\timeout-args.txt'
        $env:FAKE_CARGO_MARKER = $marker

        foreach ($value in @('0', '-1', '86401')) {
            $result = Invoke-Bounded -Destination $destination -PinFile $pinFile -TimeoutSeconds ([int]$value)
            Assert-True ($result.ExitCode -ne 0) "timeout '$value' must be rejected"
        }
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for invalid timeout arguments'
    }

    Test-Case 'bounded-build enforces a positive timeout and records it' {
        $destination = Join-Path $root 'timeout-run'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $env:FAKE_CARGO_SLEEP_MS = '10000'

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile -TimeoutSeconds 1
        Assert-True ($result.ExitCode -eq 0) "expected collector exit 0 after timeout, got $($result.ExitCode): $($result.Output)"
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].timedOut -eq $true) 'expected timedOut=true'
        Assert-True ($null -eq $records[0].exitCode) 'timed-out probe must have null exit code'
        Assert-True ($records[0].timeoutSeconds -eq 1) 'timeoutSeconds must be recorded'
        Assert-True ($records[0].durationSeconds -lt 10) 'timeout must stop the probe, not wait for it'
        Assert-True ($records[0].processStopConfirmed -eq $true) 'timeout must record confirmed process stop'
        Assert-True ($null -eq $records[0].cleanupError) 'clean timeout must not record a cleanup error'
    }

    Test-Case 'bounded-build accepts normalizable HTTPS origin variants' {
        $destination = Join-Path $root 'origin-normalized'
        $checkout = Join-Path $destination 'gauntlet'
        New-Checkout -Destination $checkout
        [void](Invoke-GitReal -Path $checkout -Arguments @('remote', 'set-url', 'origin', 'https://github.com/Owner/Repo'))
        $pinFile = Join-Path $root 'pins-normalized.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = 'https://github.com/owner/repo.git/'; commit = $script:Pin })
        $env:FAKE_CARGO_EXIT = '0'

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -eq 0) "expected normalized origin acceptance, got $($result.ExitCode): $($result.Output)"
        $records = Read-Records -Destination $destination
        Assert-True ($records.Count -eq 1 -and $records[0].kind -eq 'probe') 'normalized origin must produce a probe record'
    }

    Test-Case 'bounded-build refuses a different owner/repository origin before cargo' {
        $destination = Join-Path $root 'origin-mismatch'
        $checkout = Join-Path $destination 'gauntlet'
        New-Checkout -Destination $checkout
        [void](Invoke-GitReal -Path $checkout -Arguments @('remote', 'set-url', 'origin', 'https://example.invalid/other/repo'))
        $pinFile = Join-Path $root 'pins-origin.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = 'https://example.invalid/owner/repo'; commit = $script:Pin })
        $marker = Join-Path $root 'markers\origin-mismatch.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected origin mismatch refusal'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for a wrong origin'
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].kind -eq 'refusal' -and $records[0].message -match 'remote origin mismatch') 'refusal must name the origin mismatch'
    }

    Test-Case 'bounded-build rejects credentialed origin URLs' {
        $destination = Join-Path $root 'origin-credentials'
        $checkout = Join-Path $destination 'gauntlet'
        New-Checkout -Destination $checkout
        [void](Invoke-GitReal -Path $checkout -Arguments @('remote', 'set-url', 'origin', 'https://token@github.com/owner/repo'))
        $pinFile = Join-Path $root 'pins-credentials.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = 'https://github.com/owner/repo'; commit = $script:Pin })
        $marker = Join-Path $root 'markers\origin-credentials.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected credentialed-origin refusal'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for a credentialed origin'
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].message -match 'credentialed URLs are not accepted') 'refusal must name the credential rejection'
    }

    Test-Case 'bounded-build refuses a destination nested inside this Ansa repository' {
        $destination = Join-Path $script:RepositoryRoot ('guard-nested-' + [guid]::NewGuid().ToString('N'))
        $pinFile = Join-Path $root 'pins-ok.json'
        $marker = Join-Path $root 'markers\guard-nested.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected repository destination refusal'
        Assert-True ($result.Output -match 'inside this Ansa repository') 'refusal must name the repository boundary'
        Assert-True (-not (Test-Path -LiteralPath $destination)) 'refused destination must not be created'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for an in-repository destination'
    }

    Test-Case 'bounded-build refuses the repository root as destination without writing records' {
        $destination = $script:RepositoryRoot
        $pinFile = Join-Path $root 'pins-ok.json'
        $rootResults = Join-Path $destination 'results.jsonl'
        $hadRootResults = Test-Path -LiteralPath $rootResults
        $marker = Join-Path $root 'markers\guard-root.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected repository-root refusal'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run for the repository root'
        Assert-True ((Test-Path -LiteralPath $rootResults) -eq $hadRootResults) 'guard must not write results into the repository root'
    }

    Test-Case 'bounded-build refuses a destination reached through a junction into the repository' {
        $link = Join-Path $root 'repo-junction'
        New-Item -ItemType Junction -Path $link -Target $script:RepositoryRoot | Out-Null
        try {
            $destination = Join-Path $link ('guard-junction-' + [guid]::NewGuid().ToString('N'))
            $pinFile = Join-Path $root 'pins-ok.json'
            $marker = Join-Path $root 'markers\guard-junction.txt'
            $env:FAKE_CARGO_MARKER = $marker

            $result = Invoke-Bounded -Destination $destination -PinFile $pinFile
            Assert-True ($result.ExitCode -ne 0) 'expected junction destination refusal'
            Assert-True ($result.Output -match 'inside this Ansa repository') 'junction refusal must name the repository boundary'
            Assert-True (-not (Test-Path -LiteralPath $destination)) 'junction destination must not be created'
            Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run through a junction destination'
        }
        finally {
            if (Test-Path -LiteralPath $link) { [System.IO.Directory]::Delete($link) }
        }
    }

    Test-Case 'fetch refuses a destination nested inside this Ansa repository' {
        $destination = Join-Path $script:RepositoryRoot ('guard-fetch-' + [guid]::NewGuid().ToString('N'))
        $pinFile = Join-Path $root 'pins-ok.json'

        $result = Invoke-Fetch -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected fetch repository destination refusal'
        Assert-True ($result.Output -match 'inside this Ansa repository') 'fetch refusal must name the repository boundary'
        Assert-True (-not (Test-Path -LiteralPath $destination)) 'refused fetch destination must not be created'
    }

    Test-Case 'bounded-build appends records and isolates logs across runs' {
        $destination = Join-Path $root 'append-runs'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $env:FAKE_CARGO_EXIT = '0'

        $first = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($first.ExitCode -eq 0) "first run failed: $($first.Output)"
        $resultsPath = Join-Path $destination 'results.jsonl'
        $beforeText = [System.IO.File]::ReadAllText($resultsPath)
        $beforeRecords = Read-Records -Destination $destination
        Assert-True ($beforeRecords.Count -eq 1) 'first run must write exactly one record'
        $firstRecord = $beforeRecords[0]
        $firstLog = [System.IO.File]::ReadAllText($firstRecord.stdoutPath)
        Assert-True (-not [string]::IsNullOrWhiteSpace($firstLog)) 'first run log must contain output'

        $second = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($second.ExitCode -eq 0) "second run failed: $($second.Output)"
        $afterText = [System.IO.File]::ReadAllText($resultsPath)
        $records = Read-Records -Destination $destination
        Assert-True ($records.Count -eq 2) 'second run must append, not replace'
        Assert-True ($afterText.StartsWith($beforeText)) 'prior result bytes must be retained exactly'
        Assert-True ($records[0].runId -eq $firstRecord.runId) 'first runId must be retained'
        Assert-True ($records[1].runId -ne $records[0].runId) 'each run must have a distinct runId'
        Assert-True ($records[0].stdoutPath -ne $records[1].stdoutPath) 'log paths must differ per run'
        Assert-True ($records[1].stdoutPath -match [regex]::Escape("logs\$($records[1].runId)\gauntlet.out")) 'second log must live under its runId directory'
        Assert-True ($records[0].stdoutPath -match [regex]::Escape("logs\$($records[0].runId)\gauntlet.out")) 'first log must live under its runId directory'
        Assert-True ([System.IO.File]::ReadAllText($firstRecord.stdoutPath) -eq $firstLog) 'prior log bytes must be retained'
        Assert-True (($records[1].command -join ' ') -eq 'check -p gauntlet --locked') 'semantic command must remain separate'
        Assert-True (($records[1].arguments -join ' ') -match '-NoProfile') 'full arguments must include the injected prefix'
        Assert-True (($records[1].arguments -join ' ') -match '-File') 'full arguments must include the injected script argument'
        Assert-True ((@($records[1].arguments) -contains '--locked')) 'full arguments must include probe arguments'
    }

    Test-Case 'a refusal outside the repository preserves earlier probe records' {
        $destination = Join-Path $root 'refusal-preserve'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $wrongPinFile = Join-Path $root 'pins-wrong.json'
        $env:FAKE_CARGO_EXIT = '0'

        $first = Invoke-Bounded -Destination $destination -PinFile $pinFile
        Assert-True ($first.ExitCode -eq 0) "first run failed: $($first.Output)"
        $resultsPath = Join-Path $destination 'results.jsonl'
        $beforeText = [System.IO.File]::ReadAllText($resultsPath)

        $refused = Invoke-Bounded -Destination $destination -PinFile $wrongPinFile
        Assert-True ($refused.ExitCode -ne 0) 'wrong pin must be refused'
        $afterText = [System.IO.File]::ReadAllText($resultsPath)
        Assert-True ($afterText.StartsWith($beforeText)) 'refusal must not truncate earlier records'
        $records = Read-Records -Destination $destination
        Assert-True ($records.Count -eq 2) 'refusal must append to earlier records'
        Assert-True ($records[0].kind -eq 'probe') 'earlier probe record must remain first'
        Assert-True ($records[1].kind -eq 'refusal') 'refusal must be appended'
    }

    Test-Case 'fetch rejects a dirty existing checkout without discarding it' {
        $destination = Join-Path $root 'fetch-dirty'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        Set-Content -LiteralPath (Join-Path $destination 'gauntlet\README.md') -Value 'keep this change'
        $pinFile = Join-Path $root 'pins-ok.json'

        $result = Invoke-Fetch -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected fetch refusal exit code'
        Assert-True ($result.Output -match 'dirty') 'refusal must name the dirty checkout'
        Assert-True ((Get-Content -LiteralPath (Join-Path $destination 'gauntlet\README.md') -Raw).Trim() -eq 'keep this change') 'fetch must not discard local work'
    }

    Test-Case 'fetch rejects a clean checkout at the wrong pin without mutating it' {
        $destination = Join-Path $root 'fetch-wrong-pin'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $before = ((Invoke-GitReal -Path (Join-Path $destination 'gauntlet') -Arguments @('rev-parse', 'HEAD')) -join '').Trim()
        $pinFile = Join-Path $root 'pins-other.json'
        Write-PinFile -Path $pinFile -Entries @([ordered]@{ name = 'gauntlet'; repository = $script:Seed; commit = ('a' * 40) })

        $result = Invoke-Fetch -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -ne 0) 'expected fetch refusal exit code'
        Assert-True ($result.Output -match 'pin mismatch') 'refusal must name the pin mismatch'
        $after = ((Invoke-GitReal -Path (Join-Path $destination 'gauntlet') -Arguments @('rev-parse', 'HEAD')) -join '').Trim()
        Assert-True ($after -eq $before) 'fetch must not move a clean mismatched checkout'
    }

    Test-Case 'fetch performs a new pinned checkout' {
        $destination = Join-Path $root 'fetch-new'
        $pinFile = Join-Path $root 'pins-ok.json'

        $result = Invoke-Fetch -Destination $destination -PinFile $pinFile
        Assert-True ($result.ExitCode -eq 0) "expected fetch success, got $($result.ExitCode): $($result.Output)"
        $head = ((Invoke-GitReal -Path (Join-Path $destination 'gauntlet') -Arguments @('rev-parse', 'HEAD')) -join '').Trim()
        Assert-True ($head -eq $script:Pin) 'new checkout must be at the pin'
        $status = Invoke-GitReal -Path (Join-Path $destination 'gauntlet') -Arguments @('status', '--porcelain=v1', '--untracked-files=all')
        Assert-True (@($status | Where-Object { $_ -ne '' }).Count -eq 0) 'new checkout must be clean'
    }

    Test-Case 'fetch fails non-zero when a native git command fails' {
        $destination = Join-Path $root 'fetch-git-failure'
        $pinFile = Join-Path $root 'pins-ok.json'

        $result = Invoke-ChildScript -ScriptPath $script:FetchCandidates -Arguments @(
            '-Destination', $destination,
            '-PinFile', $pinFile,
            '-GitPath', $script:Pwsh,
            '-GitPathArgumentsJson', (Get-GitJsonArguments $script:FakeGit)
        )
        Assert-True ($result.ExitCode -ne 0) 'expected non-zero exit when git fails'
        Assert-True ($result.Output -match 'exit 7') 'error must report the native git exit code'
    }

    Test-Case 'bounded-build refuses when pin verification git call fails' {
        $destination = Join-Path $root 'verify-git-failure'
        $checkout = Join-Path $destination 'gauntlet'
        New-Item -ItemType Directory -Force -Path (Join-Path $checkout '.git') | Out-Null
        $pinFile = Join-Path $root 'pins-ok.json'
        $marker = Join-Path $root 'markers\verify-git-failure.txt'
        $env:FAKE_CARGO_MARKER = $marker

        $result = Invoke-ChildScript -ScriptPath $script:BoundedBuild -Arguments @(
            '-Destination', $destination,
            '-PinFile', $pinFile,
            '-Candidate', 'gauntlet',
            '-GitPath', $script:Pwsh,
            '-GitPathArgumentsJson', (Get-GitJsonArguments $script:FakeGitRevParse)
        )
        Assert-True ($result.ExitCode -ne 0) 'expected refusal when git rev-parse fails'
        Assert-True (-not (Test-Path -LiteralPath $marker)) 'cargo must not run when git verification fails'
        $records = Read-Records -Destination $destination
        Assert-True ($records[0].kind -eq 'refusal') 'expected a refusal record'
        Assert-True (($records[0].message -match 'exit 3')) 'refusal must report the native git exit code'
    }

    Test-Case 'dependency scan reports static counts, unknowns and no-network scope' {
        $destination = Join-Path $root 'scan-static'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $outputPath = Join-Path $destination 'dependency-scan.json'

        $result = Invoke-Scan -Destination $destination -PinFile $pinFile -OutputPath $outputPath
        Assert-True ($result.ExitCode -eq 0) "scan failed: $($result.Output)"
        Assert-True (Test-Path -LiteralPath $outputPath) 'scan must write its evidence record'
        $scan = Get-Content -LiteralPath $outputPath -Raw | ConvertFrom-Json
        $candidate = $scan.candidates[0]
        Assert-True ($scan.schema -eq 'ansa.reuse.dependency-scan/v1') 'unexpected scan schema'
        Assert-True ($candidate.commit -eq $script:Pin) 'scan must record the verified commit'
        Assert-True ($candidate.cargo.lockfiles[0].packageCount -eq 2) 'cargo lock package count is wrong'
        Assert-True ($candidate.cargo.lockfiles[0].registrySourceCount -eq 1) 'registry source count is wrong'
        Assert-True ($candidate.cargo.manifests.manifestCount -eq 2) 'manifest count is wrong'
        Assert-True ($candidate.cargo.manifests.withLicenseDeclaration -eq 1) 'declared manifest licence count is wrong'
        Assert-True ($candidate.cargo.manifests.withoutLicenseDeclaration -eq 1) 'undeclared manifest count is wrong'
        Assert-True ($candidate.node.packageJsonWithLicense -eq 1) 'package.json licence count is wrong'
        Assert-True ($candidate.node.lockfiles[0].packageEntryCount -eq 3) 'npm lock entry count is wrong'
        Assert-True ($candidate.node.lockfiles[0].entriesWithLicenseField -eq 2) 'npm lock licence-field count is wrong'
        Assert-True ($scan.totals.npmLockPackages -eq 3) 'npm total must include entries both with and without licence fields'
        Assert-True ($scan.totals.npmLockPackagesWithoutLicenseMetadata -eq 1) 'npm unknown total must stay separate from package total'
        Assert-True ($candidate.cmake.fetchContentCount -eq 1) 'CMake FetchContent count is wrong'
        Assert-True ($candidate.cmake.findPackageRequired -eq 1) 'required find_package count is wrong'
        Assert-True ($candidate.license.rootLicenseSignatures -contains 'MPL-2.0') 'root licence signature was not detected'
        Assert-True ($candidate.spdxHeaders.filesWithSpdxHeader -eq 1) 'SPDX header count is wrong'
        Assert-True ($candidate.unknowns.cargoLockPackagesWithoutLicenseMetadata -eq 2) 'unknown cargo licences must be reported'
        Assert-True ($candidate.unknowns.npmLockPackagesWithoutLicenseMetadata -eq 1) 'unknown npm licences must be reported'
        Assert-True ($candidate.unknowns.networkUsed -eq $false) 'scan must record that no network was used'
        Assert-True ($candidate.unknowns.registryOrStoreMetadataConsulted -eq $false) 'scan must record that no registry metadata was used'
    }

    Test-Case 'dependency scan refuses a dirty checkout without writing evidence' {
        $destination = Join-Path $root 'scan-dirty'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        Set-Content -LiteralPath (Join-Path $destination 'gauntlet\README.md') -Value 'local change'
        $pinFile = Join-Path $root 'pins-ok.json'
        $outputPath = Join-Path $destination 'dependency-scan.json'

        $result = Invoke-Scan -Destination $destination -PinFile $pinFile -OutputPath $outputPath
        Assert-True ($result.ExitCode -ne 0) 'dirty checkout must be refused'
        Assert-True (-not (Test-Path -LiteralPath $outputPath)) 'refused scan must not write evidence'
    }

    Test-Case 'dependency scan refuses a wrong pin without writing evidence' {
        $destination = Join-Path $root 'scan-wrong-pin'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-wrong.json'
        $outputPath = Join-Path $destination 'dependency-scan.json'

        $result = Invoke-Scan -Destination $destination -PinFile $pinFile -OutputPath $outputPath
        Assert-True ($result.ExitCode -ne 0) 'wrong pin must be refused'
        Assert-True ($result.Output -match 'pin mismatch') 'refusal must name the pin mismatch'
        Assert-True (-not (Test-Path -LiteralPath $outputPath)) 'refused scan must not write evidence'
    }

    Test-Case 'dependency scan refuses output that would overwrite candidate source' {
        $destination = Join-Path $root 'scan-overwrite'
        $checkout = Join-Path $destination 'gauntlet'
        New-Checkout -Destination $checkout
        $sourcePath = Join-Path $checkout 'Cargo.toml'
        $before = Get-Content -LiteralPath $sourcePath -Raw
        $result = Invoke-Scan -Destination $destination -PinFile (Join-Path $root 'pins-ok.json') -OutputPath $sourcePath
        Assert-True ($result.ExitCode -ne 0) 'output inside candidate checkout must be refused'
        Assert-True ($result.Output -match 'inside candidate checkout') 'refusal must name the source boundary'
        Assert-True ((Get-Content -LiteralPath $sourcePath -Raw) -ceq $before) 'source file must remain unchanged'
    }

    Test-Case 'dependency scan refuses an output path inside this Ansa repository' {
        $destination = Join-Path $root 'scan-inside-repo'
        New-Checkout -Destination (Join-Path $destination 'gauntlet')
        $pinFile = Join-Path $root 'pins-ok.json'
        $outputPath = Join-Path $script:RepositoryRoot ('reuse-scan-' + [guid]::NewGuid().ToString('N') + '.json')

        $result = Invoke-Scan -Destination $destination -PinFile $pinFile -OutputPath $outputPath
        Assert-True ($result.ExitCode -ne 0) 'in-repository output path must be refused'
        Assert-True ($result.Output -match 'inside this Ansa repository') 'refusal must name the repository boundary'
        Assert-True (-not (Test-Path -LiteralPath $outputPath)) 'refused output must not be created'
    }
}
finally {
    if ($KeepTemp) {
        Write-Host "Temp test data kept at $root"
    }
    else {
        Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ""
Write-Host "Passed: $script:Passed; Failed: $script:Failed"
if ($script:Failed -gt 0) { exit 1 }
exit 0
