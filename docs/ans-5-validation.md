# ANS-5 / F01 validation record — 2026-11-17

Status: **specification validation passed; no reference runtime exists.** This record covers the
F01 reference scope implementation in the shared worktree. It does not claim a Raycast observation,
a captured reference build, a hash, a signature or an REA Evidence ID.

## Scope delivered

| Requirement | Deliverable |
|---|---|
| Checked schema | [`docs/reference/schema/scenario-ledger.schema.json`](reference/schema/scenario-ledger.schema.json) plus an identity-manifest schema; both exercised by the validator |
| Exactly 100 unique scenario IDs | `S01-WIN`…`S50-WIN` and `S01-MAC`…`S50-MAC` in [`scenario-ledger.json`](reference/scenario-ledger.json) |
| Every A workflow on Windows/macOS | 48 required behavior keys; all 48 covered on each OS at behavior level |
| Preserve S01–S20 intent | 20 seeds × 2 OSes = 40 seed rows; machine-readable intents and capture procedure |
| Explicit unknown reference identity | Path, version, build, OS build, architecture, SHA-256, signature, account, extensions and permissions are `state=unknown`, `value=null` in both manifests |
| Distinct evidence kinds/confidence/unknowns | Five-kind policy; only vendor documentation and design proposals are used; row confidence is the weakest cited evidence; every row has explicit unknowns |
| Synthetic dedicated profile only | Every fixture requires `ansa-reference-test-profile`, `dedicated_test_profile`, `synthetic_only`; state is `specified_not_created` |
| Concrete test per row | Setup, action, numbered manual steps, pass criteria and fail criteria on all 100 rows; no executable command |
| Unsupported/deferred retained | 26 deferred + 2 unsupported rows remain included in the 100-row denominator; the other 72 are the planning bucket `specified` |
| Machine validation + negative tests | Python 3 stdlib validator and 41 tests; no Rust dependency changes |
| Ten-row design/source-baseline audit | 10 representative rows, `runtime_claim=false`, `fresh_web_fetch=false`, gaps and next capture steps recorded |
| Provenance | Inherited documentation baseline distinguished from unfetched proposed citations; no external page fetched this session |
| Cross-platform scripts | PowerShell and POSIX wrappers; Python 3.12.10 used locally |
| No commits | No commit was made |

## Commands and exact results

Local host: Windows, Python 3.12.10 from the provided embedded runtime. The `python` alias is
unavailable; these commands used the interpreter path directly.

```powershell
& 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe' `
  scripts/reference/validate_reference.py --root .
```

```text
Ansa F01 reference validation
  ledger: 100 rows (windows 50, macos 50)
  priority A rows: windows 36, macos 36
  required A behaviors: 48 (covered windows 48, macos 48)
  seeds: 20/20, seed rows: 40/40, audit: 10/10 design/source-baseline rows
  denominator: included 100, buckets {'deferred': 26, 'specified': 72, 'unsupported': 2}
  statuses: {'deferred': 26, 'documented': 2, 'specified': 70, 'unsupported': 2}
  observations: 0 observed, 100 not observed
  implementation tests: {'manual': 100} / {'planned': 100}
  evidence kinds: {'design_proposal': 5, 'vendor_documentation': 22}
PASS: schema, identity boundaries, evidence, A-coverage, seeds and audit are consistent
```

```powershell
& 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe' `
  scripts/reference/test_validate_reference.py
```

```text
Ran 41 tests in 1.783s
OK
```

(Timing varies; earlier runs completed in 1.2–1.4s.)

The suite is one baseline test plus 40 checks. `--format json` also passed and reported
`ok=True`, `errors=0`, `rows=100`, `required_a_behaviors=48`.

The PowerShell wrapper passed with the same validator and test results:

```powershell
pwsh -NoProfile -File scripts/reference/run-validation.ps1 `
  -Python 'C:\Users\abhir\AppData\Local\Temp\opencode\python-embed\python.exe'
```

The POSIX wrapper [`scripts/reference/run-validation.sh`](../scripts/reference/run-validation.sh)
could not be executed because this Windows host has no `sh`/`bash`; it is a thin `set -eu` wrapper
over the same two Python commands. This remains an untested gap, not a claimed result.

The PowerShell wrapper also passed with `-Root` pointing at a copied reference tree. Removing the
audit from that custom tree made the baseline test fail, confirming that the test harness uses the
custom root rather than silently falling back to the repository copy.

## A-workflow coverage

Coverage is enforced per behavior and per OS, not by counting rows in a feature category.

| A workflow | Required behaviors | Windows A rows | macOS A rows | Behavior coverage |
|---|---|---:|---:|---|
| Launcher | 10 | 9 | 9 | 10/10 each |
| Action/navigation | 10 | 6 | 6 | 10/10 each |
| App catalog | 7 | 4 | 4 | 7/7 each |
| File search | 6 | 6 | 6 | 6/6 each |
| Calculator | 3 A (+1 planned B currency) | 3 | 3 | 3/3 each |
| Quicklinks | 6 | 4 | 4 | 6/6 each |
| Emoji/symbols | 6 | 4 | 4 | 6/6 each |
| **Total** | **48 A** | **36** | **36** | **48/48 each** |

The behavior matrix includes the requested A details: favorites, aliases, frecency, per-command
shortcuts, conflicts; primary/secondary actions, searchable panels, nesting, breadcrumbs, toasts,
Escape and focus return; app launch/switch/reveal/install/uninstall/dedupe/missing target; file
names/paths/filters/preview/open/reveal/copy/rename/delete/scoped roots; calculator arithmetic,
precision and units/dates; quicklink URL/path/arguments/opener/aliases/tags/no-shell; emoji
variants/recent/favorite/copy/paste/names/graphemes.

## Identity and evidence boundary

Both identity manifests record every required field explicitly unknown:

- application path, version, build, architecture, artifact SHA-256, signature identity and
  signature status;
- OS version, build and architecture (OS name only is known as the target platform);
- account tier and signed-in state;
- enabled extension set;
- Accessibility, Screen Recording, Automation, Full Disk Access and Input Monitoring permission
  states.

Reasons and null evidence IDs are present for each. The validator fixes exactly two platform
manifests, rejects any known value in this frozen unknown-only baseline, any 64-hex string that is
not a verified artifact hash, and any `REA-…` ID not returned by a manifest. No observation exists:
all 100 rows are `not_observed` with null actual text, evidence ID, recording and date. The
observation gate is intentionally unreachable until a versioned schema/validator migration; the
current validator does not accept a verified capture.

Evidence kinds actually used: `vendor_documentation` (22 catalog entries) and `design_proposal`
(5 entries). `source_inspection`, `static_artifact_analysis` and `runtime_observation` are marked
unavailable with no entries. A row cannot cite runtime evidence unless the identity is complete and
the row is observed. Row confidence is 94 `low`, 6 `medium` and 0 `high`; the medium rows cite only
inherited vendor documentation, while every row that also relies on a design proposal or an
unfetched proposed citation is `low`.

### Source provenance

No external page was fetched by this F01 session. The catalog records this explicitly:

- **17 inherited entries** (`inherited_not_refetched`) come from the 2026-11-11 research snapshot
  in `outputs/research-and-sources.md` / `work/raycast-features.md`. Their `source_date` is the
  snapshot date, not a retrieval by this session.
- **5 proposed entries** (`proposed_unverified`: OS permissions, input, focus, accessibility and
  power citations) were suggested by platform knowledge but were not present as fetched sources in
  the baseline. They have `source_date=null`, no verified locator, and are capped at `low`.
- **5 local design entries** (`local_repository_source`) point at the blueprint, backlog and
  keyboard contract and are not external evidence.
- The ambiguous legacy `retrieved` field is rejected; `fetch_state`, `date_meaning` and
  `source_locator` are required. An undeclared `fetched_this_session` claim fails validation.
- Calendar dates are validated semantically with `datetime.date.fromisoformat` in the ledger,
  manifests, catalog provenance/source dates and audit. The schema regex alone is not sufficient;
  impossible dates such as `2026-99-99` fail the semantic check.

| Fetch state | Sources |
|---|---|
| `inherited_not_refetched` (17) | `EV-DOC-MANUAL`, `EV-DOC-QUICKSTART`, `EV-DOC-WINDOWS`, `EV-DOC-FILE-SEARCH`, `EV-DOC-QUICKLINKS`, `EV-DOC-CLIPBOARD`, `EV-DOC-SNIPPETS`, `EV-DOC-NOTES`, `EV-DOC-WINDOW`, `EV-DOC-SCRIPTS`, `EV-DOC-EXTENSIONS`, `EV-DOC-SYNC`, `EV-DOC-AI`, `EV-DOC-OS-HOTKEY`, `EV-DOC-OAUTH`, `EV-DOC-TUF`, `EV-DOC-WAYLAND` |
| `proposed_unverified` (5) | `EV-DOC-OS-PERMISSIONS`, `EV-DOC-OS-INPUT`, `EV-DOC-OS-FOCUS`, `EV-DOC-OS-A11Y`, `EV-DOC-OS-POWER` |
| `local_repository_source` (5) | `EV-DESIGN-BLUEPRINT`, `EV-DESIGN-BACKLOG`, `EV-DESIGN-KEYBOARD`, `EV-DESIGN-PARITY`, `EV-DESIGN-CAPTURE` |
| `fetched_this_session` | none |

## Ten-row design/source-baseline consistency audit

[`design-source-baseline-consistency-2026-11-17.json`](reference/audits/design-source-baseline-consistency-2026-11-17.json)
audits ten rows against the inherited design and source baseline. It is explicitly
`runtime_claim=false` and `fresh_web_fetch=false`, and every row is `observation_state=not_observed`.
No page was fetched and no reference build was run.

| Row | Workflow | Baseline result | Main gap |
|---|---|---|---|
| S01-WIN | Launcher | design-consistent | No Windows build/shortcut/focus capture |
| S05-MAC | Launcher | design-consistent | No physical macOS IME run |
| S04-WIN | Action/navigation | design-consistent | Escape layering and parent restore unobserved |
| S29-MAC | Action/navigation | design-consistent | No action/toast capture |
| S31-WIN | App catalog | design-consistent | No launch/switch identity trace |
| S34-MAC | App catalog | design-consistent | No install/uninstall fixture |
| S38-WIN | File search | design-consistent | No symlink/permission indexing run |
| S41-MAC | Calculator | design-consistent | No unit/date/DST capture |
| S45-WIN | Quicklinks | design-consistent | No missing-opener experiment |
| S49-MAC | Emoji/symbols | design-consistent | No VoiceOver/accessibility-tree capture |

"Design-consistent" means the row's claims stay within the cited inherited/design evidence and its
gaps are explicit. It does not mean the reference product behaves that way or that the cited URL
was recently fetched.

Citation-only correction: S38-WIN/S38-MAC now explicitly cite `EV-DESIGN-BLUEPRINT` for the
bounded cycle/denial acceptance. The vendor sources establish scoped roots and permission
semantics, not the reference's cycle behavior. A separate Sol spot-check of three vendor pages
(Quickstart, File Search, Quicklinks) is recorded in Sol's validation artifact and is not merged
into this catalog as an in-session fetch.

## Negative tests exercised

The 41-test suite covers one baseline and these 40 checks:

- **Structure/count:** duplicate scenario ID, missing row/count, platform imbalance, invalid enum.
- **Denominator:** removing deferred rows, excluding a deferred row, removing unsupported rows,
  rejecting the bucket `supported`, rejecting the unreachable `tested` status.
- **A coverage:** removing a behavior from one OS only, A row with no behavior.
- **Evidence/identity:** runtime evidence with unknown identity, observed row with unknown identity,
  observed row without catalog `runtime_observation` evidence, actual text on a `not_observed` row,
  invented 64-hex hash, value on an unknown identity field, missing permission, invented `REA-…` ID,
  unknown evidence reference, inflated confidence.
- **Provenance/catalog:** proposed citation above `low`, undeclared `fetched_this_session` source,
  duplicate evidence ID, invalid calendar dates in ledger/manifest/catalog/audit.
- **Schema subset:** unknown keyword, unsupported `format`, invalid/non-object schema, `$ref`
  siblings, and `uniqueItems` numeric equality (`1` vs `1.0` duplicate, `1` vs `true` distinct).
- **Audit shape:** empty object, empty array, null and missing file are rejected rather than
  silently accepted.
- **Tests/seeds/audit:** executable test without a command/with planned state, passed test without
  observation, missing seed row, seed backlink mismatch, audit runtime claim, nine-row audit, audit
  evidence not used by its ledger row.

All 41 passed. The mutation tests use temporary copies; the working ledger is untouched.

## Gaps and non-claims

1. **No reference application was available.** There are zero runtime observations, recordings,
   hashes, signatures, account/extension values or REA Evidence IDs. The ledger is a design and
   capture specification.
2. **All 100 implementation tests are `planned` manual procedures.** There are no executable
   scenario commands and no command points at a nonexistent Rust test. Setup/action/pass/fail are
   present, but none has run.
3. **This is a frozen unknown-only planning baseline.** The validator fixes two unknown manifests
   and rejects verified/captured manifests and observations. A future capture requires a versioned
   schema and validator migration; it must not edit these files to look observed. `tested`,
   `implemented`, `captured` and the `supported` bucket are intentionally unreachable in the
   current enum, not a current validator bug.
4. **No external page was fetched this session.** Seventeen documentation entries are inherited
   from the 2026-11-11 snapshot, five OS/standards citations are proposed/unverified at `low`, and
   five are local design documents. No retrieval date is claimed for the proposed citations.
5. **macOS was not exercised locally.** Both platform rows exist and validate structurally; only
   the Windows validator run is recorded.
6. **The POSIX wrapper is untested** on this host because no shell is installed.
7. **S03 exact ranking remains `documented`**, not specified: the public baseline establishes alias
   and frequency categories, not the exact scoring/tie-break.
8. **CI is configured but not executed remotely here.** The `reference` job runs the validator and
   tests on windows-latest, macos-latest and ubuntu-latest. Only the local Windows run is recorded
   in this artifact; no remote CI result is claimed.
9. **The design/source-baseline audit is not a runtime or fresh-web claim.** A future capture must
   add a versioned verified manifest and a separate runtime evidence record; it must not relabel
   this audit.

## CI integration command

The repository's `reference` CI job invokes both commands on a Windows/macOS/Ubuntu matrix. It is
configured but was not executed remotely from this worktree.

Portable POSIX CI step:

```sh
python3 scripts/reference/validate_reference.py --root . --format json
python3 scripts/reference/test_validate_reference.py
```

One-line gate:

```sh
python3 scripts/reference/validate_reference.py --root . && python3 scripts/reference/test_validate_reference.py
```

Windows CI step:

```powershell
pwsh -NoProfile -File scripts/reference/run-validation.ps1
```

The validator exits non-zero on schema violations, wrong platform counts, missing/duplicate IDs,
A-behavior gaps on either OS, dropped denominator rows, the disallowed `supported` bucket,
unknown-identity observations, invented hashes/REA IDs, seed backlink errors, audit runtime/fresh-web
claims, or provenance/confidence inflation. It does not require a Rust build and adds no dependency.
