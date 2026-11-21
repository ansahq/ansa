# Independent frozen-tree validation — 2026-11-20

**Automated implementation/readiness: GREEN for the tested slices. Release and human acceptance: OPEN.** Every requested Cargo command exited 0. Two tests were deliberately ignored, and the automated runs do not substitute for live product checks.

## Environment and worktree

- Worktree: `C:\Users\abhir\.local\share\opencode\worktree\1bbf61\playful-eagle`; HEAD `a5b3be03bcf60d98d9274c514b233841accc65bd`. Validated the actual dirty tree, including the untracked sources, not a checkout of HEAD.
- Microsoft Windows `10.0.26200`, X64; Rust host `x86_64-pc-windows-msvc`, LLVM `23.1.1`. Cargo commands that resolve dependencies ran offline. No implementation edits.

```text
rustc 1.99.1 (58cf8f9c3 2026-10-13)
cargo 1.99.1 (5f94df478 2026-08-27)
```

`git status --short` before validation and again before writing this report:

```text
 M Cargo.lock
 M Cargo.toml
 M README.md
 M apps/cli/Cargo.toml
 M apps/cli/src/main.rs
 M crates/core/Cargo.toml
 M crates/core/src/calculator.rs
 M crates/core/src/query.rs
 M crates/platform-api/src/lib.rs
 M crates/platform-windows/src/hotkey.rs
 M crates/platform-windows/src/lib.rs
 M crates/platform-windows/src/start_menu.rs
 M crates/platform-windows/src/types.rs
 M crates/platform-windows/src/visibility.rs
 M crates/platform-windows/src/win32.rs
 M crates/search/Cargo.toml
 M crates/search/src/lib.rs
 M crates/ui-gpui/Cargo.toml
 M crates/ui-gpui/src/lib.rs
 M crates/ui-gpui/src/smoke.rs
 M docs/implementation-status.md
?? apps/cli/tests/
?? crates/alpha/
?? crates/clipboard/
?? crates/coordinator/
?? crates/core/data/
?? crates/core/tests/
?? crates/emoji/
?? crates/onboarding/
?? crates/platform-windows/examples/
?? crates/platform-windows/src/icons.rs
?? crates/platform-windows/src/launch.rs
?? crates/platform-windows/src/packaged.rs
?? crates/platform-windows/src/reveal.rs
?? crates/platform-windows/src/shell_link.rs
?? crates/platform-windows/src/shortcut.rs
?? crates/platform-windows/src/switch.rs
?? crates/platform-windows/src/watch.rs
?? crates/quicklinks/
?? crates/registry/
?? crates/search/data/
?? crates/search/tests/
?? crates/storage/
?? crates/ui-gpui/src/daily.rs
?? crates/vault/
?? docs/alpha/
?? docs/ranking/
?? docs/reviews/
?? docs/validation-2026-11-19.md
```

## Commands and results

| Command | Observed result |
|---|---|
| `rustc --version`, `cargo --version`, `git status --short` | Versions and status above. |
| `cargo fmt --all -- --check` | Exit 0, no output. |
| `cargo test --workspace --offline --no-fail-fast` | Exit 0; 695 distinct passed, 0 failed, 2 ignored; 697 pass events counting child reruns. |
| `cargo test --locked --offline --workspace` | Exit 0; same counts, including the same two child reruns and two ignored tests. |
| `cargo clippy --workspace --all-targets --offline -- -D warnings` | Exit 0; no Clippy diagnostics. Cargo dependency future-incompatibility warning below. |
| `cargo run --offline -p ansa-cli -- doctor` | Exit 0; 156 total, 108 classic, 48 packaged, 3 broken. |
| `cargo run --offline -p ansa-cli -- calc "10 km to mi"` | Exit 0; `6.21371192237 mi` |
| `cargo run --offline -p ansa-cli -- calc "0.1+0.2"` | Exit 0; `0.3` |
| `cargo run --offline -p ansa-cli -- emoji rocket --limit 3` | Exit 0; Rocket and Tennis, 2 results, licensed dataset metadata. |
| `cargo run --offline -p ansa-cli -- alpha-smoke --cycles 1000` | Exit 0; PASS, 15,003 assertions, fingerprint `0x02807f025f6a8ad7`. |
| `cargo run --offline -p ansa-cli -- storage --demo` | Exit 0; schema 3, stale CAS rejected, backup integrity verified. |
| `cargo run --offline -p ansa-cli -- onboard --state <temp>\onboarding.json --demo` | Exit 0; completed once using fake probes. |
| `cargo run --offline -p ansa-cli -- quicklink list --data-dir <temp>\quicklinks` | Exit 0; empty library, revision 0. |
| `cargo run --offline -p ansa-cli -- doctor --data-dir <temp>\quicklinks` | Exit 0; schema 3, empty quicklink record, no onboarding state. |
| `cargo run --offline -p ansa-platform-windows --example catalog` | Exit 0; 156 apps, three broken shortcuts reported explicitly; watcher not wired. |
| `cargo run --offline -p ansa-platform-windows --example icons` | Exit 0; actual Notepad icon: valid ICO, 4,286 bytes, 32×32. |
| `cargo run --offline -p ansa-platform-windows --example watch` | Exit 0; six bounded polls; both roots exist; all unchanged. |
| `cargo run --offline -p ansa-desktop -- --smoke-test` | Exit 0 on both runs; native window startup/exit, **no stage count reported**. |
| `cargo run --offline -p ansa-desktop -- --interaction-smoke-test` | Exit 0 on both runs; **40 synthetic stages**. |

Cargo emitted this warning on Clippy and the desktop runs (not a `-D warnings` failure):

```text
warning: the following packages contain code that will be rejected by a future version of Rust: proc-macro-error2 v2.0.1
note: to see what the problems were, use the option `--future-incompat-report`, or run `cargo report future-incompatibilities --id 1`
```

## Tests and coverage

**695 distinct passed, 0 failed, 2 ignored (697 distinct discovered):** 563 unit + 128 integration + 4 doctest passes. There were 44 top-level runners (18 unit, 10 integration, 16 doctest), including zero-test runners. Two integration tests relaunch themselves in filtered child processes (one clipboard, one vault); summing the 46 literal result lines gives 693 unit/integration pass **events** + 4 doctest passes = **697 pass events**, not 697 distinct passing tests. The locked run had identical counts.

Ignored, not executed:

- `windows::tests::real_clipboard_read_does_not_modify_it` — `ignored, reads the real clipboard; run manually on a test machine`.
- `crates\ui-components\src\text_input.rs - text_input (line 11)` — ignored doctest.

| Area | Unit passed | Integration passed | Doctest passed | Coverage in this run |
|---|---:|---:|---:|---|
| Alpha | 0 | 16 | 0 | Composition, provider churn, stable selection, quicklinks in catalog |
| CLI | 0 | 10 | — | Command routing, diagnostics, persisted workflows |
| Clipboard | 85 | 4 | 1 | Opt-outs, filtering, quotas, DPAPI, locks, migration; 1 unit ignored |
| Coordinator | 31 | 0 | 0 | Budgets, deadlines, pending/terminal states, stale and cancelled work |
| Core | 52 | 8 | 1 | Query routing, calculator parsing/formatting and versioned corpus |
| Desktop binary | 0 | 0 | — | Separate native and interaction smokes below |
| Domain | 6 | 0 | 0 | Typed IDs and domain invariants |
| Emoji | 7 | 15 | 0 | Embedded dataset, graphemes, names, search and bounded collections |
| Onboarding | 18 | 18 | 1 | Flow, shortcut/launch state, skip/reset and persistence |
| Platform API | 8 | 0 | 0 | Capability and outcome contracts |
| Windows platform | 130 | 0 | 0 | Shell links and catalog identity, launch/switch decisions, hotkey lifecycle, icon/reveal/watcher |
| Quicklinks | 77 | 0 | 0 | Encoding, inert opener policy, preview/commit, import/export |
| Registry | 70 | 0 | 1 | Authorization, stale references, pending reconciliation, tombstone retention |
| Search | 26 | 2 | 0 | Ranking, corpus integrity, order and quality floors |
| Storage | 0 | 42 | 0 | SQLite recovery/migrations, CAS, backup/restore, crash/lock cases and SQLite `SQLITE_FULL` via page cap |
| UI components | 28 | 0 | 0 | Input/editor and UI component behavior; 1 doctest ignored |
| GPUI | 22 | 0 | 0 | Daily driver, integration state and synthetic setup/emoji behavior |
| Vault | 3 | 13 | 0 | DPAPI round-trip, alias/context/lock and tamper handling |
| **Distinct passed** | **563** | **128** | **4** | **695** |

<details>
<summary>All 46 literal <code>test result:</code> lines from the first no-fail-fast run, in emitted order</summary>

```text
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 4.67s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.56s
test result: ok. 85 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 3 filtered out; finished in 0.00s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 31 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.06s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 7 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 18 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.84s
test result: ok. 130 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
test result: ok. 77 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.42s
test result: ok. 70 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 42 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 1.77s
test result: ok. 28 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 22 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out; finished in 0.00s
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
```

</details>

## Key CLI output

`doctor`:

```text
Ansa doctor
platform: windows (adapter windows-start-menu-packaged-launch-switch-v5)
applications: 156 total · 108 classic · 48 packaged · 3 broken
catalog: available
packaged apps: available
application launch: available
url open: available
global hotkey: available
  hotkey probe only: no shortcut was registered by doctor
clipboard capture: unsupported
window control: unsupported
emoji dataset: emoji-1.0.0 · Unicode 16.0 · 139 entries · license Unicode License v3 · checksum e80345152b0e31eb
ranking corpus: v2026-11-19.3 (ansa-search) · calculator corpus v1.2.0 (ansa-core)
calculator trigger: explicit '=' prefix (required for units and constants), or digit-only arithmetic using + - * / ^ % ( ) .
storage: skipped (pass --data-dir <dir>)
No network access was used.
```

`emoji rocket --limit 3`:

```text
emoji query "rocket" · dataset emoji-1.0.0 · license Unicode License v3 · checksum e80345152b0e31eb
🚀  Rocket  [Travel & Places]
  accessible: Rocket
🎾  Tennis  [Activities]
  accessible: Tennis
2 result(s)
```

`alpha-smoke --cycles 1000`:

```text
Alpha smoke: PASS
cycles: 1000
accepted batches: 1000
rejected batches: 3000
stable selection checks: 1000
max pending: 1
max results: 401
catalog size: 404
assertions: 15003
elapsed: 4106.0 ms
fingerprint: 0x02807f025f6a8ad7
```

`storage --demo`:

```text
Demo data directory: C:\Users\abhir\AppData\Local\Temp\ansa-storage-demo-12836-1795113530763288100
opened: C:\Users\abhir\AppData\Local\Temp\ansa-storage-demo-12836-1795113530763288100\ansa.sqlite
schema version: 3
pragmas: foreign_keys=1 journal_mode=wal synchronous=2 busy_timeout=5000ms
test setting: maximum_results=42 (revision 1)
compare-and-swap: stale revision rejected as expected
backup verified: C:\Users\abhir\AppData\Local\Temp\ansa-storage-demo-12836-1795113530763288100\backup.sqlite · integrity ok · schema v3 · 1 settings · 0 records
Demo files were left in place for inspection.
```

`onboard --demo`, `quicklink list`, and the storage-specific tail of `doctor --data-dir` (the doctor capability/count lines matched the first run):

```text
Ansa onboarding demo (fake probes)
- This demo does NOT register a real global hotkey.
- This demo does NOT launch an application.
state file: C:\Users\abhir\AppData\Local\Temp\opencode\ansa-validation-cli-dbfb398c-7aa2-4bc0-975f-c24cea8e96f6\onboarding.json
[start] step Welcome · status not-started
[next] step Choose a shortcut · status in-progress
[shortcut] Ctrl+Space · probe available (fake) · registered true
[next] step Try a launch · status in-progress
[launch] Launched for app:demo (fake probe; no process started)
[next] step Optional integrations · status in-progress
[next] step Ready · status in-progress
[complete] status completed · completions 1
Demo probes do not register a real hotkey or launch an app.
```

```text
ID                           TITLE                    OPENER     TARGET
0 quicklinks · library revision 0 · source C:\Users\abhir\AppData\Local\Temp\opencode\ansa-validation-cli-dbfb398c-7aa2-4bc0-975f-c24cea8e96f6\quicklinks
Persisted records may include entries the catalog omits; run `ansa doctor --data-dir <dir>` for launchability reasons.
```

```text
storage: C:\Users\abhir\AppData\Local\Temp\opencode\ansa-validation-cli-dbfb398c-7aa2-4bc0-975f-c24cea8e96f6\quicklinks\ansa.sqlite · schema v3
  pragmas: foreign_keys=1 journal_mode=wal synchronous=2 busy_timeout=5000ms
  quicklinks: 0 persisted · 0 launchable · 0 omitted from the catalog (record revision None)
  onboarding: no state file
No network access was used.
```

Neither empty-store command printed a lock warning. The desktop's Integration Status source text says: “Ansa holds the exclusive storage lock while it runs, so Refresh re-reads only in-process persisted state and cannot pick up an external CLI import live. To import, close Ansa, run `ansa quicklink import <file> --data-dir <data-dir>`, then start Ansa.” This is the supported workflow, not an observed live import in this validation.

The CLI temp data root and storage-demo files were removed and checked absent. The icon example removed its cache root. Storage can retain empty lock files outside the data root in the shared per-user `ansa-storage\locks` directory by design; that shared lock directory was not purged.

## Windows example output

`catalog` (application stdout, no Cargo build chatter):

```text
total=156
classic=108
packaged=48
broken=3
duplicate_titles=2
with_arguments=29
argument_distinct_targets=6
watcher_wired=false
sample packaged=false broken=false title="Administrative Tools" path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Administrative Tools.lnk" target=None args=Some("/name Microsoft.AdministrativeTools")
sample packaged=true broken=false title="Affinity" path="shell:AppsFolder\\Canva.Affinity_8a0j1tnjnt4a4!Canva.Affinity" target=None args=None
sample packaged=false broken=false title="Android Studio" path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Android Studio\\Android Studio.lnk" target=Some("C:\\Program Files\\Android\\Android Studio\\bin\\studio64.exe") args=None
sample packaged=false broken=false title="App Recovery" path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\EA\\App Recovery.lnk" target=Some("C:\\Program Files\\Electronic Arts\\EA Desktop\\EA Desktop\\EALauncher.exe") args=Some("-recovery")
sample packaged=true broken=false title="Apple Devices" path="shell:AppsFolder\\AppleInc.AppleDevices_nzyj5cx40ttqa!App" target=None args=None
sample packaged=false broken=false title="Application Verifier (WOW)" path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Windows Kits\\Application Verifier (X86)\\Application Verifier (WOW).lnk" target=Some("C:\\Windows\\SysWOW64\\appverif.exe") args=None
sample packaged=false broken=false title="Application Verifier (X64)" path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Windows Kits\\Application Verifier (X64)\\Application Verifier (X64).lnk" target=Some("C:\\Windows\\System32\\appverif.exe") args=None
sample packaged=true broken=false title="Armoury Crate" path="shell:AppsFolder\\B9ECED6F.ArmouryCrate_qmba6cd70vzyy!App" target=None args=None
sample packaged=false broken=false title="BongoCat" path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\BongoCat\\BongoCat.lnk" target=Some("C:\\Users\\abhir\\AppData\\Local\\BongoCat\\bongocat-app.exe") args=None
sample packaged=false broken=false title="Brave" path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Brave.lnk" target=Some("C:\\Program Files\\BraveSoftware\\Brave-Browser\\Application\\brave.exe") args=Some("--source-shortcut-location=start-menu")
argument_variant target="c:\\program files\\electronic arts\\ea desktop\\ea desktop\\ealauncher.exe" arguments={"", "-recovery"}
argument_variant target="c:\\program files\\videolan\\vlc\\vlc.exe" arguments={"", "--reset-config --reset-plugins-cache vlc://quit", "-Iskins"}
argument_variant target="c:\\riot games\\riot client\\riotclientservices.exe" arguments={"", "--launch-product=valorant --launch-patchline=live"}
argument_variant target="c:\\users\\abhir\\appdata\\local\\discord\\discord.exe" arguments={"", "--processStart Discord.exe"}
argument_variant target="c:\\windows\\system32\\cmd.exe" arguments={"/d /c \"C:\\Program Files\\nodejs\\install_tools.bat\"", "/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\Common7\\Tools\\VsDevCmd.bat\"", "/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Auxiliary\\Build\\vcvars32.bat\"", "/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Auxiliary\\Build\\vcvars64.bat\"", "/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Auxiliary\\Build\\vcvarsamd64_x86.bat\"", "/k \"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\VC\\Auxiliary\\Build\\vcvarsx86_amd64.bat\"", "/k \"C:\\Program Files\\nodejs\\nodevars.bat\""}
argument_variant target="c:\\windows\\syswow64\\windowspowershell\\v1.0\\powershell.exe" arguments={"", "-NoExit -Command \"& { Import-Module Appx; Import-Module .\\AppxDebug.dll; Show-AppxDebug}\"", "-noe -c \"&{Import-Module \"\"\"C:\\Program Files (x86)\\Microsoft Visual Studio\\2022\\BuildTools\\Common7\\Tools\\Microsoft.VisualStudio.DevShell.dll\"\"\"; Enter-VsDevShell 5ff1e8fe}\""}
broken_entry title="Discord" path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Discord Inc\\Discord.lnk" reason="shortcut target 'C:\\Users\\abhir\\AppData\\Local\\Discord\\Discord.exe' no longer exists; refresh the application catalog after reinstalling or moving the application"
broken_entry title="Discord" path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Discord.lnk" reason="shortcut target 'C:\\Users\\abhir\\AppData\\Local\\Discord\\Discord.exe' no longer exists; refresh the application catalog after reinstalling or moving the application"
broken_entry title="Steam" path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Steam\\Steam.lnk" reason="shortcut target 'C:\\Program Files (x86)\\Steam\\steam.exe' no longer exists; refresh the application catalog after reinstalling or moving the application"
duplicate_title="uninstall" count=2
  duplicate packaged=false path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\HandBrake\\Uninstall.lnk"
  duplicate packaged=false path="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs\\Unigine\\Heaven Benchmark 4.0\\Uninstall.lnk"
duplicate_title="discord" count=2
  duplicate packaged=false path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Discord Inc\\Discord.lnk"
  duplicate packaged=false path="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs\\Discord.lnk"
packaged_capability=available catalog=available
```

`icons`:

```text
icon_source="C:\\WINDOWS\\System32\\notepad.exe,0" cache="C:\\Users\\abhir\\AppData\\Local\\Temp\\ansa-platform-icons-example-15968\\app-3f6cd12d16076b2abfb49996a25f0f5a.ico" size=4286 bytes dimensions=32x32 valid_ico=true
```

`watch`:

```text
watch_root="C:\\ProgramData\\Microsoft\\Windows\\Start Menu\\Programs" exists=true
watch_root="C:\\Users\\abhir\\AppData\\Roaming\\Microsoft\\Windows\\Start Menu\\Programs" exists=true
poll=1 outcome=Unchanged rescan_due=false elapsed_ms=10
poll=2 outcome=Unchanged rescan_due=false elapsed_ms=9
poll=3 outcome=Unchanged rescan_due=false elapsed_ms=8
poll=4 outcome=Unchanged rescan_due=false elapsed_ms=8
poll=5 outcome=Unchanged rescan_due=false elapsed_ms=8
poll=6 outcome=Unchanged rescan_due=false elapsed_ms=8
polls=6 interval_ms=300 rescan_due=false desktop_wiring=L11
```

No Start Menu mutation was induced. The watch example proves the real roots can be polled, not live change detection or desktop wiring.

## Desktop smokes and profile isolation

Both commands used throwaway `LOCALAPPDATA`, `APPDATA`, `TEMP` and `TMP` paths, with `ANSA_DATA_DIR` unset. Native smoke was silent after Cargo's `Running` line and exited 0. The interaction smoke printed this line and exited 0:

```text
Ansa interaction smoke: 40 synthetic widget/action stages passed, including glass, light/dark theme toggling, bounded emoji navigation and keyboard-only setup traversal. No clipboard writes or external app launches.
```

Both pairs left no isolated `Ansa\alpha` profile and no `ansa-smoke-*` directory in the isolated temp root; both roots were removed. On the first pair the aggregate comparison on the real `%LOCALAPPDATA%\Ansa\alpha` reported `BEFORE=11 AFTER=11 UNCHANGED=False` (238,169 bytes after; comparator did not retain a per-file diff). One already-existing `ansa-smoke-*` directory in real TEMP was present both before and after. That first comparison is inconclusive, not proof of no changes or attribution to either command. The immediate rerun captured per-entry paths, sizes, UTC write timestamps and SHA-256 for files, and returned `REAL_PROFILE_ENTRIES=11 DELTA_ROWS=0` after **each** command. The real profile was unchanged on that checked rerun. Neither run used the real profile as a data target.

## Independent review dispositions

These are code/slice review verdicts from all six files in [`reviews/`](reviews/), not ticket-system status changes or new manual evidence.

| Ticket | Final review verdict and residuals |
|---|---|
| ANS-14 / L01 | **Accepted at registry crate/API scope.** R2-X was reopened and fixed with a per-command pending set; terminal history is bounded. Caller-asserted grants, confirmation and namespace remain the in-process trust boundary; no durable crash journal, atomic probe/perform or desktop integration. |
| **Reopened ANS-16 / L02** | **Accepted after re-review.** Wrong shell-link candidate precedence and target/arguments/working-directory identity were fixed. Real switch/focus, packaged switching and watcher/icon desktop wiring remain outside the acceptance evidence; the packaged probe cache/timeout retains documented caveats. |
| L02a | **Accepted at platform-code scope.** Actual ICO extraction and bounded watcher example pass; reveal validation/identity tested, not real Explorer reveal. Reparse fixtures and desktop integration remain. |
| F07 | **Accepted at platform-library scope.** Shortcut validation and manager/recorder lifecycle tested (including synthetic hotkey messages). Physical summon/capture/rebind and desktop settings wiring remain open. |
| ANS-17 / L03 | **Accepted after R1/R2.** Custom lower budgets work; stale/cancelled/terminal results no longer poison the clock. Synchronous logical clock and process-local IDs; no real async/IPC runtime. |
| ANS-18 / L04 | **Accepted.** Ordered golden prefixes and metric floors tested. Corpus remains small with short prefixes/known failures; bounded result count is not bounded full-scan/sort work. Signals remain in-process/unbounded by key; recency is logical-tick based. |
| **Reopened ANS-19 / L05** | **Accepted with residual risks.** Path-equivalence and restore/WAL ordering blockers were fixed; L05a supersedes the earlier no-cross-process-lock limitation. Generic records/cache remain unencrypted; physical durability, rollback-failure injection and restore trust boundaries remain. |
| ANS-78 / L05a | **Accepted with residual risks.** File-identity lock closes the hard-link alias finding on identity-capable filesystems; SQLite page-cap `SQLITE_FULL`, locking/re-key and recovery reports covered. Advisory/per-user lock with path fallback, possible hard-link test skip, unchecked recovery corners and no physical ENOSPC/power-loss remain. The review's LOW backup-to-live-hard-link finding is addressed in this tree by the identity guard and `backup_destination_hard_link_to_live_database_is_rejected`; the workspace suite has 42 storage tests vs. 41 in that review. |
| **Reopened ANS-20 / L06** | **Accepted after opener-policy re-review, with residuals.** No shell execution in the crate. Real opener probe and GPUI authoring absent; caller confirmation for replace, stricter rules under unchanged v1, extensionless host stat, allowlist and IPv6 edge remain. |
| ANS-21 / L07 | **Accepted at crate and verified consumer scope.** Corpus v1.2.0; explicit `=` for unit/scientific routed input, approximate `f64`, deliberately non-commercial percent semantics, bounded property tests. |
| ANS-24 / L08 | **Emoji crate accepted; ticket partial.** Licensed embedded search/grapheme/name coverage; FNV is not a cryptographic signature. Later L11 adds bounded copy-mode UI, not a live copy/paste proof, full variant picker or persisted collections. |
| ANS-76 / L11 | **Accepted as a bounded first desktop slice.** R1 resolved by disclosing close/import/start, not by adding live external import. Synthetic setup focus/IME-generation guards pass; stale launch results are discarded, but the review notes a LOW stale “Testing…” footer after close. F08 and real desktop journeys remain open. |
| ANS-26 / P03 | **Accepted at vault crate/API scope.** DPAPI with no plaintext fallback, tested context and alias handling. User-bound, no independent backup/recovery; rotation is not transactional, concurrent writes can be last-writer-wins, error-path wiping and ACL/reparse/crash coverage are incomplete. |
| ANS-27 / P04 | **Accepted for fresh and normal legacy stores** after the M-1 migration delta. LOWs include missing-alias recovery, old-alias deletion/tombstone durability, best-effort wiping, marker API failure paths, non-Windows stale lock and heuristic limits. Live listener/UI and persisted capture preferences remain absent; real clipboard probe was not run. |
| ANS-29 / P05 | **Accepted at policy/API scope.** Copy-only default, fake-host target/permission/IME/secure-input/ownership and evidence checks. No Windows automatic insertion host; timing/sequence and unknown-ownership residuals remain host concerns. |
| F08 | **OPEN release gate.** Candidate semantics and labels are not a verified screen-reader bridge; neither NVDA nor Narrator ran, and setup focus announcement remains unverified. |
| L10 | **OPEN human gate.** Deterministic alpha harness and desktop `DailyDriver` are implementation evidence, not two weeks of dogfooding, a clean-machine install/upgrade/uninstall or a human crash/restart drill. |

## Not verified / not implemented

| Item | Boundary |
|---|---|
| Full GPUI quicklink authoring, preview, import and export | Not implemented. Desktop composition of eligible links exists; manual CLI close/import/start and real browser/file-manager behavior were not verified. |
| Full emoji variant picker and persisted favorites/recents | Not implemented. Bounded navigation was synthetic; real copy/paste and an accessible-name journey were not verified. |
| Automatic insertion host, live clipboard listener and history UI | Not implemented. No real clipboard test, listener or real-app insertion matrix ran. |
| Real hotkey summon/hide, conflicts and rebind | Physical checks not run; desktop rebinding UI is not implemented. Diagnostic availability is not a summon test. |
| Real launch, switch, foreground and focus return | Not verified, including minimized/denied cases and real Explorer reveal. Packaged switch is explicitly unsupported. |
| Screen reader, physical keyboard and IME | Not verified. F08 remains open: NVDA/Narrator, setup focus announcements, active CJK composition, selection/copy and recovery. |
| Setup and preferences | Full multi-screen onboarding is not implemented; manual complete/reset/restart, stored settings display, and closing/resetting with a launch in flight remain unchecked. |
| Physical ENOSPC and power-loss durability | Not verified. SQLite page cap and fault injection are not disk exhaustion or power loss; rollback-failure branch remains uninduced. |
| Generic record/cache encryption integration | Not implemented; do not put secrets in generic storage. DPAPI vault and clipboard coverage does not change this. |
| Watcher/icon desktop wiring | Not implemented. Bounded platform example and shell ICO extraction are not desktop consumer evidence. |
| Two-week dogfood and clean-machine install | Not done. Install/upgrade/uninstall, manual crash/restart, daily-driver triage and other host/accessibility lifecycle checks remain open. |

**Bottom line:** the frozen implementation passed the local automated checks. Do not call it release-ready or close the human acceptance gates based on these results.
