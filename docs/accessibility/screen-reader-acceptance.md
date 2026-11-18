# Screen-reader acceptance protocol (ANS-15 / F08)

Status: **protocol only — no journey in this document has been executed.** No NVDA, Narrator,
VoiceOver or Orca session exists for this repository. F08 remains open and blocking. See
[the ANS-15 evidence record](../evidence/ans-15-accessibility-acceptance.md) and
[the risk register](../risk-register.md).

This protocol defines what a future run must capture before anyone may claim screen-reader
support. It is written so a tester who has never seen the code can execute it and so a reviewer can
reject incomplete evidence.

## 1. What counts as evidence

Accepted:

- A screen reader speaking the application while the tester operates it.
- Audio recording (system output, not a phone held to the speaker) **plus** the tester's written
  observation of each journey, or an AT-provided speech log (NVDA Speech Viewer, Narrator speech
  diagnostics, Orca debug/speech output) saved next to the recording.
- The exact build under test, identified by commit and binary hash.
- An independent second tester repeating the same journeys and recording the result separately.

**Not acceptable, and rejected if submitted:**

- Keyboard automation, synthetic keystroke dispatch, or an interaction smoke test.
- Windows UIA tree dumps, `inspect-uia.ps1` observations, Accessibility Insights scans, UIA
  property listings, or any "the tree contains Edit/List/ListItem" argument.
- Screenshots or screen recordings without AT speech.
- Audio that was not produced by the AT (music, a human reading a UIA dump, text-to-speech of this
  document).
- "It works with keyboard only", "it looks like the ARIA pattern", or "the framework has AccessKit".
- A single tester who is also the implementer, on one platform, standing in for the others.
- Any transcript that was not captured from the AT run it is attached to.

## 2. Build under test and record fields

Every run must record all of the following. Missing fields invalidate the run.

| Field | How to capture |
| --- | --- |
| Commit | `git rev-parse HEAD` |
| Working tree | `git status --porcelain` (must be empty or listed and explained) |
| Binary | absolute path and SHA-256 of the executable actually launched |
| Candidate revision | `a19e19440d4102668830a9da579c3ee71d2fbf1a` for any semantic-tree build; "main pin 0.2.2" otherwise |
| Build command | exact command, including features |
| OS | edition, version, build (e.g. Windows 11 24H2 26100.x, macOS 15.x, Ubuntu 24.04.x) |
| Platform session | Xorg or Wayland, GNOME version, compositor |
| AT | product, exact version, add-ons/voices, verbosity profile |
| Display | resolution, scaling %, high-contrast theme, reduced motion |
| Fixture | `--a11y-fixture` (three built-ins, no discovery) or full catalog; state which |
| Recording | file name, duration, audio format |
| Testers | two names/roles; at least one must not have implemented the change |

The only build that currently exposes a semantic tree is the isolated candidate. The shipping
`gpui 0.2.2` pin exposes no UIA descendants at all (recorded in
`experiments/gpui-accessibility/default-pin-uia-negative-observation-2026-11-18.json`), so a
gate-level pass requires deciding and promoting a build first. Do not run this protocol against the
main pin and call it a framework pass; it would only confirm keyboard operation.

## 3. Journeys

All journeys start from a clean launch. "Announce" below describes required **content**, not exact
wording; verbosity and voice settings change phrasing. A pass requires the content to be spoken
without the tester prompting, repeating the control, or reading it from the screen.

### J1 — Launch, initial focus, and field identity

Steps: launch the fixture build; wait for the window; do not touch the keyboard for 3 seconds.

Required announcements: the search field is announced with its label/name and its role
("search"/"edit"/"text field" depending on AT), and the AT reports it as focused or the tester can
confirm field focus with the AT's own focus command.

Pass: label is spoken, role is spoken, no "unknown", "pane", or silence for the focused control.
Fail: AT reports the window only, or the field is unnamed.

### J2 — Type a query

Steps: type `Project 0001` one character at a time.

Required: characters are echoed (or not, according to verbosity) and the result count/selection
changes are reflected. No character is dropped, doubled, or reordered. The field value read by the
AT matches what is visible.

Pass: no dropped characters; AT value matches; results update without focus loss.
Fail: echo corruption, value mismatch, focus theft, or a crash.

### J3 — Arrow selection and position/count announcement

Steps: press Down once, then Up once.

Required: the newly selected row's title is announced with its logical position and the total
count, e.g. "Project 00001 Terminal, result 2 of 10000" content. Focus must remain in the search
field (the AT should not report the field losing focus).

Pass: each arrow press produces exactly one announcement containing title + position + total.
Fail: silence, position missing, count missing or "1 of 1" when 10,000 results exist, focus moves
out of the field, or the row is read twice.

### J4 — End/start and virtualized offscreen navigation

Steps: press Alt+End, then Alt+Home.

Required: the last result (position 10,000 of 10,000) and then the first result (1 of 10,000) are
announced. The AT must not report a stale row or an empty list.

Pass: both announcements name the correct row and position; the list is not reported as empty.
Fail: stale title, wrong position, "list empty", or a hang.

### J5 — Action panel, filter, and invocation

Steps: select a known row; open Actions; type `pin` in the action search; invoke the single
filtered command; reopen Actions and invoke Unpin; then open Actions, type a query with no matches,
and press Escape.

Required: opening the panel announces a menu with a name; the action search is announced as a
separate field; filtering announces the remaining item count; invoking announces the effect and
returns to the search field; the empty state is announced as "no matching actions" content.

Pass: menu name, item roles, filter count, invocation effect, focus return, and empty state are all
spoken. Fail: menu items announced as plain text with no role, filter result count missing, focus
trapped or lost, or invocation silent.

Stale action: open Actions, change the underlying selection or query by an allowed route, then
attempt the previously offered action. Required: the stale attempt is announced as rejected and
nothing executes. If the panel simply closes on the underlying change, record that instead — the
rejection surface is not proven by a close.

### J6 — Error, rejection, and failure surfaces

Steps: type `1/0`; read the error. Then paste or set a value longer than 4,096 scalars; read the
rejection.

Required: the calculator error reason is announced (division by zero content) and is not only a
colour change. The oversized edit is announced as rejected and the field still contains the prior
text.

Pass: both failures produce speech containing the reason, and the AT's value read-back matches the
unchanged text. Fail: silence, a generic "error" with no reason, or mutated text.

### J7 — Details and Back

Steps: select a command; run it; read the details surface; activate Back.

Required: the details surface is announced with a document/heading name and its content; Back
returns focus to the search field and the AT reports it.

Pass: heading/name spoken, content reachable, focus restored.
Fail: unnamed document, unreadable content, focus left on a dead control.

### J8 — Composition (real IME)

Steps (Windows): enable the Japanese Microsoft IME; type `ぷろじぇくと`, keep the composition
active, press Down (must not move list selection while composing), then confirm with Enter and
verify the query; repeat with Escape to cancel. Repeat on macOS with the Japanese or Pinyin IME and
on Linux with IBus + Japanese.

Required: the AT reports composition/marked text as composing (not committed), the list does not
move while composing, confirming commits once, cancelling restores the pre-composition text.

Pass: composing state is distinguishable, exactly one commit, cancel restores text and selection.
Fail: list navigation during composition, double commit, lost text, or the AT announcing committed
text for marked text.

### J9 — RTL, mixed bidi, caret and selection

Steps: paste `שלום Project 123 العربية`; move the caret with Left/Right and Shift+Left/Right across
the whole value; select one Hebrew letter and one Arabic letter.

Required: the AT reports the caret/selection at logical positions, speaks the selected characters,
and does not report reversed or duplicated text.

Pass: selection read-back matches the characters in logical order; no split scalar.
Fail: reversed announcement, selection off by one cluster, split emoji/combining sequence, or
visual order used as logical order.

### J10 — Scaling and high contrast

Steps: at 100 %, 150 % and 200 % display scaling, repeat J1–J4. With Windows High Contrast /
macOS Increase Contrast / GNOME High Contrast enabled, repeat J1–J4.

Required: the same announcements, with geometry-independent content; focus ring and text remain
perceivable (tester statement, not a screenshot alone).

Pass: identical semantic announcements at all three scales and in high contrast.
Fail: missing/clipped text, unreadable focus indication, or announcements that depend on scale.

## 4. Windows — NVDA

Setup:

- Windows 11 24H2 (build 26100 or later), 1920×1080 or larger.
- NVDA 2025.x (record exact version), default synth, speech viewer enabled (Tools → Speech Viewer)
  and its log saved.
- Disable "speak typed characters" for J2's second pass if echo noise prevents judging the rest;
  record the setting used.
- Start the fixture with `--a11y-fixture`, then start NVDA (or start NVDA first and relaunch).
- Verify NVDA reports focus in the app, not "unknown".

Run J1–J10. For J10, change Windows display scaling (Settings → Display → Scale) between runs and
relaunch the app.

Acceptance: every journey passes with speech viewer text and audio; two testers agree.

## 5. Windows — Narrator

Setup:

- Same OS/build, Narrator 2025-era version (record it), default voice, Earcons off, verbosity
  "some" and then "all" as a second pass.
- Narrator's speech diagnostics enabled where the build supports it; otherwise audio + tester
  notes.
- Use Narrator's scan mode (Caps+Space) once to inspect the control list, then leave scan mode and
  run J1–J10 with normal navigation.

Acceptance: same as NVDA. Narrator and NVDA must both pass; one passing AT does not clear Windows.

## 6. macOS — VoiceOver

Setup:

- macOS 14 or 15 (record version and hardware).
- VoiceOver enabled (Cmd+F5), default voice, "Speak announcements" enabled.
- Native build launched from Terminal so the recorded binary path is unambiguous.
- `VO+U` rotor used once to confirm the field and list appear as controls, then closed; rotor
  presence alone is not a journey pass.

Run J1–J10, substituting Option-based word movement where the app binds it. Record VoiceOver's
verbosity (hints, punctuation) and the audio.

Preflight: confirm the pinned candidate actually builds and runs on macOS and that its
`gpui_macos`/`accesskit_macos` path is active. No Ansa macOS build exists today; do not accept a
Windows-only claim as macOS support.

## 7. Linux — Orca and AT-SPI

Named target: **Ubuntu 24.04 LTS, GNOME Shell 46, Xorg session first, then a Wayland session**,
AT-SPI2 enabled (`gsettings get org.gnome.desktop.interface toolkit-accessibility` = true), Orca
46.x.

Setup:

- Install the candidate build with its `accesskit_unix` feature enabled (it is optional in the
  pinned revision); record the exact Cargo features.
- Verify the AT-SPI bus is reachable (`busctl --user list | grep -i atspi`) and that a probe such as
  Accerciser sees an application frame (probe only; a tree is not a journey).
- Start Orca, set verbosity to default, enable speech logging if available.

Run J1–J10. Include J8 with IBus + Japanese and J9 with an RTL locale.

Preflight: the Linux desktop backend is `gpui_linux`; its `accesskit_unix` adapter is optional at the
pinned revision. Confirm a build with it enabled exists before scheduling testers. Until then this
platform is **unverified/unsupported**, not "untested but probably fine".

## 8. Independent repetition

- Two testers per platform; at least one must not be the implementer of the accessibility change.
- Each tester runs the journeys independently, on a separately launched process, and records their
  own audio and notes.
- A second run on a second day is required for any journey that failed once and then "worked".
- Disagreement between testers is a **fail** until reproduced and explained.
- The reviewer accepts a journey only when both testers' evidence contains the required content.

## 9. Coverage matrix

Statuses: `automated UIA support` = structural evidence only; `real AT unrun` = no journey;
`unsupported` = no adapter/build or no claim.

| Area | Windows UIA automation | Real AT | Notes |
| --- | --- | --- | --- |
| Search field role/label/value/focus | automated UIA support (candidate spike and launcher shell) | real AT unrun | 18 Nov observations, 100 % scale |
| Search text ranges, selection, caret boundaries | automated UIA support (candidate spike and launcher shell) | real AT unrun | Includes rejection announcement event |
| Result list roles, selected state, positions | automated UIA support (candidate spike and launcher shell) | real AT unrun | Container set size not exposed; items are |
| 10k realization, end/start navigation | automated UIA support (candidate spike) | real AT unrun | Shell list has 3 onscreen rows only |
| Action panel, filter, invoke, stale rejection | automated UIA support (18 Nov shell); stale rejection not reachable | real AT unrun | Open/filter/invoke verified; stale surface blocked by menu |
| Error/permission/failure announcements | automated UIA support (18 Nov shell) | real AT unrun | Calculator error has no observed live event (A11Y-21) |
| Details/Back | automated UIA support (18 Nov shell) | real AT unrun | Document label `Ansa`; focus restored |
| Live-region event payloads | automated UIA support (candidate spike) | real AT unrun | Speech behaviour unknown |
| IME composition | unsupported | real AT unrun | No physical IME run |
| RTL/bidi and caret placement | unsupported | real AT unrun | Model tests only |
| 150 %/200 % scaling | unsupported | real AT unrun | Only 100 % observed |
| High contrast | unsupported | real AT unrun | No theme run |
| macOS VoiceOver | unsupported | real AT unrun | No macOS build |
| Linux Orca / AT-SPI | unsupported | real AT unrun | Adapter optional, no build |
| Shipping `gpui 0.2.2` pin | unsupported (0 UIA descendants) | real AT unrun | No bridge at all |

## 10. Exit criteria for F08

F08 may be marked passed only when all of the following are attached to the ticket:

1. This protocol executed on Windows with NVDA **and** Narrator, 100 % plus a 150 % repeat.
2. VoiceOver executed on macOS.
3. Orca executed on the named Ubuntu/AT-SPI setup.
4. Two independent testers per platform, with audio and AT logs.
5. Every journey J1–J10 has an explicit pass result; any fail is either fixed and re-run or
   documented as a narrowed-scope limitation approved by the release gate owner.
6. The promoted build is identified and the framework gate decision is recorded; a candidate
   experiment alone does not satisfy this.
