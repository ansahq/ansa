# Daily dogfood log template

Copy this file to a dated log (for example `dogfood-2026-11-20.md`) for each
tester. Record only what was observed. Do not record a successful outcome that
was not verified, and do not paste secrets, clipboard bodies or application
content.

## Tester and environment

| Field | Value |
| --- | --- |
| Date | |
| Tester | |
| Platform / Windows build | |
| Build source (commit, `cargo build --release` or `cargo install`) | |
| Data directory | |
| Monitor / scaling / input method | |
| Screen reader (if any) | |
| Keyboard only? | yes / no |

## Daily workflow log

| Time | Workflow | Query / keys | Expected | Observed | Outcome |
| --- | --- | --- | --- | --- | --- |
| | Summon launcher | Ctrl+Space | Launcher focused | | pass / fail |
| | Search application | e.g. `term` | Expected app first | | pass / fail |
| | Launch application | Enter | `Launched` observed | | pass / fail |
| | Switch to running app | Enter | `Switched { window }` observed | | pass / fail |
| | Denied activation | Enter | `Denied` with reason, launcher stays open | | pass / fail |
| | Calculator | `=(24+8)/2` | `16`, copy only | | pass / fail |
| | Dismiss and restore focus | Esc | Previous window restored, or an explicit skip/denial reason | | pass / fail |
| | IME composition | type, Esc | Composition consumes Escape | | pass / fail |
| | Restart | close and reopen | No data loss, catalog recovers | | pass / fail |

## Failure record

Use one block per failure. Every failure needs a disposition before the weekly
review closes.

```text
ID:
Date/time:
Tester:
Build / commit:
Platform:
Severity: critical / major / minor
Category: keyboard / focus / summon / selection / action / storage / recovery / other
Summary:
Exact repro steps:
1.
2.
3.
Expected:
Observed:
Frequency: always / intermittent (n/m) / once
Evidence: screenshot, log line, status text (no secrets)
Workaround:
Disposition: fixed / documented limitation / deferred
Rationale or ticket:
Regression? yes / no
```

### Severity guide

- **Critical:** data loss, corrupted settings/quicklinks, stuck input, focus
  stolen irrecoverably, launcher cannot be dismissed, or a claimed success that
  did not happen.
- **Major:** summon, selection or action fails repeatedly, or a keyboard-only
  workflow is blocked.
- **Minor:** cosmetic, wording, ordering or a recoverable workaround exists.

## Weekly triage checklist

Run once per week with the whole dogfood group.

- [ ] Every failure row has a disposition: fixed, documented limitation, or
      deferred with rationale and owner.
- [ ] No unresolved critical focus, data-loss or stuck-input defect remains. If
      one exists, the alpha gate is **not** met regardless of other results.
- [ ] Re-test every "fixed" item on a clean build.
- [ ] Re-check the known-limitations list; remove an item only with evidence and
      add new limitations discovered during the week.
- [ ] Verify a clean-profile install, launch and uninstall using only
      `docs/alpha/README.md`. Note every undocumented step as a documentation
      defect.
- [ ] Run `ansa alpha-smoke --cycles 1000` on the week's build and record the
      metrics line and fingerprint.
- [ ] Run `ansa doctor --data-dir <dir>` and record schema/pragma/onboarding
      status.
- [ ] Do a keyboard-only pass: summon, query, navigate, act, dismiss, recover
      from denial.
- [ ] Do a screen-reader spot check or record explicitly that it remains
      unverified.
- [ ] Confirm the final three days of the window are feature-frozen: only
      release-blocking defects are fixed.
- [ ] Record whether the week contained real daily use on every claimed
      platform. A week without daily use does not count toward the two-week
      acceptance criterion.

## Two-week gate status

| Item | Required | Status |
| --- | --- | --- |
| Consecutive daily-use days | 14 | |
| Unresolved critical focus/data-loss/stuck-input defects | 0 | |
| Every regression dispositioned | yes | |
| New tester installs/launches/uninstalls from docs | yes | |
| Keyboard-only pass | yes | |
| Screen-reader spot check | yes or explicitly unverified | |
| Final three days feature-frozen | yes | |
| 1,000-cycle harness on final build | pass | |
| No unsupported benchmark/accessibility/parity claim | yes | |

**Do not mark the phase gate met from automated tests alone.** The harness is
necessary evidence, not a substitute for the daily-use rows above.
