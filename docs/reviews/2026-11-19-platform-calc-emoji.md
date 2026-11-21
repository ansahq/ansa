# Platform discovery, calculator and emoji review — ANS-16 / L02, ANS-21 / L07, ANS-24 / L08 (2026-11-19)

**Scope:** the Windows adapter and platform contract (`crates/platform-windows/**`,
`crates/platform-api/src/lib.rs`), the calculator and corpus
(`crates/core/src/calculator.rs`, `crates/core/src/query.rs`,
`crates/core/tests/calculator_corpus.rs`, `crates/core/data/calculator_corpus.json`),
and the emoji crate (`crates/emoji/**`). `crates/alpha/src/catalog.rs`,
`crates/ui-gpui/src/lib.rs`, `apps/cli/src/main.rs` and `crates/domain` were read only
where the in-tree consumer affects acceptance. No implementation or test file was
modified. The only repository file written is this review.

**Tickets:**

| Ticket | Task id | Code under review |
|---|---|---|
| ANS-16 / L02 — application discovery, identity and launch | `ux81y5ueajzndthj8zwbhnum` | `crates/platform-windows/**`, `crates/platform-api/src/lib.rs` |
| ANS-21 / L07 — calculator policy and corpus | `rx91ssz7ec1vifm3lvyvepat` | `crates/core/src/calculator.rs`, `crates/core/tests/calculator_corpus.rs`, `crates/core/data/calculator_corpus.json` |
| ANS-24 / L08 — emoji and symbol search | `ehg7c2paupi1yducu26oxh7x` | `crates/emoji/**` |

**Reviewer:** independent subagent.

**Bottom line:** **No BLOCKER and no memory-safety defect was found in the exercised
paths.** The Shell Link parser survived a 120,000-input random/truncation/mutation
probe without a panic, the packaged enumeration is fixed-command and allowlisted,
and the FFI bindings have no obvious handle leak or buffer-size error. Three
acceptance-level issues are HIGH: a valid installed shortcut (Riot Client) is
misclassified as broken and cannot be launched because relative-path resolution is
wrong; deduplication ignores shortcut arguments and collapses genuinely distinct
commands (the 7 resolvable `cmd.exe` shortcuts became one, 3 `powershell.exe` shortcuts
became one; the IDList-only `Command Prompt` remains separate);
and the calculator product path uses `f64.to_string()`/`evaluate` instead of
`format_result`/`evaluate_extended`, so the display policy is bypassed and unit
results lose their unit. The remaining issues are MEDIUM/LOW, including unstable
identity for a direct `.exe` when an alias is added/removed, UNC path collisions in
identity normalization, a `load_from_json` checksum-pin bypass in emoji, and
calculator scientific output that cannot be parsed back.

---

## Commands and results

Run from the repository root on 2026-11-19, offline, Rust 1.99.1 (toolchain in
`rust-toolchain.toml`).

| Command | Result |
|---|---|
| `cargo test -p ansa-platform-windows -p ansa-platform-api -p ansa-core -p ansa-emoji --offline` | **PASS** — `ansa-core` 46 unit tests, calculator corpus 5, `ansa-emoji` 7 unit + 11 integration, `ansa-platform-api` 6, `ansa-platform-windows` 93; 0 failed/ignored; core doctest 1 passed; other doctest suites 0. |
| `cargo clippy -p ansa-platform-windows -p ansa-platform-api -p ansa-core -p ansa-emoji --all-targets --offline -- -D warnings` | **PASS** — after `cargo clean -p` of the four packages, all four were checked; finished with no warnings/errors. |
| `cargo fmt --check -p ansa-platform-windows -p ansa-platform-api -p ansa-core -p ansa-emoji` | **PASS** — exit 0, no diff. |
| `cargo run -q -p ansa-platform-windows --example catalog --offline` | **PASS** — `total=143 classic=95 packaged=48 broken=3 duplicate_titles=1`; `packaged_capability=available catalog=available`; measured 2.28 s including the capability probe. |
| `cargo run -q -p ansa-cli --offline -- calc ...` | **PASS (build) / finding reproduced** — `calc "0.1 + 0.2"` printed `0.30000000000000004`, `calc "1 / 3"` printed `0.3333333333333333`, `calc "10 km to mi"` printed `6.2137119223733395`, and `calc "20 C to F"` printed `68` (no `F`); see H3. The CLI now uses `launch_or_switch` (`apps/cli/src/main.rs:71`). |

Independent probes were compiled outside the repository under
`C:\Users\abhir\AppData\Local\Temp\ansa-probe`,
`...\ansa-lnk-fuzz` and `...\ansa-startmenu-probe`, with path dependencies on the
worktree crates or `#[path]` inclusion of the unmodified source files. No repository
test or implementation file was added or changed.

Additional probes:

- Shell Link parser: 120,000 random byte strings (lengths 0–599), every truncation of
  up to 40 real Start Menu `.lnk` files, 64 byte-mutation variants per file, and
  hand-built LinkInfo offset/size edge cases. **No panic; no out-of-bounds read.**
- Calculator: 200,000 pseudo-random ASCII expressions (length 0–79) through the public
  API; **no panic** (1 success, 199,999 typed errors).
- Real machine: 143 discovered entries (95 classic, 48 packaged, 3 broken); duplicate
  target groups include 8 `cmd.exe` shortcuts (7 resolvable, 1 IDList-only), 3
  `powershell.exe`, 3 VLC, 2 EA, 2 Riot and 2 Discord shortcuts.
- Identity probe: a direct `Example.exe` gets `app:9036b9b112ee45182281bdce81b50e68`
  with an `Example.lnk` alias present and `app:ac01e0abfd41000901d7c04669dbd5b9`
  after removing the alias.
- Path identity probe: `\\server\share\app.exe` and `\server\share\app.exe` normalize
  to the same id (`app:455985d8adf223ae3d40d13424d0e33e`).
- Junction-root probe: discovery over a junction root returns **0 entries** even though
  the target directory contains an `.lnk`.
- Emoji pin probe: a copy of `emoji.json` with `entries[0].name = "Tampered Name"`,
  `version = "evil-9.9.9"` and a recomputed `checksum` was accepted by
  `load_from_json` (`version=evil-9.9.9`, checksum `cef310e13f269ead`).

---

## Findings

### HIGH

#### H1 — A valid shortcut is marked broken and cannot be launched (relative-path resolution)

**Where:** `crates/platform-windows/src/shell_link.rs:183-185` (relative-path fallback),
`crates/platform-windows/src/shell_link.rs:197-205` (`join_relative`),
`crates/platform-windows/src/start_menu.rs:259-269` (mark broken when target is not a
file), `crates/platform-windows/src/launch.rs:140-150` (refuse to launch).

**Evidence (verified on the real machine):** the Start Menu shortcut
`C:\ProgramData\Microsoft\Windows\Start Menu\Programs\Riot Games\Riot Client.lnk`
has no LinkInfo; its StringData is:

```text
RelativePath = ..\Riot Games\Riot Client\RiotClientServices.exe
WorkingDir   = C:/Riot Games/Riot Client
IconLocation = C:/Riot Games/Riot Client/RiotClientServices.exe
```

The parser joins RelativePath against WorkingDir and produces
`C:/Riot Games/Riot Client\..\Riot Games\Riot Client\RiotClientServices.exe`, which
does not exist. The real target `C:\Riot Games\Riot Client\RiotClientServices.exe`
does exist, and `WScript.Shell.CreateShortcut` resolves the target correctly. The
catalog marks the app broken, and both
`open_application(Riot Client.lnk)` and `launch_or_switch(...)` return
`NoTarget { reason: "shortcut target ... no longer exists; ..." }`.

**Impact:** a valid installed application is reported as missing and cannot be
launched. This also violates “missing, moved or uninstalled targets produce
actionable errors” because the error is false. The incorrect target string is also
used to derive the app id and icon metadata.

**Minimal fix:** make relative resolution aware of the `.lnk` location (pass the
link's parent directory into `parse_shell_link`/`ShellLinkInfo`, or resolve
`relative_path` in `build_shortcut_candidate` against `path.parent()`), try both the
shortcut parent and the working directory, and if no candidate exists do **not**
treat the relative path as authoritative: return `target_path = None` and let the
shell launch the `.lnk` by its IDList. Alternatively parse the LinkTargetIDList or
use `IShellLink`/`SHGetPathFromIDList`. Add a regression fixture from this real link.

**Status:** verified on the real machine; fix not implemented.

#### H2 — Deduplication by target collapses distinct commands with different arguments

**Where:** `crates/platform-windows/src/start_menu.rs:271-278` (dedupe key is only the
resolved target), `crates/platform-windows/src/start_menu.rs:324-360` (group winner
and id), `crates/platform-windows/src/shell_link.rs:171-181` (arguments are parsed),
`crates/platform-windows/src/start_menu.rs:252-298` (arguments are never used),
`crates/platform-windows/src/types.rs:16-37` (`AppEntry` has no arguments field).

**Evidence (verified on the real machine):** the Start Menu contains these
same-target groups:

| Target | Shortcuts | Resulting catalog |
|---|---|---|
| `C:\Windows\System32\cmd.exe` | 7 resolving (`Node.js command prompt`, 5 Visual Studio tool prompts, `Install Additional Tools for Node.js`); `Command Prompt` is IDList-only and stays separate | 1 entry for the 7, `Developer Command Prompt for VS 2022`; `Command Prompt` retained separately |
| `...\SysWOW64\WindowsPowerShell\v1.0\powershell.exe` | 3 (`Windows PowerShell (x86)`, `Debuggable Package Manager`, `Developer PowerShell for VS 2022`) | 1 entry, `Debuggable Package Manager`; the System32 `Windows PowerShell` remains separate |
| `C:\Program Files\VideoLAN\VLC\vlc.exe` | 3 (plain, `-Iskins`, reset-config) | 1 entry, plain VLC |
| `...\EA Desktop\EALauncher.exe` | 2 (`EA`, `-recovery`) | 1 entry, `App Recovery` |
| `...\Riot Client\RiotClientServices.exe` | 2 (`Riot Client`, `VALORANT ...`) | 1 entry, broken `Riot Client` |
| `...\Discord\Discord.exe` | 2 (plain, `--processStart Discord.exe`) | 1 entry |

The same-target rule is reasonable for aliases that differ only in title, but these
are distinct commands. The winner is chosen by title/path, not by any intent or
argument semantics, so a user can lose the Node.js/Visual Studio command prompts,
the separate `Windows PowerShell` entry, the VLC reset action, the EA launcher, and
the VALORANT entry. (The plain `Command Prompt` survives only because its shortcut
has no resolvable target and therefore stays outside the `cmd.exe` group.)

**Impact:** installed commands silently disappear from the catalog; “same
application appears once” is satisfied only by over-merging. The app identity cannot
distinguish these commands because `AppEntry` exposes neither arguments nor working
directory.

**Minimal fix:** include normalized `arguments` (and possibly working directory) in
`dedupe_key` and in the identity namespace, or deduplicate only when both target and
arguments match; add `arguments`/`working_directory` to `AppEntry` if the UI is meant
to expose them. Add fixtures for identical target + different arguments.

**Status:** verified on the real machine; fix not implemented.

#### H3 — The calculator’s product path bypasses the display policy and loses units

**Where:** `crates/core/src/query.rs:6-7` (`QueryRoute::Calculator(Result<f64,
CalcError>)`), `crates/core/src/query.rs:16,29` (uses `evaluate`, not
`evaluate_extended`), `crates/ui-gpui/src/lib.rs:345` (copies
`value.to_string()`), `crates/ui-gpui/src/lib.rs:889` (renders
`calculation...to_string()`), `apps/cli/src/main.rs:43-46` (prints the raw `f64`).
`crates/alpha/src/catalog.rs:176-178` calls `format_result`, but `route_query` has
already discarded the `CalcResult::Quantity` unit.

**Evidence (verified by probe):**

```text
"0.1 + 0.2"       route f64 to_string=0.30000000000000004   format_result="0.3"
"1 / 3"           route f64 to_string=0.3333333333333333    format_result="0.333333333333"
"=10 km to mi"    route f64 to_string=6.2137119223733395   format_result="6.21371192237"
"=20 C to F"      route f64 to_string=68.0                  (no "F" unit)
"=1e15"           error UnexpectedCharacter { character: 'e', position: 1 }
```

The core corpus checks `format_result` and `evaluate_quantity`, but the desktop and
CLI do not use either for the routed result. The Settings text at
`crates/ui-gpui/src/lib.rs:373` still says “No units, currencies, or dates yet.”

**Impact:** the user-visible result is not byte-identical to the corpus policy
(`0.30000000000000004`, 16-digit thirds), and a successful unit conversion displays a
bare magnitude with no unit, which is semantically wrong for `20 C to F` and
`10 km to mi`. This is an acceptance-level partial implementation of L07: the
library and corpus are right, the product path is not.

**Minimal fix:** carry `CalcResult` or a formatted string in `QueryRoute` and use
`CalcResult::format()`/`format_result` in UI/CLI. Keep `evaluate` if a raw `f64` is
needed for other callers, but do not use it for display. Update the Settings text.
Add an integration test that exercises `route_query` + the UI formatting path, not
only the core helper.

**Status:** verified by source inspection, the probe, and direct CLI execution. The
desktop UI was not launched, but its display/copy code is the same raw `f64` path.

### MEDIUM

#### M1 — Direct `.exe` identity changes when a shortcut alias is added or removed

**Where:** `crates/platform-windows/src/start_menu.rs:232-245` (direct `.exe` sets
`target_id: None`), `crates/platform-windows/src/start_menu.rs:275-278` (shortcut
target id is only set when not broken), `crates/platform-windows/src/start_menu.rs:352-357`
(the group takes the first target id even if the winner is a direct `.exe`).

**Evidence (verified by probe):** a direct `Example.exe` got
`app:9036b9b112ee45182281bdce81b50e68` while `Example.lnk` was present and
`app:ac01e0abfd41000901d7c04669dbd5b9` after the alias was removed. The winner and
the path were unchanged; only the group composition changed.

**Impact:** favorites/recents/selection identity can change across a refresh when a
duplicate shortcut is installed or removed, even though the executable did not move.
This contradicts “stable platform identity” and can invalidate a selection token.

**Minimal fix:** derive the target id for direct `.exe` entries too (its target is
itself) and always use the target-derived id when a target is known; do not make id
choice depend on whether another candidate happens to be in the group. Add a test
that removes the alias and asserts the id is unchanged.

**Status:** verified by probe.

#### M2 — Identity normalization collapses UNC prefixes and does not normalize `..`

**Where:** `crates/platform-windows/src/start_menu.rs:406-429`; the loop replaces
every `/` with `\` and suppresses every repeated `\`, including the leading `\\` of
UNC and `\\?\` paths; `.`/`..` components and trailing dots/spaces are not
normalized.

**Evidence (verified by probe):** `\\server\share\app.exe` and
`\server\share\app.exe` both produce
`app:455985d8adf223ae3d40d13424d0e33e`. This is a false merge; conversely, two
spellings of the same target with `..` produce different ids/dedupe keys.

**Impact:** identity collisions for UNC/extended-length paths and missed
deduplication for lexically equivalent paths. Real Start Menu shortcuts can target
UNC or extended paths.

**Minimal fix:** preserve a leading `\\` (and `\\?\`) prefix, normalize `.`/`..`
components and trailing separators/dots/spaces, or derive identity from
`GetFullPathNameW`/`GetFinalPathNameByHandleW` where a file handle is available.
Add UNC and `..` fixtures.

**Status:** verified by probe for the UNC collision; `..` behavior verified from the
Riot target string.

#### M3 — Shell launch failures are classified as `NoTarget`

**Where:** `crates/platform-windows/src/launch.rs:56-61` (all `execute` errors become
`NoTarget` with a refresh hint), `crates/platform-windows/src/launch.rs:64-74`
(`execute` returns the `ShellExecuteW` error as `Err(String)`),
`crates/platform-windows/src/types.rs:79-83` (`NoTarget` means “no valid target”).

**Impact:** an access-denied launch (`ShellExecuteW` code 5), missing file
association (31), or other shell failure is reported to the user as “No launch
target ... refresh the application catalog”, which is misleading and does not
distinguish launch failure from a missing target. The acceptance criterion “launch
and switch outcomes are distinguished” is only partially met: switch outcomes are
distinct, but launch failures are not.

**Minimal fix:** add a `LaunchFailed { reason }` outcome (or map shell errors to
`Denied` when appropriate) and reserve `NoTarget` for validation/existence checks.
Keep the refresh hint only for actual missing/moved targets. Add a test with a
`ShellExecuteW` failure (for example an existing file with no handler) that does not
launch a real process.

**Status:** code-verified; an actual shell failure was not induced to avoid launching
a process.

#### M4 — Directory listing allocates the entire child set before the entry budget applies

**Where:** `crates/platform-windows/src/start_menu.rs:178-182`
(`read_dir(...).collect()`), and the same pattern in
`crates/platform-windows/src/watch.rs:232-238`.

**Impact:** `DEFAULT_MAX_ENTRIES`/`MAX_WATCH_ENTRIES` are checked only when an item
is popped from the stack. A single Start Menu directory containing millions of
entries is fully collected into `Vec<PathBuf>` first, so memory and time are not
bounded by the advertised limits. This matters for robustness against a corrupted
or adversarial profile, not for a normal Start Menu.

**Minimal fix:** stream `read_dir`, enforce a per-directory and global child budget
while iterating, and push only remaining budget items; or cap the collected children
at `max_entries - inspected`. Add a test with a directory containing more files than
the budget.

**Status:** code-verified; no huge real directory was created.

#### M5 — PowerShell is resolved through PATH/CWD, not an absolute system path

**Where:** `crates/platform-windows/src/packaged.rs:291-311`, specifically
`Command::new("powershell.exe")` at line 292.

**Impact:** the command string is fixed and no metadata is interpolated, but
`CreateProcessW` searches the application directory, the current directory, the
system directories and PATH. A planted `powershell.exe` in a user-writable current
directory (for example when the launcher is started from Downloads) or in a
user-writable PATH entry would be executed instead of Windows PowerShell. This is a
local code-execution/defense-in-depth issue, not a remote injection; the same-user
attacker model already has broad rights, but the adapter’s “fixed command” claim is
not sufficient on its own.

**Minimal fix:** launch
`%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` via
`GetSystemDirectoryW` (or a validated `SystemRoot` path) rather than a bare name.
Keep the fixed command string and `CREATE_NO_WINDOW`.

**Status:** code-verified; not exploited.

#### M6 — Root reparse points are silently skipped, contrary to the comment

**Where:** `crates/platform-windows/src/start_menu.rs:125-135` (accessibility uses
`fs::metadata`, which follows links), `crates/platform-windows/src/start_menu.rs:154-163`
(traversal uses `fs::symlink_metadata` and only exempts `depth > 0`), and
`crates/platform-windows/src/watch.rs:218-227`.

**Evidence (verified by probe):** a junction root whose target contains `App.lnk`
produced **0** catalog entries. `symlink_metadata` reports the junction as
`is_dir=false, is_file=false, is_symlink=true`, so the root is not traversed. The
comment says “Roots are allowed to be reparse points”.

**Impact:** redirected `APPDATA`/`ProgramData` profiles (junction or symlink
redirection) can yield an empty catalog with `Ok([])` instead of an error. The
accessibility check counts the root as accessible because it follows the link, so
this is silent.

**Minimal fix:** for `depth == 0`, use `fs::metadata` (follow) to decide whether the
root is a directory, while continuing to reject reparse points below the root; keep
the visited-directory set to prevent cycles. If following is not desired, return an
explicit error instead of an empty successful scan.

**Status:** verified by probe.

#### M7 — CatalogWatcher is not wired into any consumer, and packaged installs are not watched

**Where:** `crates/platform-windows/src/watch.rs:1-11` documents “the UI calls
`CatalogWatcher::poll`”; the only constructor uses are in the crate’s own tests.
A repository-wide search found no `CatalogWatcher` use in `apps/**` or `crates/ui-gpui/**`.
`crates/platform-windows/src/watch.rs:202-260` snapshots only Start Menu roots.

**Impact:** installing or uninstalling an application while the launcher is running
does not auto-refresh; the desktop relies on the manual `RefreshApplications`
builtin. Packaged apps are not represented by Start Menu files and are never
noticed by the watcher. The acceptance criterion “refresh after install or
uninstall” is only partially implemented.

**Minimal fix:** instantiate the watcher in the UI timer loop, poll with the
configured interval, and trigger a catalog rescan on `RescanNeeded`; add a packaged
app signal (for example a periodic packaged enumeration or a registry/AUMID check),
or document that packaged installs require a manual refresh.

**Status:** code-verified; no real install/uninstall cycle was run.

#### M8 — Minimized windows are never candidates for switch

**Where:** `crates/platform-windows/src/switch.rs:48-61` filters out
`window.minimized`.

**Impact:** clicking an application whose only running window is minimized starts a
new instance instead of restoring the existing one. “Switch to an existing window
where the platform permits” is not met for a very common state. The same applies to
hidden windows, which may be appropriate, but minimized windows are ordinary
existing windows.

**Minimal fix:** include minimized top-level windows in candidate selection, call
`ShowWindow(hwnd, SW_RESTORE)` before `SetForegroundWindow`, then verify foreground.
Keep hidden windows excluded unless there is a separate policy for tray windows.

**Status:** code-verified; no real minimized-window switch was performed.

#### M9 — `format_result` violates its own 12-significant-digit display policy at large magnitudes

**Where:** `crates/core/src/calculator.rs:65-69` (documented policy),
`crates/core/src/calculator.rs:88-97` (`FORMAT_SIGNIFICANT_DIGITS`,
`FORMAT_PLAIN_MAX_MAGNITUDE`), `crates/core/src/calculator.rs:290-306`
(`decimals = (12 - 1 - exponent).clamp(0, 20)`).

**Evidence (verified by probe):**

```text
format_result(123456789012345.0) = "123456789012345"   (15 significant digits)
format_result(999999999999999.9) = "1000000000000000"  (plain, at/above the 1e15 boundary)
format_result(123456789012.5)    = "123456789012"      (integer rounding at 1e11)
```

Because `decimals` cannot be negative, values with decimal exponent 11–14 are
rendered with all their integer digits (up to 15) rather than rounded to 12
significant digits. The 1e15 boundary is also crossed after rounding while the code
still uses plain formatting.

**Impact:** the documented precision/display claim is false for large values; f64
rounding noise is exposed at magnitudes where the policy promises only 12
significant digits. It does not corrupt arithmetic results, but it undermines the
“explicit precision” acceptance goal. The corpus only tests `1e12` and `1e15`, so it
does not catch the 1e11–1e15 band.

**Minimal fix:** either round to 12 significant digits by scaling to the appropriate
power of ten (allowing negative decimals), or move the plain/scientific boundary
down to below `1e12` and document the change. Add boundary corpus cases for
`1e11`, `1e12`, `123456789012345`, and `1e15 - 1`.

**Status:** verified by probe.

#### M10 — Scientific display output cannot be parsed back

**Where:** `crates/core/src/calculator.rs:291` emits `format!("{value:e}")`;
`crates/core/src/calculator.rs:828-861` only accepts a digit/`.` number literal
followed by a unit/constant; `crates/core/src/calculator.rs:863-894` has no exponent
grammar.

**Evidence (verified by probe):**

```text
format_result(1e15)  = "1e15"  -> evaluate("1e15")  = Err(UnexpectedCharacter { character: 'e', position: 1 })
format_result(1e-10) = "1e-10" -> evaluate("1e-10") = Err(UnexpectedCharacter { character: 'e', position: 1 })
```

**Impact:** a user who copies a displayed result back into the calculator gets an
error. Any future UI that adopts `format_result` inherits the problem; the corpus
records the scientific strings but never round-trips them.

**Minimal fix:** accept an optional `e`/`E` exponent (with optional sign) in
`parse_number_literal`, or avoid emitting scientific notation for values that can be
displayed in the documented plain range. Add a formatting→evaluate round-trip
corpus assertion.

**Status:** verified by probe.

#### M11 — `load_from_json` does not enforce `PINNED_CHECKSUM`

**Where:** `crates/emoji/src/dataset.rs:180-190` compares `raw.checksum` only with the
checksum recomputed from the same document; `crates/emoji/src/dataset.rs:277-279`
exposes that path as `load_from_json`; `crates/emoji/data/README.md:45-46` claims the
file and `PINNED_CHECKSUM` must agree.

**Evidence (verified by probe):** changing `entries[0].name` to `Tampered Name`,
`version` to `evil-9.9.9`, recomputing the canonical FNV-1a checksum and writing it
into the JSON caused `load_from_json` to return
`Ok(version=evil-9.9.9, name=Some("Tampered Name"), checksum=cef310e13f269ead)`.
The embedded dataset is still protected by the test
`embedded_dataset_loads_and_is_pinned`, which compares `dataset.checksum()` with
`PINNED_CHECKSUM`; the public loader is not.

**Impact:** the documented pin rule is not enforced for the public loader. A host
tool or test that loads an external copy can accept a tampered or stale dataset as
long as its self-declared checksum is recomputed. The dataset is embedded in normal
query paths, so this is an integrity/API issue rather than a query-time network or
code-execution issue.

**Minimal fix:** in `Dataset::from_json`, compare both the recomputed checksum and
`raw.checksum` with `PINNED_CHECKSUM`; add a separate `from_json_unpinned` for the
regeneration workflow if needed. Make the README wording match the actual check.

**Status:** verified by probe.

### LOW

- **L1 — `5 m/s` reports `UnknownUnit { unit: "s" }`.** `s` is a supported unit, but
  `parse_identifier_value` (`crates/core/src/calculator.rs:813-825`) treats a bare
  identifier as an unknown unit. The error is typed but misleading. Fix: recognize
  supported units after `/` and return `Unsupported { reason: "unit quotients ..." }`
  or a clearer error; add a corpus case.
- **L2 — Empty favorite identity is accepted.** `Collections::toggle_favorite("")`
  returns `true` and `is_favorite("")` is `true`, while `record_recent("")` is
  rejected (`crates/emoji/src/collections.rs:28-35,41-50`). Fix: reject an empty id in
  `toggle_favorite`.
- **L3 — Punctuation-only emoji queries return the full catalog.** `search("-")`
  normalizes to an empty token list (`crates/emoji/src/search.rs:78-93,182-199`) and
  enters the empty-query branch, returning the first `limit` entries. Fix: if the raw
  query is non-empty but normalization produced no tokens, return no hits.
- **L4 — `load_from_json` has no input size or entry cap.** `crates/emoji/src/dataset.rs:180-191`
  parses an arbitrary caller-provided string and builds an index proportional to
  entries. The embedded path is small, but the public tooling API is unbounded. Fix:
  cap bytes/entries or document that callers must bound it.
- **L5 — FNV-1a 64 is not collision-resistant.** `crates/emoji/src/dataset.rs:364-371`.
  The checksum is an integrity pin for a locally embedded dataset, not a security
  boundary, but a deliberate collision is cheap. If the pin must resist tampering, use
  SHA-256 or a signature.
- **L6 — Timeout kill/wait can block past the deadline.** `crates/platform-windows/src/packaged.rs:347-357`
  ignores `child.kill()` failure and then calls blocking `child.wait()`; an
  un-killable child or a child holding inherited pipe handles can outlive the
  advertised timeout. Fix: use `try_wait` with a deadline and abort/close pipes
  explicitly; keep the reader joins.
- **L7 — `host_capabilities` performs a full packaged enumeration on every call.**
  `crates/platform-windows/src/packaged.rs:87-97` calls `enumerate()`; measured
  **866.7 ms** per `host_capabilities()` on this machine. A UI that probes
  capabilities on a keystroke or startup path can block up to the 8 s timeout. Fix:
  cache a successful/failed probe with a short TTL or split “is PowerShell present”
  from “enumerate now”.
- **L8 — Non-Unicode entry paths are silently dropped.** `crates/platform-windows/src/start_menu.rs:198-202`
  skips a discovered file whose path cannot be represented as `&str`, instead of
  retaining it as broken. This is rare (unpaired surrogate paths) but contradicts
  “broken entries stay visible”. Fix: keep the path as `OsString`/`PathBuf` in a
  platform-neutral field or retain a broken entry with a reason.
- **L9 — UI Settings still claims “No units”.** `crates/ui-gpui/src/lib.rs:373`
  says “Calculator: ... No units, currencies, or dates yet.” Units are now implemented
  in the core/corpus; the text is stale and misleading. Fix: update the Settings text
  and the calculator hint (`crates/ui-gpui/src/lib.rs:928`).
- **L10 — `set_launcher_visible(hide)` does not confirm the window became hidden.**
  `crates/platform-windows/src/visibility.rs:346-353` calls `ShowWindow(SW_HIDE)` and
  returns `Ok` without checking `IsWindowVisible`. `focus::hide_launcher` does check.
  Fix: verify after the call or document the best-effort behavior.
- **L11 — Calculator corpus lacks boundary, scientific round-trip and property tests.**
  `crates/core/tests/calculator_corpus.rs` is table-driven only; the ticket test plan
  mentions property tests for parser bounds/associativity and fuzzing malformed
  expressions. The unit tests cover many cases and the ad-hoc 200k fuzz probe found
  no panic, but no checked-in property/fuzz test would prevent regressions. Fix: add
  proptest-style parser invariants and the boundary/round-trip cases from M9/M10.
- **L12 — `AppEntry` does not expose arguments/working directory despite the
  capability note.** `crates/platform-windows/src/lib.rs:223` says “Working directory,
  arguments and icon location are exposed as metadata”; only `icon_source` exists in
  `crates/platform-windows/src/types.rs:16-37`. This overclaims the API and is the
  same root cause as H2. Fix: add fields (and use them in identity) or correct the
  note.
- **L13 — Image/icon extraction, reveal-in-file-manager and copy-identity are not in
  the adapter.** `crates/platform-windows/src/lib.rs:29-33` explicitly says icon pixel
  extraction is not implemented; `crates/platform-api/src/lib.rs:254-293` has no
  reveal or copy function; the desktop only copies a path in `copy_target`
  (`crates/ui-gpui/src/lib.rs:435-452`). These are scope items rather than acceptance
  criteria, and the icon limitation is honestly disclosed, but they remain partial.
- **L14 — Percent is postfix `/100` only, so `200 + 10%` is `200.1`, not `220`.** The
  behavior is documented (`crates/core/src/calculator.rs:24-28`) and tested, but no
  corpus case covers additive percentage. If the product intends common calculator
  semantics, this policy needs an explicit decision and a UI hint.
- **L15 — Emoji activation is a pure decision helper, not a clipboard/insertion
  implementation.** `crates/emoji/src/lib.rs:57-71` maps two booleans to
  `Copied`/`Inserted`/`CopyFallback`; no code copies to the clipboard or verifies a
  real insertion target. The desktop does not expose emoji browsing
  (`crates/ui-gpui/src/lib.rs:373`), and the CLI only prints results. The
  `CopyFallback` policy is correct, but the acceptance criteria “copy by default” and
  “insertion failure reports copy-only fallback” are only modeled, not delivered.

---

## Acceptance disposition

Status values: **PASS** (criterion exercised and met), **PARTIAL** (implemented but a
finding or missing integration prevents the stated behavior), **FAIL** (not met),
**UNVERIFIED** (no real-machine/UI exercise possible).

### ANS-16 / L02

| Criterion | Status | Why |
|---|---|---|
| Same application through multiple shortcuts appears once with deterministic identity | **PARTIAL** | Exact-target aliases collapse deterministically (tests and repeated scans pass), but distinct commands with different arguments are over-merged (H2), and direct `.exe` identity depends on whether an alias exists (M1). UNC identity can collide (M2). |
| Missing, moved or uninstalled targets produce actionable errors, not silent no-ops | **PARTIAL** | Real missing Discord/Steam shortcuts are retained and report a refresh hint; malformed shortcuts are retained. Riot Client is falsely marked broken despite an existing target (H1), and shell failures are reported as `NoTarget` (M3). |
| Discovery remains responsive with a large installed set and refreshes without duplicating rows | **PARTIAL** | 143 entries discovered in the real catalog with no duplicate ids; repeated scans are idempotent in tests. A single huge directory can allocate its entire child list before the entry budget (M4), and the watcher is not connected to the UI (M7). |
| Launch and switch outcomes are distinguished; an unverified switch is not reported as success | **PARTIAL** | `LaunchOutcome`/`is_success` and the desktop UI distinguish `Launched`, `Switched`, `Denied`, `Unverified`, `NoTarget`; `Unverified` is never success. Packaged switching is not implemented, minimized windows are never switched to (M8), and launch failures are collapsed into `NoTarget` (M3). |
| Scope: packaged applications enumerated and launched by AUMID | **PASS (real machine)** | 48 packaged entries from `Get-StartApps`; strict AUMID allowlist; fixed command; launch path `shell:AppsFolder\<AUMID>`; capability reported available. |
| Scope: icon extraction | **FAIL / disclosed** | Only icon source path/index metadata; no pixel/bitmap extraction (`crates/platform-windows/src/lib.rs:29-33`). This is outside the acceptance criteria but inside the ticket scope. |
| Scope: reveal in file manager / copy identity details | **PARTIAL** | No reveal API exists; the desktop copies a target path/title (`copy_target`), not a structured identity. |
| Scope: live install/uninstall watching | **PARTIAL** | `CatalogWatcher` is bounded, debounced and tested, but no production consumer polls it; packaged installs are not observed. |
| Scope: switch to existing window | **PARTIAL** | Implemented for non-minimized, visible, non-own-process top-level windows and verified only with synthetic descriptors; no real-machine switch was performed. |

### ANS-21 / L07

| Criterion | Status | Why |
|---|---|---|
| All corpus cases produce byte-identical display output and typed errors | **PASS (core runner) / PARTIAL (product)** | The 5 corpus tests pass, including exact `format_result` strings and typed error codes. The desktop/CLI do not use `format_result` and lose units (H3), so the product does not reproduce the corpus display. |
| Unit and dimension errors are explicit; incompatible units never silently coerce | **PASS** | `UnitMismatch`, `UnknownUnit`, `ExpectedUnit`, temperature arithmetic rejection and no unit-product coercion are implemented and corpus-covered. `5 m/s` mislabels the known unit `s` as unknown (L1), but no silent coercion occurs. |
| Ordinary arithmetic works offline and within CPU/input limits | **PASS** | No shell/network in the evaluator; 256-character and 32-depth bounds; 200k-input fuzz probe found no panic; corpus covers length/depth/division/non-finite. |
| Unsupported natural-language phrasing is reported as unsupported, not partially executed | **PASS** | Parser is all-or-error; currency/date/natural-language token classification returns `Unsupported`; unsupported syntax never returns a partial value. |
| Precision/rounding policy documented; avoid binary floating-point surprises | **PARTIAL** | The policy is documented and `0.1+0.2` displays `0.3` through `format_result`, but the policy is not used by the product (H3), is false for magnitudes >= 1e11 (M9), and scientific output cannot be re-parsed (M10). |
| Explicit trigger; no shell/network | **PASS** | `route_query` requires `=` or an arithmetic-only digit-containing query; no process/network call. Units require the `=` path, and the UI hint is stale (L9), but the trigger exists. |

### ANS-24 / L08

| Criterion | Status | Why |
|---|---|---|
| Multi-codepoint emoji remain single graphemes through search, copy and paste | **PASS (data/search)** | 139 entries; every base and variant is one extended grapheme under `unicode-segmentation`; known ZWJ, flags, keycap, skin tone and variation-selector entries pass. No clipboard/insertion implementation exists to verify copy/paste end-to-end (L15). |
| Screen-reader names present and distinguishable for variants | **PASS** | `Entry::accessible_name`/`Variant::accessible_name`; distinct variant names enforced by validation and tested (waving hand has 5 skin-tone variants). |
| Dataset version and license appear in about or diagnostics | **PASS (API/CLI diagnostics)** | `Diagnostics` exposes version, Unicode version, source, license, license URL, checksum and count; `cargo run ... doctor`/`emoji` print them. The desktop About text is static, but the diagnostics surface exists. |
| Insertion failure reports copy-only fallback rather than claiming paste success | **PARTIAL** | `activation(true, false)` returns `CopyFallback`; `activation(true, true)` returns `Inserted` from caller-supplied booleans only. No actual clipboard/insertion code or target verification exists (L15). |
| No network at query time; pinned by version and checksum | **PARTIAL** | Embedded `include_str!` and no network dependencies; embedded checksum matches `e80345152b0e31eb` and tests pin it. `load_from_json` accepts a recomputed self-consistent checksum for a tampered/other version (M11); FNV is non-cryptographic (L5). |
| Favorites and recents can be cleared independently of other command history | **PASS (API) / PARTIAL (product)** | `Collections` clears recents/favorites independently, bounds both at 64 and tests it; it persists nothing and is not wired into the desktop, and `toggle_favorite("")` accepts an empty id (L2). |
| Scope: licensed, versioned source with update process | **PASS** | Unicode License v3 file, README source/version/license/checksum/update process; `emoji-1.0.0`/Unicode 16.0/CLDR 46 metadata is consistent with the data. |
| Scope: index names, keywords, groups, variants | **PASS** | Search normalizes names, keywords, groups and variant words; exact grapheme, typo (one edit) and empty-query paths are deterministic and bounded. |
| Scope: copy by default, insertion only when permitted | **FAIL / not integrated** | Only the policy helper exists; desktop emoji browsing and clipboard/insertion are explicitly not implemented (`crates/ui-gpui/src/lib.rs:373`). |

---

## Environment note and residual risks

**Environment note:** the working tree changed during this review (concurrent
in-progress work in the registry, CLI and UI). All results above were rechecked
against the final state: the requested four-package tests/clippy/fmt pass,
`ansa-cli` builds, `ansa-registry` tests pass, and the CLI `calc` commands reproduce
H3. The desktop UI was not launched, so real-machine switch/foreground and emoji
copy/insertion remain unverified. The catalog example and the independent platform
probes did run.

**Residual risks / unverified behavior:**

1. **Real-machine switch and foreground restoration were not exercised.** The switch
   tests use synthetic `WindowDescriptor` values; `SetForegroundWindow` denial and
   real HWND ownership were not tested. No real launch/reveal was performed to avoid
   launching applications during review.
2. **The Shell Link parser was fuzzed, but the packaged JSON parser was not fuzzed**
   independently beyond its unit tests. The parser has depth/size caps and explicit
   UTF-8/UTF-16 checks; a broader stateful fuzz would add confidence.
3. **PowerShell resolution and environment dependence:** `Get-StartApps` is available
   and returned 48 packaged entries on this machine, but locale, PowerShell version,
   execution policy, encoding and PATH are machine-dependent. The current decoder
   handles UTF-8/UTF-16 and BOM/no-BOM samples; the command path is not absolute (M5).
4. **Profile redirection:** roots that are junctions are skipped silently (M6);
   enterprise-redirected AppData/ProgramData is a realistic trigger.
5. **Identity is path/locale-sensitive:** UNC normalization (M2), trailing
   dots/spaces, 8.3 aliases and Unicode case folding are not canonicalized. The FNV
   two-pass ID is deterministic but not a cryptographic identity.
6. **Huge directories:** the entry budget does not bound the initial `read_dir`
   collection (M4), so a pathological Start Menu can still cause a large transient
   allocation.
7. **Emoji dataset pin:** FNV-1a 64 is not collision-resistant, and the public
   `load_from_json` path does not enforce the compiled pin (M11).
8. **Emoji integration:** no desktop emoji surface, clipboard copy, insertion or
   persisted recents/favorites; `activation` is a policy model only (L15).
9. **Icon extraction, reveal-in-file-manager and structured copy identity remain
   unimplemented** (L13); the adapter honestly discloses the icon limitation, but the
   ticket scope is broader than the acceptance criteria.
10. **Desktop UI unexercised:** the desktop and CLI now both route applications
    through `launch_or_switch` (`crates/ui-gpui/src/lib.rs:409`,
    `apps/cli/src/main.rs:71`), but only the CLI build and the synthetic switch tests
    were run. Real HWND ownership/foreground denial and UIA behavior remain
    unverified.
11. **Docs drift:** `docs/implementation-status.md` still describes L02/L07 as old
    partial slices (no packaged apps/watcher/dedupe/switch; no units), and the
    desktop Settings text still says “No units”. These are not code defects but make
    the delivered state harder to assess from documentation.

**Positive verification (no defect found):**

- Shell Link bounds/offsets: every `Reader` take/skip checks length and overflow;
  LinkInfo size/header/offset checks are performed before slicing; UTF-16 and ANSI
  strings are terminated and bounded; truncation/mutation/random probes found no
  panic.
- Deterministic exact-target dedupe and broken-shortcut retention are covered by
  tests; repeated scans are byte-identical in the tested cases.
- Packaged enumeration uses a compile-time command, `-NoProfile -NonInteractive`,
  `CREATE_NO_WINDOW`, output caps, a timeout, joined reader threads, strict UTF-8/16
  decoding, JSON depth limits and a strict AUMID allowlist.
- FFI review: process handles are closed, buffers are sized before
  `QueryFullProcessImageNameW`, `EnumWindows` state is synchronous local state,
  `MSG` includes the modern private field, GDI regions are deleted on failure and
  transferred on success, and hotkey registration is unregistered before join. No
  handle leak or obvious memory-safety defect was found.
- Calculator unit factors are the standard exact definitions, temperature uses
  direct affine formulas, dimension mismatches are explicit, and the evaluator is
  bounded/offline with no panic in the probes.
- Emoji dataset checksum matches, every base/variant is one grapheme, variant
  accessible names are distinct, and search is deterministic and bounded with no
  network path.

---

## Appendix — selected probe output

**Real catalog (2026-11-19):**

```text
total=143
classic=95
packaged=48
broken=3
duplicate_titles=1
  duplicate_title="uninstall" count=2 (different targets; correctly retained)
packaged_capability=available catalog=available
```

**Duplicate target groups on this machine (WScript.Shell view):**

```text
EALauncher.exe          2: App Recovery (-recovery), EA
vlc.exe                 3: reset, -Iskins, plain
RiotClientServices.exe  2: Riot Client, VALORANT (--launch-product=valorant ...)
Discord.exe             2: plain, --processStart Discord.exe
cmd.exe                 8: Command Prompt (IDList-only), Node.js, 5 VS tool prompts, Install Additional Tools
powershell.exe          3: Windows PowerShell, Debuggable Package Manager, Developer PowerShell
```

**Riot link parser result:**

```text
RelativePath = ..\Riot Games\Riot Client\RiotClientServices.exe
WorkingDir   = C:/Riot Games/Riot Client
parser target = C:/Riot Games/Riot Client\..\Riot Games\Riot Client\RiotClientServices.exe
target exists = false; real target C:\Riot Games\Riot Client\RiotClientServices.exe exists = true
catalog = broken; open_application = Err("... no longer exists ...")
```

**Identity probe:**

```text
with Example.lnk    id=app:9036b9b112ee45182281bdce81b50e68
without Example.lnk id=app:ac01e0abfd41000901d7c04669dbd5b9
```

**Normalization probe:**

```text
\\server\share\app.exe -> app:455985d8adf223ae3d40d13424d0e33e
\server\share\app.exe  -> app:455985d8adf223ae3d40d13424d0e33e   (false merge)
```

**Junction-root probe:** `entries through junction root=0` (target contained
`App.lnk`).

**Emoji pin probe:**

```text
original canonical checksum=e80345152b0e31eb declared=e80345152b0e31eb
adjusted checksum=cef310e13f269ead
TAMPERED DATASET LOADED: version=evil-9.9.9 name=Some("Tampered Name") checksum=cef310e13f269ead
```

**Calculator probes:**

```text
format_result(123456789012345.0) = "123456789012345"  (15 digits)
format_result(999999999999999.9) = "1000000000000000" (plain at boundary)
1e15  -> "1e15"  -> evaluate = Err(UnexpectedCharacter { character: 'e', position: 1 })
1e-10 -> "1e-10" -> evaluate = Err(UnexpectedCharacter { character: 'e', position: 1 })
```

> The calculator/emoji values in the appendix above are the **pre-fix**
> observations. The re-review below supersedes them for ANS-21 and ANS-24; the
> original findings are intentionally preserved as the historical baseline.

---

## Calculator/emoji re-review after fixes (2026-11-19)

**Scope:** ANS-21 / L07 (`crates/core/src/calculator.rs`,
`crates/core/src/query.rs`, corpus) and ANS-24 / L08 (`crates/emoji/**`).
Platform H1/H2 were not re-audited in this section; see the platform section
below. No implementation or test file was modified.

### Commands and results

| Command | Result |
|---|---|
| `cargo test -p ansa-core -p ansa-emoji --offline` | **PASS** — `ansa-core` 52 unit tests, calculator corpus 8, `ansa-emoji` 7 unit + 15 integration, core doctest 1; 0 failed/ignored. |
| `cargo clippy -p ansa-core -p ansa-emoji --all-targets --offline -- -D warnings` | **PASS** — both crates checked; no warnings/errors. |
| `cargo fmt -p ansa-core -p ansa-emoji -- --check` | **PASS** — exit 0, no diff. |
| `cargo run -q -p ansa-cli --offline -- calc ...` | **PASS** — `0.1 + 0.2` → `0.3`; `1 / 3` → `0.333333333333`; `10 km to mi` → `6.21371192237 mi`; `20 C to F` → `68 F`; `1e15` → `1e15`; `1e309` → `scientific exponent is larger than 308` (exit 1). |

Independent probes were compiled outside the repository at
`C:\Users\abhir\AppData\Local\Temp\ansa-probe` (no repository test added).

### Calculator disposition

| Finding | Status | Evidence |
|---|---|---|
| **H3** — product path used raw `f64::to_string` and lost units | **FIXED** | `QueryRoute::Calculator` now carries `Result<CalcResult, CalcError>` and routes through `evaluate_extended` (`crates/core/src/query.rs:5-9,21-22,37-38`). The desktop copies/renders `value.format()` (`crates/ui-gpui/src/lib.rs:345,671`); CLI uses `evaluate_extended` + `format()` (`apps/cli/src/main.rs:45-47`); alpha uses `value.format()` (`crates/alpha/src/catalog.rs:367-372`). Probe routes: `0.1+0.2` → `0.3`, `=10 km to mi` → `6.21371192237 mi`, `=20 C to F` → `68 F`. Search found no remaining product path calling `to_string()` on a calculator result; `ui-gpui/src/lib.rs:443` copies the input expression, not the result. |
| **M9** — 12-significant-digit policy at large magnitudes | **FIXED** | `format_result` (`crates/core/src/calculator.rs:304-352`) rounds to 12 significant digits with a negative-shift quantizer; boundary test/probe: `1e11` → `100000000000`, `1e12` → `1000000000000`, `1e14` → `100000000000000`, `123456789012345` → `123456789012000`, `999999999999999.9` and `1e15-1` → `1e15`, negatives symmetric. Corpus version 1.2.0 and tests at `calculator.rs:1494-1504`. |
| **M10** — scientific output could not be parsed back | **FIXED** | `parse_number_literal` accepts bounded `e`/`E` exponents (`calculator.rs:912-975`, limit 308); round-trip corpus (`tests/calculator_corpus.rs:113-131`) and probe for `1e15`, `1.5e-3`, `+2E4`, `-2.5e2`, `1e308`, `1e-308` pass. `1e309`/`1e-309` return `ScientificExponentOutOfRange`; `1e`, `1e+`, `1efoo` return typed errors. A 300,000-input numeric fuzz probe produced 10,227 valid results with zero format→parse→format mismatches and zero panics. |
| **L1** — `5 m/s` reported `UnknownUnit("s")` | **FIXED** | Bare/known units after an operator now return `Unsupported` (`calculator.rs:858-874`); corpus includes `5 m/s`, `5 m * s`, `s` (`calculator_corpus.json:444-455`) and the unit-quotient test (`calculator.rs:1560-1568`). |
| **L11** — corpus had no property/fuzz test | **IMPROVED, residual low** | Added a 12³ small-integer associativity loop and a 2,000-expression seeded fuzz (`tests/calculator_corpus.rs:154-189`). This is a bounded regression test, not a general arbitrary-value property test; the independent 300k numeric fuzz also found no panic/mismatch. |
| **L14** — additive percent semantics | **POLICY MADE EXPLICIT, UX caveat** | `200 + 10%` is `200.1`, not `220`; now documented, tested and in corpus (`calculator.rs:27-32,1555-1558`; corpus `valid` entries). This remains a product-semantics decision, not a defect given the documented policy. |
| **L9** — UI Settings said “No units” | **FIXED** | `crates/ui-gpui/src/lib.rs:373` now says units are supported and explains the `=` trigger. |
| **No product display policy regression** | **VERIFIED** | CLI outputs above and direct `route_query` probe preserve units and use `CalcResult::format`; `format_result` boundary/round-trip probe found no mismatches. |

### Emoji disposition

| Finding | Status | Evidence |
|---|---|---|
| **M11** — `load_from_json` accepted a recomputed self-consistent checksum | **FIXED** | `load_from_json` pins to `PINNED_CHECKSUM` (`crates/emoji/src/dataset.rs:307-312`); explicit `load_from_json_with_checksum` is separate (`:317-322`). Tampered data with recomputed checksum `7f0efca1f2258a97` is rejected with `expected=e80345152b0e31eb`; the explicit loader accepts it. Tests: `crates/emoji/tests/emoji_search.rs:59-82`. |
| **L4** — unbounded caller input | **FIXED** | `MAX_DATASET_BYTES` 1 MiB and `MAX_DATASET_ENTRIES` 10,000 (`dataset.rs:22-26`), checked before parse/after parse (`:195-207`, `:326-339`). Probe: `MAX_DATASET_BYTES+1` → `InputTooLarge`; 10,001 entries → `TooManyEntries`; embedded dataset still loads (139 entries, checksum `e80345152b0e31eb`). |
| **L2** — empty favorite accepted | **FIXED** | `toggle_favorite("")` returns `false` and does not insert (`collections.rs:38-45`); test at `emoji_search.rs:274-281`. |
| **L3** — punctuation-only query returned full catalog | **FIXED** | Non-empty queries that normalize to no tokens return no hits (`search.rs:82-86`); tests for `-`/`...` vs empty browse (`emoji_search.rs:93-98`). |
| **Grapheme/accessible-name integrity** | **PASS** | 139 entries; every base and variant one extended grapheme; known ZWJ, flag, keycap, skin-tone and variation-selector cases pass; variant accessible names distinct (`emoji_search.rs:173-230`; independent probe). |
| **L5** — FNV-1a 64 is not cryptographic | **REMAINS, documented** | README now states it is an integrity pin, not a cryptographic signature (`crates/emoji/data/README.md:45-60`). It is acceptable for a local embedded-data integrity check unless adversarial collision resistance is required. |
| **L15** — activation is only a policy helper | **REMAINS, outside crate** | `activation()` still maps caller booleans to `Copied`/`Inserted`/`CopyFallback` (`crates/emoji/src/lib.rs:65-79`); no clipboard or real insertion code exists in the emoji crate. Desktop emoji browsing/copy/insertion remains outside this code scope and is disclosed in `ui-gpui/src/lib.rs:373`. |
| Test coverage residual | **LOW** | The entry cap is verified by independent probe but has no checked-in test (the checked-in cap test covers byte size only). The FNV/activation residuals above should be tracked as product integration work, not crate blockers. |

### Verdict

- **ANS-21 / L07:** **Acceptable for the calculator crate and the verified
  consumer paths.** H3, M9, M10 and L1 are fixed; the corpus is version 1.2.0
  with scientific, formatting-boundary and round-trip cases; no BLOCKER, HIGH or
  MEDIUM remains in the calculator code. Residual UX/policy notes: additive
  `%` is deliberately `200 + 10% = 200.1`, scientific/unit input still needs the
  explicit `=` trigger, and the property test is bounded rather than a general
  arbitrary-value property test.
- **ANS-24 / L08:** **Crate-level acceptable; ticket-level partial.** M11,
  L2, L3 and L4 are fixed; dataset/grapheme/accessibility checks pass. Clipboard
  copy, real insertion, desktop emoji browsing and persisted recents/favorites
  are still outside `crates/emoji/**`, so the end-to-end copy/insertion
  acceptance criteria remain unverified. FNV-1a 64 remains a documented
  non-cryptographic integrity pin.

---

## Platform re-review after fixes (2026-11-19)

**Scope:** ANS-16 / L02 (`crates/platform-windows/**`,
`crates/platform-api/src/lib.rs`). No implementation or test file was modified.
The owner-reported counts were reproduced; this section preserves all original
findings above and records only their post-fix disposition.

### Commands and results

| Command | Result |
|---|---|
| `cargo test -p ansa-platform-windows -p ansa-platform-api --offline` | **PASS** — `ansa-platform-windows` **108** unit tests, `ansa-platform-api` **6** tests, 0 failed/ignored; no doctests. |
| `cargo clippy -p ansa-platform-windows -p ansa-platform-api --all-targets --offline -- -D warnings` | **PASS** — finished with no warnings/errors. |
| `cargo fmt -p ansa-platform-windows -p ansa-platform-api -- --check` | **PASS** — exit 0, no diff. |
| `cargo run -q -p ansa-platform-windows --example catalog --offline` | **PASS** — `total=156`, `classic=108`, `packaged=48`, `broken=3`, `with_arguments=29`, `argument_distinct_targets=6`, `watcher_wired=false`; packaged capability available. These match the owner report exactly. |
| Independent probes | `C:\Users\abhir\AppData\Local\Temp\ansa-platform-probe2`: wrong-candidate/IDList-shadow, working-directory identity, normalization and junction-root probes; 42 tests passed (including included crate tests). |
| Cache probe | `host_capabilities()` first call **814.5 ms**, second call **153 µs**, packaged status `Available` — the 30-second probe cache is active. |

### Remaining findings

#### HIGH — H1 residual: WorkingDir candidate can shadow an IDList/icon target

**Where:** `crates/platform-windows/src/shell_link.rs:173-203`. The candidate
order is WorkingDir (173-175), `.lnk` parent (176-178), decoded IDList
(181-185), then icon (187-194); the first existing file wins at 200-203. The
comment says the IDList is “more authoritative”, but it is appended after the
heuristic WorkingDir/parent candidates.

**Evidence (verified by independent fixture):** a `.lnk` with a valid
`LinkTargetIDList` for `…\real\Real\App.exe`, a relative path `Real\App.exe`,
WorkingDir `…\work`, and icon `…\real\Real\App.exe,0` selected
`…\work\Real\App.exe` when that same-named file existed. The attempted list was:

```text
Work\Real\App.exe
Programs\Real\App.exe
Real\Real\App.exe
```

The real IDList/icon target was never selected. The Riot/VALORANT real-machine
case is fixed (both now resolve to
`C:\Riot Games\Riot Client\RiotClientServices.exe` with no broken flag), but the
wrong-candidate precedence remains for ambiguous or stale WorkingDir links.

**Impact:** a same-named executable in the shortcut working directory can be
launched instead of the IDList/icon target. This is a launch-integrity issue,
not just a label; a writable WorkingDir can redirect an otherwise-valid shortcut.

**Minimal fix:** prefer decoded IDList and same-named icon candidates over
WorkingDir/parent heuristics, or require the WorkingDir candidate to match the
IDList/icon target before accepting it. If both exist but differ, mark the
shortcut broken/ambiguous rather than silently choosing the WorkingDir file.
Add a fixture with an existing IDList target plus a same-named WorkingDir file.

**Status:** verified with an independent fixture; real Riot/VALORANT resolved
correctly.

#### MEDIUM — H2 partial: working directory is not in dedupe/identity

**Where:** `crates/platform-windows/src/start_menu.rs:386-393` and `551-554`.
`target_identity_key` includes target + normalized arguments only;
`working_directory` is exposed in `AppEntry` (`types.rs:27-31`,
`platform-api/src/lib.rs:43-47`) but is not part of the key/id. The code comment
and capability note claim working directory is exposed metadata, not identity.

**Evidence (verified by independent fixture):** two shortcuts with the same
target and `/k test` arguments but WorkingDirs `C:\work-one` and `C:\work-two`
produced **one** entry; the second command is lost. `same_target_with_different_arguments_stays_distinct`
passes and the real catalog has 6 argument-distinct target groups, so argument
identity is fixed, but working-directory identity is not.

**Impact:** two commands that differ only in working directory cannot both appear
in the catalog; a shell command that relies on its working directory is
represented by whichever shortcut wins. The owner’s summary saying arguments
and working directory are included is only true for arguments.

**Minimal fix:** include normalized `working_directory` in
`target_identity_key`/`stable_target_app_id` (and direct `.exe` calls), or
explicitly state that working directory is display-only and not identity. Add
a two-WorkingDir fixture.

**Status:** verified; H2 argument/direct-exe requirements otherwise pass.

#### LOW — L6 timeout path detaches reader threads on an unkillable child

**Where:** `crates/platform-windows/src/packaged.rs:417-442`. The kill/wait
loop is now bounded to one second, but when the child cannot be reaped the
function returns without joining `stdout_reader`/`stderr_reader`; those handles
are detached and the top-level doc still says “Reader threads are joined before
returning” (`:20`). No real unkillable child was induced.

**Impact:** a pathological PowerShell process can leave reader threads alive
until it exits; the timeout no longer blocks the caller, which is the intended
tradeoff, but the documentation overstates cleanup.

**Minimal fix:** document the detached-reader tradeoff or use a bounded thread
join/process-job cleanup; update the module comment.

#### LOW — L7 cache disclosure is inconsistent at the platform API boundary

**Where:** `crates/platform-windows/src/lib.rs:233` correctly discloses the
30-second packaged probe cache, and the independent probe confirms it. But
`crates/platform-api/src/lib.rs:301-303` still says the result is “intentionally
not cached”, and `crates/platform-windows/src/lib.rs:207` says the same. The
catalog/package status is cached for 30 seconds.

**Impact:** documentation accuracy; callers may expect a fresh packaged
capability check on every call.

**Minimal fix:** document the 30-second packaged sub-probe cache in the
platform-api contract, or invalidate/remove the cache from the “fresh probe”
claim.

#### LOW — L13 partial: reveal/copy identity not disclosed; watcher/icon are disclosed

**Where:** `crates/platform-windows/src/lib.rs:232` explicitly says the desktop
and CLI do not poll `CatalogWatcher`; `:226` explicitly says no icon pixel
extraction. The top-level limitations at `:29-33` mention icon pixels but not
reveal-in-file-manager or structured copy identity, and
`crates/platform-api/src/lib.rs:254-293` has no reveal/copy function. The desktop
copies only a path/title (`ui-gpui/src/lib.rs:435-452`).

**Impact:** the watcher and icon limitations are truthful; reveal/copy identity
remain scope gaps that are not disclosed in the capability notes. Acceptance
criteria do not require them, but the L02 scope does.

**Minimal fix:** add a capability note for reveal/copy identity, or implement a
platform API and document the absence.

### Fixed/verified dispositions

| Original finding | Status | Evidence |
|---|---|---|
| **H1** Riot/VALORANT false broken | **FIXED for the real case** | Real catalog: `Riot Client` and `VALORANT` both target `C:\Riot Games\Riot Client\RiotClientServices.exe`, `broken=None`; only Discord (×2) and Steam remain broken. Relative resolution tries WorkingDir, parent, IDList and icon; unresolved attempts are included in the broken reason (`start_menu.rs:318-321`; test at `:1009-1025`). The wrong-candidate precedence above is a separate remaining H1 issue. |
| **H2** arguments in identity | **FIXED** | Real catalog has `with_arguments=29` and `argument_distinct_targets=6`; same-target `/k node` vs `/k vs` entries remain distinct and duplicate args dedupe (`start_menu.rs:924-955`); direct `.exe` identity is stable when an alias is added/removed (`:958-977`). Launch still opens the `.lnk` (`launch.rs:71-79`), so the shell applies the stored arguments. `AppEntry.arguments` is exposed (`types.rs:27-29`, `platform-api/src/lib.rs:43-45`). Working directory identity remains the MEDIUM above. |
| **M2** UNC/root/dot identity | **PASS** | `normalize_path_key` preserves leading `\\` vs `\`, keeps `\\?\`, and normalizes `.`/`..` (`start_menu.rs:503-540`); tests `:1028-1051` and independent normalization probe pass. |
| **M3** shell failure taxonomy | **PASS (code)** | `LaunchOutcome::Failed` added (`types.rs:81-91`); `launch_or_switch` maps shell errors to `Failed`, access-denied to `Denied`, and validation/missing targets to `NoTarget` (`launch.rs:56-68`); platform-api/UI/CLI handle `Failed` (`platform-api/src/lib.rs:345-346`, `ui-gpui/src/lib.rs:420`, `apps/cli/src/main.rs:86`). Actual ShellExecute failure was not induced to avoid a real process launch; verified by code and outcome tests. |
| **M4** bounded directory listing | **PASS (code)** | `remaining` budget and `read_dir(...).take(remaining)` (`start_menu.rs:182-204`; watcher `watch.rs:235-245`); no huge-directory stress test was run. The subset under budget truncation is filesystem-order dependent; acceptable residual. |
| **M5** absolute PowerShell path | **PASS** | `powershell_executable` uses `%SystemRoot%\System32`/`SysWOW64` only (`packaged.rs:313-350`), `run_powershell` uses the absolute `PathBuf` (`:358-371`), and the test asserts no PATH lookup (`:905-925`); catalog packaged=48. |
| **M6** junction roots followed | **PASS** | Discovery follows root metadata at depth 0 while still skipping child reparse points (`start_menu.rs:154-171`); test `:1056-1074` and independent junction probe (1 entry) pass. |
| **M8** minimized windows restored | **PASS (synthetic only)** | Minimized windows remain eligible (`switch.rs:49-62`); `ShowWindow(SW_RESTORE)` then `SetForegroundWindow` (`:81-91`); test `:290-317`. No real minimized-window switch was performed. |
| **L6** timeout bounded | **PARTIAL** | Kill/wait loop bounded to one second (`packaged.rs:417-432`); detached-reader edge above remains. |
| **L7** 30s probe cache | **PASS behavior / LOW docs** | `PROBE_TTL` 30s and cache (`packaged.rs:39-118`); first/second probe timing 814.5 ms / 153 µs; Windows notes disclose, platform-api docs disagree (LOW above). |
| **L8** non-Unicode broken retained | **PASS (code)** | Non-Unicode discovered paths are retained with an explicit broken reason and lossy path rather than dropped (`start_menu.rs:224-245`). No unpaired-surrogate filesystem fixture was created. |
| **L10** hide verification | **PASS** | `set_launcher_visible(false)` calls `ShowWindow(SW_HIDE)` and verifies `IsWindowVisible` (`visibility.rs:346-356`). |
| **L12** arguments/working dir exposed | **PASS (fields) / MEDIUM identity gap** | Fields are in both `AppEntry` types and copied by platform-api (`types.rs:27-31`, `platform-api/src/lib.rs:43-47,323-324`). No consumer currently displays them; working directory is not in identity (H2 MEDIUM above). |
| **M7** watcher not wired | **PASS disclosure / partial** | Example prints `watcher_wired=false`; capability note explicitly says desktop/CLI do not poll it (`lib.rs:232`). |
| **L13** icon pixels/reveal | **PARTIAL** | Icon pixel absence is disclosed (`lib.rs:226`); reveal/copy identity is not disclosed and not implemented (LOW above). |

### Acceptance disposition — ANS-16 / L02

| Criterion | Status | Why |
|---|---|---|
| Same application through multiple shortcuts appears once with deterministic identity | **PARTIAL** | Arguments and direct-`.exe` identity are fixed; same target+arguments dedupes. Working directory is not in identity (MEDIUM), and WorkingDir candidate precedence can select the wrong target (HIGH). |
| Missing, moved or uninstalled targets produce actionable errors, not silent no-ops | **PASS for the real broken set** | Discord and Steam remain explicitly broken; Riot/VALORANT are no longer false-broken; unresolved relative attempts are listed. The H1 wrong-candidate case is a different failure mode. |
| Discovery remains responsive with a large installed set and refreshes without duplicating rows | **PASS (measured) / partial watcher** | 156 entries in the real catalog; deterministic dedupe and 108 tests. Bounded read_dir is implemented. The watcher is intentionally not wired, and packaged installs still need manual refresh (documented). |
| Launch and switch outcomes are distinguished; an unverified switch is not reported as success | **PASS (code/synthetic)** | `Launched`, `Switched`, `Denied`, `Failed`, `Unverified`, `NoTarget` are distinct; `is_success` is only `Launched`/`Switched`; UI/CLI handle `Failed`. Real foreground/switch behavior was not exercised. Packaged switching remains unimplemented and disclosed. |
| Scope: packaged applications | **PASS** | 48 packaged entries, absolute PowerShell, strict AUMID allowlist, `shell:AppsFolder\...`; catalog counts match. |
| Scope: icon extraction | **PARTIAL / disclosed** | Source/index metadata only; no pixels (`lib.rs:226`). |
| Scope: reveal/copy identity | **PARTIAL** | No reveal API; desktop copies a path/title only; not disclosed in capability notes (LOW). |
| Scope: live watching | **PARTIAL / documented** | `CatalogWatcher` is bounded/tested but not polled by the desktop or CLI; packaged installs are not represented (note at `lib.rs:232`). |
| Scope: switch to existing window | **PARTIAL (unverified real)** | Minimized windows are now eligible and restored in code; only synthetic descriptor tests were run. |

### ANS-16 verdict and residual risks

**ANS-16 is substantially improved and the owner-reported build/catalog counts
reproduce exactly, but it is not yet fully acceptable as a closed ticket.**
There is no BLOCKER or memory-safety issue, and H1’s Riot/VALORANT regression is
fixed. However, the remaining H1 wrong-candidate precedence (HIGH) can launch a
working-directory shim instead of an IDList/icon target, and H2 does not include
working directory in identity (MEDIUM). Minimized-window switching, real shell
failure mapping, and foreground/UI behavior remain unverified on a real desktop.

Residual risks: real switch/foreground/UI testing; watcher not wired; icon
pixels and reveal/copy identity absent; 30-second packaged probe cache
documentation mismatch; timeout path can detach reader threads on an unkillable
child; consumer code does not yet display arguments/working directory; and
package/desktop updates may still be in progress.

---

## Final platform delta (2026-11-19)

> This delta supersedes the H1/H2 dispositions in the preceding platform
> re-review section; the original pre-fix findings above remain historical.

**Commands:** `cargo test -p ansa-platform-windows -p ansa-platform-api --offline`
— **PASS**, **110** platform-windows tests + **6** platform-api tests, 0 failed.
`cargo clippy ... --all-targets --offline -- -D warnings` — **PASS**. `cargo fmt
... -- --check` — **PASS**. Catalog example — **PASS**: `total=156`,
`classic=108`, `packaged=48`, `broken=3`, `with_arguments=29`,
`argument_distinct_targets=6`, `watcher_wired=false`; packaged capability
available. (The owner reported 108 platform tests; two new regression tests now
bring the final count to 110.)

**H1 wrong-candidate precedence — PASS.** `resolve_shell_link_file` now orders
absolute `RelativePath`, decoded `LinkTargetIDList`, matching icon executable,
`WorkingDir + RelativePath`, then `.lnk` parent (`shell_link.rs:174-203`). The
new `an_existing_id_list_target_wins_over_a_same_named_working_dir_file` test
passes, and an independent counterexample with a real IDList target plus a
same-named WorkingDir file selected the IDList target. A second counterexample
with an icon target selected the icon target rather than the WorkingDir file.
The old HIGH is closed.

**H2 working-directory identity — PASS.** `target_identity_key` and
`stable_target_app_id` now include normalized target, arguments and working
directory (`start_menu.rs:394-409,569-576`); the
`same_target_and_arguments_with_different_working_dirs_stay_distinct` test
passes, and an independent fixture produced **two** entries for different
WorkingDirs. Same-target/different-argument identity, duplicate-argument dedupe
and direct-`.exe` stability remain passing; launch still opens the `.lnk` so the
shell applies the stored arguments. The old MEDIUM is closed.

**LOW docs — mostly closed.** The timeout residual is now documented in
`packaged.rs:20-25` and `lib.rs:235`; platform-api cache wording correctly
describes the 30-second packaged sub-probe cache (`platform-api/src/lib.rs:308`);
and reveal/copy identity absence is explicitly disclosed (`lib.rs:227`).
One stale line remains: `platform-windows/src/lib.rs:207` still says
`host_capabilities` “is not cached”, and the resolution note at `lib.rs:224`
still lists the old WorkingDir-first order. These are documentation-only nits;
the behavior and disclosure notes are correct.

**ANS-16 verdict: acceptable**, with documented residual risks: real
switch/foreground/UI behavior remains unverified (synthetic descriptor tests
only); `CatalogWatcher` is not wired into the desktop/CLI and packaged installs
need manual refresh; icon pixels and reveal/copy identity are absent; and the
two stale doc lines above remain.

---

## L02a / F07 delta review (2026-11-19)

**Commands:** `cargo test -p ansa-platform-windows -p ansa-platform-api --offline`
— **PASS**, **130** platform-windows tests + **8** platform-api tests, 0 failed.
`cargo clippy ... --all-targets --offline -- -D warnings` — **PASS**.
`cargo fmt ... -- --check` — **PASS**. Examples: catalog **PASS**
(`total=156`, `classic=108`, `packaged=48`, `broken=3`); icons **PASS**
(notepad `32x32`, `4286` bytes, `valid_ico=true`); watch **PASS**
(6 bounded polls at 300 ms, both default roots, all `Unchanged`,
`desktop_wiring=L11`). Targeted hotkey tests: 9 passed with no skip messages;
icons 4 passed; reveal 2 passed.

**Icon extraction — PASS (code/real machine).** `extract_icon_to_ico` validates
an absolute, existing non-reparse regular source (`icons.rs:232-258`), bounds
the index at 999 and dimensions at 256 (`:38-42,184-229`), owns and destroys
`HICON`/bitmap/DC handles (`:583-636`), caps the ICO at 512 KiB (`:312-316`),
writes a deterministic cache name via a temporary file and cleans up failures
(`:509-539`), and validates the resulting ICO (`:118-182`). Real notepad output
is a valid 32x32 ICO. No memory/handle leak was found in the unsafe path. LOW
residuals: no checked-in live reparse/symlink fixture for the source path, and
`GetDIBits` return values are checked only for nonzero rather than equal to the
requested scanline count.

**Reveal — PASS (code; real shell call unverified).** `reveal_in_file_manager`
rejects empty, NUL, relative, missing and reparse paths before COM
(`reveal.rs:35-68`), initializes/balances COM with a `RPC_E_CHANGED_MODE` guard
(`:114-150`), parses one PIDL with `SHParseDisplayName`, frees it with
`CoTaskMemFree`, and calls `SHOpenFolderAndSelectItems` without any explorer
command string (`:71-105`). `copy_identity`/`identity_summary` is pure and
deterministic. Manual/UI reveal on a real desktop was not performed; only
validation and identity tests ran.

**Watcher example — PASS (platform scope).** `default_watch_roots` exposes the
real Start Menu roots (`watch.rs:75-82`); the example performs six bounded
polls, never mutates the Start Menu, has no background thread, and reports
`desktop_wiring=L11` (`examples/watch.rs:19-59`). Desktop wiring remains L11 by
design.

**Packaged switching — PASS (explicitly Unsupported).** The capability reports
`window_control: Unsupported`, and the notes state packaged switching cannot be
implemented without package-identity window APIs, with a documented alternative
(`lib.rs:282,293`). No `Switched` outcome is claimed for a packaged AUMID.

**F07 — PASS (library/tests; desktop rebinding still manual).** `ShortcutChord`
matches the storage validator for 1–64 bytes, control-character rejection,
one-to-four modifiers, duplicate modifiers, all documented key names,
`F1`–`F24`, and canonical modifier order (`shortcut.rs:278-339`; compare
`crates/storage/src/settings.rs:191-290`). `ShortcutRecorder` uses virtual-key
events and never injects input (`shortcut.rs:474-579`). `HotkeyManager`
register/probe/reconfigure/reapply releases the previous worker before
installing, restores the previous binding when reconfigure fails
(`hotkey.rs:424-505`), and `unregister`/Drop are idempotent; the 9 hotkey tests
passed with no skips. `reapply` failure leaves no active binding rather than
retrying the same chord (expected after a conflict, but the caller must
recover); no stuck registration was observed. Tests use a synthetic `WM_HOTKEY`
message, not physical key injection. Residual manual check: the desktop still calls
`HotkeyRegistration::register` for the default chord and does not wire
`HotkeyManager`/`ShortcutRecorder` into settings UI, so user-facing rebinding
and physical key capture remain unverified (settings text says shortcut editing
is not implemented).

**Verdict:** **L02a acceptable at platform-code scope**; manual checks remain
for real Explorer reveal, symlink/reparse fixtures, and UI integration. **F07
acceptable at platform-library scope**; manager/recorder lifecycle tests pass,
but desktop settings/rebinding and physical key capture remain manual
integration work.
