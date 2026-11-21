# Alpha known limitations

This list is public-facing for the daily-driver alpha. It records what is not
implemented or not verified. It is not a promise that unlisted behavior is safe.

## Platform and packaging

- **Windows only.** Discovery, launch, switch, focus capture/restore, packaged
  application enumeration and the global hotkey live in
  `ansa-platform-windows`. macOS, Linux and other platforms report these
  capabilities as unsupported; the alpha does not run there.
- **No installer, uninstaller, auto-update or crash reporting.** Installation is
  a Cargo build/install. Uninstallation is deleting the binary and data
  directory. There is no signed distribution.
- **Clean-profile install/upgrade/uninstall has not been executed by a new
  tester.** The documented commands are the current source of truth, but the
  acceptance evidence is still missing.
- **Packaged application switching is not implemented.** Packaged/MSIX entries
  are enumerated by AUMID and launched through `shell:AppsFolder\<AUMID>`.
  `launch_or_switch` cannot match a packaged app to an existing window, so a
  packaged app is reported as `Launched` rather than switched.
- **Classic application switching is best-effort.** A switch is reported only
  when the target window is observed as foreground. Windows can deny
  activation; the result is `Denied` or `Unverified`, never success.
- **Broken shortcuts remain in the catalog.** They are marked broken with a
  refresh hint instead of disappearing. Launching one returns `Failed`,
  `NoTarget` or an actionable error.
- **Shortcut arguments are not exposed to the command layer.** Discovery
  includes normalized arguments and working directory in application identity
  and dedupe, so argument-distinct shortcuts remain separate. Launching opens the
  `.lnk`, so the shell applies the stored arguments. `CommandKind::Application
  { path }` and `launch_or_switch(path)` still carry only the path; there is no
  structured argument API or UI.

## Keyboard, focus and summon

- **Accessibility is unverified.** No screen-reader acceptance run, no
  keyboard-only regression sweep and no UIA/AccessKit parity claim. The upstream
  GPUI accessibility candidate is not used by this alpha.
- **Hotkey conflicts and denials are runtime outcomes.** `Ctrl+Space` is
  attempted through `RegisterHotKey`; a conflicting or denied registration is
  reported instead of silently intercepting another shortcut. The CLI
  `doctor` command only probes capability and does not register a hotkey.
- **Focus restoration is checked and best-effort.** It returns to the window
  captured before the launcher was shown, only while the launcher still owns the
  foreground. Windows can deny activation. It never injects input or attaches
  thread input.
- **IME composition is guarded but not field-tested with real IMEs.** Escape and
  activation are ignored while composition is active.
- **No two-week daily-use evidence.** Keyboard, focus, summon, selection and
  action regressions from real use have not been collected or triaged.

## UI surface

- **Desktop coverage is partial.** The GPUI desktop now consumes `DailyDriver`
  for persisted static HTTP(S) quicklinks, a basic `Start Setup` onboarding
  surface and an explicit `emoji <query>` copy mode. It still has no quicklink
  authoring/preview/import/export form, no full emoji browsing/variant picker,
  and no complete Welcome/Shortcut/TryLaunch/Integrations/Done screen set.
- **The desktop consumes `DailyDriver` for catalog/onboarding/quicklinks, but
  ranking signals remain session-only.** Usage counts, recency and favorites are
  not written to storage and reset when the process exits. Backup/restore is
  available through the CLI/storage APIs, not a desktop settings screen.
- **No persistence of ranking signals.** Usage counts, recency and favorites are
  not written to storage and reset when the process exits.
- **No automatic paste.** Calculator results and copied targets go to the
  clipboard; the user pastes manually. Emoji mode copies the selected grapheme
  and says "paste manually". The P05 insertion model defaults to copy-only and
  has no Windows automatic-insertion host in this slice.
- **External quicklink imports require Ansa to be closed.** The desktop holds
  the single-instance SQLite lock. Run `ansa quicklink import --data-dir <dir>`
  while Ansa is closed, then start Ansa. Refresh only re-reads same-process
  state; it does not bypass the lock.
- **No quicklink argument entry UI.** Quicklinks with required arguments are
  deliberately omitted from the launchable catalog rather than launched with an
  unresolved template. They remain visible to `ansa quicklink list` and can be
  previewed with `--arg` in the CLI. The desktop has no argument-entry screen.
- **Local-path and `FileManager` quicklinks are omitted from the catalog.** The
  desktop primary action uses the HTTP-only `open_url` path, so only resolved
  HTTP(S) targets with `Browser`/`DefaultApp` openers are composed. Local paths
  are previewable in the CLI but not launchable from the daily UI.
- **Catalog opener availability is a static probe.** Composition cannot know
  whether the operating system currently has a browser registered; the platform
  open path revalidates at launch time and reports a truthful error.
- **Icon extraction exists but desktop icon rendering is not wired.** The
  platform adapter can extract a bounded 32×32 ICO from a real executable; the
  launcher still displays text/metadata rather than icons.
- **UI status text is truthful but not exhaustive.** A denied or unverified
  launch leaves the launcher open; other host failures are reported in the
  status line only.

## Data and recovery

- **Quicklink records are unencrypted JSON in SQLite.** They contain no secrets,
  but they are not a confidentiality boundary. Do not store tokens or clipboard
  bodies in records.
- **A corrupt quicklink record does not block startup.** `DailyDriver` loads an
  empty library and exposes a warning. An explicit save replaces the record;
  there is no automatic repair.
- **Backup verification covers integrity, schema version and row counts.** It
  does not prove application-level semantic correctness of every payload.
- **The storage crate has real subprocess-kill migration/write tests; the
  desktop alpha has not run a crash/restart dogfood drill.** SQLite WAL and
  transactions are used, but the alpha does not claim a completed desktop
  crash-recovery dogfood drill.
- **`doctor --data-dir` opens and may migrate the supplied database.** It is a
  diagnostic command, not a read-only inspector.

## Vault, clipboard and insertion

- **DPAPI is user-bound, not passphrase-protected.** The P03 vault and P04
  clipboard history are sealed with Windows DPAPI. Any same-user process can
  decrypt them; `lock()` is a session gate, not cryptographic re-authentication.
  There is no cross-user/machine recovery or backup migration yet.
- **Clipboard capture has no UI and no live listener.** P04 is a model plus
  Windows text/file-list adapter; image capture is unsupported, heuristics are
  not proof of sensitivity, and pause/private mode is session-only. A future
  P06 UI is required for browsing/search.
- **Automatic insertion is not production-wired.** P05 has the target and
  clipboard-ownership transaction, but no Windows `SendInput` host; `Inserted`
  is only fake-host tested. Copy-only is the real path.
- **External quicklink imports cannot run while Ansa is live** because of the
  single-instance storage lock. Import with Ansa closed, then start it.

## Registry lifecycle

- **The alpha exercises the registry through tests, not through the daily UI
  path.** The CLI application launch and the GPUI primary action use
  `launch_or_switch` directly. Registry-backed action preparation,
  authorization, execution and reconciliation are covered by an alpha
  integration test with a fake host.
- **Registry policy is hardened.** Application and quicklink kinds require their
  kind capability and explicit confirmation. The alpha demo grants the required
  capability explicitly, confirms the action, and reconciles an uncertain
  outcome instead of retrying. It does not depend on the old "quicklinks are
  not consequential" behavior.

## Claims deliberately not made

- No benchmark result or performance parity.
- No accessibility conformance or parity.
- No Raycast/Spotlight feature parity.
- No two consecutive weeks of daily use.
- No claim that every reported regression has a disposition; no regression
  reports exist yet.
