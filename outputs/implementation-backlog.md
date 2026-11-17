# Implementation backlog and validation plan

Companion to `rust-launcher-blueprint.md`, 11 November 2026. These tickets make the architecture reviewable and implementable. No implementation or benchmark has been performed. Priority A/B/C/D and calendar phases follow the main blueprint.

## 1. The first four weeks

| Week | Work | Concrete output | Decision |
|---|---|---|---|
| 1 | Reference capture design, GPUI pin/build on three OSes, initial text input, existing-project evaluation | Version manifest, three runnable signed-or-development artifacts, initial scenario ledger, build-versus-contribute ADR | Can the selected GPUI revision build and expose necessary host hooks? |
| 2 | Native summon/focus, 10k-result UI, composition, screen readers, primary/secondary actions | Videos/traces, accessibility tree snapshots, issue list per platform | Are there foundation blockers? |
| 3 | Clipboard insertion slice, multiline editor, process-supervised mock extension, scoped index proof | End-to-end scenario results, crash/focus tests, memory/power baseline | Can the user safely perform the main workflow? |
| 4 | Resolve bounded blockers or compare Slint, updater/install proof, threat-model review, schedule revision | Framework ADR, tested OS/backend matrix, dependency pins, committed 1.0 scope | Proceed, narrow declared support, or pivot before feature expansion |

Run source research, host scaffolding and widget work in parallel with named owners. A screen-reader or text-input blocker is not relegated to final polish. If the alternative framework needs another two-week comparison, revise the schedule openly.

## 2. Tickets for the foundation

Each ticket closes with code, a user-visible result, a relevant test and a short support/limitation note. Avoid broad tickets such as “implement extensions” without a bounded contract.

| ID | Ticket | Dependencies | Definition of done |
|---|---|---|---|
| F01 | Freeze reference scope | None | Record reference app/OS/build/hash when available; classify 100 behavior scenarios by feature, OS and priority |
| F02 | Evaluate reuse | None | Inspect Gauntlet and relevant open projects; compare architecture, current tests, license, maintainers and delta; record contribute/fork/greenfield decision |
| F03 | Establish workspace/CI | None | Rust toolchain and dependency locks; format/lint/unit jobs; all three desktop builds; dependency/license inventory |
| F04 | GPUI host slice | F03 | Show/hide, monitor positioning, native theme and shutdown work on target OSes; no unconditional focus assumptions |
| F05 | Shared input widget | F04 | Editing, selection, composition, grapheme boundaries, UTF-16 conversion, clipboard and accessible text navigation tested |
| F06 | Navigation/action state | F04–F05 | Typed view stack; correct Escape hierarchy; stable focus; no hidden callback can act on the wrong view |
| F07 | Native hotkey adapters | F04 | Registration/conflict/rebinding/lifecycle tests; CLI fallback where global shortcut support is absent |
| F08 | Accessibility spike | F05–F06 | Search, list, action panel and form completed using each target screen reader; missing semantics tracked as blocking |
| F09 | Performance harness | F04–F06 | Named hardware/corpora; raw timing traces and percentiles; full process-tree memory and idle CPU captured |
| F10 | Architecture gate | F01–F09 | GPUI accepted with explicit support matrix or comparison/pivot decision; no unowned critical risks |

## 3. Daily launcher work packages

| ID | Ticket | Dependencies | Definition of done |
|---|---|---|---|
| L01 | Command/action registry | F10 | Stable namespaced IDs, parameter schemas, requirements, action preparation/execution and error taxonomy |
| L02 | App discovery | L01 | Per-platform identities, icons, updates, deduplication and launch/switch behavior; uninstall during action handled |
| L03 | Search coordinator | L01 | Query revisions, cancellation, provider deadlines/quotas, stable selection and partial-result semantics |
| L04 | Ranking baseline | L02–L03 | Golden relevance corpus; exact/prefix/fuzzy tiers; bounded history boosts; reset/private mode |
| L05 | Settings/storage foundation | L01 | Transactional migrations, revisioned settings, backup/restore proof, hotkey/alias validation |
| L06 | Quicklinks | L05 | Structured argument parser, URL encoding, preferred opener, import/export and malicious-input fixtures |
| L07 | Local calculator | L03 | Bounded parser, precision/unit/date policy and corpus; explicit errors; network-free ordinary math |
| L08 | Emoji/symbol search | L03 | Licensed/versioned data, graphemes/variants, accessible names, copy and insertion fallback |
| L09 | Onboarding | L02/L05/F07 | User can launch an app without account/optional permissions; disabled integrations explain next step |
| L10 | Daily-driver alpha | L01–L09 | Two weeks of team dogfooding; fixed keyboard/focus regressions; installation and uninstall documented |

Calculator follow-up: currencies require data-source/license selection, refresh policy, stale-rate indication and tests. Natural-language parsing is a specified grammar; do not imply every phrase accepted by another calculator will work immediately.

## 4. Local productivity and native integration

| ID | Ticket | Dependencies | Definition of done |
|---|---|---|---|
| P01 | Scoped file metadata index | L03/L05 | Roots, exclusions, baseline scan, live changes, overflow recovery, symlink/permission tests and progress |
| P02 | File query/preview | P01 | Bounded candidates, filters, cached preview, open/reveal, stale/offline-file behavior |
| P03 | Vault and secrets | L05 | OS key storage, Linux fallback policy, encrypted payloads, locked-vault UX, backup recovery |
| P04 | Clipboard capture | P03 | Supported format adapters, sensitive flags, quota/retention, pause, source-confidence handling |
| P05 | Paste transaction | F07/P04 | Target generation/focus checks, copy-only fallback, no overwrite of a newer clipboard value |
| P06 | Clipboard UI/search | P04–P05 | Format filters, pins/delete, safe preview, memory-only plaintext index and wipe behavior |
| P07 | Snippet templates | P05/L05 | Typed placeholders, preview/manual paste, import/export; literal/template distinction |
| P08 | Window geometry | F04/L01 | Work areas, mixed DPI, minimum-size/clamping, restore and recycled-handle tests |
| P09 | Markdown notes | F05/P03 | Durable edits, basic formatting/search, recovery, undo, Markdown/attachment export |
| P10 | Native capability UI | P01–P09 | Permission revocation, unsupported compositor/integrity case and temporarily unavailable targets have useful states |
| P11 | Local productivity beta | P01–P10 | Crash/storage/clipboard/window tests pass on advertised combinations; no implicit capability promises |

Automatic text expansion, rich Notes editing and saved multi-window layouts are separate advanced tickets. Do not quietly equate manual snippets or source Markdown editing with full reference parity.

## 5. Extension platform

| ID | Ticket | Dependencies | Definition of done |
|---|---|---|---|
| E01 | Protocol/manifest schema | L01/F06 | Version negotiation, trust class, identity, command metadata, permissions and validation limits |
| E02 | Rust view renderer | E01 | List/Grid/Detail/Form/ActionPanel fixtures, stable IDs, virtualized accessibility and malformed-node rejection |
| E03 | Process supervisor | E01 | Authenticated worker channel, bounded logs/queues, cancellation, crash quarantine, descendant cleanup |
| E04 | Node/React reconciler | E02–E03 | Keyed tree patches, callback routing, navigation, errors and React-version fixtures without a browser renderer |
| E05 | Service API layer | E03/P03 | Preferences/storage, notifications, HTTP, OAuth, clipboard and file APIs with scoped broker identities |
| E06 | Developer tooling | E04–E05 | CLI scaffolding, build, local install, hot reload, docs, source maps and unsupported-API reporting |
| E07 | Registry prototype | E01/E03 | Immutable hashes, signed metadata, source/licenses/trust shown, install/pin/update/revoke/rollback |
| E08 | 50-extension corpus | E04–E07 | Stratified exact package versions, fixtures, OS-specific results and transparent compatibility denominator |
| E09 | Script commands | E03/L01 | Metadata parser, explicit interpreter, argv handling, output modes, no accidental shell evaluation |
| E10 | Security review | E01–E09 | Threat model and abuse tests; trusted Node warning matches actual confinement; broker bugs fixed |
| E11 | Restricted Wasm tier | E10 | Separate SDK capability surface, resource accounting, forbidden ambient access and hostile-module tests |

The initial corpus should contain approximately 10 simple UI utilities, 10 HTTP/OAuth integrations, 8 form/file operations, 6 clipboard/snippet tools, 6 background/tray tools, 5 subprocess/native helpers and 5 AI/resource tools. Include expected failures; otherwise the corpus overstates compatibility. Pick actual packages only after license and source review.

## 6. Production and later tracks

| Track | Required deliverables | Release gate |
|---|---|---|
| Rich editor | Document model, composition-safe undo, Markdown normalization, tasks/links/attachments, accessible navigation | Long notes, multilingual edits and crash restoration corpus passes |
| Desktop advanced | Automatic expansion, layout editor, menu search, screenshot/OCR, focus/key remap | Each adapter tested; unavailable cases explicit; no stuck modifiers |
| AI | Provider protocol, streaming, context preview, local option, chat/export, dictation | Network/auth/limits/cancel recover; capture and data destination truthful |
| Agents/tools | Agent config, skills, projects, memory, MCP, approval policy | Prompt injection cannot grant privileges; concrete side effects bound to grants |
| Scheduling | Durable job ledger, leases, time zones, retry policy, missed-run handling | Duplicate/uncertain actions handled; no false exactly-once promise |
| Sync | E2EE specification, enrollment/recovery, outbox/conflicts/tombstones, self-host server | External review and recovery/revocation/replay drills |
| Teams | Scope boundaries, roles, shared registry, key membership policy, audits | No personal-to-team leakage; revocation and offboarding tested |
| Release engineering | Signing, updater metadata, rollback, clean installers, SBOM, provenance | Tamper/interruption/low-disk/old-schema cases pass |
| Support readiness | Troubleshooting, diagnostic bundle, SECURITY.md, contributors, release ownership | A new tester can install, recover and report without maintainer intervention |

## 7. Behavioral reference and REA investigation plan

The goal is a black-box behavior specification supported by public APIs. Binary work is a focused source of evidence where needed, not a requirement to reproduce private implementation choices.

For each macOS and Windows reference target, record exact path, application version, OS/build, architecture, artifact hash, signature identity if available, account tier, enabled extensions and permission state. Bind observations to that identity. Capture user data only through synthetic fixtures and a dedicated test profile.

If REA becomes available, first discover its actual tool schema. Open the supplied/clearly resolved package with the correct route; inspect a summary inventory and relevant resources. Use focused code inspection only to answer a specific unresolved behavior question. Runtime probes require a declared action/lifecycle and cannot be inferred from static strings. Preserve returned Evidence IDs, source hashes, limitations and cleanup results. Do not invent IDs for ordinary browser or shell research.

Example evidence ledger fields:

```text
scenario_id, feature, priority, target_build, artifact_sha256,
os_build, permissions, fixture_id, inputs, expected_observation,
actual_observation, recording_or_evidence_reference,
evidence_kind, confidence, unknowns, implementation_test, parity_status
```

Evidence kinds: vendor documentation, source inspection, static artifact analysis, runtime observation, design proposal. They are not interchangeable. Statuses: documented, captured, specified, implemented, tested, unsupported, deferred. “Not observed in this test” does not mean “the product never does this.”

Suggested initial capture set:

| Scenario | What to record | What our test asserts |
|---|---|---|
| S01 summon from an editor | Monitor/focus/query state | Input is ready; target identity preserved |
| S02 rapid typing/backspace | Ordering/loading/selection | Old queries never overwrite new results |
| S03 exact alias versus frequent item | Ranking | Documented product rule is deterministic |
| S04 child view -> actions -> Escape | Stack/selection/scroll | Expected layer closes; parent state restored |
| S05 IME confirm and cancel | Composition/caret/actions | No accidental execution or dismissal |
| S06 copy while clipboard chooser is open | Clipboard generations | New user copy is not overwritten |
| S07 paste after target exits | Error/fallback | No paste into an unrelated window |
| S08 permission revoked mid-command | State and message | Safe failure and actionable recovery |
| S09 window moved to another DPI monitor | Geometry/restore | Bounds remain valid and proportional |
| S10 file renamed/deleted during search | Result/action | Stale item fails clearly or refreshes |
| S11 local note crash during save | Last acknowledged edit | Durable content recovered |
| S12 extension crash/infinite loop | UI and worker lifetime | Core remains responsive; worker quarantined |
| S13 OAuth denial/token expiry | User flow/secret handling | Retry/re-authorize without token leakage |
| S14 script args with spaces/metacharacters | Actual argv | Arguments remain literal values |
| S15 sleep/wake and locked session | Hotkeys/jobs/capture | No unexpected input or duplicate effects |
| S16 interrupted update | Installation state | Previous version or verified new version starts |
| S17 offline concurrent sync edits | Conflict/deletion behavior | No silent data loss |
| S18 revoked sync device | Future reads/writes | New epoch access denied; old-data limit stated |
| S19 AI tool with injected instructions | Requested authority | Data cannot expand permissions |
| S20 Wayland unavailable capability | Command/fallback UI | Accurate unsupported/degraded state |

Extend these into the 100-scenario ledger by varying OS, keyboard layout, permission state, data format and error timing. Capture timings for understanding, but do not treat a single recording as a reliable latency benchmark.

## 8. Risk register and stop conditions

| Risk | Early signal | Mitigation / decision |
|---|---|---|
| GPUI text/accessibility gaps | Basic screen-reader/IME slice fails | Bounded fixes, Slint comparison, revise support before feature expansion |
| Framework maintenance burden | Persistent wide fork or frequent broken pins | Own adapter layer; upstream fixes; assigned maintainer; reconsider framework |
| Wayland expectation mismatch | Core integration unavailable on target compositor | Publish capability matrix; desktop shortcut/copy-only fallbacks; no hidden input-device workaround |
| Compatibility explosion | Corpus dominated by native/private dependencies | Source-port tools, verified subsets, first-party integrations; no blanket compatibility claim |
| Node trust confusion | UI implies capabilities constrain arbitrary Node code | Separate trust classes and install language; verify OS confinement independently |
| Rich editor scope overrun | Selection/undo/input regressions proliferate | Ship honest Markdown baseline; dedicate editor owner; avoid copying unvetted code |
| Unsafe data persistence | Plaintext appears in cache/log/index/backups | End-to-end data map, secret fixtures, deletion/encryption tests before beta |
| Update/recovery failure | Migration blocks rollback or interrupted install bricks app | Snapshot strategy, compatibility checks, staged rollout and recovery drills |
| Scope outgrows team | Multiple unfinished platforms and no daily dogfooding | Protect core release; postpone C/D work, publish smaller committed scope |
| Unsustainable free services | Hosting/inference spend grows with users | Local/BYOK/self-host modes; optional funded hosting; no unlimited subsidy promise |

The first release is approved on demonstrated behavior and recovery, not on the number of menu items implemented. Every shipped platform needs an accountable maintainer and a real-machine test path.
