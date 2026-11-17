# Bounded source-only feasibility bootstrap. Does not modify the main workspace pin.
[CmdletBinding()]
param()
$ErrorActionPreference = 'Stop'
$revision = 'a19e19440d4102668830a9da579c3ee71d2fbf1a'
$expectedHash = '35D3D5C2AF07BB4F7FDCF7DDA594990730DF8C8B94E994DF7729CB19F55C5E2F'
$repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
$cache = Join-Path $repo 'target/framework-candidates'
$archive = Join-Path $cache 'zed-a19e1944.tar.gz'
$destination = Join-Path $cache 'pinned-zed'
$finalDestination = $destination
$marker = Join-Path $destination '.ansa-source-sha256'
if (Test-Path $marker) {
    if ((Get-Content $marker -Raw).Trim() -ne $expectedHash) { throw 'Candidate source marker mismatch' }
    Write-Host 'Pinned candidate source already prepared.'
    return
}
if (Test-Path $destination) { throw "Refusing to overwrite an unmarked candidate directory: $destination" }
New-Item -ItemType Directory -Path $cache -Force | Out-Null
if (-not (Test-Path $archive)) {
    & curl.exe -L --fail --max-time 180 "https://codeload.github.com/zed-industries/zed/tar.gz/$revision" -o $archive
    if ($LASTEXITCODE -ne 0) { throw 'Candidate source download failed' }
}
if ((Get-FileHash $archive -Algorithm SHA256).Hash -ne $expectedHash) { throw 'Candidate archive SHA-256 mismatch' }
$destination = Join-Path $cache ('.pinned-zed-staging-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Path $destination | Out-Null
$prefix = [IO.Path]::GetFullPath($destination) + [IO.Path]::DirectorySeparatorChar
function Assert-InCandidate([string]$Path) {
    $full = [IO.Path]::GetFullPath($Path)
    if (-not $full.StartsWith($prefix, [StringComparison]::OrdinalIgnoreCase)) { throw "Archive path escapes candidate: $Path" }
    return $full
}
$links = [Collections.Generic.List[object]]::new()
$file = [IO.File]::OpenRead($archive)
$gzip = [IO.Compression.GZipStream]::new($file, [IO.Compression.CompressionMode]::Decompress)
$reader = [System.Formats.Tar.TarReader]::new($gzip)
try {
    while ($entry = $reader.GetNextEntry()) {
        if ($entry.EntryType.ToString() -eq 'GlobalExtendedAttributes') { continue }
        $root = "zed-$revision/"
        if ($entry.Name.TrimEnd('/') -eq "zed-$revision") { continue }
        if (-not $entry.Name.StartsWith($root, [StringComparison]::Ordinal)) { throw 'Unexpected archive root' }
        $relative = $entry.Name.Substring($root.Length)
        $path = Assert-InCandidate (Join-Path $destination $relative)
        switch ($entry.EntryType.ToString()) {
            'Directory' { New-Item -ItemType Directory -Path $path -Force | Out-Null }
            { $_ -in 'RegularFile', 'V7RegularFile' } {
                New-Item -ItemType Directory -Path (Split-Path $path -Parent) -Force | Out-Null
                $output = [IO.File]::Create($path)
                try { if ($null -ne $entry.DataStream) { $entry.DataStream.CopyTo($output) } } finally { $output.Dispose() }
            }
            'SymbolicLink' {
                $target = Assert-InCandidate (Join-Path (Split-Path $path -Parent) $entry.LinkName)
                $links.Add(@{Path=$path; Target=$target})
            }
            'HardLink' {
                if (-not $entry.LinkName.StartsWith($root, [StringComparison]::Ordinal)) { throw 'Unexpected hard-link root' }
                $target = Assert-InCandidate (Join-Path $destination $entry.LinkName.Substring($root.Length))
                $links.Add(@{Path=$path; Target=$target})
            }
            default { throw "Unsupported source archive entry: $($entry.EntryType)" }
        }
    }
} finally {
    $reader.Dispose()
    $gzip.Dispose()
    $file.Dispose()
}
# Materialize internal file links without requiring Windows symlink privileges.
# No links are created; targets outside the candidate were rejected above.
for ($pass = 0; $pass -lt 8 -and $links.Count -gt 0; $pass++) {
    foreach ($link in @($links.ToArray())) {
        if (Test-Path -LiteralPath $link.Target -PathType Leaf) {
            New-Item -ItemType Directory -Path (Split-Path $link.Path -Parent) -Force | Out-Null
            Copy-Item -LiteralPath $link.Target -Destination $link.Path
            [void]$links.Remove($link)
        }
    }
}
if ($links.Count -gt 0) { throw 'Candidate has unresolved/directory/cyclic links; source was not marked prepared' }
Set-Content -LiteralPath (Join-Path $destination '.ansa-source-sha256') -Value $expectedHash
Move-Item -LiteralPath $destination -Destination $finalDestination
Write-Host "Prepared immutable candidate $revision. Only dependency crates are consumed; no Zed application code is imported into Ansa."
