# Behavioral reference ledger (F01)

Status: **frozen specification, not a captured reference.** No Raycast reference application,
installer, bundle, account, extension set or capture host was available for this pass. There are
zero runtime observations, zero invented hashes and zero REA Evidence IDs in this directory.

This directory implements F01 ("Freeze reference scope") from
[`outputs/implementation-backlog.md`](../../outputs/implementation-backlog.md) section 7 and the
priority-A acceptance boundaries in
[`outputs/rust-launcher-blueprint.md`](../../outputs/rust-launcher-blueprint.md) section 4. It
defines what will be captured, how each row is tested and how parity is counted. It does **not**
claim that any reference behavior was observed or that any Ansa feature is implemented.

**This is a frozen unknown-only planning baseline.** The schemas accept only `state=unknown`
identity manifests, exactly two platform manifests, and a ledger with no verified captures. The
current validator intentionally rejects a verified/captured manifest and any observation. Enabling
a real capture requires a versioned schema and validator migration; editing these files to look
observed is invalid.

### Acceptance follow-up — 18 November 2026

Local exact-pin validation again passed the ledger checks and all 41 validator
tests. ANS-5 remains **In Progress**, not Done: this session exposes no native
desktop-control/device tools, and no isolated Windows/macOS reference hosts or
verified builds have been established. No runtime capture was attempted.
The planning acceptance checks pass; the open work is the ticket's versioned
behavioral-reference goal and seed-capture/replay test plan, not implementation
of all 100 product workflows. The existing ten-row design audit is not being
relabelled as a runtime replay.

To unblock, provision dedicated synthetic profiles on both reference hosts and
verify their artifact/OS/permission identities under the
[capture procedure](procedures/seed-capture.md). Before importing any capture,
version the schema and validator, including a check that
`observation.evidence_id` itself resolves to runtime evidence tied to that
identity. Then execute S01–S20 and a separate ten-row runtime replay review;
retain the historical unknown manifests and design/source audit unchanged.

## What is frozen

| Property | Value |
|---|---|
| Scenario rows | exactly 100, unique IDs `S01-WIN` … `S50-WIN` and `S01-MAC` … `S50-MAC` |
| Target platforms | Windows and macOS, 50 rows each |
| Priority-A rows | 36 per platform |
| A workflows | launcher, action/navigation, app catalog, file search, calculator, quicklinks, emoji/symbols |
| Required A behaviors | 48, each covered on both platforms at behavior level |
| Seed intents | S01–S20 preserved on both platforms (40 seed-derived rows) |
| Denominator | 100 included, 0 excluded; 72 `specified`, 26 `deferred`, 2 `unsupported` |
| Statuses | 70 specified, 2 documented, 26 deferred, 2 unsupported |
| Reference identity | explicitly unknown for every identity field on both platforms (OS name is only the target label) |
| Observation state | 100/100 `not_observed` |
| Implementation tests | 100 planned manual procedures, 0 executable commands |
| Design/source-baseline audit | 10 rows, explicitly not a runtime or fresh-web claim |

## Files

| Path | Purpose |
|---|---|
| [`scenario-ledger.json`](scenario-ledger.json) | The 100-row checked ledger with inputs, expected observations, evidence references, unknowns, test setup/action/pass/fail and parity status |
| [`schema/scenario-ledger.schema.json`](schema/scenario-ledger.schema.json) | JSON Schema for the ledger, including the no-observation-without-identity rule |
| [`schema/reference-identity.schema.json`](schema/reference-identity.schema.json) | JSON Schema for one explicit-unknown identity manifest |
| [`identity/windows-reference.json`](identity/windows-reference.json) | Windows target: path/version/build/architecture/hash/signature/account/extensions/permissions all `state=unknown`, value `null` |
| [`identity/macos-reference.json`](identity/macos-reference.json) | macOS target with the same explicit-unknown fields |
| [`evidence-catalog.json`](evidence-catalog.json) | Evidence entries and an explicit policy for all five evidence kinds; unavailable kinds have no entries |
| [`coverage-matrix.json`](coverage-matrix.json) | 49 behavior keys (48 required A + planned B currency) with blueprint references |
| [`seeds/seed-intents.json`](seeds/seed-intents.json) | Machine-readable S01–S20 intent, capture question and row mapping |
| [`audits/design-source-baseline-consistency-2026-11-17.json`](audits/design-source-baseline-consistency-2026-11-17.json) | Ten-row design/source-baseline consistency audit with `runtime_claim=false` and `fresh_web_fetch=false` |
| [`procedures/seed-capture.md`](procedures/seed-capture.md) | Seed capture procedure, identity gate, synthetic profile, evidence and cleanup rules |

Tooling lives in [`scripts/reference/`](../../scripts/reference/):

- `validate_reference.py` — schema plus semantic validation, A-coverage, identity/evidence
  consistency, denominator and audit checks.
- `json_schema_lite.py` — checked-in, fail-closed standard-library JSON Schema subset.
- `test_validate_reference.py` — 41 tests that mutate temporary copies, directly exercise the
  schema subset, and accept `--root` for a custom fixture tree.
- `run-validation.ps1` / `run-validation.sh` — cross-platform wrappers.

## Validate

```sh
python3 scripts/reference/validate_reference.py --root .
python3 scripts/reference/test_validate_reference.py
```

or:

```sh
pwsh -File scripts/reference/run-validation.ps1
sh scripts/reference/run-validation.sh
```

The validator prints the ledger counts, A-behavior coverage, denominator, observation count, test
counts and evidence kinds. It exits non-zero on any schema, identity, evidence, coverage, seed or
audit violation. The wrappers pass `--root` to both the validator and the test harness, so a
custom reference tree uses its own fixtures for the negative tests too.

## Behavior-level A coverage

A workflow label alone is not coverage: the ledger names concrete behaviors, and the validator
requires every required behavior to have at least one priority-A row for **both** Windows and
macOS.

| A workflow | Required behaviors | A rows (Windows) | A rows (macOS) |
|---|---|---:|---:|
| Launcher: summon, root search, favorites, aliases, frecency, per-command shortcuts, conflicts, stale results, IME, degraded capability | 10 | 9 | 9 |
| Action/navigation: primary, secondary, searchable panel, nested views, breadcrumbs, toasts, Escape, focus return, back-state restore, consequence clarity | 10 | 6 | 6 |
| App catalog: launch, switch, reveal, install refresh, uninstall refresh, dedupe, missing target | 7 | 4 | 4 |
| File search: names/paths, filters, preview, open/reveal/copy, rename/delete reconciliation, scoped roots/permissions | 6 | 6 | 6 |
| Calculator: arithmetic, precision policy, units/dates | 3 A (+1 planned B currency) | 3 | 3 |
| Quicklinks: URL templates, path targets, argument encoding, preferred opener, aliases/tags, no shell interpolation | 6 | 4 | 4 |
| Emoji/symbols: local search, variants, recent/favorites, copy/paste, accessible names, graphemes | 6 | 4 | 4 |
| **Total** | **48 A** | **36** | **36** |

The 28 rows outside the A baseline are not hidden: 26 are `deferred` and 2 are `unsupported`
(clipboard, notes, window management, scripts, extensions, sync, AI, release update and the
Wayland-origin S20 rows). S20-WIN/S20-MAC stay `unsupported`; the accurate degraded-capability
intent is carried by S24-WIN/S24-MAC, which are priority A.

## Evidence discipline

**Evidence kinds are not interchangeable.** The catalog defines all five kinds:

| Kind | State in this pass |
|---|---|
| `vendor_documentation` | Used — inherited Raycast baseline documentation plus proposed OS/standards supporting citations |
| `source_inspection` | Unavailable — no reference source/SDK was inspected |
| `static_artifact_analysis` | Unavailable — no artifact or hash existed |
| `runtime_observation` | Unavailable — zero launches, probes or recordings |
| `design_proposal` | Used — our blueprint, backlog and keyboard contract, always labelled as a proposal |

A row's confidence is the weakest link across its cited evidence. Design proposals are `low`;
inherited vendor documentation is `medium`; unfetched proposed OS/standards citations are capped at
`low`. No row is `high`. The ledger is 94 `low`, 6 `medium`, 0 `high`.

**Provenance is explicit.** No external page was fetched by this F01 session. Raycast category
claims are inherited from the 2026-11-11 research snapshot in
`outputs/research-and-sources.md` / `work/raycast-features.md`. OS/standards URLs that were not in
that baseline are marked `proposed_unverified` with no retrieval date and low confidence. The
catalog records `fetch_state`, `source_date`, `date_meaning` and `source_locator`; the validator
rejects the ambiguous legacy `retrieved` field, an undeclared in-session fetch, or a proposed
citation above `low`. Duplicate evidence IDs are rejected before indexing, empty/array/null/missing
audits are rejected, and all claimed dates are checked against the real calendar.

**Unknown means unknown.** Each identity manifest records `path`, `version`, `build`,
`architecture`, `artifact_sha256`, `signature_identity`, `signature_status`, `account.tier`,
`account.signed_in`, `extensions.enabled` and five permission states as `state=unknown`,
`value=null`, with a reason and a null evidence ID. The validator fixes exactly two platform
manifests, rejects any known value in this baseline and rejects any 64-hex string that is not a
verified artifact hash. A future verified manifest belongs in a new schema version, not this one.

**Observations are gated, but unreachable here.** A future verified-schema row may set `observation.state=observed` only
when every identity field is known, an evidence ID and recording exist, and the manifest records at
least one runtime observation. The current unknown-only manifests make that gate fail closed. A
future capture needs a versioned schema/validator migration; this validator does not accept a
verified capture.

**No REA IDs.** The REA tools were not exposed in this work. No `REA-…` identifier may appear
unless a tool actually returned it; the validator scans for invented IDs.

**Synthetic only.** Every fixture names
`ansa-reference-test-profile` / `dedicated_test_profile` / `synthetic_only` and is marked
`specified_not_created`. The procedure forbids personal files, clipboard contents, accounts,
extensions or network access during ordinary A-baseline capture.

## Status and bucket language

| Status | Allowed here | Planning bucket |
|---|---|---|
| `documented` | Vendor/source baseline establishes the category; exact rule still needs capture (S03 ranking only) | `specified` |
| `specified` | Acceptance criterion and manual test exist; no reference capture or Ansa implementation | `specified` |
| `deferred` | Priority B/C/D behavior retained outside the A baseline | `deferred` |
| `unsupported` | Wayland-origin scope outside the Windows/macOS F01 targets | `unsupported` |

`captured`, `implemented`, `tested` and the bucket `supported` are **not** permitted by this
frozen schema. A future verified version may introduce them only for actual implemented/tested rows
with capture evidence. In particular, a row cannot currently use `tested`; that is an intentional
unreachable future condition, not a current validator bug. Calling 72 specified planning rows
"supported" would invite a false parity claim.

The reported parity fraction must always use 100 as the denominator. Do not create a
"specified-only" percentage by dropping deferred or unsupported rows; preserve the second
platform scorecard described in the blueprint.

## Known gaps

- No reference application, so no identity, observation, screenshot, trace, hash or REA ID.
- All 100 implementation tests are `planned` manual procedures with setup, action, pass and fail
  criteria. There are zero executable scenario tests in this planning ticket; no command points at
  a nonexistent Rust test.
- The validator is an unknown-only baseline. It fixes exactly two unknown manifests and rejects a
  verified capture; a future verified capture requires a versioned schema and validator.
- The positive validator and tests have run on Windows with Python 3.12.10. The POSIX wrapper and
  macOS behavior are not exercised on this host.
- A `reference` CI job is configured for Windows, macOS and Ubuntu (validator plus tests). It has
  not been executed remotely from this worktree; the local Windows results are recorded in
  [`docs/ans-5-validation.md`](../ans-5-validation.md).
- The ledger freezes acceptance, not reference truth. A future capture must introduce a new
  schema/ledger version and must not edit the unknown manifests to look observed.
