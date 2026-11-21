# Review disposition — ANS-20 / L06 quicklinks

Source review: `docs/reviews/2026-11-19-storage-quicklinks.md` (Q-1..Q-12) plus the
final Max re-review follow-up.
Scope of this change: `crates/quicklinks/**` only. No `unsafe`, no new dependencies.
`SCHEMA_VERSION` remains **1** (the export shape is unchanged); imports revalidate every
item under the current policy. A version bump was considered but would break the existing
`ansa-alpha` integration assertion, which is outside this ticket's file ownership.

## Required findings

| ID | Disposition | Evidence |
| --- | --- | --- |
| **Q-1** Import bypassed target policy | **Fixed** | `Library::import_json_with_policy` applies `resolve_target` to every item before it is accepted. Every template is resolved with its declared defaults; required arguments use a documented deterministic sample (`sample`, `1`, `true`, first choice). Unsafe defaults, blocked/unapproved schemes, opener mismatches and non-absolute samples become per-item `ImportItemError`s, so merge skips them and replace stays all-or-nothing. `import_json` keeps a `with_local_paths` baseline so persisted local-reveal links still load; hosts can pass a narrower policy. Tests: `import_rejects_unsafe_defaults_with_per_item_errors`, `import_rejects_browser_local_path_and_policy_mismatches`, `import_accepts_a_safe_required_argument_template`, `merge_import_skips_unsafe_items_and_keeps_valid_ones`, `import_rejects_executable_and_unknown_default_app_targets`. |
| **Q-2** Merge could exceed `MAX_QUICKLINKS` | **Fixed** | `import_merge` counts new identities before applying anything; if the library cannot hold them it reports a document-level error and returns without mutating. Test: `merge_import_refuses_to_exceed_the_library_cap` (fills exactly 10,000, then merges one more). |
| **Q-3** One local-path boolean; executables/UNC/device | **Fixed (final residual included)** | `OpenPolicy` has independent `allow_network_paths`, `allow_device_paths`, `allow_executables` and `allow_unknown_file_types` (all default false; older serialized policies stay restrictive). UNC/`//host/share` and `\\?\`/`\\.\` paths produce typed errors. `DefaultApp` now uses the `SAFE_DEFAULT_APP_EXTENSIONS` allowlist (documents, media, archives); every other extension is denied. Known interpreters/scripts/installers/active content (including `.py .pyw .jar .hta .pl .rb .sh .application .pif .scf .msc .settingcontent-ms .search-ms .diagcab`) return `ExecutableDisallowed`; unknown types return `UnknownFileTypeDisallowed`. `allow_unknown_file_types` permits unknown non-executable types; `allow_executables` is the broad opt-in; `FileManager` can still reveal all files. Extensionless paths/folders and trailing-separator paths are allowed. Tests: `network_and_device_paths_require_explicit_opt_ins`, `default_app_blocks_executables_until_explicitly_allowed`, `default_app_uses_a_safe_allowlist_and_denies_unknown_types`, `import_rejects_executable_and_unknown_default_app_targets`. Allowed roots are not added. |

## LOW findings

| ID | Disposition | Evidence |
| --- | --- | --- |
| **Q-4** Malformed authorities/userinfo | **Fixed** | Host labels, dot-only hosts, port range 1..=65535, userinfo and bracketed IPv6 shape (single/multiple `::`, group count, leading/trailing colon, embedded IPv4) are validated; `%00`/CR/LF escapes are rejected. Tests: `malformed_authorities_and_userinfo_are_rejected`, `malformed_urls_are_rejected`, `invalid_bracketed_ipv6_shapes_are_rejected`. |
| **Q-5** Raw Unicode literal URLs rejected | **Fixed** | Non-ASCII IRI text is accepted verbatim; the preview shows the exact bytes. Test: `literal_unicode_urls_are_accepted_verbatim`. |
| **Q-6** Bidi/zero-width spoofing | **Fixed** | Bidi overrides and zero-width format controls are rejected in titles, aliases, tags, template literals, literal path segments and resolved targets; language/emoji joiners remain allowed. Test: `invisible_format_controls_are_rejected`. |
| **Q-7** Unbounded argument encoding | **Fixed** | Raw argument length is checked against the remaining target budget before cloning/encoding; defaults are bounded at parse; final target is bounded in bytes and characters. Test: `resolved_target_is_bounded_before_large_allocation`. |
| **Q-8** Dot/space path segments | **Fixed** | Non-empty segments consisting only of `.`/space (other than separators) are rejected; device namespace prefixes are stripped before segment checks. Test: `ambiguous_dot_and_space_path_segments_are_rejected`. |
| **Q-9** Alias grammar mismatch | **Fixed** | Quicklink aliases are 1..=32 ASCII chars, start alphanumeric, then `A-Za-z0-9._-`, matching the settings alias-prefix grammar. Test: `alias_grammar_matches_settings_alias_prefixes`. |
| **Q-10** Unused fingerprint | **Fixed** | `Library::store` verifies `Preview::verify_fingerprint()` before saving; mismatch yields `CommitError::FingerprintMismatch`. The branch is exercised by a test-only corruption hook: `fingerprint_mismatch_is_rejected_at_commit`. |
| **Q-11** Platform boundary mismatch | **Partial** | Resolved targets are capped at 8,192 bytes (matching the host URL limit), raw Unicode is accepted by both layers, and Unicode whitespace/soft hyphen are now rejected like the platform adapter. The platform adapter still owns its separate allowlist; sharing one validator would require editing `crates/platform-windows`, outside this ticket's file ownership. |
| **Q-12** `ReplaceConfirmed` is a convention | **Not fixed** | Changing `ImportOptions`/`ImportMode` to require a token would break existing `ansa-alpha` and CLI callers, which are outside this crate's ownership. The variant docs state the caller-confirmation contract and dry-run reports remain available. |
| Unicode whitespace/soft hyphen | **Fixed** | `U+00A0`, `U+2000`, `U+2028`, `U+2029`, `U+00AD` and other non-ASCII whitespace are rejected in targets; ordinary ASCII spaces in local paths remain valid. Test: `unicode_whitespace_and_soft_hyphen_are_rejected_in_targets`. |
| Tightened validation not signalled by schema version | **Not fixed (deferred)** | `SCHEMA_VERSION` remains 1 because `ansa-alpha`'s integration test pins the exported version and is outside this crate's ownership; bumping it would break a consumer. Import revalidates every item under the current policy, so the old shape cannot bypass new checks. `MIN_SUPPORTED_SCHEMA_VERSION` is exported so a future bump can accept v1 explicitly. Test: `schema_v1_imports_are_accepted_and_revalidated`. |
| Unbounded library list | **Partial** | The library is hard-capped at `MAX_QUICKLINKS` (10,000), import enforces it, and `Library::list_page(offset, limit)` provides a bounded page API. `list()` still materialises up to the 10,000-entry cap for existing callers. |
| Inconsistent length limits | **Fixed for target/alias** | Target bytes/chars and alias length align with the host/settings boundaries. Title, tag, template and import limits remain explicit constants. |
| Scheme case/whitespace/encoding tricks | **Fixed** | Schemes are matched case-insensitively; leading/trailing whitespace, control characters and percent-obfuscated schemes are rejected. Test: `scheme_obfuscation_is_rejected`. |

## Validation

Run after these changes:

- `cargo test -p ansa-quicklinks --offline` — 77 passed, 0 failed, 0 ignored; 0 doc-tests.
- `cargo clippy -p ansa-quicklinks --all-targets --offline -- -D warnings` — exit 0.
- `cargo fmt -p ansa-quicklinks -- --check` — exit 0.

Consumer builds (`ansa-cli`, `ansa-alpha`) are currently blocked by unrelated in-tree `ansa-registry` WIP (`E0599`), not by this crate; the public API changes were checked before that regression appeared.
