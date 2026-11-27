# Phase 0 integration scaffold

Status: implemented 2026-11-27. This is the integration seam for the feature
work that follows. It does not enable clipboard history, snippets, scripts,
agents, MCP connections, window actions or file search.

## Purpose

Parallel feature work needs one stable shell, one composition point and one
owner per shared file. Phase 0 establishes those boundaries before feature
code lands. Existing launcher behavior, tests and the native smoke path are
unchanged.

## File ownership

One owner per shared file. Feature work edits its own view and, when it needs a
new provider/handler, registers it in its own crate; it does not edit another
owner's file.

| File | Owner | Responsibility |
|---|---|---|
| `crates/ui-gpui/src/lib.rs` | integration | Module wiring, GPUI actions, public entry points |
| `crates/ui-gpui/src/shell.rs` | shell | Window, focus, appearance, root `Render`, catalog refresh, setup surface |
| `crates/ui-gpui/src/input.rs` | input | Root text changes, IME composition gate, emoji mode |
| `crates/ui-gpui/src/navigation.rs` | navigation | Selection, action panel, Escape and Tab |
| `crates/ui-gpui/src/dispatch.rs` | dispatch | Command → `DispatchTarget` routing and execution |
| `crates/ui-gpui/src/views/clipboard.rs` | clipboard | Clipboard history surface |
| `crates/ui-gpui/src/views/snippets.rs` | snippets | Snippet library and insertion surface |
| `crates/ui-gpui/src/views/scripts.rs` | scripts | Script authoring/execution surface |
| `crates/ui-gpui/src/views/agents.rs` | agents | Agent conversation surface |
| `crates/ui-gpui/src/views/mcp.rs` | mcp | MCP server surface |
| `crates/ui-gpui/src/views/windows.rs` | windows | Window actions surface |
| `crates/ui-gpui/src/views/files.rs` | files | File search surface |
| `crates/alpha/src/feature_registry.rs` | integration | Frozen provider/handler interfaces and `FeatureRegistry` |
| `crates/alpha/src/records.rs` | integration | Generic record kinds and identities |
| `crates/alpha/src/daily_driver.rs` | integration | Composition root, vault access, record API |
| `crates/tools/src/lib.rs` | integration | Frozen `Tool` contract |
| `crates/clipboard/src/insertion.rs` | clipboard | `PasteTarget` and the checked insertion transaction |

Shared-file changes must be proposed in the owning file first. Interface changes
to the frozen items below require an ADR or an explicit review, not a drive-by
edit.

## Frozen interfaces

### `SearchProvider`

`crates/alpha/src/feature_registry.rs`

```rust
pub trait SearchProvider {
    fn name(&self) -> &str;
    fn priority(&self) -> i32;
    fn ready(&self) -> bool;
    fn query(&self, query: SearchQuery<'_>) -> Vec<SearchResult>;
    fn results(&self, query: SearchQuery<'_>) -> Vec<SearchResult>; // default: query
}
```

Rules:

- `priority` is ascending; lower runs first. Ties break by `name`.
- `ready` reports cached-state usability. `query` must not block on I/O.
- Results keep provider order. The registry only de-duplicates command
  identities and applies the caller's limit.
- Provider results are bounded by `query.limit`.

### `CommandHandler`

`crates/alpha/src/feature_registry.rs`

```rust
pub trait CommandHandler {
    fn name(&self) -> &str; // command namespace, e.g. "snippet"
    fn preview(&self, command: &Command) -> Result<CommandPreview, FeatureError>;
    fn execute(&mut self, command: &Command) -> Result<HandlerOutcome, FeatureError>;
    fn cancel(&mut self, command: &Command) -> Result<(), FeatureError>; // default: no-op
}
```

Rules:

- The handler `name` is matched against `CommandId::namespace()`.
- `preview` never executes. `execute` must re-check mutable targets; a stale
  preview is not execution permission.
- Registering a second handler for the same namespace replaces the first, so
  startup composition and tests are idempotent.

### `Tool`

`crates/tools/src/lib.rs`

```rust
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn schema(&self) -> &ToolSchema;          // serde_json::Value
    fn approval(&self) -> ApprovalRequirement;
    fn execute(&self, input: &Value) -> Result<ToolOutput, ToolError>;
}
```

Rules:

- Approval is reported, not enforced, by the tool. The calling handler must
  display and enforce it.
- Tool names are namespaced (`mcp:github.search`).
- Errors and summaries are display-safe and never contain secrets.
- This crate has no transport, storage or UI. Tool execution remains opt-in.

### `PasteTarget`

`crates/clipboard/src/insertion.rs`

```rust
pub trait PasteTarget {
    fn paste_text(
        &mut self,
        text: &str,
        policy: InsertionPolicy,
        config: InsertionConfig,
    ) -> InsertionOutcome;
}
```

`TransactionPaste` implements this trait over `InsertionHost` and
`InsertionTarget`. Snippets must use it so copy-first ordering, target
verification, IME/secure-input checks and truthful `Inserted`/`Unverified`
outcomes are identical to clipboard history.

## Vault wiring

`DailyDriver::open` now opens the native vault at `<data_dir>/vault` and keeps
the result:

- `DailyDriver::vault()` — opened vault, if available.
- `DailyDriver::vault_status()` — last-known mode/state.
- `DailyDriver::vault_error()` — typed failure reason when unavailable.
- `put_vault_secret` / `get_vault_secret` and `seal_payload` /
  `open_sealed_payload` — the shared secret path for clipboard, MCP and agent
  features.

Vault failure is non-fatal at startup. There is no plaintext fallback. Callers
must report `vault_error` rather than silently writing secrets elsewhere.

## Generic records

Feature records use the existing revisioned `user_records` table; no feature
migration was added. Kinds:

| Feature | Kind | ID shape |
|---|---|---|
| Snippets | `feature.snippet` | `feature.snippet:<local-id>` |
| Scripts | `feature.script` | `feature.script:<local-id>` |
| MCP servers | `feature.mcp-server` | `feature.mcp-server:<local-id>` |
| Agent conversations | `feature.agent-conversation` | `feature.agent-conversation:<local-id>` |

`DailyDriver` exposes `create_feature_record`, `get_feature_record`,
`list_feature_records`, `update_feature_record` and `delete_feature_record`.
Payloads are opaque, unencrypted JSON. Do not store clipboard bodies, script
secrets, MCP credentials or transcripts until the encrypted-payload layer is
wired; use the vault for secret values.

## Exit criteria

- `cargo test --workspace --offline --no-fail-fast` passes.
- `cargo clippy --workspace --all-targets --offline -- -D warnings` passes.
- `cargo build -p ansa-desktop --offline` succeeds.
- Startup smoke and the 40-stage interaction smoke still pass.
- The new interfaces are documented above and have focused unit tests in their
  owning crates.
