# Run bounded clean checks against pinned candidate checkouts.
#
# Safety contract:
#  - Every selected checkout is verified against the centralized pin data: commit,
#    remote.origin.url and tracked/untracked cleanliness. All checks run BEFORE any
#    candidate command starts.
#  - A mismatch, wrong origin, dirty checkout, missing pin or git failure refuses the
#    whole run. Nothing is reset, cleaned or discarded.
#  - A destination that resolves inside this Ansa repository (including through
#    junction/symlink ancestors) is refused before directories or records are created.
#  - CARGO_TARGET_DIR is set for the probes and restored to the caller's value
#    (including "unset") in a finally block.
#  - Confirmed probe failures and timeouts are evidence, not script failures: this
#    collector exits 0 after running probes. Refusals, invalid arguments and a process
#    that cannot be confirmed stopped exit non-zero.
#  - Evidence is append-only: records go to <Destination>/results.jsonl without
#    truncation, and each run's logs go to <Destination>/logs/<runId>/name.out/.err.
#
# The script runs no installer, installs no system library, makes no network call
# itself, and copies no candidate source into the repository.
[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:TEMP 'opencode\ansa-reuse-candidates'),
    [ValidateRange(1, 86400)]
    [int]$TimeoutSeconds = 600,
    [string[]]$Candidate = @('gauntlet', 'pop-launcher', 'anyrun', 'cosmic-launcher'),
    [string]$PinFile = (Join-Path $PSScriptRoot 'candidates.json'),
    [string]$GitPath = 'git',
    [string]$GitPathArgumentsJson = '[]',
    [string]$CargoPath = 'cargo',
    [string]$CargoPathArgumentsJson = '[]'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$gitPrefix = @(ConvertFrom-JsonStringArray -Json $GitPathArgumentsJson -ParameterName 'GitPathArgumentsJson')
$cargoPrefix = @(ConvertFrom-JsonStringArray -Json $CargoPathArgumentsJson -ParameterName 'CargoPathArgumentsJson')
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
Assert-PathOutsideRepository -Path $Destination -RepositoryRoot $repositoryRoot -Purpose 'destination'

# Commands reflect each project's documented build entry point, reduced to the
# smallest target that still resolves the workspace. Add a candidate here before
# selecting it with -Candidate.
$commands = @{
    'gauntlet'        = @('check', '-p', 'gauntlet', '--locked')
    'pop-launcher'    = @('test', '--workspace', '--locked')
    'anyrun'          = @('check', '--workspace', '--locked')
    'cosmic-launcher' = @('check', '--locked')
}

$pins = @(Get-ReuseCandidatePins -Path $PinFile)
$selected = @($Candidate | ForEach-Object { $_ -split ',' } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
if ($selected.Count -eq 0) { throw 'At least one candidate must be selected.' }
foreach ($name in $selected) {
    if (-not $commands.ContainsKey($name)) {
        throw "No bounded command is defined for candidate '$name'. Known candidates: $($commands.Keys -join ', ')."
    }
    [void](Get-ReuseCandidatePin -Pins $pins -Name $name)
}

New-Item -ItemType Directory -Force -Path $Destination | Out-Null
$runId = [guid]::NewGuid().ToString('N')
$logRoot = Join-Path $Destination 'logs'
$runLogRoot = Join-Path $logRoot $runId
New-Item -ItemType Directory -Force -Path $logRoot | Out-Null
New-Item -ItemType Directory -Force -Path $runLogRoot | Out-Null
$resultsPath = Join-Path $Destination 'results.jsonl'
if (-not (Test-Path -LiteralPath $resultsPath)) {
    New-Item -ItemType File -Path $resultsPath | Out-Null
}

# Verify all selected checkouts before any command starts. This is deliberately a
# separate pass: one bad checkout must not leave earlier candidates half-run.
$state = @{}
$refusals = @()
foreach ($name in $selected) {
    $pin = Get-ReuseCandidatePin -Pins $pins -Name $name
    $path = Join-Path $Destination $name
    Assert-PathOutsideRepository -Path $path -RepositoryRoot $repositoryRoot -Purpose "candidate '$name' checkout"
    $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -ExpectedCommit $pin.commit -ExpectedRepository $pin.repository
    $state[$name] = [pscustomobject]@{ Pin = $pin; Path = $path; Verification = $verification }
    if ($verification.Issues.Count -gt 0) {
        $refusals += [ordered]@{
            candidate      = $name
            expectedCommit = $pin.commit
            actualCommit   = $verification.Head
            problems       = @($verification.Issues)
        }
    }
}

if ($refusals.Count -gt 0) {
    $message = 'Refusing to run builds: ' + (($refusals | ForEach-Object { "$($_.candidate): $($_.problems -join '; ')" }) -join ' | ')
    Write-ReuseRecord -Path $resultsPath -Record ([ordered]@{
            schema         = 'ansa.reuse.probe/v1'
            kind           = 'refusal'
            runId          = $runId
            timestampUtc   = [DateTime]::UtcNow.ToString('o')
            timeoutSeconds = $TimeoutSeconds
            issues         = @($refusals)
            message        = $message
        })
    throw $message
}

$previousTargetDir = $env:CARGO_TARGET_DIR
$hadPreviousTargetDir = Test-Path Env:\CARGO_TARGET_DIR

try {
    $env:CARGO_TARGET_DIR = Join-Path $Destination 'target'

    foreach ($name in $selected) {
        $entry = $state[$name]
        $pin = $entry.Pin
        $path = $entry.Path

        # Re-check immediately before spawning: a build, another process or a test
        # could have changed the checkout after the upfront pass.
        $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -ExpectedCommit $pin.commit -ExpectedRepository $pin.repository
        if ($verification.Issues.Count -gt 0) {
            $message = "Refusing candidate '$name' before execution: $($verification.Issues -join '; ')"
            Write-ReuseRecord -Path $resultsPath -Record ([ordered]@{
                    schema         = 'ansa.reuse.probe/v1'
                    kind           = 'refusal'
                    runId          = $runId
                    timestampUtc   = [DateTime]::UtcNow.ToString('o')
                    candidate      = $name
                    expectedCommit = $pin.commit
                    actualCommit   = $verification.Head
                    timeoutSeconds = $TimeoutSeconds
                    issues         = @($verification.Issues)
                    message        = $message
                })
            throw $message
        }

        $cargoArguments = @($cargoPrefix) + @($commands[$name])
        $stdoutPath = Join-Path $runLogRoot "$name.out"
        $stderrPath = Join-Path $runLogRoot "$name.err"
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        Write-Host "Running $CargoPath $($commands[$name] -join ' ') in $path (limit ${TimeoutSeconds}s)"

        $process = Start-Process -FilePath $CargoPath -ArgumentList $cargoArguments `
            -WorkingDirectory $path -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath `
            -PassThru -NoNewWindow
        $finished = $process.WaitForExit([int]($TimeoutSeconds * 1000))
        $watch.Stop()

        if (-not $finished) {
            $exitCode = $null
            $timedOut = $true
            $status = 'TIMEOUT'
            $stopConfirmed = $false
            $cleanupError = $null
            try { $process.Kill($true) } catch { $cleanupError = $_.Exception.Message }
            try {
                if ($process.WaitForExit(5000)) { $stopConfirmed = $true }
            }
            catch {
                if (-not $cleanupError) { $cleanupError = $_.Exception.Message }
            }
            if (-not $stopConfirmed) {
                try {
                    $process.Refresh()
                    $stopConfirmed = $process.HasExited
                }
                catch {
                    if (-not $cleanupError) { $cleanupError = $_.Exception.Message }
                }
            }
        }
        else {
            $exitCode = $process.ExitCode
            $timedOut = $false
            $stopConfirmed = $true
            $cleanupError = $null
            $status = "exit $exitCode"
        }

        $firstErrorLine = $null
        if (Test-Path -LiteralPath $stderrPath) {
            $firstErrorLine = Get-Content -LiteralPath $stderrPath -Tail 60 -ErrorAction SilentlyContinue |
                Where-Object { $_ -match '(?i)error' } |
                Select-Object -First 1
        }

        $record = [ordered]@{
            schema          = 'ansa.reuse.probe/v1'
            kind            = 'probe'
            runId           = $runId
            timestampUtc    = [DateTime]::UtcNow.ToString('o')
            candidate       = $name
            repository      = $pin.repository
            expectedCommit  = $pin.commit
            actualCommit    = $verification.Head
            executable      = $CargoPath
            command         = @($commands[$name])
            arguments       = @($cargoArguments)
            exitCode        = $exitCode
            timedOut        = $timedOut
            processStopConfirmed = $stopConfirmed
            cleanupError    = $cleanupError
            timeoutSeconds  = $TimeoutSeconds
            durationSeconds = [math]::Round($watch.Elapsed.TotalSeconds, 1)
            stdoutPath      = $stdoutPath
            stderrPath      = $stderrPath
            firstErrorLine  = $firstErrorLine
        }
        Write-ReuseRecord -Path $resultsPath -Record $record
        Write-Host ("{0}: {1} in {2:N1}s (records: {3})" -f $name, $status, $watch.Elapsed.TotalSeconds, $resultsPath)

        if (-not $stopConfirmed) {
            throw "Timed-out process for '$name' could not be confirmed stopped (cleanup error: $cleanupError). Refusing to continue; the probe record is retained in '$resultsPath'."
        }
    }
}
finally {
    if ($hadPreviousTargetDir) {
        $env:CARGO_TARGET_DIR = $previousTargetDir
    }
    else {
        Remove-Item Env:\CARGO_TARGET_DIR -ErrorAction SilentlyContinue
    }
}
