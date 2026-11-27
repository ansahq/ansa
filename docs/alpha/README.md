# Ansa daily-driver alpha

This directory records the Phase 1 daily-launcher alpha for ANS-23. It is an
integration and dogfooding track, not a release announcement.

The Phase 0 integration scaffold and the frozen provider/handler/tool/paste
interfaces are documented in [phase-0-scaffold.md](phase-0-scaffold.md).

## What this alpha actually is

The alpha is a **Windows-only, local-only launcher slice** composed from the
already-tested crates:

- `ansa-alpha` is the composition root. `DailyDriver` opens storage in a
  supplied data directory and exposes typed settings, a versioned quicklink
  library persisted as a SQLite `UserRecord`, onboarding state in
  `<data-dir>/onboarding.json`, licensed emoji search, catalog composition,
  calculator routing and a deterministic harness.
- `ansa-cli` exposes the slice for inspection and dogfooding: `search`, `apps`,
  `calc`, `open`, `doctor`, `emoji`, `quicklink`, `storage`, `onboard` and
  `alpha-smoke`.
- The GPUI desktop's primary application action now calls `launch_or_switch`
  and reports `Launched`, `Switched`, `Denied`, `Failed`, `Unverified` or
  `NoTarget` truthfully. It records use and dismisses only for a positive
  observed outcome.
- The desktop consumes `DailyDriver` for the persisted catalog, static HTTP(S)
  quicklinks and onboarding state. A keyboard-only `Start Setup` surface
  includes Test launch/Skip/Continue/Reset; it probes the live hotkey state and
  records only positive launch outcomes. `emoji <query>` provides bounded emoji
  search and copies the selected grapheme with a paste-manually message.
- External `--data-dir` quicklink imports require Ansa to be closed because the
  desktop holds the single-instance storage lock. Import, then start Ansa; the
  desktop does not claim live cross-process import.
- Calculator routing reuses the public `ansa_core::query::route_query`; alpha
  does not duplicate trigger logic. An explicit `=` prefix requests arithmetic
  (and is required for units and constants), and a digit-containing string made
  only of `+ - * / ^ % ( ) .` and whitespace is evaluated. Names such as
  `7-Zip` and `1Password` remain searches. Results are formatted with
  `CalcResult::format`, so bounded length, mass, time and temperature units are
  preserved; currencies, dates and time zones remain unsupported.
- Registry coverage grants each kind capability explicitly, confirms
  consequential actions, refuses blind retries after an `Uncertain` outcome and
  reconciles the operation. The harness rejects stale and cross-invocation
  coordinator batches and keeps provider queues bounded.
- Catalog composition resolves every enabled quicklink through the public
  quicklink preview policy with declared defaults. Only HTTP(S) targets with
  `Browser`/`DefaultApp` openers become `CommandKind::Quicklink` commands;
  required-argument, local-path/`FileManager`, unsafe and unresolvable
  quicklinks are omitted and reported by `doctor` with a reason. The opener
  availability check is a static probe; the platform path revalidates at open
  time. The raw template source is never used as a launch target.

There is no network access, no account, no automatic paste, no installer and no
feature freeze evidence yet.

## Build and run from a clean profile

Prerequisites: Windows 10/11, Rust 1.99.1, a local Cargo cache (the commands use
`--offline`).

```powershell
# Build the CLI
cargo build --release --offline -p ansa-cli

# Or install the single binary into the Cargo bin directory
cargo install --path apps/cli --offline --force

# Run without installing
cargo run --offline -p ansa-cli -- doctor
cargo run --offline -p ansa-cli -- alpha-smoke --cycles 1000
```

The binary is `target/release/ansa.exe` after `cargo build`, or `ansa` on
`PATH` after `cargo install`.

Create an explicit data directory for any command that persists state:

```powershell
ansa storage --data-dir "$env:LOCALAPPDATA\Ansa\alpha"
ansa doctor --data-dir "$env:LOCALAPPDATA\Ansa\alpha"
```

The data directory contains `ansa.sqlite`, `cache.sqlite`,
`snapshots\` (pre-migration snapshots), optionally `backup.sqlite` and
optionally `onboarding.json`.

The desktop UI is built separately and is not installed by the CLI:

```powershell
cargo run --offline -p ansa-desktop
```

The desktop's `Ctrl+Space` shortcut registration is attempted at runtime. If
registration is denied or conflicts, the UI reports that it is unavailable.

## Uninstall from a clean profile

The alpha has no signed installer, service, scheduled task or updater.

```powershell
# If installed with cargo install
cargo uninstall ansa-cli

# If built in the repository, remove the binary
Remove-Item target\release\ansa.exe

# Remove local data (this deletes settings, quicklinks, backups and onboarding)
Remove-Item -Recurse "$env:LOCALAPPDATA\Ansa\alpha"

# Exit the desktop application so its global hotkey thread is torn down.
```

Uninstalling does not remove the repository, Cargo's cache, or any file outside
the data directory named above.

## Commands

```powershell
ansa search terminal
ansa apps
ansa calc "(24 + 8) / 2"
ansa open app:0123456789abcdef
ansa doctor [--data-dir <dir>]
ansa emoji rocket [--limit 5]
ansa quicklink list    (--library <library.json> | --data-dir <dir>)
ansa quicklink preview <quicklink-id> (--library <library.json> | --data-dir <dir>) [--arg name=value]...
ansa quicklink import  <file.json> (--library <library.json> | --data-dir <dir>) [--replace]
ansa quicklink export  (--library <library.json> | --data-dir <dir>) [--out <file.json>]
ansa storage [--data-dir <dir>] [--demo]
ansa onboard --state <onboarding.json> [--demo]
ansa alpha-smoke [--cycles N]
```

`--library <file>` operates on a portable versioned JSON file and is independent
of the desktop's storage. `--data-dir <dir>` operates on the persisted SQLite
quicklink record loaded by `DailyDriver` and the desktop: imports are written
through the storage record with compare-and-swap/revision checks, and
`ansa doctor --data-dir` immediately sees the persisted count. The two modes are
mutually exclusive and rejected together.

`--data-dir` opens the durable store, which is single-instance locked. Close a
running Ansa first: importing, exporting, listing or viewing doctor while it
holds the data directory fails with an actionable message, and the CLI does not
retry or bypass the lock.

`ansa open` prints the host's actual outcome. It exits nonzero for `Denied`,
`Failed`, `Unverified` and `NoTarget` and never prints success for them.

`ansa quicklink preview` uses a static opener probe: it shows the exact target,
opener, effect and fingerprint but does not open anything.

`ansa onboard --demo` runs a scripted fresh-profile journey with fake shortcut
and launch probes. It writes onboarding state but explicitly does **not**
register a real hotkey or start an application.

## Automated readiness evidence

Captured on Windows, Rust 1.99.1, debug profile:

```text
$ cargo run --offline -p ansa-cli -- alpha-smoke --cycles 1000
Alpha smoke: PASS
cycles: 1000
accepted batches: 1000
rejected batches: 3000
stable selection checks: 1000
max pending: 1
max results: 401
catalog size: 404
assertions: 15003
elapsed: 4988.0 ms
fingerprint: 0x02807f025f6a8ad7
```

The harness runs the real `Engine` and provider `Coordinator`. Each cycle:

- starts a fresh invocation and sets an alternating query;
- submits a stale engine batch and verifies it is rejected;
- submits a stale coordinator batch and a mismatched batch under the current
  invocation, and verifies both are rejected;
- submits the current ranked batch and verifies it is accepted;
- verifies the selected stable identity is retained, stale results never enter
  the result list, the engine catalog size is unchanged and coordinator pending
  work never exceeds one item.

The run is deterministic: the fingerprint excludes wall-clock time and repeated
50-cycle runs produce the same fingerprint and counters. The 1,000-cycle debug
run is well under the 30-second gate. Release timing was not measured in this
slice; the same deterministic workload is expected to be faster but is not
claimed as captured evidence.

Validation commands used for this slice:

```powershell
cargo test -p ansa-alpha -p ansa-cli --offline
cargo clippy -p ansa-alpha -p ansa-cli -p ansa-ui-gpui --all-targets --offline -- -D warnings
cargo check -p ansa-ui-gpui --offline
cargo fmt -p ansa-alpha -p ansa-cli -p ansa-ui-gpui
```

## Disposition: what is proven and what is not

**Proven by automated evidence:** the composition root opens real storage,
enforces compare-and-swap settings, backs up and restores a verified database,
persists and reloads quicklinks, imports and exports the versioned library,
searches licensed emoji, persists onboarding state, routes calculator input,
executes a registry action through a fake host, reconciles an uncertain
registry outcome, maps application launch outcomes truthfully in the CLI and UI,
and survives 1,000 deterministic summon/query/selection cycles with stale-batch
rejection and bounded queues. The desktop has 22 focused tests and a 40-stage
synthetic interaction smoke covering keyboard-only setup and bounded emoji
navigation. P03/P04 tests cover DPAPI sealing, tamper/wrong-context/lock,
clipboard exclusion filters, quotas, cross-process locking and legacy migration;
P05 covers the copy-only/policy transaction with a fake host.

**Not proven:** two consecutive weeks of daily human use. The acceptance
criterion for ANS-23 includes real dogfooders, a daily workflow log, triage of
every keyboard/focus/summon/selection regression, clean-machine
install/uninstall by a new tester, keyboard-only and screen-reader spot checks,
and a three-day feature freeze. Those have **not** happened. The automated
readiness gate passes; the human dogfooding acceptance remains open. See
[`known-limitations.md`](known-limitations.md) and
[`dogfood-log-template.md`](dogfood-log-template.md).

No benchmark, accessibility, platform-parity or productivity claim is made
beyond the captured evidence above.
