# Daily-driver dogfood log — starting 2026-11-20

**Status: NOT STARTED.** The two-week L10 clock must not start until the
prerequisites below have evidence. This file exists so the gate cannot be
mistaken for completed dogfooding.

## Prerequisites before day 1

| Gate | Status | Evidence or blocker |
|---|---|---|
| Desktop consumes `DailyDriver` | Partial | L11 first slice: catalog, static quicklinks, keyboard-only Start Setup, `emoji <query>` copy mode. Quicklink authoring, full emoji variants and full onboarding screens remain L06a/L08a/L09a. |
| Quicklink authoring/preview/import/export | Open | L06a; CLI `--data-dir` import requires Ansa closed. |
| Emoji clipboard/insertion | Partial | L11 copies with a paste-manually message. P05 has no Windows automatic-insertion host; persisted collections remain L08a. |
| Onboarding real probes | Partial | L11 uses the live registered-hotkey state and a real `launch_or_switch` test, but full screens/rebinding remain L09a and F07 UI wiring. |
| Screen-reader acceptance | Open | F08; NVDA/Narrator and setup focus announcements have not run. |
| Physical keyboard/IME/DPI | Open | No field run on the test machine. |
| Clean-machine install/upgrade/uninstall | Open | No signed installer; L10 requires a new-tester drill. |
| Crash/restart drill | Open | Storage has subprocess-kill tests; the desktop dogfood drill has not run. |
| Known-limitations list | Partial | `docs/alpha/known-limitations.md` is current; it must be re-checked at the dogfood start. |
| Three-day feature freeze | Not started | Must be the final three days of the two-week window. |

## Day 1–14 log

Use [`dogfood-log-template.md`](dogfood-log-template.md). Every entry needs a
date, workflow, observed result, reproduction steps for failures and a
disposition (fixed, documented limitation, deferred with rationale).

## Exit criteria

- Two consecutive weeks of daily use.
- No unresolved critical focus, data-loss or stuck-input defect.
- Every reported regression has a disposition.
- A new tester can install, launch and uninstall using only the documentation.
- Alpha notes make no benchmark, accessibility or parity claim beyond captured
  evidence.

Do not mark L10 done from this file alone.
