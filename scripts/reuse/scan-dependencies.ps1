# Static dependency and licence inventory for the pinned F02 candidate checkouts.
#
# Safety contract:
#  - Every selected checkout is verified against the centralized pin data (commit,
#    remote.origin.url, tracked/untracked cleanliness) BEFORE any file is read for
#    evidence. Dirty, mismatched or wrong-origin checkouts are refused.
#  - The destination and output path must resolve outside this Ansa repository,
#    including through junction/symlink ancestors.
#  - This script reads only files already present in the checkouts. It performs no
#    network request, consults no registry metadata, installs nothing and executes
#    no candidate script.
#  - It does not resolve licences. Cargo.lock, package-lock.json and pnpm-lock.yaml
#    do not carry a complete per-package licence field, so the number of packages
#    whose licence is unknown is reported explicitly rather than inferred.
#
# Run:
#   pwsh -NoProfile -File scripts/reuse/scan-dependencies.ps1 `
#     -Destination "$env:TEMP\opencode\ansa-reuse-candidates"
[CmdletBinding()]
param(
    [string]$Destination = (Join-Path $env:TEMP 'opencode\ansa-reuse-candidates'),
    [string[]]$Candidate = @('gauntlet', 'vicinae', 'pop-launcher', 'anyrun', 'cosmic-launcher', 'flare'),
    [string]$PinFile = (Join-Path $PSScriptRoot 'candidates.json'),
    [string]$GitPath = 'git',
    [string]$GitPathArgumentsJson = '[]',
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'common.ps1')

$script:ExcludedDirectoryNames = @('.git', 'node_modules', 'target', '.venv', 'venv', '__pycache__')
$script:SourceExtensions = @('.rs', '.ts', '.tsx', '.js', '.jsx', '.mjs', '.cjs', '.c', '.cc', '.cpp', '.h', '.hpp', '.qml', '.py', '.swift', '.svelte')
$script:SpdxFileLimit = 20000

function Get-RelativePath {
    param([string]$Root, [string]$Path)

    return ([System.IO.Path]::GetRelativePath($Root, $Path) -replace '\\', '/')
}

function Get-CandidateFiles {
    param(
        [string]$Root,
        [string]$Filter
    )

    return @(Get-ChildItem -LiteralPath $Root -Recurse -File -Filter $Filter -ErrorAction SilentlyContinue |
            Where-Object {
                $segments = $_.FullName.Substring($Root.Length).Split([char[]]@('\', '/'), [StringSplitOptions]::RemoveEmptyEntries)
                -not ($segments | Where-Object { $script:ExcludedDirectoryNames -contains $_ })
            })
}

function Get-LicenseSignatures {
    param([string]$Text)

    $signatures = [System.Collections.Generic.List[string]]::new()
    # Licence titles appear at the start of the file. Anchoring the copyleft
    # checks to the title area avoids misreading GPL/LGPL cross-references in the
    # body of MPL or GPL texts as the root licence.
    $head = $Text.Substring(0, [Math]::Min(800, $Text.Length))
    if ($head -match '(?i)Mozilla Public License\s+Version\s+2\.0') { $signatures.Add('MPL-2.0') }
    if ($head -match '(?i)GNU\s+LESSER\s+GENERAL\s+PUBLIC\s+LICENSE') { $signatures.Add('LGPL-3.0') }
    elseif ($head -match '(?i)GNU\s+GENERAL\s+PUBLIC\s+LICENSE' -and $head -match '(?i)Version\s+3') { $signatures.Add('GPL-3.0') }
    if ($head -match '(?i)Apache License\s+Version\s+2\.0') { $signatures.Add('Apache-2.0') }
    if ($Text -match '(?i)Permission is hereby granted, free of charge') { $signatures.Add('MIT') }
    if ($Text -match '(?i)Permission to use, copy, modify, and(/or)? distribute this software') { $signatures.Add('ISC') }
    return @($signatures | Select-Object -Unique)
}

function Get-LicenseSummary {
    param([string]$Root)

    $rootCandidates = @('LICENSE', 'LICENSE.md', 'LICENSE.txt', 'LICENCE', 'LICENCE.md', 'COPYING', 'COPYING.md')
    $rootLicense = $null
    $signatures = @()
    foreach ($name in $rootCandidates) {
        $path = Join-Path $Root $name
        if (Test-Path -LiteralPath $path) {
            $rootLicense = $name
            $text = Get-Content -LiteralPath $path -Raw -ErrorAction SilentlyContinue
            if ($null -ne $text) { $signatures = Get-LicenseSignatures -Text $text }
            break
        }
    }

    $nested = @(Get-ChildItem -LiteralPath $Root -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object {
                $_.Name -in $rootCandidates -and
                $_.DirectoryName -ne $Root -and
                $_.FullName.Substring($Root.Length).Split([char[]]@('\', '/'), [StringSplitOptions]::RemoveEmptyEntries) -notcontains '.git'
            } |
            ForEach-Object { Get-RelativePath -Root $Root -Path $_.FullName } |
            Sort-Object)

    return [ordered]@{
        rootLicenseFile       = $rootLicense
        rootLicenseSignatures = @($signatures)
        nestedLicenseFiles   = @($nested | Select-Object -First 50)
        nestedLicenseCount   = $nested.Count
        note                  = 'Signature detection is a keyword check of licence text, not legal identification or clearance.'
    }
}

function Get-CargoLockSummary {
    param(
        [string]$Root,
        [string]$Path
    )

    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    $text = Get-Content -LiteralPath $Path -Raw
    $chunks = [regex]::Split($text, '(?m)^\[\[package\]\]\s*$')
    $packages = [System.Collections.Generic.List[object]]::new()
    for ($i = 1; $i -lt $chunks.Count; $i++) {
        $chunk = $chunks[$i]
        $name = [regex]::Match($chunk, '(?m)^name\s*=\s*"([^"]*)"').Groups[1].Value
        if ([string]::IsNullOrWhiteSpace($name)) { continue }
        $source = [regex]::Match($chunk, '(?m)^source\s*=\s*"([^"]*)"').Groups[1].Value
        $packages.Add([pscustomobject]@{ Name = $name; Source = $source })
    }

    $registry = @($packages | Where-Object { $_.Source -like 'registry+*' }).Count
    $git = @($packages | Where-Object { $_.Source -like 'git+*' }).Count
    $local = @($packages | Where-Object { [string]::IsNullOrWhiteSpace($_.Source) }).Count

    return [ordered]@{
        path                       = Get-RelativePath -Root $Root -Path $Path
        packageCount               = $packages.Count
        registrySourceCount        = $registry
        gitSourceCount             = $git
        localOrWorkspaceSourceCount = $local
        otherSourceCount           = $packages.Count - $registry - $git - $local
        licenseFieldPresent        = $false
        licenseMetadataAvailable   = $false
        note                       = 'Cargo.lock records identity, source and checksum, not licence. All entries count as licence-unknown here.'
    }
}

function Get-CargoManifestSummary {
    param([string]$Root)

    $files = @(Get-CandidateFiles -Root $Root -Filter 'Cargo.toml' | Sort-Object FullName)
    $declarations = [System.Collections.Generic.List[object]]::new()
    $withLicense = 0
    $withoutLicense = 0
    $withPackageSection = 0

    foreach ($file in $files) {
        $text = Get-Content -LiteralPath $file.FullName -Raw -ErrorAction SilentlyContinue
        if ($null -eq $text) { continue }
        $packageSection = [regex]::Match($text, '(?ms)^\s*\[package\]\s*$(.*?)(?=^\s*\[[^\]]+\]\s*$|\z)')
        $workspaceSection = [regex]::Match($text, '(?ms)^\s*\[workspace\.package\]\s*$(.*?)(?=^\s*\[[^\]]+\]\s*$|\z)')
        $license = $null
        $licenseFile = $null
        $inheritsWorkspace = $false
        if ($packageSection.Success) {
            $withPackageSection++
            $licenseMatch = [regex]::Match($packageSection.Groups[1].Value, '(?m)^\s*license\s*=\s*"([^"]*)"')
            if ($licenseMatch.Success) { $license = $licenseMatch.Groups[1].Value }
            $fileMatch = [regex]::Match($packageSection.Groups[1].Value, '(?m)^\s*license-file\s*=\s*"([^"]*)"')
            if ($fileMatch.Success) { $licenseFile = $fileMatch.Groups[1].Value }
            $inheritsWorkspace = [regex]::IsMatch($packageSection.Groups[1].Value, '(?m)^\s*license\.workspace\s*=\s*true')
            if ($inheritsWorkspace -and $workspaceSection.Success) {
                $workspaceLicense = [regex]::Match($workspaceSection.Groups[1].Value, '(?m)^\s*license\s*=\s*"([^"]*)"')
                if ($workspaceLicense.Success) { $license = $workspaceLicense.Groups[1].Value }
            }
            if ([string]::IsNullOrWhiteSpace($license) -and [string]::IsNullOrWhiteSpace($licenseFile)) {
                $withoutLicense++
            }
            else {
                $withLicense++
            }
        }

        $declarations.Add([ordered]@{
                path               = Get-RelativePath -Root $Root -Path $file.FullName
                hasPackageSection  = $packageSection.Success
                license            = $license
                licenseFile        = $licenseFile
                inheritsWorkspace  = $inheritsWorkspace
            })
    }

    return [ordered]@{
        manifestCount          = $files.Count
        packageSectionCount    = $withPackageSection
        withLicenseDeclaration = $withLicense
        withoutLicenseDeclaration = $withoutLicense
        declarations           = @($declarations)
        note                   = 'Only static manifest fields are read. A declaration is not proof that every file in the crate carries that licence.'
    }
}

function Get-PnpmLockSummary {
    param(
        [string]$Root,
        [string]$Path
    )

    $lines = @(Get-Content -LiteralPath $Path -ErrorAction SilentlyContinue)
    $packagesIndex = -1
    $snapshotsIndex = -1
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($packagesIndex -lt 0 -and $lines[$i] -match '^packages:\s*$') { $packagesIndex = $i }
        if ($snapshotsIndex -lt 0 -and $lines[$i] -match '^snapshots:\s*$') { $snapshotsIndex = $i }
    }

    $packageEntryCount = 0
    if ($packagesIndex -ge 0) {
        $end = if ($snapshotsIndex -gt $packagesIndex) { $snapshotsIndex - 1 } else { $lines.Count - 1 }
        $packageEntryCount = @($lines[$packagesIndex..$end] | Where-Object { $_ -match "^  '?[^ ].*'?:\s*$" }).Count
    }
    $snapshotEntryCount = 0
    if ($snapshotsIndex -ge 0) {
        $snapshotEntryCount = @($lines[$snapshotsIndex..($lines.Count - 1)] | Where-Object { $_ -match "^  '?[^ ].*'?:\s*$" }).Count
    }

    return [ordered]@{
        path                      = Get-RelativePath -Root $Root -Path $Path
        kind                      = 'pnpm'
        lockfileVersion           = (($lines | Where-Object { $_ -match '^lockfileVersion:' } | Select-Object -First 1) -replace '^lockfileVersion:\s*', '').Trim("'", '"')
        packageEntryCount         = $packageEntryCount
        snapshotEntryCount        = $snapshotEntryCount
        parsedBy                  = 'line heuristic; no YAML parser was used'
        licenseMetadataAvailable  = $false
        note                      = 'Counts section keys, not a resolved dependency graph. pnpm lockfiles carry no per-package licence field.'
    }
}

function Get-NodeSummary {
    param([string]$Root)

    $packageJsonFiles = @(Get-CandidateFiles -Root $Root -Filter 'package.json' | Sort-Object FullName)
    $declarations = [System.Collections.Generic.List[object]]::new()
    $withLicense = 0
    foreach ($file in $packageJsonFiles) {
        $license = $null
        $private = $null
        $parseError = $null
        try {
            $json = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json -ErrorAction Stop
            if ($json.PSObject.Properties['license']) {
                if ($json.license -is [string]) { $license = $json.license }
                elseif ($json.license.PSObject.Properties['type']) { $license = [string]$json.license.type }
            }
            if ($json.PSObject.Properties['private']) { $private = [bool]$json.private }
        }
        catch { $parseError = $_.Exception.Message }
        if (-not [string]::IsNullOrWhiteSpace($license)) { $withLicense++ }
        $declarations.Add([ordered]@{
                path       = Get-RelativePath -Root $Root -Path $file.FullName
                license    = $license
                private    = $private
                parseError = $parseError
            })
    }

    $lockfiles = [System.Collections.Generic.List[object]]::new()
    foreach ($file in @(Get-CandidateFiles -Root $Root -Filter 'package-lock.json' | Sort-Object FullName)) {
        try {
            $json = Get-Content -LiteralPath $file.FullName -Raw | ConvertFrom-Json -AsHashtable -ErrorAction Stop
            $packages = $json['packages']
            $entries = if ($packages) { @($packages.GetEnumerator()) } else { @() }
            $lockEntriesWithLicense = @($entries | Where-Object {
                    $value = $_.Value
                    $value -is [System.Collections.IDictionary] -and $value.Contains('license') -and -not [string]::IsNullOrWhiteSpace([string]$value['license'])
                }).Count
            $lockfiles.Add([ordered]@{
                    path                     = Get-RelativePath -Root $Root -Path $file.FullName
                    kind                     = 'npm'
                    lockfileVersion          = $json['lockfileVersion']
                    packageEntryCount        = $entries.Count
                    entriesWithLicenseField  = $lockEntriesWithLicense
                    licenseMetadataAvailable = ($lockEntriesWithLicense -gt 0)
                })
        }
        catch {
            $lockfiles.Add([ordered]@{
                    path       = Get-RelativePath -Root $Root -Path $file.FullName
                    kind       = 'npm'
                    parseError = $_.Exception.Message
                })
        }
    }

    foreach ($file in @(Get-CandidateFiles -Root $Root -Filter 'pnpm-lock.yaml' | Sort-Object FullName)) {
        $lockfiles.Add((Get-PnpmLockSummary -Root $Root -Path $file.FullName))
    }

    foreach ($file in @(Get-CandidateFiles -Root $Root -Filter 'yarn.lock' | Sort-Object FullName)) {
        $lockfiles.Add([ordered]@{
                path                     = Get-RelativePath -Root $Root -Path $file.FullName
                kind                     = 'yarn'
                packageEntryCount        = $null
                licenseMetadataAvailable = $false
                note                     = 'Not parsed; no YAML/berry parser was used.'
            })
    }

    return [ordered]@{
        packageJsonCount          = $packageJsonFiles.Count
        packageJsonWithLicense    = $withLicense
        packageJsonWithoutLicense = $packageJsonFiles.Count - $withLicense
        declarations              = @($declarations)
        lockfiles                = @($lockfiles)
        note                     = 'Node package manifests and lockfiles are static inputs; no npm registry metadata was consulted.'
    }
}

function Get-CMakeSummary {
    param([string]$Root)

    $files = [System.Collections.Generic.List[string]]::new()
    $rootCMake = Join-Path $Root 'CMakeLists.txt'
    if (Test-Path -LiteralPath $rootCMake) { $files.Add($rootCMake) }
    $cmakeDir = Join-Path $Root 'cmake'
    if (Test-Path -LiteralPath $cmakeDir) {
        foreach ($file in @(Get-ChildItem -LiteralPath $cmakeDir -File -Filter '*.cmake' -ErrorAction SilentlyContinue)) {
            $files.Add($file.FullName)
        }
    }

    $fetchContent = [System.Collections.Generic.List[object]]::new()
    $findPackages = [System.Collections.Generic.List[object]]::new()
    foreach ($file in $files) {
        $text = Get-Content -LiteralPath $file -Raw -ErrorAction SilentlyContinue
        if ($null -eq $text) { continue }
        foreach ($match in [regex]::Matches($text, '(?s)FetchContent_Declare\(\s*(?<name>[A-Za-z0-9_.\-]+)(?<body>.*?)\)')) {
            $body = $match.Groups['body'].Value
            $fetchContent.Add([ordered]@{
                    name          = $match.Groups['name'].Value
                    source        = Get-RelativePath -Root $Root -Path $file
                    gitRepository = ([regex]::Match($body, 'GIT_REPOSITORY\s+(\S+)')).Groups[1].Value
                    gitTag        = ([regex]::Match($body, 'GIT_TAG\s+(\S+)')).Groups[1].Value
                    url           = ([regex]::Match($body, '(?m)^\s*URL\s+(\S+)')).Groups[1].Value
                    urlHash       = ([regex]::Match($body, 'URL_HASH\s+(\S+)')).Groups[1].Value
                })
        }
        foreach ($match in [regex]::Matches($text, '(?m)find_package\(\s*(?<name>[A-Za-z0-9_]+)(?<body>[^\)]*)\)')) {
            $findPackages.Add([ordered]@{
                    name     = $match.Groups['name'].Value
                    required = ($match.Groups['body'].Value -match '\bREQUIRED\b')
                    source   = Get-RelativePath -Root $Root -Path $file
                })
        }
    }

    $vendorDirs = @()
    $vendorPath = Join-Path $Root 'vendor'
    if (Test-Path -LiteralPath $vendorPath) {
        $vendorDirs = @(Get-ChildItem -LiteralPath $vendorPath -Directory -ErrorAction SilentlyContinue | ForEach-Object { $_.Name } | Sort-Object)
    }

    $nodeRuntime = [ordered]@{ version = $null; sha256EntryCount = 0 }
    if (Test-Path -LiteralPath $rootCMake) {
        $rootText = Get-Content -LiteralPath $rootCMake -Raw
        $nodeRuntime.version = ([regex]::Match($rootText, 'VICINAE_NODE_RUNTIME_VERSION\s+"([^"]+)"')).Groups[1].Value
        $nodeRuntime.sha256EntryCount = ([regex]::Matches($rootText, '_node_runtime_sha256\s+"[0-9a-fA-F]{64}"')).Count
    }

    return [ordered]@{
        filesInspected          = @($files | ForEach-Object { Get-RelativePath -Root $Root -Path $_ })
        fetchContent            = @($fetchContent)
        fetchContentCount       = $fetchContent.Count
        findPackages            = @($findPackages)
        findPackageRequired     = @($findPackages | Where-Object { $_.required }).Count
        vendorDirectories       = @($vendorDirs)
        pinnedNodeRuntime       = $nodeRuntime
        transitiveLicensesResolved = $false
        note                    = 'Only the root CMakeLists.txt and cmake/*.cmake are read. FetchContent targets, vendor directories and system find_package targets were not licence-resolved.'
    }
}

function Get-SpdxHeaderSummary {
    param([string]$Root)

    $scanned = 0
    $withHeader = 0
    $expressions = [System.Collections.Generic.List[string]]::new()
    $capped = $false
    foreach ($file in @(Get-ChildItem -LiteralPath $Root -Recurse -File -ErrorAction SilentlyContinue |
                Where-Object {
                    $script:SourceExtensions -contains $_.Extension.ToLowerInvariant() -and
                    -not ($_.FullName.Substring($Root.Length).Split([char[]]@('\', '/'), [StringSplitOptions]::RemoveEmptyEntries) | Where-Object { $script:ExcludedDirectoryNames -contains $_ })
                })) {
        if ($scanned -ge $script:SpdxFileLimit) { $capped = $true; break }
        $scanned++
        $head = @(Get-Content -LiteralPath $file.FullName -TotalCount 25 -ErrorAction SilentlyContinue)
        $match = $head | Where-Object { $_ -match 'SPDX-License-Identifier:\s*(.+)' } | Select-Object -First 1
        if ($match) {
            $withHeader++
            $expression = ([regex]::Match($match, 'SPDX-License-Identifier:\s*(.+)')).Groups[1].Value.Trim()
            if ($expression) { $expressions.Add($expression) }
        }
    }

    return [ordered]@{
        filesScanned             = $scanned
        filesWithSpdxHeader      = $withHeader
        uniqueSpdxExpressions    = @($expressions | Select-Object -Unique | Select-Object -First 20)
        scanCappedAtFileCount    = $capped
        note                     = 'Header presence only; absence of a header is not evidence that a file is unlicensed, and header counts are not a per-file audit.'
    }
}

function Get-CandidateScan {
    param(
        [object]$Pin,
        [string]$Path
    )

    $cargoLocks = [System.Collections.Generic.List[object]]::new()
    foreach ($file in @(Get-CandidateFiles -Root $Path -Filter 'Cargo.lock' | Sort-Object FullName)) {
        $summary = Get-CargoLockSummary -Root $Path -Path $file.FullName
        if ($summary) { $cargoLocks.Add($summary) }
    }
    $cargoManifests = Get-CargoManifestSummary -Root $Path
    $node = Get-NodeSummary -Root $Path
    $cmake = $null
    if (Test-Path -LiteralPath (Join-Path $Path 'CMakeLists.txt')) {
        $cmake = Get-CMakeSummary -Root $Path
    }
    $license = Get-LicenseSummary -Root $Path
    $spdx = Get-SpdxHeaderSummary -Root $Path

    $cargoPackageTotal = (@($cargoLocks | ForEach-Object { $_.packageCount }) | Measure-Object -Sum).Sum
    if ($null -eq $cargoPackageTotal) { $cargoPackageTotal = 0 }
    $npmPackageTotal = 0
    $npmWithoutLicense = 0
    $pnpmPackageTotal = 0
    foreach ($lock in @($node.lockfiles)) {
        if ($lock.kind -eq 'npm') {
            $count = [int]$lock.packageEntryCount
            $npmPackageTotal += $count
            $npmWithoutLicense += ($count - [int]$lock.entriesWithLicenseField)
        }
        elseif ($lock.kind -eq 'pnpm') {
            $pnpmPackageTotal += [int]$lock.packageEntryCount
        }
    }

    $unknowns = [ordered]@{
        cargoLockPackagesWithoutLicenseMetadata = [int]$cargoPackageTotal
        cargoManifestsWithoutLicenseDeclaration  = [int]$cargoManifests.withoutLicenseDeclaration
        npmLockPackagesWithoutLicenseMetadata    = [int]$npmWithoutLicense
        pnpmLockPackageEntriesWithoutLicenseMetadata = [int]$pnpmPackageTotal
        cmakeFetchContentWithoutLicenseResolution = if ($cmake) { [int]$cmake.fetchContentCount } else { 0 }
        registryOrStoreMetadataConsulted         = $false
        networkUsed                              = $false
    }

    return [ordered]@{
        name             = $Pin.name
        repository       = $Pin.repository
        commit           = $Pin.commit
        license          = $license
        cargo            = [ordered]@{
            lockfiles = @($cargoLocks)
            manifests = $cargoManifests
        }
        node             = $node
        cmake            = $cmake
        spdxHeaders      = $spdx
        unknowns         = $unknowns
    }
}

$gitPrefix = @(ConvertFrom-JsonStringArray -Json $GitPathArgumentsJson -ParameterName 'GitPathArgumentsJson')
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\..'))
Assert-PathOutsideRepository -Path $Destination -RepositoryRoot $repositoryRoot -Purpose 'destination'
if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    $OutputPath = Join-Path $Destination 'dependency-scan.json'
}
Assert-PathOutsideRepository -Path $OutputPath -RepositoryRoot $repositoryRoot -Purpose 'output path'

$pins = @(Get-ReuseCandidatePins -Path $PinFile)
foreach ($pin in $pins) {
    $checkoutPath = Join-Path $Destination $pin.name
    if (Test-PathWithin -Path $OutputPath -Parent $checkoutPath) {
        throw "Refusing output path inside candidate checkout '$($pin.name)'; scanning must not modify source files."
    }
}
$selected = @($Candidate | ForEach-Object { $_ -split ',' } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | Select-Object -Unique)
if ($selected.Count -eq 0) { throw 'At least one candidate must be selected.' }
foreach ($name in $selected) { [void](Get-ReuseCandidatePin -Pins $pins -Name $name) }

$state = @{}
$refusals = @()
foreach ($name in $selected) {
    $pin = Get-ReuseCandidatePin -Pins $pins -Name $name
    $path = Join-Path $Destination $name
    Assert-PathOutsideRepository -Path $path -RepositoryRoot $repositoryRoot -Purpose "candidate '$name' checkout"
    $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $gitPrefix -Path $path -ExpectedCommit $pin.commit -ExpectedRepository $pin.repository
    $state[$name] = [pscustomobject]@{ Pin = $pin; Path = $path; Verification = $verification }
    if ($verification.Issues.Count -gt 0) {
        $refusals += "$name`: $($verification.Issues -join '; ')"
    }
}
if ($refusals.Count -gt 0) {
    throw "Refusing dependency scan: $($refusals -join ' | ')"
}

$candidates = [System.Collections.Generic.List[object]]::new()
foreach ($name in $selected) {
    $entry = $state[$name]
    Write-Host "Scanning static dependency files for $name at $($entry.Verification.Head)"
    $candidates.Add((Get-CandidateScan -Pin $entry.Pin -Path $entry.Path))
}

$document = [ordered]@{
    schema          = 'ansa.reuse.dependency-scan/v1'
    kind            = 'static-dependency-inventory'
    timestampUtc    = [DateTime]::UtcNow.ToString('o')
    host            = [ordered]@{
        os               = [System.Runtime.InteropServices.RuntimeInformation]::OSDescription
        powershell       = $PSVersionTable.PSVersion.ToString()
        git              = ((Invoke-NativeCapture -FilePath $GitPath -Arguments (@($gitPrefix) + @('--version'))).Lines -join ' ')
    }
    scope           = 'Static files at the verified pinned commits. No network, no registry metadata, no package installation, no candidate script execution, no licence resolution or legal review.'
    selectedCandidates = @($selected)
    totals          = [ordered]@{
        candidates                              = $candidates.Count
        cargoLockPackages                       = [int]((@($candidates | ForEach-Object { $_.unknowns.cargoLockPackagesWithoutLicenseMetadata }) | Measure-Object -Sum).Sum)
        cargoManifests                          = [int]((@($candidates | ForEach-Object { $_.cargo.manifests.manifestCount }) | Measure-Object -Sum).Sum)
        cargoManifestsWithoutLicenseDeclaration = [int]((@($candidates | ForEach-Object { $_.unknowns.cargoManifestsWithoutLicenseDeclaration }) | Measure-Object -Sum).Sum)
        npmLockPackages                         = [int]((@($candidates | ForEach-Object { $_.node.lockfiles | Where-Object { $_.kind -eq 'npm' } | ForEach-Object { $_.packageEntryCount } }) | Measure-Object -Sum).Sum)
        npmLockPackagesWithoutLicenseMetadata   = [int]((@($candidates | ForEach-Object { $_.unknowns.npmLockPackagesWithoutLicenseMetadata }) | Measure-Object -Sum).Sum)
        pnpmLockPackageEntries                  = [int]((@($candidates | ForEach-Object { $_.unknowns.pnpmLockPackageEntriesWithoutLicenseMetadata }) | Measure-Object -Sum).Sum)
        cmakeFetchContentDeclarations           = [int]((@($candidates | ForEach-Object { $_.unknowns.cmakeFetchContentWithoutLicenseResolution }) | Measure-Object -Sum).Sum)
        lockPackagesWithoutResolvedLicense      = [int]((@($candidates | ForEach-Object { $_.unknowns.cargoLockPackagesWithoutLicenseMetadata + $_.unknowns.npmLockPackagesWithoutLicenseMetadata + $_.unknowns.pnpmLockPackageEntriesWithoutLicenseMetadata }) | Measure-Object -Sum).Sum)
    }
    candidates      = @($candidates)
    limitations     = @(
        'A lockfile entry count is not a licence inventory. Cargo.lock and npm/pnpm lockfiles do not carry a complete per-package licence field.',
        'No registry, store, vendor portal or GitHub metadata was queried for package licences.',
        'CMake FetchContent targets and vendor directories were named but their transitive licences were not resolved.',
        'SPDX header counts are keyword presence checks, not a per-file licence audit or coverage proof.',
        'Ansa copies no candidate source. This scan does not authorise reuse and is not legal advice.'
    )
}

$outputDirectory = Split-Path $OutputPath -Parent
if ($outputDirectory) { New-Item -ItemType Directory -Force -Path $outputDirectory | Out-Null }
$json = $document | ConvertTo-Json -Depth 12
Set-Content -LiteralPath $OutputPath -Value $json -Encoding utf8

Write-Host ''
Write-Host ('{0,-18} {1,10} {2,10} {3,10} {4,10}' -f 'candidate', 'cargo', 'npm', 'pnpm', 'cmake')
for ($scanIndex = 0; $scanIndex -lt $candidates.Count; $scanIndex++) {
    $scanCandidate = $candidates[$scanIndex]
    $cargo = $scanCandidate.unknowns.cargoLockPackagesWithoutLicenseMetadata
    $npm = [int]((@($scanCandidate.node.lockfiles | Where-Object { $_.kind -eq 'npm' } | ForEach-Object { $_.packageEntryCount }) | Measure-Object -Sum).Sum)
    $pnpm = $scanCandidate.unknowns.pnpmLockPackageEntriesWithoutLicenseMetadata
    $cmakeCount = $scanCandidate.unknowns.cmakeFetchContentWithoutLicenseResolution
    Write-Host ('{0,-18} {1,10} {2,10} {3,10} {4,10}' -f $scanCandidate.name, $cargo, $npm, $pnpm, $cmakeCount)
}
Write-Host ''
Write-Host "Static scan record: $OutputPath"
Write-Host 'No network request was made, no registry metadata was consulted and no candidate build ran.'
