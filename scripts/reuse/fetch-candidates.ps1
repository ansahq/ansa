# Fetch the pinned F02 candidate commits into an external scratch directory.
#
# Safety contract:
#  - Existing checkouts are verified against the centralized pin data: commit,
#    remote.origin.url and clean tracked/untracked state. Dirty, mismatched or
#    wrong-origin checkouts are refused. Nothing is reset, cleaned, stashed or
#    discarded.
#  - A destination that resolves inside this Ansa repository (including through
#    junction/symlink ancestors) is refused before anything is created.
#  - Every native git call is checked for its exit code; a failure aborts the run.
#  - No candidate source is copied into this repository, and no candidate is built.
[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:TEMP 'opencode\ansa-reuse-candidates'),
    [string]$PinFile = (Join-Path $PSScriptRoot 'candidates.json'),
    [string]$GitPath = 'git',
    [string]$GitPathArgumentsJson = '[]'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$gitPrefix = @(ConvertFrom-JsonStringArray -Json $GitPathArgumentsJson -ParameterName 'GitPathArgumentsJson')
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
Assert-PathOutsideRepository -Path $Destination -RepositoryRoot $repositoryRoot -Purpose 'destination'
$pins = @(Get-ReuseCandidatePins -Path $PinFile)

New-Item -ItemType Directory -Force -Path $Destination | Out-Null

foreach ($pin in $pins) {
    $path = Join-Path $Destination $pin.name
    Assert-PathOutsideRepository -Path $path -RepositoryRoot $repositoryRoot -Purpose "candidate '$($pin.name)' checkout"
    $hasGitDirectory = Test-Path -LiteralPath (Join-Path $path '.git')

    if ($hasGitDirectory) {
        $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -ExpectedCommit $pin.commit -ExpectedRepository $pin.repository
        if ($verification.Issues.Count -gt 0) {
            throw "Refusing existing checkout for '$($pin.name)' at '$path':$([Environment]::NewLine) - $($verification.Issues -join ([Environment]::NewLine + ' - '))"
        }
        Write-Host "Verified existing checkout: $($pin.name) @ $($verification.Head)"
        continue
    }

    if ((Test-Path -LiteralPath $path) -and @(Get-ChildItem -LiteralPath $path -Force).Count -gt 0) {
        throw "Destination '$path' exists but is not a git checkout. Refusing to modify it."
    }

    Write-Host "Fetching $($pin.name) @ $($pin.commit)"
    New-Item -ItemType Directory -Force -Path $path | Out-Null

    [void](Invoke-GitChecked -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -Arguments @('init', '--quiet'))
    [void](Invoke-GitChecked -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -Arguments @('remote', 'add', 'origin', $pin.repository))
    [void](Invoke-GitChecked -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -Arguments @('fetch', '--depth', '1', '--no-tags', 'origin', $pin.commit))
    [void](Invoke-GitChecked -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -Arguments @('checkout', '--quiet', '--detach', 'FETCH_HEAD'))

    $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -ExpectedCommit $pin.commit -ExpectedRepository $pin.repository
    if ($verification.Issues.Count -gt 0) {
        throw "Fetched checkout for '$($pin.name)' failed verification:$([Environment]::NewLine) - $($verification.Issues -join ([Environment]::NewLine + ' - '))"
    }
    Write-Host "Verified new checkout: $($pin.name) @ $($verification.Head)"
}

Write-Host "Pinned checkouts are in $Destination. Nothing was copied into the Ansa repository and no build was run."
