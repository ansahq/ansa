# ANS-82–99 — third-pass narrow verification

Scope: the two fix2 code findings and the flagged wording. Native acceptance remains **0/8** and is out of scope.

## 1. X11 event-bound generation — resolved

- [The real drain](../../crates/clipboard/src/linux/x11.rs#L145-L176) returns on the first genuine XFixes event for this monitor/selection. It leaves subsequent events queued for `sync_selection`; 4,096 polls with no match return `ClipboardChanged`, not `CaptureLimit`.
- [Under the grab](../../crates/clipboard/src/linux/x11.rs#L223-L267), an owner mismatch, a consumed match, and a no-match cap all reject before `ConvertSelection`. [Transfer](../../crates/clipboard/src/linux/x11.rs#L527-L561) applies `conversion_invalidates_generation` to all three, calls `set_owner(self.owner)` for a fresh generation, then propagates the rejection. The next sync reconciles the owner and later notifications.
- The drain cannot consume this request's `SelectionNotify`: it runs before the request is sent, transfers are sequential, and the read uses a fresh requestor window to isolate late replies. The grab orders the server's recipient and covers accepted reassertions **processed** before the grab, not merely queued peer requests or the bytes the same provider chooses to return.
- [The fake-server fixtures](../../crates/clipboard/src/linux/x11.rs#L1029-L1139) cover owner mismatch, same-XID rejection, a match at slot 4,096 of 4,098 with the tail retained, and exactly 4,096 unrelated events. The cap tests assert zero conversions, grab release, and the invalidation predicate. They model the drain, not the real wire queue or `Reader`'s generation; Linux compile/lint only.

## 2. Mixed volatile budget and disk metadata — resolved in-tree, not for arbitrary embedders

- [At open](../../crates/clipboard/src/store.rs#L695-L768), any `Some(limit)` takes `open_volatile` before even testing the path: no directory access/creation, lock, `history.json` or `manifest.json`; `is_persistent()` is false. Production `ClipboardStore::open` still sends a memory-only vault directly there. [Persistence, rollback I/O and disk reload](../../crates/clipboard/src/store.rs#L1472-L1597) short-circuit for volatile stores, so their final history write cannot fail.
- [Pressure test](../../crates/clipboard/src/store.rs#L2331-L2421): the public `open` path evicts under encoded pressure, decrypts the original pinned row and checks its exact plaintext, rejects the all-pinned capture, and leaves the root absent. [The explicit-backend test](../../crates/clipboard/src/store.rs#L2423-L2465) captures twice with eviction and asserts no root, `.lock`, history or manifest. Both passed, as did the public-path prefilled-vault preservation test.
- **No in-tree caller depends on mixed mode.** The pressure test no longer uses it; the new explicit test uses a volatile `VaultBackend`. Other direct test callers use `MemorySealBackend` (no encoded budget); persistent production vaults never report a budget.
- **Same loss path still reachable through the embedding API (source trace).** The [public trait](../../crates/clipboard/src/store.rs#L126-L185) does not require `None`/`Some` to be stable. A backend can have no budget at open, then enable one. [Capture](../../crates/clipboard/src/store.rs#L1003-L1128) re-reads `Some` without checking persistence: reduced disk history can commit, `seal_replacing` can delete the victim, and the final disk write can fail. Error cleanup cannot restore that victim. Reject a later `Some` on a persistent store before mutation to make the guarantee unconditional. This is not a newly introduced regression.

## Documentation

- [Platform](../../crates/platform-api/HOST-VALIDATION.md#L54-L101): opener rejection is limited to checked points, with ancestor/MIME and pathname-handoff limitations; xauth blocking I/O and the worker join are outside the socket guarantee; the slow-save fixture is described as timestamp simulation.
- [Clipboard](../../crates/clipboard/PLATFORM-VALIDATION.md#L47-L87): the XAUTHORITY/FIFO wording now distinguishes metadata preflight from uncapped blocking I/O. The docs correctly describe [in-memory rollback](../../crates/clipboard/PLATFORM-VALIDATION.md#L298-L312) and [pinned-row decryption](../../crates/clipboard/PLATFORM-VALIDATION.md#L189-L197). Qualify the mixed-mode claim to cover budgets reported at open; say **processed**, not **queued**, for the X11 barrier (also at lines 277–282).
- [Vault](../../crates/vault/PLATFORM-VALIDATION.md#L49-L75): MEDIUM-3 now says the lock recheck also runs for noninteractive query failures, but only interactive denial with a confirmed lock changes the issue. `Locked` reports state, not unlock instructions. The Linux `CreateItem` prompt remains explicitly open.
- [UI](../../crates/ui-gpui/PLATFORM-VALIDATION.md#L85-L101): counts are current (43 platform-api, 30 UI, 0 desktop). [The Reduce Transparency procedure](../../crates/ui-gpui/PLATFORM-VALIDATION.md#L176-L200) requires activation or a glass toggle and does not claim live notification.
- **Still stale:** [batch](../platform-batch-implementation.md#L79-L101) and [status](../implementation-status.md#L88-L114) present 767 and focused 35/36/105/29 as current; label historical or use coordinator results: 785 pass events, 0 failed, 2 ignored across 50 **Cargo result lines**, not binaries; doctests 4 passed/1 ignored; focused 43/41/109 (+1 ignored)/30/0. The batch's [“implementation … complete”](../platform-batch-implementation.md#L137-L155) should acknowledge documented implementation residuals as well as native evidence gaps.
- A few source comments still contradict the corrected records: Xauthority reads are not bounded by the [clipboard predicate](../../crates/clipboard/src/linux/x11.rs#L694-L704); [rollback writes](../../crates/clipboard/src/store.rs#L1522-L1549) are best effort; [Linux `ensure_unlocked`](../../crates/vault/src/native_linux.rs#L51-L60) doesn't prompt and [macOS `Locked`](../../crates/vault/src/native_macos.rs#L157-L166) isn't a system-unlock instruction. These are wording issues, not separate code blockers.

## Independent validation (Windows x86_64, Rust 1.99.1)

| Command | Result |
|---|---|
| `cargo test -p ansa-vault -p ansa-clipboard --locked --offline` | Exit 0: vault 41 passed; clipboard 109 passed (includes one doctest), 1 ignored. Two child relaunches excluded from counts. |
| `cargo clippy -p ansa-vault -p ansa-clipboard --all-targets --locked --offline -- -D warnings` | Exit 0 |
| `cargo fmt -p ansa-vault -p ansa-clipboard -- --check` | Exit 0 |
| `cargo check` / `cargo clippy` for `ansa-clipboard`, `--target x86_64-unknown-linux-gnu --all-targets --locked --offline` (Clippy with `-- -D warnings`) | Both exit 0; no Linux link or execution |
| Embedded Python 3.13: `-B -m unittest discover -s scripts -p test_platform_acceptance_gate.py -v` | Exit 0: 55 passed, 1 skipped (host symlink creation unavailable) |
| Same Python: `-B scripts/platform_acceptance_gate.py --check` | Exit 0: ledger valid, 0/8 accepted |

**Code-only merge verdict: HOLD.** The X11 fix is resolved. The mixed-store fix closes the loss path for every in-tree backend and any backend reporting a budget at open, but not for the full embedding API. No new functional regression was found in the reviewed changes.
