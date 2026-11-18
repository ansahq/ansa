# ANS-8 evidence — navigation, view stack, and action state

Status: **implemented and verified with synthetic and model-level tests; F06 is not fully closed.**
Physical keyboard-only operation, real screen-reader announcements, physical IME
behaviour, and pointer-driven pixel scroll restoration remain unverified.

Branch: `ans-foundation-evidence` (no commit made by this task).

## Implemented contracts

### Typed view stack (`ansa_core::navigation`)

- `ViewId`: opaque, validated label (`[A-Za-z0-9._:-]`, ≤ 64 chars). Display
  text with spaces or user prose is rejected, so a view can never be identified
  by the current query.
- `ViewFrame`: view id, captured `ParentState` (exact query text, stable selected
  `CommandId`, logical `ScrollAnchor`, `ParentActions::{Closed, Open}` marker),
  and a monotonically increasing frame generation.
- `ViewStack`: `push`, `pop`, `current`, `depth`, `generation`,
  `restored_count`, `frames`, plus `discard_top`. Depth is bounded by
  `MAX_VIEW_DEPTH = 32`. The root is never a frame and `pop` at the root returns
  `None`.
- Pushing allocates a new generation and captures the parent. Popping marks the
  frame consumed, removes it, allocates a new generation, and returns exactly one
  `RestoredView { view, query, selected, scroll, actions, frame_generation,
  generation }`. Popping twice can only pop the next frame, never restore the
  same parent twice.
- `Engine::push_view(ViewId, ScrollAnchor)` captures the parent from engine state.
  `Engine::pop_view()` re-applies the captured query/selection once, bumps the
  query revision exactly once, and issues a new view generation. Old action
  references — including references issued in the parent before the push — stay
  stale by design.
- `Engine::discard_view()` drops a frame without restoring it (used when the user
  replaces the query instead of navigating back) and still advances the
  generation.
- `Engine::resolve_escape() -> EscapeAction` is the typed hierarchy:
  `CancelComposition` → `CloseActions` → `RestoreView(RestoredView)` →
  `HideRoot` → `None`. No string comparison is involved. `Engine::escape()`
  remains as a thin compatibility wrapper returning `EscapeOutcome`.

### Invocation-scoped action references (`ansa_core::action_state`)

- `ActionRef` is opaque (private fields). Only `ActionRegistry::issue` /
  `issue_panel` can construct one. It captures: launcher invocation, query
  revision, view id, view generation, target (`Command(CommandId)` or exact
  `Calculator(String)`), typed `ActionKey`, and bounded parameters (≤ 4
  parameters, ≤ 256 chars each, no control characters).
- `ActionRegistry::resolve(&ActionRef, &Engine, &ViewStack) -> Result<ResolvedAction,
  ActionError>` revalidates with fixed precedence: `UnknownAction` →
  `StaleInvocation` → `CrossView` → `ExpiredView` → `StaleTarget`.
- `ActionKey` validates ASCII label identities; unknown keys are rejected at
  issue. Panels can bind to a frame via `ActionPanel::open_at(..., generation)`;
  `ActionPanel::is_current` then also requires the same view generation.
- UI panel execution resolves the reference captured when the panel opened. It
  never dispatches `ActionId` directly. Stale/cross-view references produce a
  typed status line and the panel closes; the user reopens actions.

### UI integration (`ansa-ui-gpui`)

- `detail: Option<String>` was replaced by `Option<navigation::ChildView>`
  (`ViewId` + static body). About and Integration Status push typed frames and
  capture query, selected command and logical scroll anchor.
- Opening/focusing Back or pressing Escape pops and restores the parent once:
  query text, selected command and scroll row. The parent command is not re-run.
- Escape order in the UI: focused input's
  `TextInput::action_outcome(EditIntent::Escape)` (`CancelComposition` cancels
  the marked text and consumes the key) → action panel close → child view
  pop/restore → root hide. It no longer relies only on `is_composing`.
- Tab/Shift+Tab still cycle the query field and footer action control; Tab closes
  an open panel and moves to the footer as before.
- Panel refs are issued at open time (`panel_refs`) and resolved at execution;
  the existing reopen-on-invalidated-panel behaviour is preserved with typed
  messages.

## Exact checks and results

| Command | Result |
| --- | --- |
| `cargo test -p ansa-core` | 58 passed, 0 failed (44 unit + 10 `action_state_fuzz` + 3 `navigation_property` + 1 doctest) |
| `cargo test -p ansa-ui-gpui` | 0 failed (crate has no unit tests; compiled clean) |
| `cargo test --workspace` | 266 passed, 0 failed, 1 ignored doctest |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo fmt -p ansa-core -p ansa-ui-gpui -- --check` | exit 0 |
| `cargo run -p ansa-desktop -- --interaction-smoke-test` | 46 synthetic stages passed |
| `cargo run -p ansa-desktop -- --smoke-test` | exit 0 |
| `cargo run -p ansa-desktop -- --lifecycle-smoke-test --lifecycle-cycles=25` | `ANSA_LIFECYCLE_OK cycles=25` |

Interaction smoke output (stage count printed by the test):

```text
Ansa interaction smoke: 46 synthetic widget/action stages passed, including typed view restore, Escape ordering, stale action refs, Tab/Shift+Tab cycles, glass and theme toggling. No clipboard writes or external app launches.
```

## Test volumes

- View-stack property test: 8 deterministic seeds × 500 random operations =
  4000 push/pop/escape/set-query/actions/invocation steps. Asserts bounded depth,
  monotonic generations, unique restored frame generations, root never popped,
  and exact parent query/scroll restoration.
- Deterministic nested-view test asserts exact parent query, selected identity
  and scroll for two nested frames, restoring inner then outer, with the
  revision bumped exactly once per restore.
- Action identity fuzz: 4000 random action-id strings (spaces, punctuation,
  control characters, emoji, lengths 0–299) — every outcome is `Ok` or a typed
  `ActionError`, never a panic.
- Parameter fuzz: 1000 random parameter strings up to 399 chars.
- Source/generation fuzz: 2000 random issue → random mutation (invocation,
  query, selection, push/pop, catalog replacement, actions) → resolve cases,
  checked against an independent snapshot oracle; `resolve` is asserted
  read-only.
- Focused tests cover delayed callbacks, stale invocation, stale revision with
  identical text/selection, changed selection, cross-view same-command,
  same-view new-generation, unknown action, malformed identity, and
  malformed/overlong parameters.

## Unverified items

- Physical keyboard-only run on Windows (the smoke test dispatches synthetic
  keystrokes and GPUI actions, not hardware input).
- Real screen-reader announcements (Narrator/UIA) for view changes, restored
  selection, or typed action-ref errors.
- Physical IME composition on a real input method: cancellation is asserted
  through `TextInput::cancel_composition`, not a native IME session.
- Pointer-driven or pixel-exact scroll restoration; only logical row anchors are
  captured and re-applied.
- The upstream-candidate feature build of the accessibility experiment was not
  re-run for this ticket.
