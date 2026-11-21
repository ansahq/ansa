# Review disposition — coordinator and ranking (2026-11-19)

Source review: `docs/reviews/2026-11-19-coordinator-ranking.md` (reviewer-owned,
not modified). Scope of fixes: `crates/coordinator/**`, `crates/search/**` and
`docs/ranking/**`.

## Re-review residuals

| ID | Status | Evidence |
|---|---|---|
| R1 | Fixed | `LocalRankProvider` now stores a result budget (`new` defaults to 200, `new_with_budget`, `for_spec`, `with_result_budget`). `batch()` truncates to that stored budget, so a coordinator budget of 10/50 cannot trigger `BatchOverBudget`. `batch_with_budget` remains for explicit calls. Test `local_rank_provider_uses_configured_budget`. The in-tree alpha constructor is outside this change's ownership; `for_spec` is the intended replacement there. |
| R2 | Fixed | `submit` validates invocation, cancellation, provider state and batch size before touching the clock. Stale/cancelled/terminal submissions do not advance `now`. In-flight accepted work advances the clock; a far-future submission clamps to the provider deadline. Test `stale_submission_does_not_poison_the_clock`. |

## Original findings

| ID | Status | Evidence |
|---|---|---|
| H1 | Fixed | `LocalRankProvider::batch` now truncates to `ProviderSpec::DEFAULT_RESULT_BUDGET`; `batch_with_budget` accepts an explicit budget. Truncated batches set `ProviderBatch::is_truncated` and return `CompletedTruncated` instead of `BatchOverBudget`. Test `local_rank_provider_caps_default_budget_and_reports_truncation` uses a 1,000-command catalog and both default and 200-item budgets. |
| H2 | Fixed | `SearchState::Loading` is the only non-terminal state. A provider stays `Loading` while in flight even with results; deadline expiry or failure finalizes it as `Partial` (results retained) or `Failed` (no results). `SearchState::is_terminal` now includes `Partial`. `tick` finalizes in-flight providers at their deadline. Tests `partial_results_finalize_at_the_provider_deadline`, `tick_expires_hanging_providers_and_rejects_late_batches`, `partial_results_survive_a_later_failure_and_are_not_collapsed`, and `complete_success_is_not_downgraded_by_a_late_failure`. |
| H3 | Fixed | `ProviderBatch` now carries `CoordinatorId` and `InvocationId`; `submit` rejects a mismatched coordinator with `CoordinatorError::BatchCoordinatorMismatch`. Test `batches_from_another_coordinator_are_rejected` uses two coordinators both at invocation id 1. |
| H4 | Fixed | The corpus runner now enforces the full ordered expected prefix, not only top-1, and asserts no problems after every split. The four documented failures are marked `known_failure`. `expected_order_regression_is_detected` injects a position-2/3 swap and requires detection. Category/tier, duplicate-query, disjoint-split, expected-id existence/uniqueness and pinned-count checks run in `validate_corpus`. |
| M1 | Fixed | `submit` and `fail` reject unavailable providers with `CoordinatorError::ProviderUnavailable` before deadline/clock/state mutation; `dispatch` skips them and `tick` never expires them. Test `unavailable_provider_is_sticky_and_not_dispatchable`. |
| M2 | Fixed | Added terminal `SearchState::Cancelled`. `cancel` clears pending work, marks non-unavailable providers cancelled, refreshes the aggregate and retains results; `tick` cannot turn cancellation into failure. Test `cancellation_is_best_effort_and_late_batches_are_still_ignored`. |
| M3 | Fixed | Queue limits are validated before mutation. Per-provider and global overflow return typed `ProviderQueueFull`/`GlobalQueueFull` errors and queue nothing; live work is never evicted. `dispatch` targets only in-flight providers that have not already received the invocation, so a partial submit does not cause duplicate work. Tests `queue_limits_reject_instead_of_evicting_live_work`, `partial_results_finalize_at_the_provider_deadline`, and `query_flood_keeps_queues_and_results_bounded`. |
| M4 | Fixed | Token-labelled queries now genuinely hit the token tier (`windows console`, `explorer file`, `monitor system`); validation exact uses the distinct `東京 駅`; duplicate `café` was removed. The runner asserts category-to-tier consistency and normalized query uniqueness within and across splits. |
| L1 | Fixed | A duplicate final batch with retained results returns `Completed { accepted: 0 }` (or `Empty` only when the provider is genuinely empty). Test `duplicate_final_batch_reports_completed_not_empty`. |
| L2 | Fixed | `submit` validates batch size and provider availability before advancing the clock. The over-budget test now submits at `now = 50` and asserts the snapshot and clock are unchanged. |
| L3 | Partial | `ProviderSpec` documentation now states that the budget is an item count, not a byte bound. Failure reasons are truncated to `MAX_FAILURE_REASON_CHARS = 256`. A serialized-byte budget is not implemented; untrusted text sizing remains a caller responsibility. |
| L4 | Fixed | Added fallible `try_begin`/`try_begin_at` and `CoordinatorError::InvocationIdExhausted`; the infallible `begin_at` no longer panics at exhaustion, returning a non-current fail-closed sentinel instead. Test `invocation_exhaustion_is_explicit_and_fail_closed`. |
| L5 | Fixed | Type documentation now distinguishes private-mode signal reads/writes from explicit resets and the readable logical clock. Private mode still blocks timestamp recording; `now()` remains a clock by design. |
| L6 | Fixed | Search module docs now list all six tiers and include recency; the policy page now states that exact/prefix inspect title/alias while token matching includes subtitle/keyword fields. |
| L7 | Fixed | Added `selection_stability_interleaves_navigation_and_coordinator_snapshots`, which moves selection between real coordinator snapshots and covers duplicate row identities. The original scenario matrix remains. |
| L8 | Fixed | Corpus integrity assertions cover expected-id existence, expected-id uniqueness, non-empty query uniqueness per split and across splits, command/query counts, and corpus version. |
| L9 | Partial | Added public `rank_bounded` and made `LocalRankProvider` use it. The policy page now states that `rank_bounded` still scans and sorts the whole catalog and only truncates returned output, and that `SearchSignals` maps remain unbounded. Callers must treat catalog/signal size as explicit budgets. |

## Minor follow-ups

| Item | Status | Evidence |
|---|---|---|
| Unreachable `ProviderQueueFull` | Documented | The variant remains as a defensive per-provider invariant; the doc comment states that with one current invocation and duplicate coalescing the global limit is the reachable limit. |
| `rank_bounded` claims | Documented (scope-limited) | `docs/ranking/ranking-policy.md` now states that `rank_bounded` still scans and sorts the whole catalog and only truncates returned output. The Rust docstring lives in `crates/search`, outside this follow-up's coordinator/docs scope, and was left unchanged. |
| "Bounded" signal wording | Documented (scope-limited) | The policy page no longer calls the signal writer bounded and notes that `SearchSignals` maps grow with distinct ids. The Rust docstring in `crates/search` was left unchanged for the same scope reason. |
| Late failure vs Ready | Fixed | `fail` on a terminal `Ready`/`Empty` provider returns `BatchOutcome::Settled { state }` and leaves the state unchanged. |
| Zero-budget truncation | Fixed | A zero-budget local batch reports `CompletedTruncated { accepted: 0 }` rather than plain `Empty`. |
| Stale comments/thresholds | Fixed | Corpus thresholds remain the measured `2026-11-19.3` values; runner comments and policy docs match the current ordered/tier checks. |
| Cross-process `CoordinatorId` | Documented | The type doc states it is a process-local counter, not a cross-process session identity. |
| Short corpus expected lists | Documented | Recall uses the expected-list length; order is enforced over that list length, not a padded top-10. |

## Verification

- `cargo test -p ansa-coordinator -p ansa-search --offline`
- `cargo clippy -p ansa-coordinator -p ansa-search --all-targets --offline -- -D warnings`
- `cargo fmt -p ansa-coordinator -p ansa-search --check`

Corpus metrics for version `2026-11-19.3` are recorded in
[`baseline-2026-11-19.md`](baseline-2026-11-19.md).
