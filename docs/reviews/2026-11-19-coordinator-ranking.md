# Search coordinator and ranking review — ANS-17 / L03 and ANS-18 / L04 (2026-11-19)

**Scope:** `crates/coordinator/**` (ANS-17, task `nmkgc9gttfba9smr8smo940n`) and
`crates/search/**` plus `docs/ranking/**` (ANS-18, task `p60xt6aw2yua0r8umz8tbpmm`).
`crates/domain/src/lib.rs` was read for `Command`/`CommandId` identity rules, and
`crates/alpha/src/coordinator.rs` was read only to understand the current in-tree
consumer. No implementation or test file was modified. The only repository file written
is this review.

**Reviewer:** independent subagent.

**Bottom line:** **No BLOCKER.** The central stale-batch invariant holds: no
same-coordinator sequence was found in which an older invocation replaces a newer one,
including cancellation failure, `begin`/`begin_at`, restart-shaped identity, unknown
providers and duplicate submits. The remaining issues are **4 HIGH**, **4 MEDIUM** and
**9 LOW**. H1 is the closest to a blocker—the public `LocalRankProvider` adapter emits an
uncapped "complete" batch and hard-errors against the default 200-item result budget,
leaving the provider `Loading`—but the in-tree alpha consumer sizes the budget to the
catalog and therefore avoids it. The other HIGHs are H2 (a finalized partial result set
is not terminal, so `is_terminal()` can remain false forever after a timeout or failure),
H3 (`ProviderBatch` carries only `InvocationId`, so a batch from another coordinator with
the same numeric id is accepted), and H4 (the golden corpus runner does not enforce
expected ordering beyond position 1, so ranking-order regressions pass CI).

---

## Commands and results

Run from the repository root on 2026-11-19, offline.

| Command | Result |
|---|---|
| `cargo test -p ansa-coordinator -p ansa-search --offline` | **PASS** — coordinator 20 passed, search 26 passed, relevance corpus 1 passed; 0 failed, 0 ignored; 0 doctests |
| `cargo test --locked -p ansa-coordinator -p ansa-search --offline` | **PASS** — same counts |
| `cargo clippy -p ansa-coordinator -p ansa-search --all-targets --offline -- -D warnings` | **PASS** — finished with no warnings or errors |
| `cargo fmt -p ansa-coordinator -p ansa-search -- --check` | **PASS** — exit 0, no diff |
| `cargo fmt --all -- --check` | **FAIL** — workspace-level at review time, unrelated to these crates: 39 diff sites under `apps/cli/src/main.rs`, `apps/cli/tests/cli.rs`, and `crates/alpha/src/*` / `crates/alpha/tests/*`. No diff was reported under `crates/coordinator` or `crates/search`. |
| `cargo test --locked --offline` (full workspace context) | **FAIL** — at review time, unrelated registry work in progress: `ansa-registry` 3 tests fail (`command::tests::command_kinds_are_data_and_classify_consequence`, `error::tests::error_maptable_is_stable_and_complete`, `registry::tests::prepare_applies_defaults_and_reports_the_effect_before_execution`). All 10 `ansa-alpha` integration tests pass, including the 1,000-cycle coordinator harness. The requested L03/L04 package tests pass. |
| `cargo test -p ansa-search --test relevance_corpus --offline -- --nocapture` | **PASS** — metrics reproduce exactly, see below |

Measured corpus metrics (identical to `docs/ranking/baseline-2026-11-19.md`):

- tuning 23 queries: all categories and overall top-1 / recall@10 / MRR = 1.000;
- validation 40 queries: typo 0.714 / 0.667 / 0.714, unicode 0.667 / 0.667 / 0.667,
  overall 0.900 / 0.924 / 0.900;
- combined 63 queries: overall 0.937 / 0.953 / 0.937.

The runner reported exactly the four documented held-out failures: `calender`,
`termianl`, `cafe`, and `uber`. All expected ids exist in the corpus; command ids are
unique (independently checked with a JSON script).

Independent probes were compiled and run outside the repository at
`C:\Users\abhir\AppData\Local\Temp\opencode\probe-coord`,
`...\probe-search` and `...\probe-corpus` (path dependencies on the worktree crates).
No repository test was added or changed. Probe outputs are quoted under the findings.

---

## Findings

| ID | Severity | Location | Finding |
|---|---|---|---|
| H1 | HIGH | `crates/coordinator/src/lib.rs:1171-1207`, `:450-455`, `:834-841` | `LocalRankProvider::batch` returns every ranked match as a "complete" batch. With the default `ProviderSpec` result budget (200), any catalog over 200 commands produces `Err(BatchOverBudget)` for an empty/broad query, no results, and a provider stuck in `Loading`. The in-tree alpha consumer sizes its budget to the catalog, so it happens to avoid this; the public adapter/default contract is still unsafe. |
| H2 | HIGH | `crates/coordinator/src/lib.rs:270-276`, `:967-982`, `:1080-1089`, `:1105-1144` | A provider that produced a partial batch and then times out is never finalized: `tick` only expires `Loading` providers, so the provider stays `Partial` with no failure reason and the aggregate stays `Partial` forever. The same shape occurs when a provider fails after producing results, or fails after a complete success. `is_terminal()` therefore can never become true once a partial result exists and any peer failed/timed out. |
| H3 | HIGH | `crates/coordinator/src/lib.rs:287-309`, `:808-822`; `crates/coordinator/src/lib.rs:116-142` | `ProviderBatch` stores only `InvocationId`, not the `CoordinatorId` from its `Invocation`. Two coordinators both start at invocation id 1, so a batch built from coordinator B's invocation is accepted by coordinator A under A's current invocation 1. Probe reproduced `Completed { accepted: 1 }` and a foreign result in A. This is a cross-query isolation hole, not a same-coordinator stale-batch bug. |
| H4 | HIGH | `crates/search/tests/relevance_corpus.rs:219-269`, `:354-439`; `docs/ranking/baseline-2026-11-19.md:14-17, 45-54`; `docs/ranking/ranking-policy.md:107-109` | The golden runner checks top-1, set-based recall@10 and MRR, but never asserts the ordered `expected_top10` list after position 1. A synthetic swap of tuning/browse positions 2 and 3 passed the test unchanged. A top-1 regression did fail correctly. The docs' "ordered expected top-10" and "fails on any regression" claims are therefore too broad. |
| M1 | MEDIUM | `crates/coordinator/src/lib.rs:843-853`, `:920-947`, `:1105-1112` | An unavailable provider can be mutated into `Failed`. `submit` checks the deadline before checking `SearchState::Unavailable`, and `fail` never checks availability. A coordinator whose only provider is unavailable reports `Unavailable` initially, then `Failed` after `fail` or after a submit at/after the deadline. |
| M2 | MEDIUM | `crates/coordinator/src/lib.rs:949-960`, `:270-276`, `:1105-1144` | `cancel` sets `cancelled` and clears pending work but does not refresh the aggregate or provide a cancelled/settled state. After cancelling, the aggregate remains `Loading` (or `Partial`) and `is_terminal()` is false; a later `tick` turns it into `Failed`. Late submits are correctly rejected as `Cancelled`. |
| M3 | MEDIUM | `crates/coordinator/src/lib.rs:733-802`, `:1063-1078`, `:1846-1910` | The queue limits are largely unreachable and the flood test does not exercise them. `begin` clears pending, only the current invocation can be dispatched, and dispatches for the same invocation coalesce, so a provider's pending list never holds more than one item. The global limit can evict the just-dispatched current work; probe with 3 providers and global limit 1 reported 3 dispatches, evicted 2, and pending `[0, 0, 1]`. A re-dispatch can then duplicate work already reported. The flood test uses global limit 3 with 3 providers, so eviction never runs. |
| M4 | MEDIUM | `crates/search/data/relevance_corpus.json:58,60,95,110`; `crates/search/tests/relevance_corpus.rs:271-323` | Category labels are not tier-pure: tuning/token `"windows term"` and `"file exp"` are actually `TitlePrefix`, validation/token `"system mon"` is `TitlePrefix`, and validation/exact `"東京"` is `TitlePrefix`. The non-empty query `"東京"` is also duplicated between tuning/unicode and validation/exact, so validation/exact's `n=7` includes a repeated prefix case. Per-category metrics therefore do not measure only the tier their name implies. |
| L1 | LOW | `crates/coordinator/src/lib.rs:898-911`, `:332-353` | A duplicate `complete` batch with no new items returns `BatchOutcome::Empty` while the provider retains results and is `Ready`. Probe: first submit `Completed { accepted: 1 }`; duplicate submit `Empty`, state `Ready`, results 1. `BatchOutcome::Empty` is documented as "the provider is empty". |
| L2 | LOW | `crates/coordinator/src/lib.rs:824`, `:834-841`, `:1764-1786` | An over-budget error advances the logical clock (`self.now`) before validation, so `snapshot()` changes even though the test is named "do not change state". Probe: `now_before=0`, `now_after=50`, snapshots differ. The test passes only because it submits at `now=0`. |
| L3 | LOW | `crates/coordinator/src/lib.rs:438-439`, `:501-503`, `:936-944`; `:548` | The result budget is a count, not a byte bound, despite the `ProviderSpec` doc claiming it bounds "the memory one provider can hand to the coordinator". A 200-item batch can contain arbitrarily large titles/keywords/subtitles. Failure reasons are also stored and cloned without a length cap. |
| L4 | LOW | `crates/coordinator/src/lib.rs:696-702` | `begin_at` uses `checked_add(1).expect(...)`, so invocation-id exhaustion panics. It is fail-closed and practically unreachable (2^64 calls), but the crate advertises no panics on adversarial counts. |
| L5 | LOW | `crates/search/src/lib.rs:149-153`, `:213-231`, `:91-97` | `now()` returns the newest recorded timestamp even in private mode, while `last_used_at` and `recency` are gated. `reset_usage`, `reset_favorites` and `reset_all` also mutate stored signals while private, contradicting the type-level "private mode makes both reads and writes inert" wording. The policy page is clearer than the type doc. |
| L6 | LOW | `crates/search/src/lib.rs:3-15`; `docs/ranking/ranking-policy.md:52-59` | Documentation drift: the module says "first four tiers" but lists six and omits recency from its favorite/usage summary. The policy says subtitle fields participate in "exact, prefix and token" matching; the code includes subtitles only in token matching (exact/prefix inspect title/alias only). |
| L7 | LOW | `crates/coordinator/src/lib.rs:1951-1996` | The "property selection stability" test is a deterministic 24×40 scenario matrix, not a randomized property. It covers insertion, reversal, omission, retention, repair and clear, but never interleaves `move_by` with `apply_rows`, never uses duplicate ids, and never applies real coordinator snapshots. |
| L8 | LOW | `crates/search/tests/relevance_corpus.rs:354-374` | Corpus integrity assertions are incomplete: the runner checks command-id uniqueness and the version string, but not that every `expected_top10` id exists, that expected ids are unique per query, that non-empty query strings are unique, or that the corpus content is pinned by a hash/count. All current data is valid (verified independently), but a bad expectation can silently become an unfixed failure. |
| L9 | LOW | `crates/search/src/lib.rs:463-498`, `:355-400`, `:91-105` | `rank` returns every match (the empty query returns the whole catalog), fuzzy matching is O(haystack²·query) per field, and `SearchSignals` is documented as "bounded" but its `HashSet`/`HashMap`s accept unbounded distinct ids. There is no result cap or signal pruning in this crate; the coordinator's quota is the only cap. |

### HIGH detail

**H1 — `LocalRankProvider` violates the default result budget.**

`LocalRankProvider::rank` (`lib.rs:1194-1201`) returns every ranked match and
`batch` (`:1204-1206`) wraps all of them in `ProviderBatch::complete`. `submit`
rejects a batch whose item count exceeds `spec.result_budget` (`:834-841`). The
defaults are quota 50 and budget 200 (`:450-455`). Probe with an empty query:

```
catalog=150 -> Ok(QuotaExhausted { accepted: 50, dropped: 100 }), results=50, Ready
catalog=200 -> Ok(QuotaExhausted { accepted: 50, dropped: 150 }), results=50, Ready
catalog=201 -> Err(BatchOverBudget { size: 201, budget: 200 }), results=0, state=Loading
catalog=500 -> Err(BatchOverBudget { size: 500, budget: 200 }), results=0, state=Loading
```

The error is returned before `remove_pending`, so the provider is still `Loading`
and the aggregate has no failure indication. The in-tree consumer
`crates/alpha/src/coordinator.rs:17-31` derives the budget from the catalog size and
therefore works, which is why the alpha harness and integration tests pass. That is
a consumer workaround, not an adapter guarantee.

Impact: any other consumer following the documented `LocalRankProvider`/`ProviderSpec`
defaults gets zero local results for a realistic catalog (>200 apps/quicklinks), with
no terminal failure state. Fix: make the adapter take a batch limit (or the
`ProviderSpec`/quota) and cap before constructing the batch; alternatively, let the
coordinator truncate an ordered local batch to the budget and return
`QuotaExhausted`, reserving `BatchOverBudget` for remote/protocol violations. Add an
integration test with a 400-item catalog and the default spec.

**H2 — partial results cannot be finalized.**

`tick` expires only providers whose state is exactly `Loading`
(`lib.rs:967-982`, condition at `:975`). `expire_provider` itself would keep a provider
with results as `Partial` (`:1080-1089`), and `fail` does the same (`:936-944`).
`compute_aggregate_state` maps "has results + any failed/partial" to `Partial`
(`:1126-1131`), while `is_terminal()` excludes `Partial` (`:270-276`). Note that
`submit` removes pending work for *any* batch, including a non-final partial batch
(`:843`), so `pending_total() == 0` is not a reliable "no more work" signal either.
Probe:

```
partial submit: Accepted { accepted: 1 }
tick(100): expired=1, aggregate=Partial, provider=Partial, failure=None, pending=0
late complete after tick: Expired, aggregate=Partial
local complete + remote fail: aggregate=Partial, is_terminal=false; tick(1000) expired=0
ready then late fail: aggregate=Partial, is_terminal=false, results=1
```

There is no public way to know that a `Partial` aggregate is final. A renderer that
stops its spinner on `is_terminal()` will spin forever; a renderer that treats `Partial`
as final will mislabel an in-progress search. The deadline is explicit per provider,
but a partial provider's deadline has no observable consequence unless a later batch
happens to arrive.

Impact: state-machine correctness and the "per-provider deadline" requirement; also
the "Partial, empty and failed states are visually and semantically distinct" criterion
is only half met (distinct labels, no finality semantics).

Fix: track whether a provider still has live work separately from its result state
(e.g. `LoadingWithResults` vs `PartialDone`, or an `in_flight`/`settled` flag), and make
the aggregate terminal when no provider is loading/pending. On deadline expiry of a
partial provider, record `deadline exceeded` and finalize it. Add tests for
partial-then-timeout, partial-then-fail, and complete-then-fail.

**H3 — batch identity is not bound to the coordinator.**

`Invocation` carries `CoordinatorId` + `InvocationId` (`:116-142`), and
`validate_invocation` correctly rejects a foreign invocation token (`:1043-1054`).
`ProviderBatch` stores only `invocation: InvocationId` (`:288-292`) and compares only
that id (`:817-822`). Probe:

```
a.begin("qa").id = 1; b.begin("qb").id = 1
a.dispatch(inv_a);
a.submit(inv_a, "local", ProviderBatch::complete(inv_b, [app:foreign]))
-> Completed { accepted: 1 }; results = ["app:foreign"]
```

The batch's `InvocationId` is valid for A, so A cannot tell it was produced by B. A
shared provider runtime serving two windows, or a mistaken batch construction, leaks
B's query results into A. The same collision can occur across processes because
`CoordinatorId` is a per-process static.

Impact: cross-coordinator/cross-window result contamination; the documented
"invocations from another coordinator are a programming error" boundary is incomplete.

Fix: add `CoordinatorId` to `ProviderBatch` (the constructors already receive the whole
`Invocation`) and reject when it does not match. Add a test with two coordinators both
at invocation id 1.

**H4 — the golden corpus does not enforce expected ordering after top-1.**

`evaluate` computes top-1, set-based recall@10 and MRR (`relevance_corpus.rs:219-269`).
`recall@10` uses a `HashSet` intersection; `MRR` uses the first expected id found; the
`problems` list only reports top-1 mismatch and missing expected ids. The ordered
`expected_top10` value is never compared. Verified in a disposable copy of the crate:

- replacing tuning/browse's first expected id with a wrong id -> test failed as intended
  (`tuning/browse top-1 regression: 0.000000 < 1.000000`);
- swapping tuning/browse expected positions 2 and 3 (`notes`/`settings`) -> test passed
  with all metrics unchanged.

The current corpus's full expected order does match the actual ranked prefix for every
passing query (independently compared), so the data is correct today. The gap is the
runner, not the data.

Impact: a ranker change that reorders results inside the top 10—exactly a user-visible
ranking regression—can pass CI. The docs describe the lists as ordered and the baseline
as failing "on any regression"; that is not true yet.

Fix: for every query whose actual first result matches the expected first, assert the
actual ranked prefix equals `expected_top10`; add an explicit `known_failure` flag for
the four held-out queries that currently return nothing. Add a meta-test that injects a
position-2/3 swap and asserts failure.

### MEDIUM detail

**M1 — unavailable providers can become failed.** Probe on an unavailable-only
coordinator: initial aggregate `Unavailable`; `submit(..., now=10)` at deadline 3
returned `Expired` and changed both provider and aggregate to `Failed`; `fail(...)`
returned `Failed { retained: 0 }` and also changed the aggregate to `Failed`. `submit`
checks `now >= deadline` at `:843-848` before the `Unavailable` guard at `:850-853`;
`fail` has no guard. Fix: return `BatchOutcome::Unavailable` early for an unavailable
provider in both `submit` and `fail` (and never mutate its state in `expire_provider`).
Add tests that `Unavailable` is sticky.

**M2 — cancel is not a settled state.** Probe: after `begin` + `dispatch` + `cancel`,
aggregate `Loading`, `is_terminal=false`, `is_cancelled=true`, pending 0; submit returns
`Cancelled`; `tick(100)` then moves the aggregate to `Failed`. With results already
present, cancel leaves `Partial` and still `is_terminal=false`. Fix: either add a
`Cancelled` terminal state (and include it in `is_terminal`), or expose a settled flag
and refresh the aggregate from `cancel`; decide whether canceling clears displayed
results. The `cancelled` flag alone is not a state-machine answer.

**M3 — queue limits are bookkeeping theater and can evict live work.** Because
`begin_at` calls `reset` on every provider (`:696-718`, `:600-611`), pending queues are
empty at the start of each invocation; because only the current invocation can be
dispatched and duplicate dispatch coalesces (`:766-772`), each provider's pending list
holds at most one item. The per-provider `queue_limit` loop therefore never runs, and
the flood test's `global_limit=3` with 3 providers never triggers global eviction
(`:1846-1910`). With 3 providers and global limit 1, probe output was:

```
report dispatches=3 evicted=2 pending_total=1 pending a/b/c=[0, 0, 1]
```

The two evicted providers still received dispatch instructions and can still submit
results; their pending accounting is gone. Re-dispatching can report them again. Fix:
validate `global_queue_limit >= provider count` (or refuse dispatch that cannot be
tracked), evict only stale work, use a global sequence for FIFO, and add a test where
the limit is smaller than the provider count and assert no live work is evicted and the
report matches `pending_len`.

**M4 — corpus categories and duplicate query.** The actual tiers from the probe are:
tuning/token `"windows term"` → `TitlePrefix`, `"file exp"` → `TitlePrefix`,
`"studio code"` / `"video player"` → `Token`; validation/token `"system mon"` →
`TitlePrefix`; validation/exact `"東京"` → `TitlePrefix`. `"東京"` also appears in
tuning/unicode, so it is a non-empty duplicate across splits. Fix: either rename the
categories to scenario names (e.g. `"title-token-phrase"`) or reclassify the queries;
compute a tier distribution and assert it; deduplicate non-empty queries or mark them
as deliberate repeats. The metrics themselves (top-1, recall@10, MRR) are computed
correctly for the queries assigned to each category.

### LOW detail

- **L1** duplicate complete batch: `BatchOutcome::Empty` at `:898-905` should be
  `Completed { accepted: 0 }` when `runtime.results` is non-empty, or a new distinct
  outcome. Probe confirmed `Empty` with state `Ready` and one retained result.
- **L2** over-budget error: `self.now = self.now.max(now)` at `:824` precedes the size
  check at `:834-841`. Move the clock update after validation or make the test assert
  the clock change; the current "do not change state" test is only true at `now=0`.
- **L3** count-only budget: `ProviderSpec` doc at `:438-439` overclaims memory bounds;
  `BatchOverBudget` uses `items.len()` only. Failure reasons (`fail` at `:938`,
  snapshot at `:548`) are unbounded strings. Consider a byte/serialized-size budget or
  an explicit "trusted local provider" distinction, and a bounded reason.
- **L4** invocation exhaustion: `begin_at` panics via `expect` at `:696-702`. Prefer a
  `Result` or a documented fail-closed panic; not reachable in practice.
- **L5** private-mode edges: `now()` at `:149-153` returns the stored logical tick even
  in private mode; `reset_*` at `:213-231` mutate while private. The type doc at
  `:91-97` says "both reads and writes inert"; the policy page says resets clear. Make
  the type doc match the policy and gate `now()` if the clock is considered a signal.
- **L6** docs: `lib.rs:3-15` says "first four tiers" then lists six and omits recency
  from the summary; `ranking-policy.md:52-59` says subtitles participate in exact and
  prefix matching, but `evaluate` only checks title/alias for exact/prefix and includes
  subtitle only in token matching. The baseline's "fails on any regression" is covered
  by H4.
- **L7** selection test: the property test at `:1951-1996` is a deterministic matrix;
  add a randomized/interleaved test that calls `move_by` between row applications and
  uses a real `CoordinatorSnapshot`/`LocalRankProvider` flow, or rename it to a
  scenario test. It currently proves retention/repair/clear well but not "during active
  navigation".
- **L8** corpus integrity: add assertions that every expected id exists and is unique,
  that non-empty query strings are unique (or explicitly repeated), and pin the corpus
  content with a hash or query/command counts so a version bump is enforced. All
  current expectations are valid; this is a guard against future mistakes.
- **L9** unbounded ranking work: `rank` returns all matches (empty query = whole
  catalog), fuzzy matching can be O(title²·query) per field, and `SearchSignals` maps
  are unbounded despite the "bounded" doc. At minimum cap result count at the caller,
  document the catalog-size assumption, and consider pruning signal history. The
  coordinator quota can cap the integrated path, but direct `rank` callers (including
  `crates/core/src/lib.rs:109-116`) have no cap.

---

## Acceptance disposition

### ANS-17 / L03 — search coordinator

**Not accepted as-is; no BLOCKER.** The core revision safety is sound:

- same-coordinator late/reordered batches are rejected by `InvocationId`, including
  after `cancel`, after `begin`/`begin_at`, and with a `ProviderBatch` carrying an old
  id under a current token;
- local results survive remote failure/timeout, and partial results are not collapsed
  into `Failed`;
- quotas drop excess deterministically, over-budget batches are explicit errors, and
  aggregate ordering/deduplication is stable;
- `SelectionSnapshot` retains selected identity across insertion/reorder/removal;
- provider/query counts and ids do not panic in the probes; the only panic path is
  theoretical invocation-id exhaustion.

Required before sign-off: H1 (adapter/default budget), H2 (partial finality and
deadline), H3 (batch coordinator binding), M1 (unavailable stickiness), M2 (cancel
settled state), M3 (queue accounting). The alpha integration covers H1 by sizing the
budget, but the public adapter must not rely on every consumer doing that.

### ANS-18 / L04 — ranking baseline

**Not accepted as-is; no BLOCKER.** Tier order, within-tier ordering, capped usage,
recency formula/cap, private-mode reads/writes, reset scoping, deterministic ID ties and
Unicode/typo limitations all behave as documented. The measured metrics match the
baseline exactly, and the validation failures are reported honestly rather than tuned
away. Required before sign-off: H4 (ordered expectations and regression sensitivity)
and M4 (category/tier labeling and duplicate query). Also add the corpus integrity
assertions in L8. The held-out provenance itself is an honor-system claim: the checkout
has no history proving when validation queries or thresholds were authored, although
the ranker contains no query-specific code and the known failures are consistent with
the stated process.

---

## Residual risks

- **No provider runtime.** The crate is a synchronous logical-tick state machine; there
  are no threads, async tasks, channels or locks. Deadlines advance only when the caller
  calls `tick`. This meets the behavioral tests with a deterministic simulator, but it
  does not literally implement the ticket's "bounded channels" scope or a general
  asynchronous provider runtime. A future remote provider must be driven by a
  caller-owned scheduler.
- **No UI/consumer of the finality states.** The alpha harness uses the coordinator
  synchronously and does not exercise partial timeout, cancel, failed-after-results or
  unavailable mutation. H2/M1/M2 are therefore not caught by the current integration.
- **Default adapter contract.** H1 is a hard failure for the default `ProviderSpec` +
  `LocalRankProvider` combination above 200 items. The alpha consumer happens to size
  the budget; other consumers (or future remote catalogs) may not.
- **Corpus scale.** 31 commands and 63 queries, with 1–7 queries per category. Per-
  category floors are coarse, aggregate floors can be offset by an equal improvement
  elsewhere, and there is no precision@k, no latency/large-catalog test, and no test
  that a ranker reorder inside the expected set fails. H4 addresses the largest gap.
- **Held-out provenance.** The validation queries and thresholds are committed together
  and thresholds equal measured validation values. This is disclosed and consistent
  with the docs, but cannot be independently proven from the checkout; future changes
  must keep the tuning/validation separation honest.
- **Recency semantics.** Recency is aged against the newest *recorded* logical tick, not
  a wall clock. An idle session can leave a previously used command at maximum recency
  until another use advances the clock. This is documented but may surprise callers who
  expect time-based decay.
- **Unbounded storage-adjacent state.** `SearchSignals` maps/sets grow with distinct
  ids; failure reasons are unbounded strings; provider count is unbounded (each dispatch
  clones the query per provider). Query length and per-provider result count are the
  only enforced bounds.
- **Workspace health.** `cargo fmt --all -- --check` and full-workspace `cargo test`
  are currently red in unrelated `apps/cli`/`crates/alpha` formatting and `ansa-
  registry` tests. The reviewed packages pass their requested commands, but CI cannot
  go green until those unrelated failures are resolved.

---

# Re-review after fixes — 2026-11-19 (corpus `2026-11-19.3`)

**Scope:** re-audit of the fixes listed in
`docs/ranking/review-disposition-2026-11-19.md` against the original findings above.
The original findings are preserved; this section records what changed, what was
verified independently, and what remains. No implementation or test file was modified.
The only repository file written remains this review.

**Reviewer:** independent subagent.

**Bottom line:** **No BLOCKER or HIGH remains.** H1–H4 and M1–M4 are fixed for the
stated cases, and the requested re-audit checks pass. Two MEDIUM residuals remain:
`LocalRankProvider::batch()` still reproduces the old `BatchOverBudget`/stuck-`Loading`
failure when the coordinator uses a custom result budget below the adapter's default 200,
and a rejected stale/cancelled submit can still advance the coordinator's logical clock
before rejection, allowing a far-future `now` to poison all subsequent queries. Both are
small, local fixes. ANS-18 is acceptable; ANS-17 is acceptable for the shipped/default
and in-tree alpha path but not yet unconditional as a public library API until R1/R2 are
addressed.

## Commands and results (re-run)

| Command | Result |
|---|---|
| `cargo test -p ansa-coordinator -p ansa-search --offline` | **PASS** — coordinator 29 passed, search 26 passed, relevance corpus 2 passed; 0 failed, 0 ignored; 0 doctests |
| `cargo test --locked -p ansa-coordinator -p ansa-search --offline` | **PASS** — same counts |
| `cargo clippy -p ansa-coordinator -p ansa-search --all-targets --offline -- -D warnings` | **PASS** — no warnings or errors |
| `cargo fmt -p ansa-coordinator -p ansa-search -- --check` | **PASS** — exit 0 |
| `cargo fmt --all -- --check` | **PASS** — exit 0; the unrelated formatting debt noted in the original review is gone |
| `cargo test --locked --offline` (full workspace) | **PASS** — all workspace tests and doctests pass; the 10 alpha integration tests and 6 catalog-composition tests pass, including the 1,000-cycle coordinator harness |
| `cargo test -p ansa-alpha --offline` | **PASS** — 10 integration + 6 catalog-composition tests |

Independent probes were rebuilt outside the repository at
`C:\Users\abhir\AppData\Local\Temp\opencode\probe-coord` (updated API) and
`...\probe-search`, plus a disposable copy of the search crate at
`...\probe-corpus-r2` for corpus-mutation tests.

## Verification matrix

| Item | Status | Evidence |
|---|---|---|
| H1 — `LocalRankProvider` default budget | **FIXED for the stated default** | 1,000-command catalog, empty query, default `ProviderSpec`: `batch().len() == 200`, `is_truncated() == true`, two builds have identical ids; submit returns `QuotaExhausted { accepted: 50, dropped: 150 }`, 50 results, `Ready`, terminal. With quota/budget 200: `CompletedTruncated { accepted: 200 }`, 200 results, `Ready`, terminal. No `BatchOverBudget`, no stuck `Loading`. |
| H1 — custom lower budget | **MEDIUM residual (R1)** | With `ProviderSpec::with_result_budget(50)` and the natural `batch()` call, the adapter still emits 200 items, `submit` returns `BatchOverBudget`, results 0, state `Loading`, terminal false. `batch_with_budget(50)` works and returns `CompletedTruncated { accepted: 50 }`. |
| H2 — partial finality/deadline | **FIXED** | Partial submit keeps provider `Loading` with results; `tick(9)` stays `Loading`; `tick(10)` expires it to `Partial`, terminal, retains 1 result, records `deadline exceeded`; further ticks return 0 and remain `Partial`. A late complete batch returns `Expired` without adding results. Partial-then-fail finalizes `Partial`; complete-success-then-late-fail leaves `Ready` with results and no failure. `Loading` is the only state for which `is_terminal()` is false. |
| H2 — nonterminal sequences | **No internal leak found** | Never-dispatched provider: `Loading` until `tick(10)`, then `Failed`, terminal. Global-queue rejection: providers stay `Loading`, but `tick(10)` expires both to `Failed`, terminal. Cancel: `Cancelled`, terminal, results retained, ticks stable. See R2 for a clock-trust edge. |
| H2 — rejected-batch clock mutation | **MEDIUM residual (R2)** | After `cancel`, a stale/cancelled submit at `now = u64::MAX` returns `Cancelled` but sets `now = u64::MAX`; `begin("new")` starts at `u64::MAX`, and every subsequent `tick`/submit sees `now >= deadline`, so the new query immediately expires/fails. Rejected batches therefore still mutate observable coordinator state. |
| H3 — cross-coordinator batch | **FIXED in-process** | Two coordinators both at invocation id 1 (observed CoordinatorIds 12 and 13) — submitting B's batch to A returns `BatchCoordinatorMismatch`; A retains zero foreign results; A's own batch is accepted. Residual: `CoordinatorId` is a per-process static, so a future serialized cross-process remote batch could still collide at id 1 (R8). |
| H4 — ordered corpus | **FIXED** | Runner has two tests; current corpus passes. In a disposable copy, swapping tuning/browse positions 2 and 3 (notes/settings) failed with `ordered expectation mismatch` at `evaluate_split`'s `problems.is_empty()` assertion. Top-1/recall/MRR and ordered-prefix checks are all active for non-`known_failure` queries. |
| H4 — corpus integrity/disjointness | **FIXED** | `validate_corpus` asserts 31 commands, 23 tuning + 39 validation queries, expected ids exist and are unique, 1..=10 expectations, normalized non-empty query uniqueness within and across splits, and pinned counts. Independent Node check found no duplicate normalized non-empty queries and no unknown expected ids. |
| H4 — categories/tiers | **FIXED** | Probe tiers: tuning/token `windows console`, `studio code`, `explorer file`, `video player` are all `Token`; validation/token `monitor system` is `Token`; validation/exact `東京 駅` is `ExactTitle`; `code` is `ExactAlias`; prefix/fuzzy/browse are consistent. No category/tier problems from the runner. |
| H4 — metrics/floors | **FIXED** | Independent recomputation: tuning 23 = 1.000/1.000/1.000; validation 39 = 0.897/0.923/0.897; combined 62 = 0.935/0.952/0.935. Exact fractions match the pinned floors: typo 5/7, 6/9, 5/7; unicode 3/5; overall 35/39, 60/65, 35/39. One-query metric drops fall below these floors, and ordered-prefix checks catch swaps even when aggregates are unchanged. |
| M1 — unavailable | **FIXED** | Unavailable-only coordinator: `dispatch` returns no work; `submit` and `fail` return `ProviderUnavailable`; `tick(1000)` returns 0; provider and aggregate stay `Unavailable`; `is_terminal()` true. |
| M2 — cancel | **FIXED** | Cancel after a partial result: state `Cancelled`, terminal, results retained (1), provider `Cancelled`, failure `cancelled`; late submit returns `Cancelled`; `tick(1000)` returns 0 and state stays `Cancelled`; new begin resets. |
| M3 — queue overflow | **FIXED for global path; LOW residual** | Global limit 1 with 3 providers: `GlobalQueueFull { limit: 1, requested: 3, queued: 0 }`, pending 0, queue nothing. Global limit 3: 3 dispatches, 0 evicted, pending 3; one submit leaves pending 2; duplicate dispatch returns 0. No live work evicted. `ProviderQueueFull` is unreachable because `dispatched` prevents more than one pending unit per provider (R3). |
| M4 — token/duplicate Unicode | **FIXED** | Token-labelled queries now hit `Token`; validation exact uses distinct `東京 駅`; `東京` (tuning unicode) and `東京駅` (validation unicode) are distinct normalized queries; duplicate `café` removed; runner's uniqueness and category/tier assertions pass. |
| L1 — duplicate final batch | **FIXED** | First complete returns `Completed { accepted: 1 }`; identical duplicate returns `Completed { accepted: 0 }`, state `Ready`, one result. |
| L2 — rejected over-budget clock | **FIXED for over-budget** | Over-budget submit at `now=50` leaves snapshot and `now` unchanged (test asserts both); the residual stale/cancelled clock path is R2. |
| L3 — count-only budget/reason | **FIXED/partial as documented** | `ProviderSpec` doc explicitly says item counts, not bytes, and assigns text-size limits to the caller; `MAX_FAILURE_REASON_CHARS = 256` truncates stored reasons. Serialized-byte budgeting remains intentionally unimplemented and documented. |
| L4 — exhaustion | **FIXED** | `try_begin`/`try_begin_at` return `InvocationIdExhausted`; infallible `begin_at` returns a non-current sentinel and leaves dispatch stale; test passes. |
| L5 — private/documented | **FIXED** | `SearchSignals` docs distinguish private-mode signal reads/writes from explicit resets and the readable logical clock; policy matches. |
| L6 — documentation | **FIXED for search** | Search module lists all six tiers and recency; policy correctly says exact/prefix inspect title/alias and token includes keywords/subtitles. Coordinator module still says in-flight partial results are labelled `Partial` (R7). |
| L7 — selection test | **FIXED** | Added `selection_stability_interleaves_navigation_and_coordinator_snapshots`: moves selection between real coordinator snapshots, retains identity across partial/complete remote batches, and covers duplicate row identities. Original matrix retained. |
| L8 — corpus integrity | **FIXED** | Counts, expected-id existence/uniqueness, non-empty query uniqueness across splits, and version are asserted. No content hash, but pinned counts + version are present. |
| L9 — bounded ranking/docs | **PARTIAL / LOW residual (R4)** | `rank_bounded` exists and `LocalRankProvider` uses it; module/policy docs honestly say `rank` scans the catalog. But `rank_bounded`'s own doc claims callers "avoid allocating and sorting more output than they will display" — it calls `rank` (full scan/sort) and truncates afterwards. `SearchSignals` type doc still says "bounded ranking signals" while its maps/sets are unbounded. |

## Corpus `2026-11-19.3` metrics (independently recomputed)

| Split/category | n | Top-1 | Recall@10 | MRR |
|---|---:|---:|---:|---:|
| tuning overall | 23 | 1.000 | 1.000 | 1.000 |
| validation exact | 7 | 1.000 | 1.000 | 1.000 |
| validation prefix | 7 | 1.000 | 1.000 | 1.000 |
| validation token | 6 | 1.000 | 1.000 | 1.000 |
| validation fuzzy | 6 | 1.000 | 1.000 | 1.000 |
| validation typo | 7 | 0.714 | 0.667 | 0.714 |
| validation unicode | 5 | 0.600 | 0.600 | 0.600 |
| validation browse | 1 | 1.000 | 1.000 | 1.000 |
| validation overall | 39 | 0.897 | 0.923 | 0.897 |
| combined overall | 62 | 0.935 | 0.952 | 0.935 |

The measured values are exactly the requested values and exactly the pinned floors. A
single non-`known_failure` query regression lowers its category/overall metric below the
floor, and the ordered-prefix assertion independently catches reorders that leave the
aggregate metrics unchanged. The four `known_failure` queries (`calender`, `termianl`,
`cafe`, `uber`) still count in the metrics and are skipped only for order/tier
assertions because they already fail as documented.

## Remaining findings

| ID | Severity | Location | Finding |
|---|---|---|---|
| R1 | MEDIUM | `crates/coordinator/src/lib.rs:1436-1442`, `:1444-1470`, `:987-996` | `LocalRankProvider::batch()` always truncates to `ProviderSpec::DEFAULT_RESULT_BUDGET` (200). If a coordinator uses a custom `result_budget < 200` (a normal configuration) and the caller uses the natural `batch()` method, it emits more than the budget; `submit` returns `BatchOverBudget`, retains zero results, and leaves the provider `Loading`. The stated H1 default case is fixed and `batch_with_budget` works, but the adapter and coordinator spec are not coupled. |
| R2 | MEDIUM | `crates/coordinator/src/lib.rs:998-1005`, `:1205-1220` | `submit` advances `self.now` before checking whether the invocation is current/cancelled. A stale or cancelled batch is correctly returned as `StaleInvocation`/`Cancelled`, but it still mutates the logical clock. A far-future `now` (or a unit mismatch such as milliseconds versus seconds) then leaves the next invocation `Loading` until a tick in that unit, or makes every future submission/tick expire immediately. Rejected batches should not mutate observable coordinator state. |
| R3 | LOW | `crates/coordinator/src/lib.rs:897-918`, `:453-456`, `:2547-2616` | The global queue error path is fixed and tested, but `ProviderQueueFull` is unreachable through the public API: `dispatch` only targets `dispatched == false`, `dispatched` is set whenever pending work is added, and a partial submit clears pending without clearing `dispatched`; `begin` resets both. The per-provider queue limit/error is effectively dead, and the disposition's "per-provider and global overflow" claim overstates the evidence. No functional harm: one pending unit per provider is bounded. |
| R4 | LOW | `crates/search/src/lib.rs:511-516`, `:99` | L9 is only partially documented honestly. `rank_bounded`'s function doc says callers "avoid allocating and sorting more output than they will display", but the implementation calls `rank` (full result vector and full sort) and truncates afterwards. `SearchSignals` still says "Locally stored, bounded ranking signals" although its favorite/usage/timestamp maps are unbounded by key count. Module and policy docs are honest; these two item docs are not. |
| R5 | LOW | `crates/coordinator/src/lib.rs:1163-1166`, `:2465-2475` | `fail` after a provider has reached `Ready` or `Empty` returns `BatchOutcome::Failed { retained }` while the provider state remains `Ready`/`Empty` and the failure reason is dropped from the snapshot. State does not regress (the H2 requirement), but the outcome contradicts the state and can make a caller display a failure that the coordinator does not record. |
| R6 | LOW | `crates/coordinator/src/lib.rs:1451-1468`, `:1112-1116` | `batch_with_budget(0)` returns a zero-item truncated batch; `submit` then reaches the Loading/complete/empty branch and returns `BatchOutcome::Empty`, not `CompletedTruncated`, losing the truncation report. The default budget 200 path is unaffected. |
| R7 | LOW | `crates/search/tests/relevance_corpus.rs:142-143`; `crates/coordinator/src/lib.rs:15-17` | Documentation/comment drift: the tuning threshold comment still says "version 2026-11-19.2" while the corpus is .3; the coordinator module says in-flight partial results are "labelled Partial" although they are now `Loading` until finalization. |
| R8 | LOW | `crates/coordinator/src/lib.rs:36`, `:103`, `:303-310` | `CoordinatorId` remains a per-process `AtomicU64` starting at 1. The in-process H3 test passes, but a future remote provider that serializes batches across processes can still collide at coordinator id 1. The current typed API has no serialization path, so this is a future-remote residual, not a present cross-talk hole. |
| R9 | LOW | `crates/search/tests/relevance_corpus.rs:291-318`; corpus expected lists | `known_failure` queries skip all order/tier assertions (appropriate for documented failures, but a future fix that changes their failure mode is not asserted), and many expected lists contain only 1–2 ids. A wrong result after the expected prefix is not checked; precision beyond the specified prefix/10 is not measured. The requested #2/#3 swap and metric floors are caught. |

### R1 detail

The H1 acceptance text (default budget; 1,000-command catalog) passes exactly. The
remaining issue is the public adapter contract: `batch()` uses
`ProviderSpec::DEFAULT_RESULT_BUDGET` unconditionally, while a caller can configure
`ProviderSpec::with_result_budget(50)`. Probe:

```
H1 default: batch_len=200 truncated=true deterministic=true
H1 default submit: QuotaExhausted { accepted: 50, dropped: 150 } results=50 Ready terminal
H1 roomy default: CompletedTruncated { accepted: 200 } results=200 Ready terminal
H1 lower-budget batch(): len=200 submit=Err(BatchOverBudget { size: 200, budget: 50 }) results=0 Loading terminal=false
H1 lower-budget batch_with_budget(50): CompletedTruncated { accepted: 50 } results=50 Ready terminal
```

Impact: a consumer that configures a smaller budget and calls the obvious `batch()`
recreates the original H1 failure with no result and a non-terminal provider. The in-tree
alpha consumer uses `batch_with_budget(catalog.len().max(1))`, so it is safe. Minimal fix:
make the budget explicit in `batch` (e.g. `batch_for_spec(&ProviderSpec)`) or document
`batch` as default-spec-only and add a regression test with a 50-item budget. The current
`batch_with_budget` API is a viable workaround.

### R2 detail

`submit` performs coordinator/invocation/availability/size checks before touching the
clock, but the `self.now = self.now.max(now)` update is before the current/cancelled
checks (`:998-1005`). Probe:

```
begin old, dispatch, cancel
submit(old, ..., now=u64::MAX) -> Cancelled, now=u64::MAX
begin new -> started_at=u64::MAX, deadline=u64::MAX, state Loading
tick(1000) -> expired=1, state Failed (the new query immediately fails)
subsequent begin/submit/tick remain in the u64::MAX clock domain
```

Impact: a malformed late-batch timestamp or a caller unit mismatch can poison every
later query on that coordinator instance. The caller owns the logical clock, so this is
not a remote-provider-only issue, but the coordinator already rejects the batch and
should not let it mutate `now`. Minimal fix: move the clock update after the
current/cancelled checks (keep a local `now` for expiry) and add a test that a stale or
cancelled submit with `u64::MAX` leaves `now()` unchanged and the next query can still
complete.

### R3 detail

The global path is genuinely fixed: `dispatch` computes all targets, checks
`pending_total() + targets.len() > global_queue_limit`, and returns
`GlobalQueueFull` before mutating. The probe with limit 1 and 3 providers showed
`pending=0` and no dispatch; limit 3 queued all three, and a partial submit left two
without duplicate dispatch. However, `ProviderQueueFull` cannot occur: the only way a
provider has pending work is a successful `dispatch`, which also sets `dispatched=true`;
`dispatch` skips `dispatched` providers, and `submit` does not clear `dispatched`.
`begin`/`reset` clears both. The per-provider limit is therefore not tested by any
reachable sequence. This is bookkeeping dead code, not an eviction bug.

### R4 detail

`rank_bounded` is implemented as `rank(...)` + `truncate(limit)` at `:517-525`; it caps
the returned vector but still builds and sorts every match. The function comment at
`:511-516` says callers can "avoid allocating and sorting more output than they will
display", which is false. `LocalRankProvider::batch_with_budget` also calls
`rank_bounded(..., budget+1)`, so the adapter still does full-catalog work; it avoids
returning more than the page, not the scan/sort. The module doc (`:21-23`) and policy
(`ranking-policy.md:61-62`) are honest about the catalog scan, so this is a local doc
correction, not a design regression.

## Acceptance disposition (after fixes)

**No remaining BLOCKER or HIGH.**

**ANS-17 / L03 — conditionally acceptable.** The requested H1 default, H2 state
machine, H3 in-process binding, M1 unavailable stickiness, M2 cancellation, M3 global
queue rejection and M4 category/Unicode fixes all pass, including the 1,000-command
adapter test and the alpha 1,000-cycle harness. I would accept the shipped/default and
in-tree alpha path now. I would not call the public coordinator API complete until **R1**
(custom lower-budget `batch()` trap) and **R2** (rejected-batch clock mutation) are fixed
or explicitly documented as caller preconditions; both are small and local. R3–R9 are
LOW/housekeeping.

**ANS-18 / L04 — acceptable.** H4 is fixed: ordered prefixes are enforced, a synthetic
#2/#3 swap fails, the corpus is disjoint and category/tier-consistent, the integrity
checks pass, and the measured metrics match the baseline exactly. The pinned floors are
not loose: a one-query metric regression falls below the measured fraction, and the
ordered-prefix check catches reorder-only regressions. R4 (rank_bounded/SearchSignals
docs) and R9 (short expected lists/known-failure skip) remain as LOW documentation and
corpus-scale risks, not acceptance blockers.

## Residual risks (updated)

- **R1 custom-budget adapter.** The default path works; a non-default
  `ProviderSpec::result_budget < 200` must use `batch_with_budget`, or it recreates the
  old over-budget/stuck-Loading behavior.
- **R2 trusted logical clock.** Rejected stale/cancelled submits still advance `now`; a
  far-future timestamp or unit mismatch can leave later queries perpetually Loading or
  immediately expired. This is a one-line clock-ordering fix; until then, callers must
  keep the `now` domain consistent and treat late-batch timestamps as trusted.
- **R8 future remote identity.** `CoordinatorId` is process-local. A future serialized
  cross-process provider protocol needs a globally unique session nonce or equivalent;
  the current typed API has no cross-process batch path.
- **Async runtime scope.** The coordinator remains a synchronous logical-tick state
  machine; there are still no channels, locks or provider runtime. The alpha drives it
  synchronously. This is unchanged from the original review and is a scope/design
  choice, not a new defect.
- **Corpus scale.** 31 commands and 62 queries; many expected lists are 1–2 ids, so
  precision after the specified prefix is not measured. `known_failure` queries are
  excluded from order/tier assertions by design. The four documented failures still
  count in metrics.
- **L9 bounded-work claims.** Integrated callers can cap returned results, but
  `rank_bounded` still performs a full catalog scan/sort and `SearchSignals` key sets
  remain unbounded. Docs are partially corrected; R4 covers the remaining overclaims.
- **Recency semantics.** Unchanged: recency ages against the newest recorded logical
  tick, not a wall clock; idle sessions can retain maximum recency until another
  timestamped use advances the clock.
- **No current blocker in CI.** At re-review time `cargo fmt --all -- --check` and full
  workspace `cargo test --locked --offline` both pass, unlike the original review; the
  earlier unrelated `apps/cli`/`crates/alpha`/`ansa-registry` failures are resolved.

---

## Final delta re-review — R1/R2 closed (2026-11-19)

**R1 — FIXED.** `LocalRankProvider` now stores `result_budget` (`new` = 200,
`new_with_budget`, `for_spec`, `with_result_budget`); `batch()` truncates to that stored
value. Probe with budgets 10/5/0: batch lengths 10/5/0, `is_truncated()` true, outcomes
`CompletedTruncated { 10 }`, `{ 5 }`, `{ 0 }`, results 10/5/0, states `Ready`/`Empty`,
all terminal; no `BatchOverBudget` and no stuck `Loading`. `for_spec` and
`with_result_budget(5)` also returned 10/5. The alpha constructor path still works:
`build_coordinator` is unchanged, its harness uses `batch_with_budget(catalog.len())`,
and `cargo test -p ansa-alpha --offline` passes 10 integration + 6 catalog-composition
tests.

**R2 — FIXED.** Stale/cancelled/terminal submits are clock-neutral: stale submit with
`now = u64::MAX - 1` returned `StaleInvocation` and left `now = 0`; the current query
then settled normally (`tick(9) = 0`, `tick(10) = 1`, `Failed` at `now = 10`); cancelled
submit returned `Cancelled` with `now = 0`; terminal `Ready` far-future submit left
`now` and state unchanged. Accepted in-flight work at `now = 5` advanced `now` to 5.
A far-future in-flight submit returned `Expired`, clamped `now` to the provider deadline
10 (not `u64::MAX`), and the current query became terminal `Failed`. Test
`stale_submission_does_not_poison_the_clock` covers the stale/cancelled cases.

**LOWs.** Late failure now returns `BatchOutcome::Settled { state }` and leaves `Ready`
unchanged; zero-budget batch reports `CompletedTruncated { accepted: 0 }`; policy and
disposition now explicitly say `rank_bounded` still scans/sorts and `SearchSignals` maps
are unbounded, and document the process-local `CoordinatorId` and short expected lists.
Remaining LOW docs: the Rust docstrings for `rank_bounded` ("avoid allocating and sorting
more output") and `SearchSignals` ("bounded ranking signals") are still stale, and the
tuning-floor comment still says corpus `2026-11-19.2`. No behavioral impact.

**Commands:** `cargo test -p ansa-coordinator -p ansa-search --offline` **PASS** —
31 coordinator, 26 search, 2 relevance-corpus tests; 0 failed. `cargo clippy -p
ansa-coordinator -p ansa-search --all-targets --offline -- -D warnings` **PASS** (exit
0). `cargo fmt -p ansa-coordinator -p ansa-search -- --check` **PASS** (exit 0).

**Acceptance:** **ANS-17 is acceptable.** R1/R2 are closed; remaining items are LOW
documentation drift and the previously documented operational residuals (per-provider
`ProviderQueueFull` defensive-only, process-local `CoordinatorId`, short corpus lists,
logical-tick recency). No BLOCKER/HIGH/MEDIUM remains.
