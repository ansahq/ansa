# Immutable GPUI accessibility candidate

This independent, unpublished workspace is **not** the shipping launcher. It tests whether the existing input widget and renderer-independent core can compile against upstream GPUI revision `a19e19440d4102668830a9da579c3ee71d2fbf1a`, with structural search/list semantics on a 10k fixture. It does not switch the main dependency pin or close the accessibility gate.

```powershell
pwsh -File experiments/gpui-accessibility/prepare.ps1
cargo check --manifest-path experiments/gpui-accessibility/Cargo.toml
cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --target x86_64-pc-windows-msvc
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1
```

The bootstrap verifies the source archive SHA-256 (`35d3d5c2af07bb4f7fdcf7dda594990730df8c8b94e994df7729cb19f55c5e2f`) and materializes internal file links inside an ignored cache, avoiding a full-history clone and Windows symlink privileges. It does not alter system settings or overwrite an unmarked cache. The archive includes Zed's mixed-license workspace, but only dependency crates are consumed; nothing from the Zed application is copied into the Ansa source tree. Audit the actual resolved dependency licenses before adopting a production pin.

The root `Cargo.lock` and working GPUI 0.2.2 application are independent. This candidate has its own lockfile and a small `upstream-candidate` adapter feature in the shared widget; that feature must not be enabled against the main crates.io renderer. Compile differences found so far: blanket `IntoElement`, two additional text-paint arguments, platform factory startup, native focus arguments and the window-closed callback signature. The upstream cache is explicitly excluded from the main workspace so Cargo resolves its inherited dependencies from the correct root.

## Observed locally

The pinned candidate compiled and built on Windows, completed its two-frame native smoke test, and passed the UIA structural probe. The probe reported `ControlType.Edit` with the search label, `ControlType.List`, 9 realized rows, first label `Project 00000 Terminal`, and logical position 1 of 10,000. It queries only the fixture process it starts and closes that process afterward; it does not enable a screen reader or inspect unrelated app trees.

The first UIA run caught `position=2; size=-1`. AccessKit stores **zero-based** positions, and its Windows adapter adds one. Logical set size belongs on the **list container**, not each row. The corrected candidate now passes the asserted values. The ARIA-named builder methods do not by themselves establish WAI-ARIA index semantics.

The probe also asserts that search is keyboard-focusable/focused, exposes a value pattern, and that the first result exposes selected state. A semantic wrapper alone did not expose focus; tracking the real field's focus handle fixed it. An explicit `SetValue` handler uses the shared field's bounded, composition-aware replacement path. The probe sets a synthetic query via real UIA, waits on UIA value/structure events (no sleep-based polling), and confirms the expected filtered row and logical count. See [`uia-observation.json`](uia-observation.json) for the local observation; it is not a portable guarantee.

## Actual launcher shell and text-range checks (12 November)

The native appearance regression checks do not require foreground activation or screenshots:

```powershell
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -AppearanceOnly
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -NativeGeometryOnly -Executable target/debug/ansa-desktop.exe
```

The second command targets the default backend, whose old GPUI does not expose the semantic
tree. Both commands query only the process they start. Native checks assert equal client/window
bounds, absence of window chrome, DirectComposition style and a rounded region reaching all
four edges while excluding all four corner pixels. The candidate additionally checks no content
gutter and stable window/list bounds across glass toggles. These are geometry/configuration
checks, **not proof of visible desktop transparency**.

The optional shell binary reuses `crates/ui-gpui`, rather than duplicating the launcher:

```powershell
cargo +stable build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc --features launcher-shell --bins
& experiments/gpui-accessibility/target/x86_64-pc-windows-msvc/debug/ansa-launcher-accessibility-candidate.exe --interaction-smoke-test
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath experiments/gpui-accessibility/launcher-uia-observation-2026-11-12.json
```

`--a11y-fixture` loads only three built-in commands, skips discovery/hotkey registration and
prints an owned-window readiness handshake. The probe selects without execution, independently
filters actions, invokes pin/unpin, verifies stable row RuntimeId across reordering, opens/closes
About details and reads a calculator error. It does not invoke clipboard or external-launch actions.
Both fixtures now pass Unicode TextPattern document/selection/bounds/hit-testing and atomic
oversized-edit rejection checks. The 10k fixture also passes end/start realization and actual
polite live-region event checks; its keyboard smoke has eight stages. The actual launcher retains
its 26-stage synthetic interaction smoke on both backends.

See [the validation record](../../docs/validation-2026-11-12.md) and the dated JSON observations.
Real NVDA/Narrator journeys, real IME, RTL/mixed-bidi and mixed-DPI geometry, full offscreen
provider enumeration, performance and other OSes remain unverified. No promotion of the shipping
dependency pin or general accessibility acceptance is implied.

## 18 November — extended checks and a candidate compatibility break

> Update later the same day: the launcher-shell build break below was fixed by the renderer owner
> (`cx.background_executor().timer(Duration::from_secs(2)).await` in place of `gpui::Timer`). The
> shell rebuilt and its UIA probe was refreshed; see
> [Launcher shell refreshed](#launcher-shell-refreshed-18-november). The historical note is kept
> because the earlier observation files still date from the broken-build period.

The isolated spike still builds and passes on the immutable revision. At the time of the first run
in this section, the actual launcher shell did not build at `HEAD` (details below); the renderer
fixed that later the same day and the shell probe below was refreshed. Nothing here is
screen-reader evidence.

```powershell
cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc
& experiments/gpui-accessibility/target/x86_64-pc-windows-msvc/debug/ansa-gpui-accessibility-spike.exe --interaction-smoke-test
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -OutputPath experiments/gpui-accessibility/uia-observation-2026-11-18.json
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-pinned-uia.ps1 -OutputPath experiments/gpui-accessibility/default-pin-uia-negative-observation-2026-11-18.json
```

The spike smoke passed its **8 stages**. The extended owned-process UIA probe passed and wrote
[`uia-observation-2026-11-18.json`](uia-observation-2026-11-18.json). Additions to the previously
saved assertions:

- realized logical positions are consecutive from 1, and every realized item (not the list
  container) carries the logical set size; the list container reports `-1`, which the probe
  records instead of assuming;
- `SelectionItem.Select` publishes a polite live-region event naming the selected row
  (`Project 00001 Terminal, result 2 of 10000`);
- the rejected oversized edit is announced through the same polite live region, not only visible
  as static status text;
- collapsed caret selection round-trips at document start and end, and character movement before
  the start / past the end clamps without mutating text;
- the filtered row keeps its RuntimeId, so result identity survives a query change as well as
  pin reordering;
- the probe records the window scale (`100`), confirms UIA bounds are physical pixels, and states
  that 150 %/200 % and non-100 % rendering were **not** run.

The probe now activates only the fixture window it started before asserting focus (the same owned
window operation the shell probe already used) and waits for focus through a UIA property event
instead of sleeping. When Windows' foreground lock denied activation while another application was
foreground, the probe failed with `Search UIA focus does not match the native input focus` and the
foreground window identity; after adding the explicit owned-window activation it passed, and the
second observation was saved as
[`uia-observation-2026-11-18-rerun.json`](uia-observation-2026-11-18-rerun.json). A later invocation
was denied again while another app held the foreground (the failure named that window) and the next
run passed. A denied activation is reported as a failure, never as a pass; focus-dependent evidence
requires a session where the owned fixture may take foreground.

The first probe invocation on 18 November failed with `First row label/identity differs from the
fixture` even though the tree contained the expected rows; a rerun passed. The script had assumed
UIA `FindAll` descendant order is logical order. It now finds the first row by name and validates
the set of positions; that flake is a probe robustness defect, not an app change, and is recorded
rather than hidden.

`inspect-pinned-uia.ps1` records the shipping pin's absence of semantics: with an active UIA
client attached to its owned `--a11y-fixture` process, the window exposes **0 descendants**
(`EditElements = 0`, `ListElements = 0`, no labelled search element). This is negative structural
evidence, not a screen-reader test.

### Launcher-shell build was blocked at HEAD (fixed later the same day)

```text
cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline `
  --target x86_64-pc-windows-msvc --features launcher-shell --bins
error[E0432]: unresolved import `gpui::Timer`
 --> crates/ui-gpui/src/host_lifecycle.rs:18:36
```

`crates/ui-gpui` (commit `5b25e7b`, still present at `HEAD`) used `Timer::after`, which resolves to
`pub use smol::Timer` in crates.io `gpui 0.2.2` but did **not** exist in the pinned upstream revision
`a19e1944`. The experiment compiles that crate with the `upstream-candidate` feature, so the
launcher shell and its UIA probe could not be rebuilt until the timer use became backend-neutral.
The renderer owner made that change and the shell probe now runs (see
[Launcher shell refreshed](#launcher-shell-refreshed-18-november)). The 12 November shell
observation and geometry JSON remain historical artifacts of an earlier binary. This workspace did
not edit `crates/ui-gpui`.

The launcher-shell C# additions (`DetailsDocumentLabel`, `CalculatorErrorLiveEventObserved`,
`WindowScalePercent`, `UiaBoundsMatchNativePhysicalPixels`, list position fields, stale-action
fields) compile through `Add-Type` and now run in the refreshed shell probe.

`--locked` initially refused because the experiment lockfile predated new path-crate dependencies;
an `--offline` build refreshed it (it gained the `ansa-host` entry) and later `--locked` builds
succeed. No dependency versions or registries changed.

## Launcher shell refreshed (18 November)

After the renderer owner replaced the 0.2.2-only `Timer` with the pinned revision's executor timer,
the shell builds again and the owned-process UIA probe was rerun against the real `--a11y-fixture`
launcher, not the small spike.

```powershell
cargo build --manifest-path experiments/gpui-accessibility/Cargo.toml --locked --offline --target x86_64-pc-windows-msvc --features launcher-shell --bins
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath experiments/gpui-accessibility/launcher-uia-observation-2026-11-18.json
powershell.exe -NoProfile -File experiments/gpui-accessibility/inspect-uia.ps1 -LauncherShell -OutputPath experiments/gpui-accessibility/launcher-uia-observation-2026-11-18-rerun.json
```

Both probe runs passed and produced identical key fields
([first](launcher-uia-observation-2026-11-18.json),
[rerun](launcher-uia-observation-2026-11-18-rerun.json)). Observed in the three-command fixture at
100 % scale:

- `ControlType.List` named `Search results`, realized positions `1,2,3`, item set size `3`, all rows
  onscreen; the list container again reports set size `-1`.
- selected row label `Integration Status. See what this development build supports` (title plus
  subtitle).
- UIA row selection does not execute; action search filters independently; Pin/Unpin works and the
  selected row keeps its RuntimeId across reordering.
- Details opens with document label `Ansa` and Back restores search focus.
- The Unicode TextPattern checks (document text, emoji selection, positive bounds, point hit-test,
  caret at start/end, clamped boundary movement, atomic oversized rejection) pass in the shell too;
  the oversized rejection announces through the polite live region.
- DPI is `100`, UIA bounds equal native physical pixels, native chrome is absent, glass defaults on,
  and glass/theme toggles preserve bounds and search focus.

Two surfaces did **not** provide complete evidence, and neither is presented as a pass:

1. **Calculator error has no live-region announcement.** The error is exposed as
   `ControlType.Text` named `Cannot calculate: division by zero`, but no live event carried that
   text within 3 s and the polite status label at that moment was still
   `Unpinned for this session`. The provider rejects the UIA `LiveSetting` property
   (`Unsupported Property`), so the live setting cannot be read back from UIA either. Recorded as
   `CalculatorErrorLiveEventObserved=false`; see A11Y-21.
2. **The stale-action rejection surface is not reachable through UIA here.** With the Actions menu
   open, `SelectionItem.Select` on an underlying row did not change selection within 3 s
   (`StaleActionSelectionBlockedByMenu=true`), so the menu target could not be invalidated without
   synthetic keys or pointer input, which this probe does not inject. The menu was closed through its
   Actions button and the rest of the probe continued. `StaleActionSurfaceExercised=false`;
   `StaleActionRejected` and `StaleActionAnnouncementObserved` are therefore false, not failed.

The shell probe also hit desktop focus contention. While another application was in active use,
several invocations failed with `Launcher search could not receive UIA focus; owned window is
foreground: True` (or the action-search equivalent), and retries passed. Only passing observations
are saved; a failed attempt is never converted into a pass. The probe still activates only its own
fixture window and fails loudly when Windows or another window keeps focus elsewhere. This is the
same tooling fragility recorded for the spike (A11Y-19).

No screen reader was run, and the JSON keeps `ScreenReaderAcceptance=false`. This refresh is
structural evidence only.
