# Research ledger and primary sources

Prepared 11 November 2026. This ledger supports `rust-launcher-blueprint.md` and `implementation-backlog.md`. Three research subagents covered product/API scope, UI frameworks, and platform/security design. The UI and security specialists also reviewed the consolidated blueprint; their concrete corrections were incorporated.

## Evidence classification

| Question | Status | Evidence and remaining work |
|---|---|---|
| What should broad Raycast parity cover? | Answered at documented feature-family level | Official current manual, Windows product and SDK pages; exact per-build behavior still requires capture |
| Is a Rust UI compatible with React-authored extensions? | Architecture specified | Custom reconciler and declarative protocol are a proposed implementation; no compatibility runtime built here |
| Which Rust UI framework best fits the preference? | Recommendation made; empirical gate open | GPUI selected provisionally from upstream capabilities and product fit; no benchmark or screen-reader trial performed |
| Can every desktop OS expose identical automation? | Constraints established; exact support matrix open | OS API/portal specifications; actual versions/compositors require device testing |
| How should isolation and data security work? | Design specified; review/testing required | Explicit trusted Node class, restricted Wasm option, broker, vault, update and sync design |
| What are Raycast's private internals? | Unresolved and unnecessary for this plan | No binary analysis, database inspection or private protocol observation |
| Is the schedule proven? | Estimate only | Assumes 6–8 contributors, test hardware, bounded scope and successful framework gate |
| Has the application been built or production-tested? | No | Deliverables are a design, backlog, evidence map and validation plan |

There are no REA-generated Evidence IDs. Installing/reading the skill did not expose the REA server. The diagnostic download failed at npm DNS resolution; no setup/configuration mutation was performed. Ordinary shell searches are not REA evidence. The future capture plan records target identity, inputs, permissions and observations explicitly.

## Raycast product reference map

The descriptions below identify source scope; they are not claims of observed execution or identical support on every OS. Current online documentation can change and includes migration-era qualifications. Freeze target binaries and fixture expectations before making a compatibility claim.

| Source | What it informs |
|---|---|
| [Manual index](https://manual.raycast.com/) and [Quickstart](https://manual.raycast.com/quickstart) | Overall feature inventory, root search and keyboard workflow |
| [Windows product](https://www.raycast.com/windows) | Windows availability and advertised feature surface; cached download versions were inconsistent, so no exact latest version is asserted |
| [File Search](https://manual.raycast.com/file-search) | Root/dedicated file workflows, metadata/content distinctions |
| [Quicklinks](https://manual.raycast.com/quicklinks) | Parameterized links, paths, preferred applications and organization |
| [Clipboard History](https://manual.raycast.com/clipboard-history) | Format-aware history, filtering/pinning, exclusions, OCR/QR and retention scope |
| [Snippets](https://manual.raycast.com/snippets) | Search/manual insertion, expansion, placeholders and import |
| [Notes](https://manual.raycast.com/notes) | Separate note surface, Markdown-style editing, tasks and search |
| [Window Management](https://manual.raycast.com/window-management) | Geometry, display/desktop operations, restore and custom layouts |
| [Navigation](https://manual.raycast.com/navigation) | Window switching and application menu search |
| [Script Commands](https://manual.raycast.com/script-commands) | Script metadata, arguments, interpreters and invocation workflows |
| [Screenshots](https://manual.raycast.com/screenshots) | Media search, OCR, previews and latest-image operations |
| [Focus](https://manual.raycast.com/focus) and [Hyper Key](https://manual.raycast.com/hyper-key) | Focus sessions and keyboard remapping as distinct subsystems |
| [Dictation](https://manual.raycast.com/ai/dictation) | Recording/transcription/cleanup/insertion workflow |
| [Screen Awareness](https://manual.raycast.com/ai/screen-awareness) | Window/selection/screenshot context collection |
| [Agents](https://manual.raycast.com/ai/agents) | Saved instruction/model/tool configurations |
| [Skills](https://manual.raycast.com/ai/skills) | Reusable skill-file context and discovery |
| [Projects](https://manual.raycast.com/ai/projects) | Related chats, instructions, working directory and memory |
| [Automations](https://manual.raycast.com/ai/automations) | Scheduled AI workflows; exact lifecycle needs versioned testing |
| [Local Models](https://manual.raycast.com/ai/local-models) | Local-model feature category; no reason for this alternative to impose a subscription gate |
| [Cloud Sync](https://manual.raycast.com/cloud-sync) | Selective sync and explicit exclusions; current docs exclude clipboard history, script bodies and credentials, and qualify v2 mobile sync |
| [Teams shared features](https://manual.raycast.com/teams/shared-features) | Shared categories/private extensions; some v2 features are unfinished and shared Skills are marked forthcoming |
| [Pricing](https://www.raycast.com/pricing) and [Billing](https://manual.raycast.com/billing) | Free versus paid reference scope; exact changing prices are intentionally not part of the engineering specification |

## SDK and architecture references

| Source | Use and boundary |
|---|---|
| [API introduction](https://developers.raycast.com/) | React/TypeScript/Node extension model; not proof of compatibility with another host |
| [API reference](https://developers.raycast.com/api-reference) | UI primitives, navigation, storage, preferences, native operations and service APIs |
| [Manifest](https://developers.raycast.com/information/manifest.md) | Package/command modes, platform declarations and extension metadata |
| [Tool API](https://developers.raycast.com/api-reference/tool.md) | Tool/resource and confirmation surface for AI integration |
| [Public extensions](https://github.com/raycast/extensions) | Inspectable extension sources; repository-level licensing is not a substitute for per-package/asset audit |
| [Script examples](https://github.com/raycast/script-commands) | Metadata and cross-interpreter examples; not a portability guarantee |
| [Raycast technical architecture, May 2026](https://www.raycast.com/blog/a-technical-deep-dive-into-the-new-raycast) | Vendor description of its v2 hybrid architecture, including reasons for its own framework choices; no private internals were verified here |

Older API FAQ material can describe the previous AppKit renderer. Prefer the dated v2 architecture article for current architecture context, while using versioned API contracts for compatibility tests. Neither article substitutes for target-build observation.

## Framework sources

| Source | Evidence boundary |
|---|---|
| [GPUI README](https://github.com/zed-industries/zed/blob/main/crates/gpui/README.md) | Rust GPU UI and active/pre-1.0 status; choose a tested immutable dependency |
| [GPUI accessibility](https://github.com/zed-industries/zed/blob/main/crates/gpui/src/_accessibility.rs) | Semantic authoring infrastructure; not a successful screen-reader test |
| [GPUI input example](https://github.com/zed-industries/zed/blob/main/crates/gpui/examples/input.rs) | Composition/selection/caret implementation foundations; not a complete production editor |
| [GPUI macOS](https://github.com/zed-industries/zed/blob/main/crates/gpui_macos/Cargo.toml), [Windows](https://github.com/zed-industries/zed/blob/main/crates/gpui_windows/Cargo.toml), [Linux](https://github.com/zed-industries/zed/blob/main/crates/gpui_linux/Cargo.toml) | Platform/AccessKit dependencies in current source; released version and end-to-end behavior still need checking |
| [Zed license overview](https://zed.dev/software-overview) | Different licensing for reusable components and application code |
| [GPUI Kit](https://github.com/longbridge/gpui-kit) | Component reuse candidate, subject to actual version/license/quality audit |
| [Slint backend integration](https://github.com/slint-ui/slint/blob/master/docs/development/window-backend-integration.md), [licensing choices](https://slint.dev/get-started) | Native fallback with Rust API and separate UI DSL; compare the same interaction slice |
| [Iced](https://github.com/iced-rs/iced), [accessibility issue](https://github.com/iced-rs/iced/issues/552) | Rust alternative with accessibility work requiring explicit verification |
| [egui accessibility](https://github.com/emilk/egui/blob/main/docs/accessibility.md) | Rust tooling/UI alternative; fit assessment is our design judgment |
| [Tauri engine versions](https://v2.tauri.app/reference/webview-versions/), [capabilities](https://tauri.app/security/capabilities/) | Hybrid comparison, not the selected all-Rust application UI |

All main/master URLs are mutable. Phase 0 must replace implementation dependency references with exact versions/commits and archive the relevant build/test manifest. This report does not invent commit hashes it did not retrieve.

## OS and security sources

| Source | Consequence for the plan |
|---|---|
| [Apple Accessibility trust](https://developer.apple.com/documentation/applicationservices/1459186-axisprocesstrustedwithoptions) | Permission prompting and actual authorization are different states |
| [NSPasteboard change count](https://developer.apple.com/documentation/appkit/nspasteboard/changecount) | Clipboard-change tracking primitive; polling/power behavior requires measurement |
| [RegisterHotKey](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-registerhotkey) | Register and handle conflicts explicitly |
| [Clipboard listener](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener) | Windows message-loop integration |
| [SendInput](https://learn.microsoft.com/windows/win32/api/winuser/nf-winuser-sendinput) | Integrity-boundary limitations on synthetic input |
| [SetForegroundWindow](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setforegroundwindow) | Activation is subject to OS rules, not guaranteed |
| [SetWindowPos](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-setwindowpos) | Native geometry integration with session/window constraints |
| [Windows clipboard formats](https://learn.microsoft.com/en-us/windows/win32/dataxchg/clipboard-formats) | Multiple formats and history/cloud exclusion markers |
| [GlobalShortcuts portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.GlobalShortcuts.html) | Backend-dependent Wayland shortcut path |
| [Clipboard portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.Clipboard.html), [RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) | Session-mediated features; not universal silent clipboard/input access |
| [X11 EWMH](https://specifications.freedesktop.org/wm/latest-single/) | Window-manager requests, with capabilities and observed outcomes to check |
| [Node permission model](https://nodejs.org/api/permissions.html) | Explicitly not protection against malicious code; trusted worker classification is essential |
| [Wasmtime security](https://docs.wasmtime.dev/security.html), [interruption](https://docs.wasmtime.dev/examples-interrupting-wasm.html) | Restricted runtime building blocks; host functions and resource limits remain our responsibility |
| [Native OAuth](https://www.rfc-editor.org/rfc/rfc8252), [PKCE](https://www.rfc-editor.org/rfc/rfc7636) | Browser authorization and binding against intercepted callbacks |
| [TUF security](https://theupdateframework.io/docs/security/) | Signed update metadata, freshness, rotation and rollback threats |
| [Apple Developer ID](https://developer.apple.com/developer-id/) | Signing/notarization release requirements and operational costs |

The exact breadth of compositor-specific clipboard/window protocols was not established in this research. No claim is made that all GNOME, KDE or wlroots versions share a capability. The implementation backlog explicitly includes that matrix.

## Decisions that are proposals, not sourced facts

GPUI as the preferred renderer; process/crate layout; UI dimensions and keyboard defaults; ranking tiers; database entities; extension trust classes and rollout order; protocol limits; performance budgets; MPL/permissive licensing direction; staffing and calendar estimates; release gates; and optional server architecture are design recommendations. Sources establish constraints and available building blocks, not that these choices have already succeeded.

The next evidence-producing work is Phase 0: build the Rust interaction slice, capture pinned reference scenarios, and publish actual platform/input/accessibility/performance results before committing to the full roadmap.
