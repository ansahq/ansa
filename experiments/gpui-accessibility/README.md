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
