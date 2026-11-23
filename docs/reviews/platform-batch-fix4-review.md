# ANS-82–99 — fix4 micro-confirmation

Scope: the late-budget guard, preservation of the fix3 paths, and validation wording. Native acceptance stays **0/8** and was not re-evaluated.

## Code
- [Capture](../../crates/clipboard/src/store.rs#L977-L1105) samples the budget once, wipes the temporary envelope, and returns `InvalidArgument` for `Some` on a persistent store before changing live rows, the manifest, disk metadata or seals. The earlier insertion can evict only from a clone. Both encoded-budget eviction and the sole `seal_replacing` call are below the guard; ordinary persistent eviction still deletes payloads only after history commit.
- [The regression](../../crates/clipboard/src/store.rs#L2108-L2147) passes: the late toggle leaves exactly one row (ID 1 still opens to `first`), the original alias's exact payload envelope, and byte-identical `history.json`. The test double stores envelope bytes, not ciphertext; the test does not force a victim or disk-write failure.
- [X11](../../crates/clipboard/src/linux/x11.rs#L145-L176) still stops at the first matching notification and fails closed at 4,096 polls. An owner mismatch, a match, or the no-match cap rejects before conversion and [advances the generation](../../crates/clipboard/src/linux/x11.rs#L536-L553). Source-checked only this pass.
- [Volatile open](../../crates/clipboard/src/store.rs#L695-L758) still bypasses the filesystem for memory-only vaults and budgets reported at open. [Persistence is a no-op](../../crates/clipboard/src/store.rs#L1566-L1594); the pressure, explicit-backend and failed-volatile-capture tests passed.

## Documentation
- [Batch](../platform-batch-implementation.md#L86-L95), [status](../implementation-status.md#L95-L107) and [clipboard](../../crates/clipboard/PLATFORM-VALIDATION.md#L134-L142) match the Windows reruns: workspace **786 pass events, 0 failed, 2 ignored** across **50 Cargo result lines**, including two child relaunches; doctests **4 passed, 1 ignored**; platform-api **43**, vault **41**, clipboard **110 + 1 ignored**, UI **30** (focused totals exclude duplicate child passes).
- **Stale:** [vault validation](../../crates/vault/PLATFORM-VALIDATION.md#L58-L61) still lists clipboard **109** (101 lib) in the combined run; label it pre-fix4 or refresh to **110** (102 lib). [Status](../implementation-status.md#L112-L118) still says only two fix-verification rounds.
- Open residuals are explicit: [Xauthority I/O is outside the deadline](../../crates/clipboard/PLATFORM-VALIDATION.md#L366-L377); [Keychain recovery/prompt behavior is unverified and Linux `CreateItem` can still prompt](../../crates/vault/PLATFORM-VALIDATION.md#L71-L75); [live macOS Reduce Transparency needs activation or a glass toggle](../../crates/ui-gpui/PLATFORM-VALIDATION.md#L176-L185).
- **Overclaims:** [batch X11 wording](../platform-batch-implementation.md#L124-L126) is too broad, and the [source comment](../../crates/clipboard/src/linux/x11.rs#L208-L222) still says a rival request merely queued before the grab has generated an event. The guarantee is for reassertions the server **processed** before the grab. [Clipboard validation](../../crates/clipboard/PLATFORM-VALIDATION.md#L330-L342) calls `MemorySealBackend` persistent; only the store metadata is persistent in that fixture.
- The [rollback comment](../../crates/clipboard/src/store.rs#L1533-L1541) still implies reduced history can survive on disk; only the volatile branch precommits it. The [Keychain load comment](../../crates/vault/src/native_macos.rs#L126-L131) should qualify `Locked`: the remap requires interactive `AccessDenied` plus a confirmed lock.
- [Batch](../platform-batch-implementation.md#L79-L89) and [clipboard](../../crates/clipboard/PLATFORM-VALIDATION.md#L351-L359) should limit `--locked --offline` to commands that use those flags, not `cargo fmt`.

## Windows validation
| Command | Exit |
|---|---:|
| `cargo test -p ansa-clipboard --locked --offline` | 0 |
| `cargo clippy -p ansa-clipboard --all-targets --locked --offline -- -D warnings` | 0 |
| `cargo fmt -p ansa-clipboard -- --check` | 0 |
| `cargo test --workspace --locked --offline --no-fail-fast` | 0 |
| `cargo test --workspace --doc --locked --offline` | 0 |
| `cargo test -p ansa-platform-api -p ansa-vault -p ansa-ui-gpui --locked --offline` | 0 |

**Code-only verdict: fix3 HOLD cleared; no new regression found in the reviewed paths.**
