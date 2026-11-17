# Shared helpers for the F02 reuse scripts.
#
# This file is dot-sourced by fetch-candidates.ps1 and bounded-build.ps1, and by
# tests/run-tests.ps1. It contains no candidate source and makes no network calls.

function Get-ReuseCandidatePins {
    [CmdletBinding()]
    param(
        [string]$Path = (Join-Path $PSScriptRoot 'candidates.json')
    )

    if (-not (Test-Path -LiteralPath $Path)) {
        throw "Pin file not found: $Path"
    }

    try {
        $data = Get-Content -LiteralPath $Path -Raw -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        throw "Pin file '$Path' is not valid JSON: $($_.Exception.Message)"
    }

    $pins = @($data)
    if ($pins.Count -eq 0) {
        throw "Pin file '$Path' contains no candidates."
    }

    $seen = @{}
    foreach ($pin in $pins) {
        foreach ($field in @('name', 'repository', 'commit')) {
            if (-not $pin.PSObject.Properties[$field] -or [string]::IsNullOrWhiteSpace([string]$pin.$field)) {
                throw "Pin file '$Path' has an entry without '$field'."
            }
        }
        if ($pin.commit -notmatch '^[0-9a-fA-F]{40}$') {
            throw "Pin for '$($pin.name)' is not a 40-character hexadecimal commit: '$($pin.commit)'"
        }
        if ($seen.ContainsKey([string]$pin.name)) {
            throw "Pin file '$Path' contains duplicate candidate '$($pin.name)'."
        }
        $seen[[string]$pin.name] = $true
        $pin.commit = $pin.commit.ToLowerInvariant()
    }

    return $pins
}

function Get-ReuseCandidatePin {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [object[]]$Pins,
        [Parameter(Mandatory)]
        [string]$Name
    )

    $matches = @($Pins | Where-Object { $_.name -eq $Name })
    if ($matches.Count -ne 1) {
        throw "No pin entry for candidate '$Name'. Expected exactly one."
    }
    return $matches[0]
}

function ConvertFrom-JsonStringArray {
    [CmdletBinding()]
    param(
        [AllowEmptyString()]
        [string]$Json = '[]',
        [string]$ParameterName = 'argument'
    )

    try {
        $value = ConvertFrom-Json -InputObject $Json -ErrorAction Stop
    }
    catch {
        throw "$ParameterName must be a JSON array of strings. Received: $Json"
    }

    if ($null -eq $value) { return @() }
    $items = @($value)
    foreach ($item in $items) {
        if ($item -isnot [string]) {
            throw "$ParameterName must contain only strings. Received: $Json"
        }
    }
    return $items
}

function Invoke-NativeCapture {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$FilePath,
        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    $raw = @()
    $exitCode = $null
    $errorMessage = $null
    try {
        $raw = @(& $FilePath @Arguments 2>&1)
        $exitCode = $LASTEXITCODE
    }
    catch {
        $errorMessage = $_.Exception.Message
        $exitCode = -1
    }

    [pscustomobject]@{
        ExitCode = $exitCode
        Lines    = @($raw | ForEach-Object { ([string]$_).TrimEnd("`r", "`n") })
        Error    = $errorMessage
    }
}

function Invoke-GitCapture {
    [CmdletBinding()]
    param(
        [string]$GitPath = 'git',
        [string[]]$GitPathArguments = @(),
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    $allArguments = @($GitPathArguments) + @('-C', $Path) + @($Arguments)
    Invoke-NativeCapture -FilePath $GitPath -Arguments $allArguments
}

function Invoke-GitChecked {
    [CmdletBinding()]
    param(
        [string]$GitPath = 'git',
        [string[]]$GitPathArguments = @(),
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string[]]$Arguments
    )

    $result = Invoke-GitCapture -GitPath $GitPath -GitPathArguments $GitPathArguments -Path $Path -Arguments $Arguments
    if ($result.ExitCode -ne 0) {
        $detail = if ($result.Error) { $result.Error } else { ($result.Lines -join [Environment]::NewLine) }
        throw "git $($Arguments -join ' ') failed in '$Path' with exit $($result.ExitCode).$([Environment]::NewLine)$detail"
    }
    return $result.Lines
}

function Get-RepositoryIdentity {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [AllowEmptyString()]
        [string]$Value
    )

    $result = [ordered]@{ Kind = $null; Value = $null; Error = $null }
    $text = $Value.Trim()
    if ([string]::IsNullOrWhiteSpace($text)) {
        $result.Error = 'empty repository value'
        return [pscustomobject]$result
    }

    if ($text -match '(?i)^https://') {
        try {
            $uri = [uri]$text
        }
        catch {
            $result.Error = 'not a valid HTTPS URL'
            return [pscustomobject]$result
        }
        if ($uri.UserInfo) {
            $result.Error = 'credentialed URLs are not accepted'
            return [pscustomobject]$result
        }
        if ($uri.Query -or $uri.Fragment) {
            $result.Error = 'query strings and fragments are not accepted'
            return [pscustomobject]$result
        }
        if (-not $uri.IsDefaultPort) {
            $result.Error = 'non-default ports are not accepted'
            return [pscustomobject]$result
        }
        $hostName = $uri.Host.ToLowerInvariant()
        $path = $uri.AbsolutePath.TrimEnd('/')
        if ($path.EndsWith('.git', [StringComparison]::OrdinalIgnoreCase)) {
            $path = $path.Substring(0, $path.Length - 4)
        }
        $path = $path.TrimEnd('/')
        if ($path -notmatch '^/[^/]+/[^/]+$') {
            $result.Error = 'expected exactly an owner/repository path'
            return [pscustomobject]$result
        }
        $result.Kind = 'https'
        $result.Value = ($hostName + $path).ToLowerInvariant()
        return [pscustomobject]$result
    }

    if ($text -match '(?i)^file://') {
        try {
            $uri = [uri]$text
        }
        catch {
            $result.Error = 'not a valid file URL'
            return [pscustomobject]$result
        }
        if ($uri.Query -or $uri.Fragment) {
            $result.Error = 'query strings and fragments are not accepted'
            return [pscustomobject]$result
        }
        $localPath = $uri.LocalPath
    }
    elseif ($text -match '^[a-zA-Z][a-zA-Z0-9+.-]*://') {
        $result.Error = 'only HTTPS and local file repositories are accepted'
        return [pscustomobject]$result
    }
    elseif ($text -match '^[^/\\]+@[^:]+:') {
        $result.Error = 'SCP-style git remotes are not accepted'
        return [pscustomobject]$result
    }
    else {
        $localPath = $text
    }

    try {
        $fullPath = [System.IO.Path]::GetFullPath($localPath)
    }
    catch {
        $result.Error = 'not a valid local repository path'
        return [pscustomobject]$result
    }
    $result.Kind = 'file'
    $result.Value = $fullPath.TrimEnd([System.IO.Path]::DirectorySeparatorChar, [System.IO.Path]::AltDirectorySeparatorChar).ToLowerInvariant()
    return [pscustomobject]$result
}

function Resolve-RealPath {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    $fullPath = [System.IO.Path]::GetFullPath($Path)
    $root = [System.IO.Path]::GetPathRoot($fullPath)
    if ([string]::IsNullOrEmpty($root)) { return $fullPath }

    $current = $root
    $relative = $fullPath.Substring($root.Length)
    foreach ($part in $relative.Split([char[]]@('\', '/'), [StringSplitOptions]::RemoveEmptyEntries)) {
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
            if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
                $target = $item.ResolveLinkTarget($true)
                if ($null -eq $target) {
                    throw "Unable to resolve reparse point '$current'."
                }
                $current = $target.FullName
            }
        }
    }
    return [System.IO.Path]::GetFullPath($current)
}

function Test-PathWithin {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$Parent
    )

    $resolvedParent = (Resolve-RealPath -Path $Parent).TrimEnd('\', '/')
    $resolvedPath = (Resolve-RealPath -Path $Path).TrimEnd('\', '/')
    if ([string]::Equals($resolvedPath, $resolvedParent, [StringComparison]::OrdinalIgnoreCase)) {
        return $true
    }
    $prefix = $resolvedParent + [System.IO.Path]::DirectorySeparatorChar
    return $resolvedPath.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)
}

function Assert-PathOutsideRepository {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$RepositoryRoot,
        [string]$Purpose = 'destination'
    )

    $resolved = Resolve-RealPath -Path $Path
    if (Test-PathWithin -Path $Path -Parent $RepositoryRoot) {
        throw "Refusing $Purpose '$Path' (resolves to '$resolved'): it is inside this Ansa repository '$RepositoryRoot'. No files were created."
    }
}

function Get-CheckoutVerification {
    [CmdletBinding()]
    param(
        [string]$GitPath = 'git',
        [string[]]$GitPathArguments = @(),
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$ExpectedCommit,
        [Parameter(Mandatory)]
        [string]$ExpectedRepository
    )

    $issues = [System.Collections.Generic.List[string]]::new()
    $head = $null
    $dirtyEntries = @()

    if (-not (Test-Path -LiteralPath $Path)) {
        $issues.Add('path does not exist')
    }
    elseif (-not (Test-Path -LiteralPath (Join-Path $Path '.git'))) {
        $issues.Add('not a git checkout (missing .git)')
    }
    else {
        $headResult = Invoke-GitCapture -GitPath $GitPath -GitPathArguments $GitPathArguments -Path $Path -Arguments @('rev-parse', 'HEAD')
        if ($headResult.ExitCode -ne 0) {
            $detail = if ($headResult.Error) { $headResult.Error } else { ($headResult.Lines -join ' ') }
            $issues.Add("git rev-parse HEAD failed with exit $($headResult.ExitCode): $detail")
        }
        else {
            $head = (($headResult.Lines | Where-Object { $_ -ne '' }) -join "`n").Trim()
            if ([string]::IsNullOrWhiteSpace($head)) {
                $issues.Add('git rev-parse HEAD returned an empty commit')
            }
        }

        $statusResult = Invoke-GitCapture -GitPath $GitPath -GitPathArguments $GitPathArguments -Path $Path -Arguments @('status', '--porcelain=v1', '--untracked-files=all')
        if ($statusResult.ExitCode -ne 0) {
            $detail = if ($statusResult.Error) { $statusResult.Error } else { ($statusResult.Lines -join ' ') }
            $issues.Add("git status failed with exit $($statusResult.ExitCode): $detail")
        }
        else {
            $dirtyEntries = @($statusResult.Lines | Where-Object { $_ -ne '' })
        }

        $originResult = Invoke-GitCapture -GitPath $GitPath -GitPathArguments $GitPathArguments -Path $Path -Arguments @('config', '--get', 'remote.origin.url')
        if ($originResult.ExitCode -ne 0) {
            $detail = if ($originResult.Error) { $originResult.Error } else { ($originResult.Lines -join ' ') }
            $issues.Add("missing remote.origin.url (git config exit $($originResult.ExitCode)): $detail")
        }
        else {
            $origin = (($originResult.Lines | Where-Object { $_ -ne '' }) -join "`n").Trim()
            if ([string]::IsNullOrWhiteSpace($origin)) {
                $issues.Add('missing remote.origin.url value')
            }
            else {
                $expectedIdentity = Get-RepositoryIdentity -Value $ExpectedRepository
                $actualIdentity = Get-RepositoryIdentity -Value $origin
                if ($expectedIdentity.Error) {
                    $issues.Add("expected repository is not a valid identity: $($expectedIdentity.Error)")
                }
                elseif ($actualIdentity.Error) {
                    $issues.Add("remote.origin.url is not an accepted repository identity: $($actualIdentity.Error)")
                }
                elseif ($expectedIdentity.Kind -ne $actualIdentity.Kind -or $expectedIdentity.Value -ne $actualIdentity.Value) {
                    $issues.Add("remote origin mismatch: expected $($expectedIdentity.Kind):$($expectedIdentity.Value), found $($actualIdentity.Kind):$($actualIdentity.Value)")
                }
            }
        }

        if ($head -and $head -ne $ExpectedCommit) {
            $issues.Add("pin mismatch: expected $ExpectedCommit, found $head")
        }
        if ($dirtyEntries.Count -gt 0) {
            $preview = ($dirtyEntries | Select-Object -First 10) -join ' | '
            $issues.Add("checkout is dirty: $($dirtyEntries.Count) tracked/untracked change(s); refusing to build. $preview")
        }
    }

    [pscustomobject]@{
        Path         = $Path
        Head         = $head
        Dirty        = ($dirtyEntries.Count -gt 0)
        DirtyEntries = $dirtyEntries
        Issues       = $issues.ToArray()
    }
}

function Assert-CleanPinnedCheckout {
    [CmdletBinding()]
    param(
        [string]$GitPath = 'git',
        [string[]]$GitPathArguments = @(),
        [Parameter(Mandatory)]
        [string]$Name,
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [string]$ExpectedCommit,
        [Parameter(Mandatory)]
        [string]$ExpectedRepository
    )

    $verification = Get-CheckoutVerification -GitPath $GitPath -GitPathArguments $GitPathArguments -Path $Path -ExpectedCommit $ExpectedCommit -ExpectedRepository $ExpectedRepository
    if ($verification.Issues.Count -gt 0) {
        throw "Refusing candidate '$Name' at '$Path':$([Environment]::NewLine) - $($verification.Issues -join ([Environment]::NewLine + ' - '))"
    }
    return $verification
}

function Write-ReuseRecord {
    [CmdletBinding()]
    param(
        [Parameter(Mandatory)]
        [string]$Path,
        [Parameter(Mandatory)]
        [object]$Record
    )

    $json = $Record | ConvertTo-Json -Compress -Depth 8
    # Append-only evidence: never truncate earlier records. Insert a separator only
    # if an existing file does not already end with a newline.
    $needsLeadingNewline = $false
    if (Test-Path -LiteralPath $Path) {
        $stream = [System.IO.File]::Open($Path, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::ReadWrite)
        try {
            if ($stream.Length -gt 0) {
                [void]$stream.Seek(-1, [System.IO.SeekOrigin]::End)
                if ($stream.ReadByte() -ne 10) { $needsLeadingNewline = $true }
            }
        }
        finally {
            $stream.Dispose()
        }
    }
    if ($needsLeadingNewline) {
        [System.IO.File]::AppendAllText($Path, [Environment]::NewLine)
    }
    [System.IO.File]::AppendAllText($Path, $json + [Environment]::NewLine)
}
