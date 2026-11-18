# Negative owned-process probe for the shipping pin.
# The main launcher has no semantic bridge, so an active UIA client sees no tree.
# This records that absence; it is not accessibility evidence and asserts nothing
# about what a screen reader would say (there is nothing to say).
[CmdletBinding()]
param(
    [string]$Executable = '',
    [string]$OutputPath = '',
    [int]$TimeoutSeconds = 25
)
$ErrorActionPreference = 'Stop'
if ([string]::IsNullOrEmpty($Executable)) {
    $repo = Split-Path (Split-Path $PSScriptRoot -Parent) -Parent
    $Executable = Join-Path $repo 'target/debug/ansa-desktop.exe'
}
if ([string]::IsNullOrEmpty($OutputPath)) {
    $OutputPath = Join-Path $PSScriptRoot 'default-pin-uia-negative-observation.json'
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw 'Build the desktop executable first.' }
$start = [System.Diagnostics.ProcessStartInfo]::new((Resolve-Path $Executable).Path)
$start.Arguments = '--a11y-fixture'
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::new()
$process.StartInfo = $start
[void]$process.Start()
try {
    $ready = $process.StandardOutput.ReadLineAsync()
    if (-not $ready.Wait($TimeoutSeconds * 1000)) { throw "No readiness handshake within ${TimeoutSeconds}s" }
    if ($ready.Result -notmatch '^ANSA_UIA_READY:(\d+)$') { throw 'Fixture readiness handshake was missing or invalid.' }
    $hwnd = [IntPtr]([long]$Matches[1])
    # Only the process started above is queried.
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($hwnd)
    if ($root.Current.ProcessId -ne $process.Id) { throw 'UIA root belongs to an unexpected process.' }
    $nodes = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, [System.Windows.Automation.Condition]::TrueCondition)
    $types = [ordered]@{}
    foreach ($node in $nodes) {
        $name = $node.Current.ControlType.ProgrammaticName
        if ($types.Contains($name)) { $types[$name]++ } else { $types[$name] = 1 }
    }
    $named = @($nodes | Where-Object { -not [string]::IsNullOrEmpty($_.Current.Name) } |
        ForEach-Object { '{0}: {1}' -f $_.Current.ControlType.ProgrammaticName, $_.Current.Name })
    $result = [pscustomobject]@{
        ObservedAtUtc = [DateTime]::UtcNow.ToString('o')
        Executable = (Resolve-Path $Executable).Path
        ExecutableLastWriteTimeUtc = (Get-Item $Executable).LastWriteTimeUtc.ToString('o')
        OsVersion = [Environment]::OSVersion.VersionString
        Command = 'ansa-desktop.exe --a11y-fixture with an active UIA client'
        RootControlType = $root.Current.ControlType.ProgrammaticName
        DescendantCount = $nodes.Count
        ControlTypes = $types
        NamedDescendants = $named
        LabelSearchElements = @($nodes | Where-Object { $_.Current.Name -eq 'Search applications and commands' }).Count
        EditElements = @($nodes | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::Edit }).Count
        ListElements = @($nodes | Where-Object { $_.Current.ControlType -eq [System.Windows.Automation.ControlType]::List }).Count
        SemanticTreePresent = ($nodes.Count -gt 0)
        ScreenReaderAcceptance = $false
    }
    $json = $result | ConvertTo-Json -Depth 5
    [IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath), $json, [Text.UTF8Encoding]::new($false))
    $json
} finally {
    if (-not $process.HasExited) {
        # This is the one fixture process started above, not a discovered user process.
        $process.Kill()
        $process.WaitForExit()
    }
    $process.Dispose()
}
