using System;
using System.Diagnostics;
using System.Threading;
using System.Windows.Automation;
using System.Windows.Automation.Text;

public sealed class AnsaNavigationObservation {
    public string SelectedLabel { get; set; }
    public int Position { get; set; }
    public int LogicalCount { get; set; }
    public int LiveEvents { get; set; }
    public string LiveAnnouncement { get; set; }
    public bool SearchFocusRetained { get; set; }
}

public sealed class AnsaTextObservation {
    public bool DocumentTextMatches { get; set; }
    public string SelectedText { get; set; }
    public int BoundingRectangles { get; set; }
    public bool SelectionRoundTrip { get; set; }
    public bool OversizedValueRejectedAtomically { get; set; }
    public bool GeometryWithinField { get; set; }
    public bool PointRangeWithinSelectedScalar { get; set; }
}

public sealed class AnsaShellObservation {
    public bool NativeWindowChromeAbsent { get; set; }
    public bool GlassToggleRoundTrip { get; set; }
    public bool GlassDefaultsOn { get; set; }
    public bool ThemeToggleRoundTrip { get; set; }
    public bool AppearanceBoundsUnchanged { get; set; }
    public int FixtureRows { get; set; }
    public bool RowSelectionDidNotExecute { get; set; }
    public bool ActionSearchIndependent { get; set; }
    public bool PinAndUnpinWorked { get; set; }
    public bool RowRuntimeIdSurvivedReordering { get; set; }
    public bool DetailsOpenedAndClosed { get; set; }
    public bool CalculatorErrorExposed { get; set; }
    public AnsaTextObservation TextRangeSmoke { get; set; }
    public bool ScreenReaderAcceptance { get; set; }
    public bool ClipboardActionsInvoked { get; set; }
    public bool ExternalLaunchCommandsInvoked { get; set; }
}

public static class AnsaCandidateValueProbe {
    [System.Runtime.InteropServices.StructLayout(System.Runtime.InteropServices.LayoutKind.Sequential)]
    struct NativeRect { public int Left, Top, Right, Bottom; }
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool GetWindowRect(IntPtr hwnd, out NativeRect bounds);
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool GetClientRect(IntPtr hwnd, out NativeRect bounds);
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern int GetWindowRgn(IntPtr hwnd, IntPtr region);
    [System.Runtime.InteropServices.DllImport("gdi32.dll")]
    static extern IntPtr CreateRectRgn(int left, int top, int right, int bottom);
    [System.Runtime.InteropServices.DllImport("gdi32.dll")]
    static extern bool PtInRegion(IntPtr region, int x, int y);
    [System.Runtime.InteropServices.DllImport("gdi32.dll")]
    static extern bool DeleteObject(IntPtr region);
    [System.Runtime.InteropServices.DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
    static extern int GetWindowStyle(IntPtr hwnd, int index);
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern IntPtr GetForegroundWindow();
    [System.Runtime.InteropServices.DllImport("user32.dll")]
    static extern bool SetForegroundWindow(IntPtr hwnd);
    sealed class Changes : IDisposable {
        readonly AutomationElement root;
        readonly AutoResetEvent changed = new AutoResetEvent(false);
        readonly AutomationPropertyChangedEventHandler property;
        readonly StructureChangedEventHandler structure;
        readonly AutomationEventHandler selection;
        readonly AutomationEventHandler live;
        readonly AutomationEvent liveEvent;
        volatile bool disposed;
        public volatile string LastLiveLabel;
        public int LiveEvents;

        public Changes(AutomationElement root) {
            this.root = root;
            property = (sender, args) => Wake();
            structure = (sender, args) => Wake();
            selection = (sender, args) => Wake();
            live = (sender, args) => {
                try {
                    var element = sender as AutomationElement;
                    if (element != null) {
                        LastLiveLabel = element.Current.Name;
                        Interlocked.Increment(ref LiveEvents);
                    }
                } catch (ElementNotAvailableException) { }
                Wake();
            };
            liveEvent = AutomationEvent.LookupById(20024);
            if (liveEvent == null) throw new NotSupportedException("Client has no UIA live-region event identifier");
            Automation.AddAutomationPropertyChangedEventHandler(root, TreeScope.Subtree, property,
                ValuePattern.ValueProperty, AutomationElement.NameProperty, SelectionItemPattern.IsSelectedProperty,
                AutomationElement.HasKeyboardFocusProperty);
            Automation.AddStructureChangedEventHandler(root, TreeScope.Subtree, structure);
            Automation.AddAutomationEventHandler(TextPattern.TextSelectionChangedEvent, root, TreeScope.Subtree, selection);
            Automation.AddAutomationEventHandler(liveEvent, root, TreeScope.Subtree, live);
        }

        void Wake() {
            if (disposed) return;
            // A callback may already be queued when its registration is removed.
            try { changed.Set(); } catch (ObjectDisposedException) { }
        }

        public void Wait(Func<bool> condition, string description) {
            var timer = Stopwatch.StartNew();
            for (;;) {
                try { if (condition()) return; }
                catch (ElementNotAvailableException) { }
                int remaining = 10000 - (int)timer.ElapsedMilliseconds;
                if (remaining <= 0) throw new TimeoutException(description);
                changed.WaitOne(remaining);
            }
        }

        public void Dispose() {
            disposed = true;
            Automation.RemoveAutomationPropertyChangedEventHandler(root, property);
            Automation.RemoveStructureChangedEventHandler(root, structure);
            Automation.RemoveAutomationEventHandler(TextPattern.TextSelectionChangedEvent, root, selection);
            Automation.RemoveAutomationEventHandler(liveEvent, root, live);
            changed.Dispose();
        }
    }

    static AutomationElement Find(AutomationElement root, string name, ControlType type) {
        return root.FindFirst(TreeScope.Descendants, new AndCondition(
            new PropertyCondition(AutomationElement.NameProperty, name),
            new PropertyCondition(AutomationElement.ControlTypeProperty, type)));
    }

    static bool IsSelected(AutomationElement element) {
        object pattern;
        return element != null && element.TryGetCurrentPattern(SelectionItemPattern.Pattern, out pattern)
            && ((SelectionItemPattern)pattern).Current.IsSelected;
    }

    static AutomationElement FindPrefix(AutomationElement root, string prefix, ControlType type) {
        foreach (AutomationElement element in root.FindAll(TreeScope.Descendants,
            new PropertyCondition(AutomationElement.ControlTypeProperty, type))) {
            if (element.Current.Name.StartsWith(prefix, StringComparison.Ordinal)) return element;
        }
        return null;
    }

    static bool LabelContains(AutomationElement root, string text) {
        foreach (AutomationElement element in root.FindAll(TreeScope.Descendants, Condition.TrueCondition)) {
            if (element.Current.Name.IndexOf(text, StringComparison.OrdinalIgnoreCase) >= 0) return true;
        }
        return false;
    }

    static bool SameRuntimeId(int[] first, int[] second) {
        if (first.Length != second.Length) return false;
        for (int index = 0; index < first.Length; index++) if (first[index] != second[index]) return false;
        return true;
    }

    static int PositionOf(AutomationElement root, string prefix) {
        var row = FindPrefix(root, prefix, ControlType.ListItem);
        return row == null ? -1 : (int)row.GetCurrentPropertyValue(AutomationProperty.LookupById(30152));
    }

    static void Invoke(AutomationElement element) {
        if (element == null) throw new InvalidOperationException("Expected invocable control is missing");
        ((InvokePattern)element.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
    }

    static AutomationElement OpenActions(AutomationElement root, Changes changes) {
        Invoke(Find(root, "Actions", ControlType.Button));
        changes.Wait(() => Find(root, "Search actions", ControlType.Edit) != null,
            "Actions did not expose its independent search input");
        var input = Find(root, "Search actions", ControlType.Edit);
        if (!input.Current.HasKeyboardFocus) throw new InvalidOperationException("Action search did not receive focus");
        return input;
    }

    public static object InspectNativeAppearance(IntPtr hwnd) {
        NativeRect bounds, client;
        if (!GetWindowRect(hwnd, out bounds) || !GetClientRect(hwnd, out client))
            throw new InvalidOperationException("Cannot query owned fixture geometry");
        int width = bounds.Right - bounds.Left, height = bounds.Bottom - bounds.Top;
        if (width != client.Right || height != client.Bottom)
            throw new InvalidOperationException("Window and client bounds differ");
        if ((GetWindowStyle(hwnd, -16) & 0x00CF0000) != 0)
            throw new InvalidOperationException("Native chrome is still present");
        if ((GetWindowStyle(hwnd, -20) & 0x00200000) == 0)
            throw new InvalidOperationException("DirectComposition is disabled");
        var region = CreateRectRgn(0, 0, 0, 0);
        if (region == IntPtr.Zero) throw new InvalidOperationException("Cannot allocate probe region");
        try {
            if (GetWindowRgn(hwnd, region) != 3)
                throw new InvalidOperationException("Launcher has no rounded native region");
            if (PtInRegion(region, 0, 0) || PtInRegion(region, width - 1, 0)
                || PtInRegion(region, 0, height - 1) || PtInRegion(region, width - 1, height - 1))
                throw new InvalidOperationException("A square native corner is still inside the window");
            if (!PtInRegion(region, width / 2, 0) || !PtInRegion(region, 0, height / 2)
                || !PtInRegion(region, width - 1, height / 2) || !PtInRegion(region, width / 2, height - 1))
                throw new InvalidOperationException("Window region has an invisible inset");
        } finally {
            if (!DeleteObject(region)) throw new InvalidOperationException("Cannot release probe region");
        }
        return new {
            Scope = "Only the owned launcher HWND; native geometry, not screen capture",
            ClientEqualsWindow = true, NativeChromeAbsent = true, DirectCompositionEnabled = true,
            AllFourNativeCornersClipped = true, RegionReachesAllFourEdges = true,
            DesktopPixelTransparencyVerified = false, PhysicalWidth = width, PhysicalHeight = height
        };
    }

    public static object InspectAppearance(AutomationElement root) {
        var nativeGeometry = InspectNativeAppearance(new IntPtr(root.Current.NativeWindowHandle));
        var list = Find(root, "Search results", ControlType.List);
        if (list == null) throw new InvalidOperationException("No results list");
        var original = root.Current.BoundingRectangle;
        var listBounds = list.Current.BoundingRectangle;
        // Leave only the actual surface border, not a shadow-sized input gutter.
        if (Math.Abs(listBounds.Left - original.Left) > 3 || Math.Abs(listBounds.Right - original.Right) > 3)
            throw new InvalidOperationException("Results surface is still inset from the native window sides");
        using (var changes = new Changes(root)) {
            Invoke(Find(root, "Disable glass appearance", ControlType.Button));
            changes.Wait(() => Find(root, "Enable glass appearance", ControlType.Button) != null, "Glass did not disable");
            Invoke(Find(root, "Enable glass appearance", ControlType.Button));
            changes.Wait(() => Find(root, "Disable glass appearance", ControlType.Button) != null, "Glass did not enable");
            if (root.Current.BoundingRectangle != original || list.Current.BoundingRectangle != listBounds)
                throw new InvalidOperationException("Glass changed window/content bounds");
        }
        return new {
            Scope = "Only the owned launcher fixture; native geometry and semantic state, not screen capture",
            NativeGeometry = nativeGeometry, NoInvisibleSideGutter = true,
            GlassTogglePreservesBounds = true, DesktopPixelTransparencyVerified = false
        };
    }

    public static AnsaShellObservation InspectShell(AutomationElement root, AutomationElement search) {
        var result = new AnsaShellObservation();
        int style = GetWindowStyle(new IntPtr(root.Current.NativeWindowHandle), -16);
        // WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX
        if ((style & 0x00CF0000) != 0) throw new InvalidOperationException("Launcher still has native window chrome: " + style.ToString("X8"));
        result.NativeWindowChromeAbsent = true;
        result.FixtureRows = root.FindAll(TreeScope.Descendants,
            new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.ListItem)).Count;
        if (result.FixtureRows != 3) throw new InvalidOperationException("Launcher fixture must contain only three built-in commands");
        using (var changes = new Changes(root)) {
            var originalBounds = root.Current.BoundingRectangle;
            var originalSearchBounds = search.Current.BoundingRectangle;
            var ownedWindow = new IntPtr(root.Current.NativeWindowHandle);
            if (GetForegroundWindow() != ownedWindow && !SetForegroundWindow(ownedWindow))
                throw new InvalidOperationException("Windows denied foreground activation of the owned fixture");
            search.SetFocus();
            changes.Wait(() => search.Current.HasKeyboardFocus, "Launcher search could not receive UIA focus; owned window is foreground: " + (GetForegroundWindow().ToInt64() == root.Current.NativeWindowHandle));
            if (Find(root, "Disable glass appearance", ControlType.Button) == null)
                throw new InvalidOperationException("Glass is not the default appearance");
            result.GlassDefaultsOn = true;
            Invoke(Find(root, "Disable glass appearance", ControlType.Button));
            changes.Wait(() => Find(root, "Enable glass appearance", ControlType.Button) != null,
                "Opaque appearance did not restore");
            if (root.Current.BoundingRectangle != originalBounds || search.Current.BoundingRectangle != originalSearchBounds)
                throw new InvalidOperationException("Disabling glass changed window or content geometry");
            Invoke(Find(root, "Enable glass appearance", ControlType.Button));
            changes.Wait(() => Find(root, "Disable glass appearance", ControlType.Button) != null,
                "Glass appearance did not enable");
            changes.Wait(() => search.Current.HasKeyboardFocus, "Glass toggle stole search focus");
            if (root.Current.BoundingRectangle != originalBounds || search.Current.BoundingRectangle != originalSearchBounds)
                throw new InvalidOperationException("Enabling glass changed window or content geometry");
            result.GlassToggleRoundTrip = true;
            Invoke(Find(root, "Switch to light theme", ControlType.Button));
            changes.Wait(() => Find(root, "Switch to dark theme", ControlType.Button) != null,
                "Light theme did not enable");
            Invoke(Find(root, "Switch to dark theme", ControlType.Button));
            changes.Wait(() => Find(root, "Switch to light theme", ControlType.Button) != null,
                "Dark theme did not restore");
            if (!search.Current.HasKeyboardFocus) throw new InvalidOperationException("Theme toggle stole search focus");
            if (root.Current.BoundingRectangle != originalBounds || search.Current.BoundingRectangle != originalSearchBounds)
                throw new InvalidOperationException("Theme toggle changed window or content geometry");
            result.ThemeToggleRoundTrip = true;
            result.AppearanceBoundsUnchanged = true;
            var integration = FindPrefix(root, "Integration Status", ControlType.ListItem);
            if (integration == null) throw new InvalidOperationException("Fixture integration command missing");
            int[] originalIdentity = integration.GetRuntimeId();
            ((SelectionItemPattern)integration.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
            changes.Wait(() => IsSelected(FindPrefix(root, "Integration Status", ControlType.ListItem)),
                "Launcher row could not be selected through UIA");
            if (root.FindFirst(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Document)) != null)
                throw new InvalidOperationException("Selecting a row executed it");
            result.RowSelectionDidNotExecute = true;

            var actions = OpenActions(root, changes);
            ((ValuePattern)actions.GetCurrentPattern(ValuePattern.Pattern)).SetValue("pin");
            changes.Wait(() => FindPrefix(root, "Pin command", ControlType.MenuItem) != null
                && root.FindAll(TreeScope.Descendants, new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.MenuItem)).Count == 1,
                "Action query did not filter the menu independently");
            if (((ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern)).Current.Value != "")
                throw new InvalidOperationException("Action search changed root query");
            result.ActionSearchIndependent = true;
            Invoke(FindPrefix(root, "Pin command", ControlType.MenuItem));
            changes.Wait(() => Find(root, "Search actions", ControlType.Edit) == null
                && PositionOf(root, "Integration Status") == 1,
                "Pinning did not close the menu and reorder the selected command");
            if (!SameRuntimeId(originalIdentity, FindPrefix(root, "Integration Status", ControlType.ListItem).GetRuntimeId()))
                throw new InvalidOperationException("Result accessibility identity changed while pinning reordered it");
            result.RowRuntimeIdSurvivedReordering = true;
            OpenActions(root, changes);
            changes.Wait(() => FindPrefix(root, "Unpin command", ControlType.MenuItem) != null, "Pinned action did not become Unpin");
            Invoke(FindPrefix(root, "Unpin command", ControlType.MenuItem));
            changes.Wait(() => Find(root, "Search actions", ControlType.Edit) == null
                && PositionOf(root, "Integration Status") == 2,
                "Unpinning did not restore fixture order");
            result.PinAndUnpinWorked = true;

            var about = FindPrefix(root, "About Ansa", ControlType.ListItem);
            ((SelectionItemPattern)about.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
            changes.Wait(() => IsSelected(FindPrefix(root, "About Ansa", ControlType.ListItem)), "About fixture did not select");
            OpenActions(root, changes);
            Invoke(FindPrefix(root, "Run command", ControlType.MenuItem));
            changes.Wait(() => root.FindFirst(TreeScope.Descendants,
                new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Document)) != null,
                "Explicit Run command did not expose command details");
            Invoke(Find(root, "Back", ControlType.Button));
            changes.Wait(() => root.FindFirst(TreeScope.Descendants,
                new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.Document)) == null
                && search.Current.HasKeyboardFocus, "Back did not close details and return search focus");
            result.DetailsOpenedAndClosed = true;

            ((ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern)).SetValue("1/0");
            changes.Wait(() => LabelContains(root, "division by zero"), "Calculator error reason is not exposed");
            result.CalculatorErrorExposed = true;
        }
        result.TextRangeSmoke = TextRangesAndWait(root, search);
        return result;
    }

    public static AutomationElement SetValueAndWait(AutomationElement root, AutomationElement search) {
        using (var changes = new Changes(root)) {
            var pattern = (ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern);
            pattern.SetValue("Project 09999");
            changes.Wait(() => pattern.Current.Value == "Project 09999"
                && Find(root, "Project 09999 Terminal", ControlType.ListItem) != null,
                "UIA SetValue did not update query/results");
            return Find(root, "Project 09999 Terminal", ControlType.ListItem);
        }
    }

    public static AnsaNavigationObservation NavigateAndWait(AutomationElement root, AutomationElement search,
        string controlName, string expectedLabel, int position, int count) {
        var control = Find(root, controlName, ControlType.Button);
        if (control == null) throw new InvalidOperationException("Navigation control missing: " + controlName);
        string expectedAnnouncement = expectedLabel + ", result " + position + " of " + count;
        using (var changes = new Changes(root)) {
            ((InvokePattern)control.GetCurrentPattern(InvokePattern.Pattern)).Invoke();
            changes.Wait(() => IsSelected(Find(root, expectedLabel, ControlType.ListItem))
                && changes.LastLiveLabel == expectedAnnouncement,
                "Navigation did not realize/select the expected row and publish a live announcement");
            var row = Find(root, expectedLabel, ControlType.ListItem);
            int observedPosition = (int)row.GetCurrentPropertyValue(AutomationProperty.LookupById(30152));
            int observedCount = (int)row.GetCurrentPropertyValue(AutomationProperty.LookupById(30153));
            if (observedPosition != position || observedCount != count) throw new InvalidOperationException("Navigated row has wrong logical metadata");
            if (!search.Current.HasKeyboardFocus) throw new InvalidOperationException("Navigation moved keyboard focus away from search");
            if (((ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern)).Current.Value != "") throw new InvalidOperationException("Navigation changed the query");
            return new AnsaNavigationObservation {
                SelectedLabel = row.Current.Name, Position = observedPosition, LogicalCount = observedCount,
                LiveEvents = changes.LiveEvents, LiveAnnouncement = changes.LastLiveLabel, SearchFocusRetained = true
            };
        }
    }

    public static bool SelectRowAndWait(AutomationElement root, AutomationElement search) {
        using (var changes = new Changes(root)) {
            var row = Find(root, "Project 00001 Terminal", ControlType.ListItem);
            if (row == null) throw new InvalidOperationException("Second fixture row is not realized");
            ((SelectionItemPattern)row.GetCurrentPattern(SelectionItemPattern.Pattern)).Select();
            changes.Wait(() => IsSelected(Find(root, "Project 00001 Terminal", ControlType.ListItem))
                && !IsSelected(Find(root, "Project 00000 Terminal", ControlType.ListItem)),
                "SelectionItem.Select did not select only its target");
            return search.Current.HasKeyboardFocus;
        }
    }

    public static AnsaTextObservation TextRangesAndWait(AutomationElement root, AutomationElement search) {
        const string value = "A\U0001F600e\u0301\u6F22";
        using (var changes = new Changes(root)) {
            var pattern = (ValuePattern)search.GetCurrentPattern(ValuePattern.Pattern);
            object rawText;
            if (!search.TryGetCurrentPattern(TextPattern.Pattern, out rawText)) throw new InvalidOperationException("No UIA TextPattern exposed");
            var text = (TextPattern)rawText;
            pattern.SetValue(value);
            changes.Wait(() => pattern.Current.Value == value && text.DocumentRange.GetText(-1) == value,
                "Unicode text did not reach the UIA document range");
            var document = text.DocumentRange;
            var emoji = document.Clone();
            emoji.MoveEndpointByRange(TextPatternRangeEndpoint.End, document, TextPatternRangeEndpoint.Start);
            emoji.MoveEndpointByUnit(TextPatternRangeEndpoint.Start, TextUnit.Character, 1);
            emoji.MoveEndpointByUnit(TextPatternRangeEndpoint.End, TextUnit.Character, 1);
            if (emoji.GetText(-1) != "\U0001F600") throw new InvalidOperationException("Character range split or skipped the astral scalar");
            emoji.Select();
            changes.Wait(() => {
                var ranges = text.GetSelection();
                return ranges.Length == 1 && ranges[0].GetText(-1) == "\U0001F600";
            }, "UIA range.Select did not round-trip the selection");
            var rectangles = text.GetSelection()[0].GetBoundingRectangles();
            if (rectangles.Length < 1 || rectangles[0].Width <= 0 || rectangles[0].Height <= 0) throw new InvalidOperationException("Selected text has no useful bounding rectangle");
            var center = new System.Windows.Point(rectangles[0].X + rectangles[0].Width / 2, rectangles[0].Y + rectangles[0].Height / 2);
            if (!search.Current.BoundingRectangle.Contains(center)) throw new InvalidOperationException("Text geometry lies outside its field");
            var point = text.RangeFromPoint(center);
            if (point.CompareEndpoints(TextPatternRangeEndpoint.Start, emoji, TextPatternRangeEndpoint.Start) < 0
                || point.CompareEndpoints(TextPatternRangeEndpoint.Start, emoji, TextPatternRangeEndpoint.End) > 0)
                throw new InvalidOperationException("Point-to-text range is outside the selected scalar");
            string oversized = new string('x', 4097);
            pattern.SetValue(oversized);
            changes.Wait(() => {
                foreach (AutomationElement status in root.FindAll(TreeScope.Descendants,
                    new PropertyCondition(AutomationElement.ControlTypeProperty, ControlType.StatusBar))) {
                    if (status.Current.Name.Contains("Input not changed:")) return true;
                }
                return false;
            }, "Oversized UIA edit was not acknowledged as rejected");
            if (pattern.Current.Value != value || text.DocumentRange.GetText(-1) != value)
                throw new InvalidOperationException("Rejected UIA edit mutated text");
            if (text.GetSelection()[0].GetText(-1) != "\U0001F600")
                throw new InvalidOperationException("Rejected UIA edit mutated selection");
            return new AnsaTextObservation {
                DocumentTextMatches = true, SelectedText = text.GetSelection()[0].GetText(-1),
                BoundingRectangles = rectangles.Length, SelectionRoundTrip = true,
                OversizedValueRejectedAtomically = true, GeometryWithinField = true,
                PointRangeWithinSelectedScalar = true
            };
        }
    }
}
