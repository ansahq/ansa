# ADR 0002: accessibility gate — current GPUI branch vs pinned 0.2.2

Date: 2026-11-11 · Status: structural candidate built and probed; production decision pending full native acceptance
The original sections below are source research. Parent-session compile/runtime findings are recorded in the final section; the main application dependency pin is unchanged.

## Question

Can Ansa expose real AccessKit semantics (roles, labels, selected state, list positions,
text-input value) without adopting a broad fork or an unbounded dependency change?

## Evidence base

Immutable upstream revision read for every claim below:

- repo: `zed-industries/zed`
- commit: `a19e19440d4102668830a9da579c3ee71d2fbf1a` (2026-11-11T16:08:47Z, `main` at time of reading)
- all paths below are `crates/...` at that revision

Local baseline (source audit, no build run): `gpui = "=0.2.2"` from crates.io
(`Cargo.toml`, `Cargo.lock:2138`), checksum `979b45c…`, **no `accesskit` anywhere in
`Cargo.lock`**. Published `gpui` 0.2.2 was released 2025-10-22 and its crates.io dependency
list contains no AccessKit crate — the published release predates the accessibility work.

Everything below is **source evidence**. Nothing here is runtime accessibility validation;
no screen reader was run.

## What the current API actually is

Re-exports (`crates/gpui/src/gpui.rs:114`, `:5`):

```rust
pub use accesskit::{Action as AccessibleAction, Orientation, Role, Toggled};
pub extern crate accesskit;          // gpui::accesskit::Node — no direct dep needed
pub mod _accessibility;              // guide lives at crates/gpui/src/_accessibility.rs
```

`Element` hooks (`crates/gpui/src/element.rs:66`, `:113-137`):

```rust
fn id(&self) -> Option<ElementId>;                       // unchanged from 0.2.2
fn a11y_role(&self) -> Option<accesskit::Role> { None }  // node requires id + role
fn write_a11y_info(&self, _node: &mut accesskit::Node) {}
fn a11y_synthetic_children(&mut self, prepaint, builder: &mut A11ySubtreeBuilder) {}
```

Builder surface for `div()` (all on `StatefulInteractiveElement`, i.e. **after `.id()`**,
`crates/gpui/src/elements/div.rs`):

| API | line | note |
| --- | --- | --- |
| `role(accesskit::Role)` | 1357 | |
| `aria_label(impl Into<SharedString>)` | 1379 | |
| `aria_description` | 1388 | |
| `aria_keyshortcuts` | 1398 | announced only, does not bind keys |
| `aria_active_descendant()` | 1418 | set on the *child*, honoured when a focused ancestor exists |
| `aria_selected(bool)` | 1441 | |
| `aria_expanded` / `aria_toggled(accesskit::Toggled)` | 1447 / 1453 | |
| `aria_numeric_value` / `_step` / `_min` / `_max` | 1459–1492 | range semantics |
| `aria_value(impl Into<SharedString>)` | 1473 | text content of a simple input |
| `aria_placeholder` | 1480 | |
| `aria_level(usize)` | 1504 | headings |
| `aria_position_in_set(usize)` / `aria_size_of_set(usize)` | 1510 / 1516 | **list logical index** |
| `aria_row_index` / `_column_index` / `_row_count` / `_column_count` | 1522–1540 | tables/grids, not lists |
| `on_a11y_action(action, listener)` | 1550 | `FnMut(Option<&ActionData>, &mut Window, &mut App)` |

`Role` has 182 variants including `List`, `ListItem`, `ListBox`, `TextInput`, `SearchInput`,
`ComboBox`, `Button`, `Label`, `Status`, `Alert`. `AccessibleAction` variants include `Click`,
`Focus`, `Blur`, `Increment`, `Decrement`, `SetValue`, `SetTextSelection`,
`ReplaceSelectedText`, `ScrollIntoView`, `ShowContextMenu`.

Action dispatch (`crates/gpui/src/window.rs:7328-7388`): registered listeners run first and
stop dispatch if matched; otherwise only `Click` (synthesised mouse down/up at node bounds),
`Focus` and `Blur` are handled. `SetValue`, `SetTextSelection`, `ReplaceSelectedText`,
`Increment`, `ScrollIntoView` are **not** implemented by the framework — they log
`Unhandled a11y action` unless the app registers a listener.

Text nodes: `impl Element for SharedString` returns `id() == None`
(`crates/gpui/src/elements/text.rs:312-318`), so plain `&str`/`String` children emit **no**
AccessKit node. `Text` only reports `Role::Label` when it carries an id
(`text.rs:181-191`). Visible strings in the current renderer therefore need explicit
`aria_label` on an id'd, role'd ancestor, or the `text!` macro.

Free wins from existing code: `.on_click()` auto-registers `AccessibleAction::Click`
(`div.rs:3699-3701`) and `.track_focus()`/`.focusable()` auto-registers `Focus`
(`div.rs:3702-3703`).

Windows plumbing (`crates/gpui_windows/src/window.rs:1082-1106`): a real
`accesskit_windows::Adapter::new(HWND, …)`, `update_if_active(|| tree_update)`,
`handle_wm_getobject` in `events.rs:849-860`. Tree construction is gated on
`A11y::is_active()` (`window/a11y.rs:126-165`), which is driven by the OS adapter's
activation callback, not by any app-side flag.

## Compatibility cost against the 0.2.2 renderer

Confirmed breaks/needs:

1. `Application::new()` **no longer exists** (`crates/gpui/src/app.rs:240-242` exposes only
   `Application::with_platform` / `new_inaccessible`). Our single call site
   `crates/ui-gpui/src/lib.rs:829` must become `gpui_platform::application().run(...)`.
2. `gpui_platform` / `gpui_windows` are version `0.1.0` and **not published** (crates.io 404),
   so the framework must be consumed from one git revision. `gpui` on `main` still says
   `version = "0.2.2"`, so a `rev` pin is mandatory — `branch = "main"` would be a silent
   moving dependency.
3. Our custom painted text element (`crates/ui-components/src/text_input.rs:426-430`,
   `fn id(&self) -> Option<ElementId> { None }`) gets no a11y node until it returns an id.
4. `uniform_list` / `list` have **zero** a11y/`aria_`/AccessKit references
   (`crates/gpui/src/elements/list.rs`, `uniform_list.rs`), so a `Role::List` container must be
   added by wrapping them in a `div()`.
5. Initial source sampling found familiar `Element` signatures, `WindowOptions`
   (`window_bounds`, `window_min_size`, `titlebar`, `app_id` all present in
   `crates/gpui/src/platform.rs`), `uniform_list`, `cx.processor`, `cx.open_window`, keybindings.
    This sampling was not a compile-verified migration inventory. The later build found additional differences; see the final section.

Dependency churn for the swap: `gpui` git closure adds `accesskit 0.24`, `accesskit_windows
0.34` (pulls `windows 0.62`; its `winit` dep is dev-only) and `gpui_windows`,
`gpui_platform`, `gpui_shared_string`, `gpui_scheduler`; upgrades `windows 0.61.3 → 0.62`,
`taffy 0.9.0 → 0.14`, `resvg 0.45.1 → 0.48`. First fetch clones the Zed repo
(GitHub reports 553,889 KB ≈ 540 MB). No new native toolchain: the Windows feature set
(Direct3D11/Fxc, DirectComposition, DirectWrite, Dwm, UI_Controls, UIAutomationCore) is the
same class 0.2.2 already needs, and the existing VS/SDK `rc.exe` path for the manifest is
unchanged.

Licences: `gpui`, `gpui_platform`, `gpui_windows` declare Apache-2.0
(`crates/gpui/Cargo.toml`, `crates/gpui_platform/Cargo.toml`, `crates/gpui_windows/Cargo.toml`);
`accesskit` and `accesskit_windows` are MIT OR Apache-2.0. The Zed *repository* is
`NOASSERTION`/mixed — consume only `crates/gpui*`, never the app. Toolchain: upstream pins
`1.98.1` (`rust-toolchain.toml`), local is `rustc 1.99.1` and the workspace declares
`rust-version = "1.99"` → compatible, and `gpui.rs` has no `#![feature]` gate.

## Fork evaluation (gpui-ce)

`gpui-ce` 0.3.0, published 2026-10-20, Apache-2.0, `rust-version 1.95`, repo
`gpui-ce/gpui-ce` (pushed 2026-11-11, 1,590 stars, not archived), ~21k recent downloads.

Verified from its published `src/elements/div.rs` (docs.rs source view): it **does** carry
`accesskit` and the same `role` / `aria_label` / `aria_selected` / `aria_position_in_set` /
`aria_size_of_set` / `on_a11y_action` surface, plus extras upstream does not have
(`aria_hidden`, `aria_disabled`, `aria_modal`, `aria_live`, `aria_live_atomic`). Its platform
crate `gpui_ce_platform` 0.3.0 exists on crates.io and exposes `application()` / `headless()`.

Assessment: viable **fallback**, not a recommendation. It is a re-published fork with every
crate renamed (`gpui_ce_*`) and its own 0.3.x version line, so code written against it does not
port 1:1 to upstream (extra `aria_*` methods), the divergence from Zed `main` is unaudited, and
it carries none of Zed's CI. Prefer the pinned upstream revision; only revisit if the ~540 MB
git dependency is unacceptable and a real audit of the fork diff is funded.

## Decision

Implement an isolated a11y slice on the **pinned upstream revision, not on crates.io 0.2.2**.
Do not promote the main renderer merely because source APIs exist. The source-only estimate
understated migration work; compilation and real UIA findings below supersede that estimate.

Bounded to semantics that the existing renderer already has data for. Explicitly **out of
scope** for the spike: caret/selection announcements, `SetValue`/`ReplaceSelectedText`
handling, and announced list-selection movement (see blockers).

## Recommended minimal spike (no fork, no renderer rewrite)

1. `Cargo.toml`: replace `gpui = "=0.2.2"` with
   `gpui = { git = "https://github.com/zed-industries/zed", rev = "a19e19440d4102668830a9da579c3ee71d2fbf1a" }`
   and add the same for `gpui_platform`. Add `gpui_platform.workspace = true` to
   `crates/ui-gpui/Cargo.toml` (Windows platform arrives transitively via `gpui_platform`).
   Lockfile update and first fetch are the parent's step.
2. `crates/ui-gpui/src/lib.rs:829`: `gpui_platform::application().run(…)`; drop the
   `Application` import. This is the only entry-point change.
3. Results rows — one existing `div` (`lib.rs`, the `.id(("result", index))` row), no layout
   change:
   ```rust
   div()
       .id(("result", index))
       .role(Role::ListItem)
       .aria_label(SharedString::from(format!("{title}. {subtitle}")))
       .aria_selected(selected)
                                .aria_position_in_set(index)
   ```
   `.on_click` already supplies the `Click` action. Optionally key the id by command id instead
   of index so filtered reorders are announced as updates rather than replacements.
4. Wrap the existing `uniform_list("results", count, …)` in
    `div().id("results-list").role(Role::List).aria_label("Results").aria_size_of_set(count).flex_1().min_h_0()`
   (uniform_list has no role API; the wrapper must preserve the current flex sizing).
5. Query field: keep the painted element node-less and wrap it once —
   `div().id("query").role(Role::SearchInput).aria_label("Search applications and commands")
    .aria_value(self.input.read(cx).text().to_string()).child(self.input.clone())`.
    The real probe additionally required tracking the input focus handle on this semantic wrapper;
    a role/label alone did not expose keyboard focus. `SetValue` also needs an explicit handler.
6. Give the two footer controls ids and `.role(Role::Button)` + `aria_label`; give the status
   line `.role(Role::Status)` where it already has an id.
7. Acceptance: compiles on Windows; `window.is_a11y_enabled()` true; with a UIA client attached
   the tree shows List/ListItem/SearchInput with labels, selected state and 1-based positions.
   Keep the existing `--interaction-smoke-test` path untouched.

That was a source-only effort estimate, not measured migration work. The native experiment
required compatibility and semantic corrections documented below.

## Blockers and risks

- **Framework pin is all-or-nothing.** `gpui_platform`/`gpui_windows` are unpublished, and both
  the git tree and crates.io report `0.2.2`; mixing releases is not possible.
- **Text input actions are not reachable from a custom `Element`.** `GlobalElementId::
  accesskit_node_id()` is `pub(crate)` (`element.rs:261`), so a custom element cannot compute its
  own node id to call the public `Window::on_a11y_action`. Wiring `SetTextSelection` /
  `ReplaceSelectedText` needs the `div()` wrapper from step 5 plus `ActionData` handling, and
  AccessKit character indices must be converted from our byte/UTF-16 offsets
  (`text_input/model.rs`). Defer.
- **No inactive in-process tree assertions.** Nodes are only built when `A11y::is_active()`
  (`window/a11y.rs:126-165`; `element.rs:436`), which is set by the OS adapter. The public
  `Window::debug_a11y_tree_json()` only holds a tree from a frame where a client was attached.
   Validation requires a real UIA client in a Windows session. The later experiment supplies
   such a client through .NET UI Automation; hosted CI hardware/session availability is still open.
- **Selection does not follow focus.** Ansa changes the list selection while keyboard focus
  stays in the query input. `aria_selected` will be in the tree, but a screen reader's own
  focus does not move, so the change is not announced. `aria_active_descendant()` only helps
  when the focused ancestor is the list container. Solving that means adopting the ARIA
  combobox/listbox pattern (focus or active-descendant ownership) — a UX decision, not
  attribute plumbing.
- **Git dependency cost/supply chain.** ~540 MB clone at first fetch; pinned SHA is immutable,
  Apache-2.0, but it is an unpinned-by-semver surface that must be re-audited when bumped.
- **Not verified in the original source-only investigation:** compilation of the pinned revision against this renderer, lockfile
  resolution, and any screen-reader behaviour. No NVDA, Narrator or Accessibility Insights run
  was performed; no claim of working accessibility is made.

## Consequences

Ansa can evaluate structural semantics without a framework fork, but shipping them requires
completing the actual accessibility journeys, not simply adopting attribute APIs. The original
pin remains framework-blocked; production adoption of the newer revision remains a gate decision.

## Parent-session compile and runtime findings

An independent workspace now exists at `experiments/gpui-accessibility/`. Its source is the exact
revision above, downloaded as a 24.54 MiB archive with SHA-256
`35d3d5c2af07bb4f7fdcf7dda594990730df8c8b94e994df7729cb19f55c5e2f`.
The bootstrap validates that hash and materializes internal file links without Windows symlink
privileges. No Zed application code was copied into Ansa. The experiment has its own lockfile;
the main launcher pin/lockfile is unchanged.

Observed: Windows candidate check/build and two-frame startup succeeded. Shared-widget compile
differences were a new blanket `IntoElement` implementation and two additional text-paint
arguments, handled by a small experiment-only feature. The candidate also needs the platform
factory, the newer native-focus signature and a two-argument window-closed callback. Actual
resolution includes AccessKit 0.24.1, accesskit_windows 0.34.0, windows 0.62.2, taffy 0.15.0 and
resvg 0.46.0; this supersedes the source-only churn estimates above.

The owned-process UIA probe passed and its observation is saved as
`experiments/gpui-accessibility/uia-observation.json`:

- labelled `ControlType.Edit`, keyboard-focusable and focused, with a value pattern;
- labelled `ControlType.List`, 9 realized items out of the 10k fixture;
- first item's logical position 1, set size 10,000, and selected state exposed;
- a real UIA `ValuePattern.SetValue("Project 09999")` request updated the shared input and
  filtered the core results to `Project 09999 Terminal`, with logical size 1.

The first probe found position **2** and set size **-1**. AccessKit stores zero-based positions,
and the Windows adapter adds one (`accesskit_windows/src/node.rs`). Set size is read from an
ancestor container (`accesskit_consumer/src/node.rs`). The experiment therefore uses
`aria_position_in_set(index)` on rows and `aria_size_of_set(count)` on the list wrapper. Merely
using ARIA-named methods with WAI-ARIA's numeric convention was incorrect. An initial focus probe
also reported a non-focusable semantic wrapper; tracking the actual input focus handle corrected
it. `SetValue` is explicitly handled through the same bounded, composition-aware input path.

No NVDA/Narrator journey, text-range/selection acceptance, virtualized offscreen navigation,
selection announcement or cross-platform accessibility gate has passed. This is not a claim
that the main application is accessible, and the production dependency pin has not been promoted.
