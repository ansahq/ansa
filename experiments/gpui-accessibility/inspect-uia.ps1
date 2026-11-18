# Run with Windows PowerShell (powershell.exe) for its built-in .NET UIA assemblies.
[CmdletBinding()]
param(
    [string]$Executable = '',
    [string]$OutputPath = '',
    [switch]$LauncherShell,
    [switch]$AppearanceOnly,
    [switch]$NativeGeometryOnly
)
$ErrorActionPreference = 'Stop'
if ($AppearanceOnly -and -not $LauncherShell) { throw '-AppearanceOnly requires -LauncherShell.' }
if ($NativeGeometryOnly -and -not $LauncherShell) { throw '-NativeGeometryOnly requires -LauncherShell.' }
if ([string]::IsNullOrEmpty($Executable)) {
    $name = if ($LauncherShell) { 'ansa-launcher-accessibility-candidate' } else { 'ansa-gpui-accessibility-spike' }
    $Executable = Join-Path $PSScriptRoot "target/x86_64-pc-windows-msvc/debug/$name.exe"
}
function Write-ProbeResult([object]$Snapshot) {
    $json = $Snapshot | ConvertTo-Json -Depth 5
    if ([string]::IsNullOrEmpty($OutputPath)) { $json }
    else {
        [IO.File]::WriteAllText([IO.Path]::GetFullPath($OutputPath), $json, [Text.UTF8Encoding]::new($false))
        Write-Host 'Owned-process UIA probe passed; UTF-8 observation saved.'
    }
}
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName WindowsBase
Add-Type -Path (Join-Path $PSScriptRoot 'UiaProbe.cs') -ReferencedAssemblies @('System.dll', [System.Windows.Automation.AutomationElement].Assembly.Location, [System.Windows.Automation.AutomationProperty].Assembly.Location, [System.Windows.Rect].Assembly.Location)
if (-not (Test-Path -LiteralPath $Executable -PathType Leaf)) { throw 'Build the candidate executable first.' }
$start = [System.Diagnostics.ProcessStartInfo]::new((Resolve-Path $Executable).Path)
$start.Arguments = if ($LauncherShell) { '--a11y-fixture' } else { '--uia-probe' }
$start.UseShellExecute = $false
$start.RedirectStandardOutput = $true
$start.CreateNoWindow = $true
$process = [System.Diagnostics.Process]::new()
$process.StartInfo = $start
if (-not $process.Start()) { throw 'Candidate failed to start.' }
try {
    # Console-subsystem Rust binaries do not reliably support WaitForInputIdle.
    # Use the fixture's frame-ready handshake instead of sleeping/polling windows.
    $ready = $process.StandardOutput.ReadLineAsync()
    if (-not $ready.Wait(15000)) { throw 'Candidate did not report a rendered native window.' }
    if ($ready.Result -notmatch '^ANSA_UIA_READY:(\d+)$') { throw 'Candidate readiness handshake was missing or invalid.' }
    $handle = [IntPtr]([long]$Matches[1])
    # Query only the process just launched, never the desktop's other application trees.
    $root = [System.Windows.Automation.AutomationElement]::FromHandle($handle)
    if ($root.Current.ProcessId -ne $process.Id) { throw 'UIA root belongs to an unexpected process.' }
    # The probe activates only the fixture window it started. Windows' foreground
    # lock can deny this; report that instead of pretending focus was verified.
    if (-not [AnsaCandidateValueProbe]::ActivateOwned($handle)) {
        throw ('Windows denied foreground activation of the owned fixture: ' + [AnsaCandidateValueProbe]::ForegroundInfo())
    }
    if ($NativeGeometryOnly) {
        Write-ProbeResult ([AnsaCandidateValueProbe]::InspectNativeAppearance($handle))
        return
    }
    $scope = [System.Windows.Automation.TreeScope]::Descendants
    $searchCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, 'Search applications and commands')
    $search = $root.FindFirst($scope, $searchCondition)
    if ($null -eq $search) { throw 'No labelled search element exposed through UIA.' }
    if ($search.Current.ControlType -ne [System.Windows.Automation.ControlType]::Edit) { throw 'Search role did not map to UIA Edit.' }
    if ($LauncherShell) {
        if ($AppearanceOnly) {
            Write-ProbeResult ([AnsaCandidateValueProbe]::InspectAppearance($root))
            return
        }
        Write-ProbeResult ([AnsaCandidateValueProbe]::InspectShell($root, $search))
        return
    }
    $listCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::NameProperty, 'Search results')
    $list = $root.FindFirst($scope, $listCondition)
    if ($null -eq $list) { throw 'No labelled results container exposed through UIA.' }
    if ($list.Current.ControlType -ne [System.Windows.Automation.ControlType]::List) { throw 'Results role did not map to UIA List.' }
    $rowCondition = [System.Windows.Automation.PropertyCondition]::new([System.Windows.Automation.AutomationElement]::ControlTypeProperty, [System.Windows.Automation.ControlType]::ListItem)
    $rows = $list.FindAll($scope, $rowCondition)
    if ($rows.Count -eq 0) { throw 'No virtualized result items exposed through UIA.' }
    # UIA descendant order is not guaranteed to be logical order; identify the
    # first result by name and verify the logical positions as a set instead.
    $labels = @($rows | ForEach-Object { $_.Current.Name })
    $firstRow = $null
    foreach ($row in $rows) { if ($row.Current.Name -eq 'Project 00000 Terminal') { $firstRow = $row; break } }
    if ($null -eq $firstRow) { throw ('First fixture row is missing; realized rows: ' + ($labels -join '; ')) }
    # IDs verified in windows 0.62.2's UIA_PROPERTY_ID definitions. Older managed
    # clients may not know these properties; that is reported, not assumed passed.
    $positionProperty = [System.Windows.Automation.AutomationProperty]::LookupById(30152)
    $sizeProperty = [System.Windows.Automation.AutomationProperty]::LookupById(30153)
    $position = $null
    $size = $null
    $observedPositions = @()
    $containerSize = $null
    if ($null -ne $positionProperty -and $null -ne $sizeProperty) {
        $position = $firstRow.GetCurrentPropertyValue($positionProperty)
        $size = $firstRow.GetCurrentPropertyValue($sizeProperty)
        if ($position -ne 1 -or $size -ne 10000) { throw "Logical list position/count differ from the 10k fixture: position=$position; size=$size" }
        $observedPositions = @($rows | ForEach-Object { [int]$_.GetCurrentPropertyValue($positionProperty) } | Sort-Object)
        for ($index = 0; $index -lt $observedPositions.Count; $index++) {
            if ($observedPositions[$index] -ne ($index + 1)) { throw "Realized logical positions are not consecutive from 1: $($observedPositions -join ', ')" }
        }
        $containerSize = $list.GetCurrentPropertyValue($sizeProperty)
        # AccessKit's Windows adapter exposes set size on the items; the list
        # container reports -1 (not exposed). Record it instead of asserting.
        $rowSizes = @($rows | ForEach-Object { [int]$_.GetCurrentPropertyValue($sizeProperty) })
        foreach ($rowSize in $rowSizes) {
            if ($rowSize -ne 10000) { throw "A realized result does not expose the logical set size: $rowSize" }
        }
    }
    $listSelectionPattern = $null
    $listHasSelectionPattern = $list.TryGetCurrentPattern([System.Windows.Automation.SelectionPattern]::Pattern, [ref]$listSelectionPattern)
    $listSelectedCount = $null
    if ($listHasSelectionPattern) { $listSelectedCount = $listSelectionPattern.Current.GetSelection().Count }
    $searchValuePattern = $null
    $hasValuePattern = $search.TryGetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern, [ref]$searchValuePattern)
    $valuePattern = $searchValuePattern
    $textPatternProbe = $null
    $hasTextPattern = $search.TryGetCurrentPattern([System.Windows.Automation.TextPattern]::Pattern, [ref]$textPatternProbe)
    $selectionPattern = $null
    $hasSelectionPattern = $firstRow.TryGetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern, [ref]$selectionPattern)
    [void][AnsaCandidateValueProbe]::WaitForSearchFocus($root, $search)
    if (-not $search.Current.IsKeyboardFocusable) {
        throw "Search element is not keyboard-focusable: focusable=$($search.Current.IsKeyboardFocusable); foreground=$([AnsaCandidateValueProbe]::ForegroundInfo())"
    }
    if (-not $hasValuePattern) { throw 'Search value is not exposed through UIA.' }
    if (-not $hasSelectionPattern -or -not $selectionPattern.Current.IsSelected) { throw 'Selected result state is not exposed through UIA.' }
    $scalePercent = [AnsaCandidateValueProbe]::WindowScalePercent($handle)
    $snapshot = [pscustomobject]@{
        ObservedAtUtc = [DateTime]::UtcNow.ToString('o')
        ProbeCommand = 'powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1'
        Executable = (Resolve-Path $Executable).Path
        ExecutableLastWriteTimeUtc = (Get-Item $Executable).LastWriteTimeUtc.ToString('o')
        OsVersion = [Environment]::OSVersion.VersionString
        Scope = 'Only the owned synthetic candidate process'
        SearchRole = $search.Current.ControlType.ProgrammaticName
        SearchLabel = $search.Current.Name
        SearchKeyboardFocusable = $search.Current.IsKeyboardFocusable
        SearchHasKeyboardFocus = $search.Current.HasKeyboardFocus
        SearchValuePatternAvailable = $hasValuePattern
        SearchValueReadOnly = $(if ($hasValuePattern) { $valuePattern.Current.IsReadOnly } else { $null })
        SearchTextPatternAvailable = $hasTextPattern
        ListRole = $list.Current.ControlType.ProgrammaticName
        ListSelectionPatternAvailable = $listHasSelectionPattern
        ListSelectedCount = $listSelectedCount
        ListLogicalSetSize = $containerSize
        RealizedRows = $rows.Count
        RealizedRowLabels = $labels
        RealizedRowPositions = $observedPositions
        FirstRowLabel = $firstRow.Current.Name
        FirstRowPosition = $position
        FirstRowOffscreen = $firstRow.Current.IsOffscreen
        LogicalResultCount = $size
        FirstRowSelectionPatternAvailable = $hasSelectionPattern
        FirstRowSelected = $(if ($hasSelectionPattern) { $selectionPattern.Current.IsSelected } else { $null })
        WindowScalePercent = $scalePercent
        UiaBoundsMatchNativePhysicalPixels = [AnsaCandidateValueProbe]::UiaBoundsMatchNativePhysicalPixels($root)
        ScalingMatrixVerified = 'current monitor only; 150% and 200% unrun'
        ScreenReaderAcceptance = $false
        TextRangeAndSelectionAcceptance = $false
    }
    $selection = [AnsaCandidateValueProbe]::SelectRowAndWait($root, $search, 'Project 00001 Terminal', 2, 10000)
    $snapshot | Add-Member -NotePropertyName RowSelectionWithoutActivation -NotePropertyValue $true
    $snapshot | Add-Member -NotePropertyName RowSelectionAnnouncement -NotePropertyValue $selection
    $last = [AnsaCandidateValueProbe]::NavigateAndWait($root, $search, 'Last result', 'Project 09999 Terminal', 10000, 10000)
    $snapshot | Add-Member -NotePropertyName OffscreenLastResult -NotePropertyValue $last
    $first = [AnsaCandidateValueProbe]::NavigateAndWait($root, $search, 'First result', 'Project 00000 Terminal', 1, 10000)
    $snapshot | Add-Member -NotePropertyName ReturnToFirstResult -NotePropertyValue $first
    $filtered = [AnsaCandidateValueProbe]::SetValueAndWait($root, $search)
    $snapshot | Add-Member -NotePropertyName SetValueUpdatedQueryAndResults -NotePropertyValue $true
    $snapshot | Add-Member -NotePropertyName FilteredRowLabel -NotePropertyValue $filtered.Current.Name
    $snapshot | Add-Member -NotePropertyName ResultIdentityStableAcrossFilter -NotePropertyValue ((($filtered.GetRuntimeId()) -join '.') -eq $last.RuntimeId)
    if ($null -ne $sizeProperty) {
        $filteredSize = $filtered.GetCurrentPropertyValue($sizeProperty)
        if ($filteredSize -ne 1) { throw 'UIA query update did not publish the filtered logical set size.' }
        $snapshot | Add-Member -NotePropertyName FilteredLogicalCount -NotePropertyValue $filteredSize
    }
    $snapshot | Add-Member -NotePropertyName TextRangeSmoke -NotePropertyValue ([AnsaCandidateValueProbe]::TextRangesAndWait($root, $search))
    Write-ProbeResult $snapshot
} finally {
    if (-not $process.HasExited) {
        [void]$process.CloseMainWindow()
        if (-not $process.WaitForExit(5000)) {
            # This is the one fixture process started above, not a discovered user process.
            $process.Kill()
            $process.WaitForExit()
        }
    }
    $process.Dispose()
}
