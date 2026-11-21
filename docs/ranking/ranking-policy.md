# Ranking policy and corpus provenance

Status: implemented policy for the in-memory `ansa-search` ranker. This page
describes current behavior; it does not claim to reproduce another launcher's
unknown formula.

## Tiers

Results are ordered by explicit tiers, best to worst:

1. **Exact alias** — a normalized query exactly equals a normalized alias.
2. **Exact title** — a normalized query exactly equals the normalized title.
3. **Title prefix** — the normalized title starts with the normalized query.
4. **Token match** — every query token is a prefix of some title, alias, keyword
   or subtitle token.
5. **Fuzzy match** — the query is a subsequence of a non-subtitle field.
6. **Fallback/browse** — an empty query returns the catalog in browse order.

The first applicable tier wins for a command. A weak fuzzy match can therefore
never outrank an exact alias, no matter how often it is used or favorited.

## Ordering inside a tier

Within the same tier:

1. Favorite first (true before false).
2. Capped usage descending (`MAX_USAGE_SIGNAL = 100`).
3. Capped recency descending (`MAX_RECENCY_SIGNAL = 100`).
4. Match quality: field importance, earlier start, tighter span, fewer gaps.
5. Normalized title ascending.
6. Stable `CommandId` ascending.

Because favorites, usage and recency are compared only after the tier, they can
reorder rows inside a tier but cannot cross a tier boundary. Usage saturates at
the cap, so a command used thousands of times does not gain unbounded weight.
Recency decays to zero after `RECENCY_HORIZON_TICKS = 100` logical ticks. The
final `CommandId` tie-break makes results reproducible across catalog insertion
order and process restarts.

## Normalization

`normalize` does the following, using Unicode scalar values only:

- folds case with `char::to_lowercase`;
- collapses whitespace and the separators `-`, `_`, `.`, `/`, `\`, `:`, `,`,
  `;` to a single space;
- preserves other punctuation, symbols, accents, CJK and emoji.

It never slices by byte offset, so multi-byte input is safe. Queries are bounded
to `MAX_QUERY_CHARS = 128` scalar values before normalization.

Deliberate limits:

- Accents are preserved. `CAFÉ` matches `Café`, but `cafe` does not.
- Typo tolerance is whatever the fuzzy subsequence tier provides. Deletion typos
  such as `termnal` match; substitutions and transpositions are not claimed.
- Exact-alias and exact-title matching inspect the title and alias only.
  Keywords and subtitles participate in token matching. Subtitles are excluded
  from fuzzy matching so long paths and descriptions do not create accidental
  matches.
- `rank` returns every match. `rank_bounded` caps the returned result count but
  still scans and sorts the whole catalog; it bounds output, not work. Very large
  direct callers must treat catalog size as an explicit work budget.
- `SearchSignals` maps are not internally size-bounded. Long-lived processes
  should prune signals against the live catalog.

## Ranking signals: favorites, usage, recency, private mode and reset

`SearchSignals` owns favorites, usage and recency.

### Recency formula

- `record_use_at(id, now)` increments the capped usage count, advances the
  logical tick to `max(now, current tick)` and stores the newest timestamp for
  the command.
- `record_use(id)` is the legacy timestamp-free API. It increments usage but does
  not create or change recency.
- `recency(id)` computes `age = now - last_used_at`, then returns
  `MAX_RECENCY_SIGNAL - min(age, RECENCY_HORIZON_TICKS)`.
- `MAX_RECENCY_SIGNAL = 100` and `RECENCY_HORIZON_TICKS = 100`, so the boost is
  capped at 100 and reaches zero at 100 ticks.
- Commands without a timestamp start at zero recency. Recency is never allowed to
  move a command across a tier; it is only a within-tier tie-break after
  favorites and usage.

### Privacy and reset

- `set_private_mode(true)` makes `is_favorite` return false, `usage`/`recency`
  return 0, `last_used_at` return `None`, and makes `set_favorite`,
  `record_use` and `record_use_at` no-ops. Timestamps are not recorded in private
  mode. Stored values are not deleted; turning private mode off restores them.
  The logical `now()` tick is a clock rather than a ranking signal and remains
  readable; it does not advance on timestamped writes while private.
- `reset_usage()` clears usage, timestamps and the logical tick. Favorites are
  unaffected.
- `reset_favorites()` clears only favorites. Usage, recency and private mode are
  unaffected.
- `reset_all()` clears favorites, usage and timestamps, and resets the logical
  tick. Private mode is a policy flag, not a signal, so it is left unchanged.
- `clear_usage`, `clear_favorites` and `clear` remain as backward-compatible
  aliases.

Private mode is isolation, not encryption or at-rest deletion. A higher layer
remains responsible for persistence and for wiping durable storage when the user
asks to delete data.

## Corpus provenance

- File: `crates/search/data/relevance_corpus.json`.
- Runner: `crates/search/tests/relevance_corpus.rs`.
- Baseline report: [`baseline-2026-11-19.md`](baseline-2026-11-19.md).
- Review disposition: [`review-disposition-2026-11-19.md`](review-disposition-2026-11-19.md).
- Current corpus version: `2026-11-19.3`.

The corpus contains 31 commands and 62 queries split into a 23-query tuning half
and a 39-query held-out validation half. Each half carries its own favorite/usage
profile so split isolation is testable. Every query has an ordered expected
top-10 list and a note describing the tier or tie-break being exercised.
Categories are `exact`, `prefix`, `token`, `fuzzy`, `typo`, `unicode` and
`browse`.

The held-out validation half contains deliberately harder cases: ambiguous
prefixes, separator noise, substitutions and transpositions, unaccented
diacritics and near-ties. They were authored without changing ranker code or
thresholds to fit them. Their measured result is below the tuning result in
`typo` and `unicode`; that is recorded rather than tuned away.

Authoring rules:

1. Expectations come from the documented tier policy, not from copying whatever
   the ranker currently prints.
2. The validation split is not used to change ranking behavior. Harder held-out
   cases are recorded with their real failures and marked `known_failure` so
   order/tier assertions do not hide them while metrics still count them.
3. A change to expected relevance or to the corpus requires a version bump and a
   new baseline report.
4. Deletion typos are supported by the fuzzy subsequence tier. Substitution and
   transposition typos are not promised; current failures are documented.
5. Unicode cases cover case-folded accents, CJK and emoji. Accent-insensitive
   folding is not implemented.
6. The runner reports top-1 accuracy, recall@10 and MRR per category, per split
   and overall. It enforces the full ordered expected prefix (not just top-1),
   checks category-to-tier consistency, and asserts that expected ids exist,
   expected ids are unique per query, non-empty queries are unique within a split
   and disjoint across splits, and command/query counts stay pinned.
7. CI asserts the tuning floors and the measured held-out validation floors
   separately.

## Coordinator integration

`ansa-coordinator` can adapt the local ranker with `LocalRankProvider`, which
runs `rank` over a supplied catalog and returns a deduplicated, deterministically
ordered complete batch. The adapter stores its own result budget: `new` defaults
to 200, `new_with_budget`/`with_result_budget` set it explicitly, and `for_spec`
copies the provider spec's ID and effective budget. A batch is truncated to that
budget and marked truncated, so a coordinator configured with a smaller budget
does not receive an over-budget batch.

Remote providers submit batches carrying the current coordinator and invocation
identity. Aggregate results preserve provider registration order and deduplicate
by stable `CommandId`; late, cancelled or cross-coordinator batches are rejected
before they can replace a newer invocation. See `crates/coordinator/src/lib.rs`.
