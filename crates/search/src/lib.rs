//! Deterministic, in-memory command ranking.
//!
//! The ranker is intentionally independent of any renderer or platform. It
//! implements all six tiers from the architecture blueprint:
//!
//! 1. exact alias
//! 2. exact title
//! 3. title prefix
//! 4. token match
//! 5. fuzzy subsequence match
//! 6. fallback/browse ordering for an empty query
//!
//! Favorite, capped usage and capped recency signals only reorder commands
//! *within* a tier, so a frequently used weak match can never beat an exact
//! alias. Every final tie is broken by the stable [`CommandId`].
//!
//! Exact-alias and exact-title matching inspect the title and alias only; token
//! matching additionally inspects keywords and subtitles. Subtitles are excluded
//! from fuzzy matching.
//!
//! `rank` scans the supplied in-memory catalog. `rank_bounded` caps the returned
//! output; callers with very large catalogs should use it and treat catalog size
//! as an explicit budget.
//!
//! All text handling is Unicode scalar-value based. Nothing slices strings by
//! byte offset, so multi-byte titles, aliases and queries are safe.

use ansa_domain::{Command, CommandId};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Maximum amount of usage signal applied to any command.
pub const MAX_USAGE_SIGNAL: u32 = 100;

/// Maximum recency boost. Recency decays linearly from this value to zero over
/// [`RECENCY_HORIZON_TICKS`] logical ticks.
pub const MAX_RECENCY_SIGNAL: u32 = 100;

/// Age in logical ticks at which the recency boost reaches zero.
pub const RECENCY_HORIZON_TICKS: u64 = 100;

/// Queries longer than this many Unicode scalar values are truncated before
/// normalization. Launcher queries are short; the bound keeps ranking work
/// predictable even if input arrives from an untrusted source.
pub const MAX_QUERY_CHARS: usize = 128;

/// The explicit ranking tier. Variants are ordered best to worst.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchTier {
    ExactAlias,
    ExactTitle,
    TitlePrefix,
    Token,
    Fuzzy,
    /// Empty-query browsing (favorites/frequency then title/id).
    Fallback,
}

impl MatchTier {
    pub fn label(self) -> &'static str {
        match self {
            Self::ExactAlias => "Exact alias",
            Self::ExactTitle => "Exact title",
            Self::TitlePrefix => "Title prefix",
            Self::Token => "Token match",
            Self::Fuzzy => "Fuzzy match",
            Self::Fallback => "Browse",
        }
    }
}

/// Deterministic quality key within a tier.
///
/// A lower field index is a more important field (title, alias, keyword,
/// subtitle). For fuzzy matches, an earlier start, a tighter span, and fewer
/// gaps rank better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct MatchQuality {
    pub field: u8,
    pub start: usize,
    pub span: usize,
    pub gaps: usize,
}

/// A ranked command plus the reason it ranked where it did.
#[derive(Debug, Clone)]
pub struct SearchResult<'a> {
    pub command: &'a Command,
    pub tier: MatchTier,
    pub quality: MatchQuality,
    pub favorite: bool,
    pub usage: u32,
    /// Capped recency boost for the current logical tick.
    pub recency: u32,
    /// Precomputed normalized title for deterministic secondary ordering.
    sort_title: String,
}

/// Locally stored ranking signals.
///
/// Per-command values (favorites, usage, recency) are bounded, but the maps grow
/// with the number of distinct command IDs supplied by the caller. Persistence
/// and catalog-size policy belong to a higher layer.
///
/// This type is intentionally not persisted here; storage and privacy policy
/// belong to a higher layer. Private mode makes ranking reads inert and blocks
/// new favorite/usage/recency writes: ranking behaves as if there were no
/// signals, and the underlying values are neither changed nor deleted. Turning
/// private mode off restores the previously stored values. Explicit reset calls
/// still clear stored signals while private mode is active; they are maintenance
/// operations, not new signal writes. The logical [`SearchSignals::now`] tick is
/// a clock, not a ranking signal, and remains readable in private mode.
#[derive(Debug, Clone, Default)]
pub struct SearchSignals {
    favorites: HashSet<CommandId>,
    usage: HashMap<CommandId, u32>,
    last_used: HashMap<CommandId, u64>,
    now: u64,
    private: bool,
}

impl SearchSignals {
    pub fn new() -> Self {
        Self::default()
    }

    /// True when ranking signals are ignored and writes are discarded.
    pub fn is_private_mode(&self) -> bool {
        self.private
    }

    /// Enables or disables private mode without mutating stored signals.
    pub fn set_private_mode(&mut self, private: bool) {
        self.private = private;
    }

    pub fn is_favorite(&self, id: &CommandId) -> bool {
        !self.private && self.favorites.contains(id)
    }

    pub fn set_favorite(&mut self, id: &CommandId, favorite: bool) {
        if self.private {
            return;
        }
        if favorite {
            self.favorites.insert(id.clone());
        } else {
            self.favorites.remove(id);
        }
    }

    /// Capped usage count for ranking.
    pub fn usage(&self, id: &CommandId) -> u32 {
        if self.private {
            return 0;
        }
        self.usage
            .get(id)
            .copied()
            .unwrap_or(0)
            .min(MAX_USAGE_SIGNAL)
    }

    /// The logical tick used to age recency values. This is the newest
    /// timestamp ever recorded, not a wall clock.
    pub fn now(&self) -> u64 {
        self.now
    }

    /// Last recorded use timestamp, or `None` when the command has no
    /// timestamped use or private mode is active.
    pub fn last_used_at(&self, id: &CommandId) -> Option<u64> {
        if self.private {
            return None;
        }
        self.last_used.get(id).copied()
    }

    /// Capped recency boost for the current logical tick.
    ///
    /// Formula: `age = now - last_used_at`; boost is
    /// `MAX_RECENCY_SIGNAL - min(age, RECENCY_HORIZON_TICKS)`. Commands without
    /// a timestamp, and all commands in private mode, receive zero.
    pub fn recency(&self, id: &CommandId) -> u32 {
        if self.private {
            return 0;
        }
        let Some(last_used) = self.last_used.get(id).copied() else {
            return 0;
        };
        let age = self.now.saturating_sub(last_used);
        let capped_age = age.min(RECENCY_HORIZON_TICKS);
        (MAX_RECENCY_SIGNAL as u64)
            .saturating_sub(capped_age)
            .min(MAX_RECENCY_SIGNAL as u64) as u32
    }

    /// Records one use without a timestamp, saturating at
    /// [`MAX_USAGE_SIGNAL`]. Existing recency is left untouched.
    ///
    /// Does nothing in private mode.
    pub fn record_use(&mut self, id: &CommandId) {
        if self.private {
            return;
        }
        self.bump_usage(id);
    }

    /// Records one use at logical tick `now`, saturating usage and keeping the
    /// newest timestamp for the command.
    ///
    /// Does nothing in private mode, including refusing to advance [`now`].
    pub fn record_use_at(&mut self, id: &CommandId, now: u64) {
        if self.private {
            return;
        }
        self.bump_usage(id);
        self.now = self.now.max(now);
        let timestamp = self.last_used.entry(id.clone()).or_insert(now);
        *timestamp = (*timestamp).max(now);
    }

    fn bump_usage(&mut self, id: &CommandId) {
        let entry = self.usage.entry(id.clone()).or_insert(0);
        *entry = entry.saturating_add(1).min(MAX_USAGE_SIGNAL);
    }

    /// Resets usage history and recency timestamps. Favorites and private mode
    /// are unaffected.
    pub fn reset_usage(&mut self) {
        self.usage.clear();
        self.last_used.clear();
        self.now = 0;
    }

    /// Resets favorites. Usage, recency and private mode are unaffected.
    pub fn reset_favorites(&mut self) {
        self.favorites.clear();
    }

    /// Resets every ranking signal. Private mode is a policy flag, not a
    /// signal, so it is left unchanged.
    pub fn reset_all(&mut self) {
        self.favorites.clear();
        self.reset_usage();
    }

    /// Backwards-compatible alias for [`SearchSignals::reset_usage`].
    pub fn clear_usage(&mut self) {
        self.reset_usage();
    }

    /// Backwards-compatible alias for [`SearchSignals::reset_favorites`].
    pub fn clear_favorites(&mut self) {
        self.reset_favorites();
    }

    /// Backwards-compatible alias for [`SearchSignals::reset_all`].
    pub fn clear(&mut self) {
        self.reset_all();
    }
}

/// Normalizes text for comparison.
///
/// Case folding uses `char::to_lowercase`, and whitespace plus common command
/// separators (`-`, `_`, `.`, `/`, `\`, `:`, `,`, `;`) collapse to one space.
/// Other punctuation and symbols are preserved.
pub fn normalize(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut pending_space = false;

    for character in input.chars() {
        for folded in character.to_lowercase() {
            if folded.is_whitespace() || is_separator(folded) {
                if !output.is_empty() {
                    pending_space = true;
                }
            } else {
                if pending_space {
                    output.push(' ');
                    pending_space = false;
                }
                output.push(folded);
            }
        }
    }

    output
}

/// Normalized, whitespace-separated tokens.
pub fn tokens(input: &str) -> Vec<String> {
    normalize(input)
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn is_separator(character: char) -> bool {
    matches!(character, '-' | '_' | '.' | '/' | '\\' | ':' | ',' | ';')
}

fn bounded_normalize(input: &str) -> String {
    let bounded: String = input.chars().take(MAX_QUERY_CHARS).collect();
    normalize(&bounded)
}

/// Builds a field list ordered by display importance.
///
/// Title is always present; other fields are included only when non-empty.
fn fields(command: &Command) -> Vec<(u8, String)> {
    let mut fields = Vec::with_capacity(3 + command.keywords.len());
    fields.push((0, normalize(&command.title)));
    if let Some(alias) = &command.alias {
        let normalized = normalize(alias);
        if !normalized.is_empty() {
            fields.push((1, normalized));
        }
    }
    for keyword in &command.keywords {
        let normalized = normalize(keyword);
        if !normalized.is_empty() {
            fields.push((2, normalized));
        }
    }
    if let Some(subtitle) = &command.subtitle {
        let normalized = normalize(subtitle);
        if !normalized.is_empty() {
            fields.push((3, normalized));
        }
    }
    fields
}

fn exact_alias(fields: &[(u8, String)], normalized_query: &str) -> bool {
    fields
        .iter()
        .any(|(kind, value)| *kind == 1 && value == normalized_query)
}

fn token_match(fields: &[(u8, String)], query_tokens: &[&str]) -> Option<MatchQuality> {
    let mut minimum_field = u8::MAX;
    let mut token_index_sum = 0usize;

    for query_token in query_tokens {
        let mut best: Option<(u8, usize)> = None;
        for (field_kind, value) in fields {
            if let Some(index) = value
                .split_whitespace()
                .position(|candidate| candidate.starts_with(query_token))
            {
                best = Some((*field_kind, index));
                break;
            }
        }
        let (field, index) = best?;
        minimum_field = minimum_field.min(field);
        token_index_sum = token_index_sum.saturating_add(index);
    }

    Some(MatchQuality {
        field: minimum_field,
        start: token_index_sum,
        span: 0,
        gaps: 0,
    })
}

fn fuzzy_quality(query: &[char], value: &str) -> Option<MatchQuality> {
    if query.is_empty() {
        return None;
    }
    let haystack: Vec<char> = value
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();
    if haystack.len() < query.len() {
        return None;
    }

    let mut best: Option<(usize, usize, usize)> = None;
    for start in 0..haystack.len() {
        if haystack[start] != query[0] {
            continue;
        }

        let mut query_index = 1;
        let mut cursor = start + 1;
        while query_index < query.len() && cursor < haystack.len() {
            if haystack[cursor] == query[query_index] {
                query_index += 1;
            }
            cursor += 1;
        }
        if query_index != query.len() {
            continue;
        }

        let end = cursor - 1;
        let span = end - start + 1;
        let gaps = span.saturating_sub(query.len());
        let candidate = (start, span, gaps);
        if best.is_none_or(|current| candidate < current) {
            best = Some(candidate);
        }
    }

    best.map(|(start, span, gaps)| MatchQuality {
        field: 0,
        start,
        span,
        gaps,
    })
}

fn fuzzy_match(fields: &[(u8, String)], query: &[char]) -> Option<MatchQuality> {
    let mut best: Option<MatchQuality> = None;
    for (field_kind, value) in fields {
        // Descriptions and filesystem paths can contain a query's letters
        // spread over dozens of unrelated words. Keep subtitle token matches,
        // but do not promote these accidental subsequences into app results.
        if *field_kind == 3 {
            continue;
        }
        if let Some(mut quality) = fuzzy_quality(query, value) {
            quality.field = *field_kind;
            if best.is_none_or(|current| quality < current) {
                best = Some(quality);
            }
        }
    }
    best
}

fn evaluate(
    command: &Command,
    normalized_query: &str,
    query_tokens: &[&str],
    query_chars: &[char],
) -> Option<(MatchTier, MatchQuality)> {
    if normalized_query.is_empty() {
        return Some((MatchTier::Fallback, MatchQuality::default()));
    }

    let fields = fields(command);

    if exact_alias(&fields, normalized_query) {
        return Some((MatchTier::ExactAlias, MatchQuality::default()));
    }
    if fields[0].1 == normalized_query {
        return Some((MatchTier::ExactTitle, MatchQuality::default()));
    }
    if fields[0].1.starts_with(normalized_query) {
        return Some((MatchTier::TitlePrefix, MatchQuality::default()));
    }
    if let Some(quality) = token_match(&fields, query_tokens) {
        return Some((MatchTier::Token, quality));
    }
    if let Some(quality) = fuzzy_match(&fields, query_chars) {
        return Some((MatchTier::Fuzzy, quality));
    }
    None
}

fn compare_results(left: &SearchResult<'_>, right: &SearchResult<'_>) -> Ordering {
    left.tier
        .cmp(&right.tier)
        // true (favorite) sorts before false
        .then_with(|| right.favorite.cmp(&left.favorite))
        .then_with(|| right.usage.cmp(&left.usage))
        .then_with(|| right.recency.cmp(&left.recency))
        .then_with(|| left.quality.cmp(&right.quality))
        .then_with(|| left.sort_title.cmp(&right.sort_title))
        .then_with(|| left.command.id.cmp(&right.command.id))
}

/// Ranks `commands` for `query`.
///
/// Returns matching commands best-first. An empty query returns every command
/// in browse order (favorite, capped usage, normalized title, stable id).
pub fn rank<'a>(
    commands: &'a [Command],
    query: &str,
    signals: &SearchSignals,
) -> Vec<SearchResult<'a>> {
    let normalized_query = bounded_normalize(query);
    let query_tokens: Vec<&str> = normalized_query.split_whitespace().collect();
    let query_chars: Vec<char> = normalized_query
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect();

    let mut results = Vec::new();
    for command in commands {
        if let Some((tier, quality)) =
            evaluate(command, &normalized_query, &query_tokens, &query_chars)
        {
            results.push(SearchResult {
                command,
                tier,
                quality,
                favorite: signals.is_favorite(&command.id),
                usage: signals.usage(&command.id),
                recency: signals.recency(&command.id),
                sort_title: normalize(&command.title),
            });
        }
    }

    results.sort_by(compare_results);
    results
}

/// Ranks `commands` for `query` and returns at most `limit` results.
///
/// The catalog is still scanned (exact/prefix/token/fuzzy matching is
/// catalog-sized work), but callers that only need a bounded page can avoid
/// allocating and sorting more output than they will display. `limit == 0`
/// returns no results.
pub fn rank_bounded<'a>(
    commands: &'a [Command],
    query: &str,
    signals: &SearchSignals,
    limit: usize,
) -> Vec<SearchResult<'a>> {
    let mut results = rank(commands, query, signals);
    results.truncate(limit);
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use ansa_domain::{BuiltinCommand, CommandKind};

    fn command(id: &str, title: &str) -> Command {
        Command::new(
            CommandId::parse(id).unwrap(),
            title,
            CommandKind::Builtin(BuiltinCommand::About),
        )
    }

    fn ids<'a>(results: &[SearchResult<'a>]) -> Vec<&'a str> {
        results
            .iter()
            .map(|result| result.command.id.as_str())
            .collect()
    }

    #[test]
    fn normalization_is_unicode_safe_and_stable() {
        assert_eq!(normalize("  CAFÉ  "), "café");
        assert_eq!(normalize("My-App_Name"), "my app name");
        assert_eq!(normalize("a   b"), "a b");
        assert_eq!(normalize("🚀 Launch"), "🚀 launch");
        assert_eq!(normalize("東京"), "東京");
        assert_eq!(tokens("My-App_Name"), vec!["my", "app", "name"]);
    }

    #[test]
    fn exact_alias_outranks_exact_title_and_signals_cannot_cross_tiers() {
        let mut alias = command("app:a", "Preferences");
        alias.alias = Some("settings".into());
        let mut title = command("app:b", "Settings");
        title.alias = None;

        let commands = vec![alias, title];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[1].id, true);
        for _ in 0..1000 {
            signals.record_use(&commands[1].id);
        }

        let results = rank(&commands, "settings", &signals);
        assert_eq!(ids(&results), vec!["app:a", "app:b"]);
        assert_eq!(results[0].tier, MatchTier::ExactAlias);
        assert_eq!(results[1].tier, MatchTier::ExactTitle);
    }

    #[test]
    fn tiers_are_ordered_exact_prefix_token_fuzzy() {
        let commands = vec![
            command("app:fuzzy", "The Remote Machine"),
            command("app:token", "Windows Terminal"),
            command("app:prefix", "Terminal"),
            command("app:exact", "Term"),
        ];

        let results = rank(&commands, "term", &SearchSignals::new());
        assert_eq!(
            ids(&results),
            vec!["app:exact", "app:prefix", "app:token", "app:fuzzy"]
        );
        assert_eq!(results[0].tier, MatchTier::ExactTitle);
        assert_eq!(results[1].tier, MatchTier::TitlePrefix);
        assert_eq!(results[2].tier, MatchTier::Token);
        assert_eq!(results[3].tier, MatchTier::Fuzzy);
    }

    #[test]
    fn token_match_requires_every_query_token() {
        let commands = vec![
            command("app:two", "My Windows Terminal"),
            command("app:one", "Windows Settings"),
        ];
        let results = rank(&commands, "windows term", &SearchSignals::new());
        assert_eq!(ids(&results), vec!["app:two"]);
        assert_eq!(results[0].tier, MatchTier::Token);
    }

    #[test]
    fn fuzzy_matches_subsequence_across_separators() {
        let commands = vec![command("app:term", "Windows-Terminal")];
        let results = rank(&commands, "wtrm", &SearchSignals::new());
        assert_eq!(ids(&results), vec!["app:term"]);
        assert_eq!(results[0].tier, MatchTier::Fuzzy);
    }

    #[test]
    fn path_subtitles_do_not_create_accidental_fuzzy_matches() {
        let commands = vec![
            command("app:other", "Other App")
                .with_subtitle("C:/t-e-r-m-i-n-a-l/Programs/Other.lnk"),
        ];
        assert!(rank(&commands, "terminal", &SearchSignals::new()).is_empty());
        assert_eq!(rank(&commands, "programs", &SearchSignals::new()).len(), 1);
    }

    #[test]
    fn unicode_exact_and_prefix_work() {
        let commands = vec![
            command("app:cafe", "Café"),
            command("app:rocket", "🚀 Launch"),
            command("app:tokyo", "東京 駅"),
        ];

        assert_eq!(
            ids(&rank(&commands, "CAFÉ", &SearchSignals::new())),
            vec!["app:cafe"]
        );
        assert_eq!(
            ids(&rank(&commands, "🚀", &SearchSignals::new())),
            vec!["app:rocket"]
        );
        assert_eq!(
            ids(&rank(&commands, "東京", &SearchSignals::new())),
            vec!["app:tokyo"]
        );
    }

    #[test]
    fn separator_normalization_makes_exact_title_match() {
        let commands = vec![command("app:demo", "My-App")];
        let results = rank(&commands, "my app", &SearchSignals::new());
        assert_eq!(ids(&results), vec!["app:demo"]);
        assert_eq!(results[0].tier, MatchTier::ExactTitle);
    }

    #[test]
    fn stable_id_breaks_complete_ties() {
        let commands = vec![
            command("app:z", "Same"),
            command("app:a", "Same"),
            command("app:m", "Same"),
        ];
        assert_eq!(
            ids(&rank(&commands, "same", &SearchSignals::new())),
            vec!["app:a", "app:m", "app:z"]
        );
    }

    #[test]
    fn favorites_and_usage_reorder_only_within_a_tier() {
        let commands = vec![
            command("app:a", "Alpha"),
            command("app:b", "Beta"),
            command("app:c", "Gamma"),
        ];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[2].id, true);
        signals.record_use(&commands[1].id);

        let results = rank(&commands, "", &signals);
        assert_eq!(ids(&results), vec!["app:c", "app:b", "app:a"]);
        assert!(
            results
                .iter()
                .all(|result| result.tier == MatchTier::Fallback)
        );
    }

    #[test]
    fn usage_signal_is_capped() {
        let command = command("app:a", "Alpha");
        let mut signals = SearchSignals::new();
        for _ in 0..(MAX_USAGE_SIGNAL * 5) {
            signals.record_use(&command.id);
        }
        assert_eq!(signals.usage(&command.id), MAX_USAGE_SIGNAL);
        signals.clear();
        assert_eq!(signals.usage(&command.id), 0);
    }

    #[test]
    fn favorite_breaks_ties_within_same_tier() {
        let commands = vec![command("app:a", "Same"), command("app:b", "Same")];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[1].id, true);

        let results = rank(&commands, "same", &signals);
        assert_eq!(ids(&results), vec!["app:b", "app:a"]);
        assert!(results[0].favorite);
    }

    #[test]
    fn keywords_and_subtitle_participate_in_search() {
        let mut command = command("app:note", "Ideas");
        command.keywords = vec!["write".into(), "markdown".into()];
        command.subtitle = Some("Open the scratchpad".into());

        assert_eq!(
            ids(&rank(&[command.clone()], "markdown", &SearchSignals::new())),
            vec!["app:note"]
        );
        assert_eq!(
            ids(&rank(&[command], "scratchpad", &SearchSignals::new())),
            vec!["app:note"]
        );
    }

    #[test]
    fn no_match_returns_no_results_and_long_input_is_bounded() {
        let commands = vec![command("app:a", "Alpha")];
        assert!(rank(&commands, "zzzz", &SearchSignals::new()).is_empty());
        let long_query = "a".repeat(MAX_QUERY_CHARS * 4);
        let results = rank(&commands, &long_query, &SearchSignals::new());
        assert!(results.is_empty());
    }

    #[test]
    fn private_mode_ignores_and_does_not_record_signals() {
        let commands = vec![
            command("app:a", "Alpha"),
            command("app:b", "Beta"),
            command("app:c", "Gamma"),
        ];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[0].id, true);
        signals.record_use(&commands[1].id);
        assert!(signals.is_favorite(&commands[0].id));
        assert_eq!(signals.usage(&commands[1].id), 1);

        signals.set_private_mode(true);
        assert!(signals.is_private_mode());
        assert!(!signals.is_favorite(&commands[0].id));
        assert_eq!(signals.usage(&commands[1].id), 0);

        // Writes are discarded rather than deferred.
        signals.set_favorite(&commands[2].id, true);
        signals.record_use(&commands[2].id);
        assert!(!signals.is_favorite(&commands[2].id));
        assert_eq!(signals.usage(&commands[2].id), 0);

        let results = rank(&commands, "", &signals);
        assert_eq!(ids(&results), vec!["app:a", "app:b", "app:c"]);

        signals.set_private_mode(false);
        assert!(signals.is_favorite(&commands[0].id));
        assert_eq!(signals.usage(&commands[1].id), 1);
        assert!(!signals.is_favorite(&commands[2].id));
        assert_eq!(signals.usage(&commands[2].id), 0);
    }

    #[test]
    fn reset_operations_are_scoped_and_leave_private_mode_alone() {
        let commands = [command("app:a", "Alpha"), command("app:b", "Beta")];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[0].id, true);
        signals.set_favorite(&commands[1].id, true);
        signals.record_use(&commands[0].id);
        signals.record_use(&commands[1].id);
        signals.set_private_mode(true);

        signals.reset_usage();
        assert!(signals.is_private_mode());
        assert!(!signals.is_favorite(&commands[0].id));
        signals.set_private_mode(false);
        assert!(signals.is_favorite(&commands[0].id));
        assert_eq!(signals.usage(&commands[0].id), 0);

        signals.record_use(&commands[0].id);
        signals.reset_favorites();
        assert!(!signals.is_favorite(&commands[0].id));
        assert!(!signals.is_favorite(&commands[1].id));
        assert_eq!(signals.usage(&commands[0].id), 1);

        signals.set_favorite(&commands[1].id, true);
        signals.reset_all();
        assert!(!signals.is_favorite(&commands[1].id));
        assert_eq!(signals.usage(&commands[0].id), 0);
        assert!(!signals.is_private_mode());
    }

    #[test]
    fn property_tier_order_is_always_respected() {
        // Each command matches exactly one documented tier. Signals are
        // deliberately adversarial: the weakest match is favorite and heavily
        // used, and every stronger match has no signal at all.
        for usage in [0, 1, 2, 7, MAX_USAGE_SIGNAL, MAX_USAGE_SIGNAL + 50] {
            let mut weakest = command("app:z-fuzzy", "The Remote Machine");
            weakest.alias = None;
            let commands = vec![
                weakest,
                command("app:token", "Windows Terminal"),
                command("app:prefix", "Terminal"),
                command("app:exact", "Term"),
            ];
            let mut signals = SearchSignals::new();
            signals.set_favorite(&commands[0].id, true);
            for _ in 0..usage {
                signals.record_use(&commands[0].id);
            }

            let results = rank(&commands, "term", &signals);
            let tiers: Vec<MatchTier> = results.iter().map(|result| result.tier).collect();
            let mut sorted = tiers.clone();
            sorted.sort();
            assert_eq!(tiers, sorted, "usage={usage}");
            assert_eq!(
                ids(&results),
                vec!["app:exact", "app:prefix", "app:token", "app:z-fuzzy"],
                "usage={usage}"
            );
        }
    }

    #[test]
    fn property_stable_tie_breaks_are_permutation_independent() {
        let ids_in_input = [
            "app:zz", "app:aa", "app:kk", "app:mm", "app:bb", "app:cc", "app:dd", "app:ee",
        ];
        let mut reference = Vec::new();
        for id in ids_in_input {
            reference.push(command(id, "Same"));
        }
        let expected = ids(&rank(&reference, "same", &SearchSignals::new())).join(",");
        assert_eq!(
            expected,
            "app:aa,app:bb,app:cc,app:dd,app:ee,app:kk,app:mm,app:zz"
        );

        // Reverse, rotate and interleave the catalog. The ranked identity
        // sequence must not depend on input order.
        let mut permutations: Vec<Vec<Command>> = Vec::new();
        let mut reversed = reference.clone();
        reversed.reverse();
        permutations.push(reversed);
        let mut rotated = reference.clone();
        rotated.rotate_left(3);
        permutations.push(rotated);
        let mut interleaved = Vec::new();
        for (index, command) in reference.iter().enumerate() {
            if index % 2 == 0 {
                interleaved.push(command.clone());
            }
        }
        for (index, command) in reference.iter().enumerate() {
            if index % 2 == 1 {
                interleaved.push(command.clone());
            }
        }
        permutations.push(interleaved);

        for commands in permutations {
            let actual = ids(&rank(&commands, "same", &SearchSignals::new())).join(",");
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn property_usage_boost_is_capped_and_favorites_cannot_cross_tiers() {
        let weak = command("app:weak", "Remote Alpha");
        let strong = command("app:strong", "Alpha");
        let commands = vec![weak, strong];
        for usage in [0, 1, 10, MAX_USAGE_SIGNAL, MAX_USAGE_SIGNAL * 3] {
            let mut signals = SearchSignals::new();
            signals.set_favorite(&commands[0].id, true);
            for _ in 0..usage {
                signals.record_use(&commands[0].id);
            }
            assert!(signals.usage(&commands[0].id) <= MAX_USAGE_SIGNAL);
            let results = rank(&commands, "alpha", &signals);
            assert_eq!(ids(&results), vec!["app:strong", "app:weak"]);
            assert_eq!(results[0].tier, MatchTier::ExactTitle);
            assert_eq!(results[1].tier, MatchTier::Token);
        }
    }

    #[test]
    fn property_unicode_and_deletion_typos_keep_documented_behavior() {
        let commands = vec![
            command("app:cafe", "Café"),
            command("app:tokyo", "東京 駅"),
            command("app:rocket", "🚀 Launch"),
            command("app:terminal", "Terminal"),
            command("app:settings", "Settings"),
        ];

        assert_eq!(
            ids(&rank(&commands, "CAFÉ", &SearchSignals::new())),
            vec!["app:cafe"]
        );
        assert_eq!(
            ids(&rank(&commands, "東京", &SearchSignals::new())),
            vec!["app:tokyo"]
        );
        assert_eq!(
            ids(&rank(&commands, "🚀", &SearchSignals::new())),
            vec!["app:rocket"]
        );

        // Deletion typos are valid subsequences and therefore enter the fuzzy
        // tier. They never promote above a real prefix/title match.
        let typo = rank(&commands, "termnal", &SearchSignals::new());
        assert_eq!(ids(&typo), vec!["app:terminal"]);
        assert_eq!(typo[0].tier, MatchTier::Fuzzy);
        let typo = rank(&commands, "setings", &SearchSignals::new());
        assert_eq!(ids(&typo), vec!["app:settings"]);
        assert_eq!(typo[0].tier, MatchTier::Fuzzy);

        // Accent-insensitive and transposition-insensitive matching are not
        // claimed; documenting the limit here prevents an accidental policy
        // change from silently entering the corpus.
        assert!(rank(&commands, "cafe", &SearchSignals::new()).is_empty());
        assert!(rank(&commands, "teh", &SearchSignals::new()).is_empty());
    }

    #[test]
    fn ranking_is_reproducible_across_reconstructed_state() {
        let commands = vec![
            command("app:a", "Alpha").with_alias("first"),
            command("app:b", "Beta"),
            command("app:c", "Café"),
        ];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[1].id, true);
        signals.record_use(&commands[2].id);
        let first = ids(&rank(&commands, "", &signals)).join(",");

        let cloned_commands = commands.clone();
        let mut cloned_signals = SearchSignals::new();
        cloned_signals.set_favorite(&cloned_commands[1].id, true);
        cloned_signals.record_use(&cloned_commands[2].id);
        let second = ids(&rank(&cloned_commands, "", &cloned_signals)).join(",");
        assert_eq!(first, second);
    }

    #[test]
    fn recency_reorders_within_a_tier_and_is_capped() {
        let commands = vec![command("app:a", "Alpha"), command("app:b", "Beta")];
        let mut signals = SearchSignals::new();
        signals.record_use_at(&commands[0].id, 1);
        signals.record_use_at(&commands[1].id, 100);

        assert_eq!(signals.now(), 100);
        assert_eq!(signals.recency(&commands[1].id), MAX_RECENCY_SIGNAL);
        assert_eq!(signals.recency(&commands[0].id), 1);
        assert_eq!(signals.usage(&commands[0].id), 1);

        let results = rank(&commands, "", &signals);
        assert_eq!(ids(&results), vec!["app:b", "app:a"]);
        assert_eq!(results[0].recency, MAX_RECENCY_SIGNAL);
        assert_eq!(results[1].recency, 1);

        // The boost is bounded even after many timestamps.
        signals.record_use_at(&commands[1].id, 1_000);
        assert_eq!(signals.recency(&commands[1].id), MAX_RECENCY_SIGNAL);

        // After the horizon, recency is zero rather than negative.
        signals.record_use_at(&commands[0].id, 1_100);
        assert_eq!(signals.recency(&commands[1].id), 0);
        assert_eq!(signals.recency(&commands[0].id), MAX_RECENCY_SIGNAL);
    }

    #[test]
    fn recency_never_crosses_tiers_even_when_favorite_and_frequent() {
        let exact = command("app:exact", "Term");
        let weak = command("app:weak", "The Remote Machine");
        let commands = vec![weak, exact];
        let mut signals = SearchSignals::new();
        signals.set_favorite(&commands[0].id, true);
        for _ in 0..(MAX_USAGE_SIGNAL * 5) {
            signals.record_use_at(&commands[0].id, 1);
        }
        // Advance the logical clock past the recency horizon.
        signals.record_use_at(&commands[1].id, 1_000);
        assert_eq!(signals.recency(&commands[0].id), 0);
        assert_eq!(signals.recency(&commands[1].id), MAX_RECENCY_SIGNAL);

        let results = rank(&commands, "term", &signals);
        assert_eq!(ids(&results), vec!["app:exact", "app:weak"]);
        assert_eq!(results[0].tier, MatchTier::ExactTitle);
        assert_eq!(results[1].tier, MatchTier::Fuzzy);
    }

    #[test]
    fn private_mode_ignores_and_does_not_record_recency() {
        let a = command("app:a", "Alpha");
        let b = command("app:b", "Beta");
        let mut signals = SearchSignals::new();
        signals.record_use_at(&a.id, 40);
        assert_eq!(signals.last_used_at(&a.id), Some(40));
        assert_eq!(signals.recency(&a.id), MAX_RECENCY_SIGNAL);

        signals.set_private_mode(true);
        assert_eq!(signals.last_used_at(&a.id), None);
        assert_eq!(signals.recency(&a.id), 0);
        signals.record_use_at(&a.id, 60);
        signals.record_use_at(&b.id, 60);
        assert_eq!(signals.last_used_at(&a.id), None);
        assert_eq!(signals.last_used_at(&b.id), None);
        assert_eq!(signals.usage(&a.id), 0);
        assert_eq!(signals.usage(&b.id), 0);

        signals.set_private_mode(false);
        assert_eq!(signals.last_used_at(&a.id), Some(40));
        assert_eq!(signals.recency(&a.id), MAX_RECENCY_SIGNAL);
        assert_eq!(signals.usage(&a.id), 1);
        assert_eq!(signals.last_used_at(&b.id), None);
        assert_eq!(signals.usage(&b.id), 0);
    }

    #[test]
    fn reset_operations_clear_recency_without_touching_unrelated_signals() {
        let a = command("app:a", "Alpha");
        let b = command("app:b", "Beta");
        let mut signals = SearchSignals::new();
        signals.set_favorite(&a.id, true);
        signals.record_use_at(&a.id, 10);
        signals.record_use_at(&b.id, 20);
        assert_eq!(signals.now(), 20);

        signals.reset_usage();
        assert!(signals.is_favorite(&a.id));
        assert_eq!(signals.last_used_at(&a.id), None);
        assert_eq!(signals.recency(&a.id), 0);
        assert_eq!(signals.usage(&a.id), 0);
        assert_eq!(signals.now(), 0);

        signals.record_use_at(&a.id, 30);
        signals.record_use_at(&b.id, 40);
        signals.reset_favorites();
        assert!(!signals.is_favorite(&a.id));
        assert_eq!(signals.usage(&a.id), 1);
        assert_eq!(signals.last_used_at(&a.id), Some(30));
        assert_eq!(signals.recency(&a.id), 90);

        signals.set_favorite(&b.id, true);
        signals.reset_all();
        assert!(!signals.is_favorite(&b.id));
        assert_eq!(signals.last_used_at(&a.id), None);
        assert_eq!(signals.usage(&a.id), 0);
        assert_eq!(signals.now(), 0);
    }

    #[test]
    fn equal_timestamps_use_stable_id_tie_breaks() {
        let commands = vec![command("app:b", "Same"), command("app:a", "Same")];
        let mut signals = SearchSignals::new();
        for command in &commands {
            signals.record_use_at(&command.id, 50);
        }
        assert_eq!(signals.recency(&commands[0].id), MAX_RECENCY_SIGNAL);
        assert_eq!(signals.recency(&commands[1].id), MAX_RECENCY_SIGNAL);
        assert_eq!(
            ids(&rank(&commands, "same", &signals)),
            vec!["app:a", "app:b"]
        );

        // The legacy timestamp-free API preserves zero recency.
        let mut legacy = SearchSignals::new();
        legacy.record_use(&commands[0].id);
        assert_eq!(legacy.recency(&commands[0].id), 0);
        assert_eq!(legacy.last_used_at(&commands[0].id), None);
    }
}
