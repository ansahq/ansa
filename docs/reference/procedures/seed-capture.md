# Seed capture procedure S01–S20

This procedure turns the backlog's initial capture set into evidence without inventing anything.
It applies to a future session where a real, frozen reference build is available. **It has not been
run.** No reference identity, observation, recording, hash or REA ID exists today.

The machine-readable intent is in [`../seeds/seed-intents.json`](../seeds/seed-intents.json). Every
seed maps to one Windows and one macOS row in [`../scenario-ledger.json`](../scenario-ledger.json).

## 1. Identity gate — do not observe before this is complete

Record a **new dated manifest** for each target. Do not edit the existing
`ref-win-unknown-2026-11` / `ref-mac-unknown-2026-11` manifests to look captured.

| Field | Required before launch |
|---|---|
| Application path | Absolute bundle/executable path |
| Version and build | Product version plus platform build/channel |
| OS version and build | Exact host OS build, update level, display/session context |
| Architecture | Artifact and OS architecture (`arm64`, `x86_64`, …) |
| Artifact SHA-256 | Hash of the exact installed artifact or installer the build came from |
| Signature identity | Signer/team ID and verification status, or an explicit "unsigned" finding with evidence |
| Account tier | Free/paid/team/enterprise and signed-in state |
| Enabled extensions | Exact set, or explicitly "none"; disabled/unknown is not the same |
| Permissions | Accessibility, screen recording, automation, full disk access, input monitoring — each `granted`, `denied` or `unknown` |
| Capture host | Machine identity class, keyboard layout, input sources, display/scale, locale/time zone |

If any of these is missing, **stop**. A capture without identity is not a ledger observation. A
credential, token, license key or personal account detail must never be written into the manifest.

The local unknown manifests already enumerate these fields with `state=unknown` and `value=null`.
A future manifest uses `state=known`, the verified value and the evidence ID that proves it.

## 2. Dedicated synthetic profile only

- Create a fresh profile named `ansa-reference-test-profile`.
- Use a disposable account alias if the product requires sign-in. Do not use a personal account.
- Do not import a personal profile, settings, clipboard, snippets, notes, history, extensions or
  browser session.
- Generate fixtures from the ledger inputs. Every fixture ID is prefixed `FX-`.
- Keep the machine offline except for an explicitly declared fixture endpoint; ordinary A-baseline
  capture must work offline.
- Record the input source, keyboard layout, locale, display scale and assistive technology before
  the first step.

## 3. Record per row

Use the ledger's fields. At minimum:

```text
scenario_id, feature, priority, target_build, artifact_sha256,
os_build, permissions, fixture_id, inputs, expected_observation,
actual_observation, recording_or_evidence_reference,
evidence_kind, confidence, unknowns, implementation_test, parity_status
```

Rules:

- `actual_observation` is what happened, not what the documentation predicts.
- A screenshot/video/trace must have a stable path or identifier. Do not invent one.
- `evidence_kind` is exactly one of `vendor_documentation`, `source_inspection`,
  `static_artifact_analysis`, `runtime_observation`, `design_proposal`.
- A runtime row needs the frozen identity, recording and permission state. "Not observed in this
  test" is a limitation, not proof the product never does it.
- Preserve failures and partial results. A timeout, denial or crash is evidence.
- Do not record user data, secrets, tokens or unrelated windows.

## 4. Seed sequence

Capture in an order that freezes identity, then tests the keyboard shell, then the feature rows.
Run both Windows and macOS rows; do not assume parity from one OS.

| Seed | Action | Record | Ledger rows |
|---|---|---|---|
| S01 | Focus a generated editor document, summon with the actual configured shortcut, type, dismiss | Focus target before/after, query state, document unchanged | `S01-WIN`, `S01-MAC` |
| S02 | Type/backspace rapidly while a slow provider is in flight | Revision ordering, stale-result behavior, selection movement | `S02-WIN`, `S02-MAC` |
| S03 | Compete an exact alias against a frequent item, cold and warm history | Winner, frequency window, tie-break, full ordered list | `S03-WIN`, `S03-MAC` |
| S04 | Open child view → action panel → Escape twice | Which layer closes per Escape, restored query/selection/scroll | `S04-WIN`, `S04-MAC` |
| S05 | Use a physical IME to compose, convert, confirm; then cancel with Escape | Marked-text ownership, key effects, caret/selection, execution/dismissal | `S05-WIN`, `S05-MAC` |
| S06 | Select an older clipboard item, copy a new value externally, return and paste | Clipboard generation, stale-selection handling, whether the new copy survives | `S06-WIN`, `S06-MAC` |
| S07 | Start a paste, close the target before insertion, observe fallback | Error/copy-only state, foreground window, clipboard content | `S07-WIN`, `S07-MAC` |
| S08 | Revoke read access to a scoped root during search/action | Failure message, cache invalidation, recovery path, data exposure | `S08-WIN`, `S08-MAC` |
| S09 | Move a target across mixed-DPI monitors and disconnect one | Bounds, scale, minimum size, restore and on-screen result | `S09-WIN`, `S09-MAC` |
| S10 | Rename and delete indexed files after selection | Reconciliation, stale-action error, exact path opened | `S10-WIN`, `S10-MAC` |
| S11 | Kill the process during a note save, then relaunch | Last acknowledged edit, recovery/conflict state | `S11-WIN`, `S11-MAC` |
| S12 | Trigger a fixture extension infinite loop and crash | Core responsiveness, quota/cancellation, quarantine/retry | `S12-WIN`, `S12-MAC` |
| S13 | Deny OAuth consent, then expire a token | Retry/reauthorize path, token leakage, stale-data marking | `S13-WIN`, `S13-MAC` |
| S14 | Pass spaces, quotes, `&`, `;`, `$()` to a script | Literal argv, interpreter, cancellation/descendant behavior | `S14-WIN`, `S14-MAC` |
| S15 | Lock/sleep/wake with a scheduled job and hotkey | Replayed input, duplicate effects, capture state | `S15-WIN`, `S15-MAC` |
| S16 | Kill an update at download, verification and swap | Version that starts, signature/rollback state, recovery message | `S16-WIN`, `S16-MAC` |
| S17 | Edit/delete the same offline item on two profiles, reconnect | Conflict copies, tombstones, silent-loss check | `S17-WIN`, `S17-MAC` |
| S18 | Revoke an enrolled device, then attempt read/write | Future-access denial, epoch behavior, old-plaintext warning | `S18-WIN`, `S18-MAC` |
| S19 | Put hostile instructions in a document summarised by an AI tool | Requested authority, approvals, side effects, data destination | `S19-WIN`, `S19-MAC` |
| S20 | Review the Wayland-only capability case and map its intent | Why it is unsupported on Windows/macOS, and that S24 covers degraded state | `S20-WIN`, `S20-MAC` |

S20 is intentionally unsupported in this F01 denominator. Do not manufacture a Windows/macOS
capability gap to "cover" it. Use `S24-WIN`/`S24-MAC` for the accurate-degraded-state behavior.

## 5. Platform notes

**Windows**

- Record the actual global shortcut chord; do not assume `Ctrl+Space`.
- Capture UIA properties only for the reference process you own/authorize. Do not inspect unrelated
  applications' trees.
- Record integrity level, foreground-window rules and any `RegisterHotKey` conflict.
- For files, record ACL/TCC-equivalent state and whether a cloud placeholder triggers a download.

**macOS**

- Record TCC state for Accessibility, Screen Recording, Automation, Full Disk Access and Input
  Monitoring separately.
- Record the physical input source and VoiceOver state.
- Do not treat a system prompt as a grant; record the actual resulting state.
- For files, record Files-and-Folders/Full Disk Access behavior and cloud placeholder state.

## 6. Evidence handling and confidence

- Vendor documentation establishes categories, not this build's behavior.
- Source inspection must name the exact source revision and what was inspected.
- Static artifact analysis must name the artifact hash and tool output.
- Runtime observation must name the frozen identity, recording and permission state.
- A design proposal is our own plan and cannot be used as evidence that the reference behaves that
  way.
- Inherited documentation keeps `fetch_state=inherited_not_refetched` and the snapshot date;
  unfetched OS/standards URLs are `proposed_unverified` with `source_date=null` and `low`
  confidence. Never write a retrieval date for a page this session did not fetch.
- Set confidence to the weakest cited kind. Do not promote a design-only row to `high` because the
  test passed on our own implementation.

## 7. Stop conditions

Stop and record a gap when:

- identity fields are incomplete or the artifact hash/signature cannot be verified;
- the profile contains personal data or an unknown extension set;
- a permission state cannot be determined;
- the observation depends on an unrelated window, private service or hidden account;
- an REA tool is unavailable — never invent an Evidence ID;
- a step would require modifying a real user account, installing an unsigned production artifact or
  reading another application's private data.

## 8. Cleanup

After extracting evidence:

1. Export the synthetic fixtures and recordings to the evidence store with stable names.
2. Delete the dedicated profile and generated fixture directory.
3. Remove any temporary helper process, shortcut registration and test app target.
4. Record the cleanup result and any residual artifact.
5. Confirm no credential, token, personal path or unrelated content was captured.

The current F01 schemas deliberately admit only `state=unknown` identity manifests and a
`reference_app_available=false` ledger. The validator also fixes exactly two platform manifests and
rejects a verified manifest or observed row. A capture session must first introduce a versioned
schema/validator migration for verified identity and observations; it must not edit these frozen
artifacts to look observed. The existing validator will reject such edits.

The migration must replace the hard-coded unknown-only identity, catalog,
ledger and audit gates together; merely allowing `state=observed` is unsafe.
Its rejection tests must establish that:

- `observation.evidence_id` belongs to the row's `evidence_refs` and resolves
  directly to a `runtime_observation` catalog entry, not a vendor/design entry
  or an arbitrary string accompanied by unrelated runtime evidence;
- the runtime entry identifies the covered scenario, reference manifest,
  artifact SHA-256, platform, OS build and recording, all matching that row;
- the manifest explicitly lists that runtime evidence ID, and known identity
  fields have resolvable evidence IDs (including a correctly formatted hash);
- confidence includes the observation evidence, and catalog entries receive
  schema, provenance and invented-value checks too.

These are future capture-import requirements, not implemented verified-schema
features or evidence that a capture took place.

## 9. Update the ledger and validate

Only after a capture session, under the new versioned schema/validator:

- Add a new dated identity manifest with verified values; keep the historical unknown manifests.
- Change one row at a time from `not_observed` to `observed` with actual text, evidence ID,
  recording and date.
- Update `parity_status`/bucket only to the status the evidence supports. The current planning
  schema permits `documented`, `specified`, `deferred` and `unsupported`; `supported` is reserved
  for actual implemented/tested rows in a future verified schema.
- Keep unsupported/deferred rows in the denominator.
- Keep the ten-row design/source-baseline consistency audit; add a separate runtime capture record
  rather than relabeling the baseline audit.
- Run:

```sh
python3 scripts/reference/validate_reference.py --root .
python3 scripts/reference/test_validate_reference.py
```

The validator rejects an observation without a complete identity, an invented hash/REA ID, a
missing A behavior on either OS, dropped denominator rows and inflated confidence.
