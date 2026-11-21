//! Bounded, deterministic local search over the curated dataset.

use crate::dataset;
use crate::dataset::{Dataset, DatasetError, Entry};

/// Maximum Unicode scalar values considered from a query.
pub const MAX_QUERY_CHARS: usize = 64;

/// Maximum number of results returned by [`search`], regardless of `limit`.
pub const MAX_RESULTS: usize = 50;

/// One ranked search result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SearchHit {
    pub entry: &'static Entry,
    pub score: u32,
}

#[derive(Debug)]
pub(crate) struct SearchIndex {
    pub(crate) name: String,
    pub(crate) name_words: Vec<String>,
    pub(crate) keywords: Vec<String>,
    pub(crate) group: String,
    pub(crate) base: String,
    pub(crate) variant_graphemes: Vec<String>,
    variant_words: Vec<String>,
}

impl SearchIndex {
    pub(crate) fn new(entry: &Entry) -> Self {
        let name = normalize(entry.name());
        let name_words = name.split_whitespace().map(str::to_owned).collect();
        let keywords = entry
            .keywords()
            .iter()
            .map(|keyword| normalize(keyword))
            .collect();
        let variant_words = entry
            .variants()
            .iter()
            .flat_map(|variant| {
                normalize(variant.name())
                    .split_whitespace()
                    .map(str::to_owned)
                    .collect::<Vec<_>>()
            })
            .collect();
        Self {
            name,
            name_words,
            keywords,
            group: normalize(entry.group()),
            base: entry.base().to_owned(),
            variant_graphemes: entry
                .variants()
                .iter()
                .map(|variant| variant.grapheme().to_owned())
                .collect(),
            variant_words,
        }
    }
}

/// Searches the embedded dataset. The query is truncated to
/// [`MAX_QUERY_CHARS`] and the result list to [`MAX_RESULTS`]. An empty query
/// returns entries ordered by name and then id.
pub fn search(query: &str, limit: usize) -> Result<Vec<SearchHit>, DatasetError> {
    Ok(search_in(dataset()?, query, limit))
}

fn search_in(dataset: &'static Dataset, query: &str, limit: usize) -> Vec<SearchHit> {
    let limit = limit.min(MAX_RESULTS);
    if limit == 0 {
        return Vec::new();
    }

    let raw: String = query.trim().chars().take(MAX_QUERY_CHARS).collect();
    let normalized = normalize(&raw);
    let tokens: Vec<&str> = normalized.split_whitespace().collect();

    // A non-empty query that normalizes to punctuation/separators only must not
    // fall through to the empty-query browse behavior.
    if !raw.is_empty() && tokens.is_empty() {
        return Vec::new();
    }

    let mut hits: Vec<SearchHit> = dataset
        .entries()
        .iter()
        .zip(dataset.index())
        .filter_map(|(entry, index)| {
            let score = if tokens.is_empty() {
                0
            } else {
                score_entry(index, &raw, &normalized, &tokens)?
            };
            Some(SearchHit { entry, score })
        })
        .collect();

    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.entry.name().cmp(right.entry.name()))
            .then_with(|| left.entry.id().cmp(right.entry.id()))
    });
    hits.truncate(limit);
    hits
}

fn score_entry(index: &SearchIndex, raw: &str, normalized: &str, tokens: &[&str]) -> Option<u32> {
    if index.base == raw || index.variant_graphemes.iter().any(|value| value == raw) {
        return Some(1_100);
    }
    if index.name == normalized {
        return Some(1_000);
    }

    let mut total = 0_u32;
    for token in tokens {
        total = total.saturating_add(token_score(index, token)?);
    }
    if index.name.starts_with(normalized) {
        total = total.max(850);
    } else if index.name_words.iter().any(|word| word == normalized) {
        total = total.max(700);
    }
    Some(total)
}

fn token_score(index: &SearchIndex, token: &str) -> Option<u32> {
    if index.name == token {
        return Some(300);
    }
    if index.name_words.iter().any(|word| word == token) {
        return Some(260);
    }
    if index.name.starts_with(token) {
        return Some(240);
    }
    if index.name_words.iter().any(|word| word.starts_with(token)) {
        return Some(220);
    }
    if index.keywords.iter().any(|keyword| keyword == token) {
        return Some(200);
    }
    if index
        .keywords
        .iter()
        .any(|keyword| keyword.starts_with(token))
    {
        return Some(170);
    }
    if index.variant_words.iter().any(|word| word == token) {
        return Some(150);
    }
    if index.name.contains(token) {
        return Some(140);
    }
    if index.keywords.iter().any(|keyword| keyword.contains(token)) {
        return Some(130);
    }
    if index
        .variant_words
        .iter()
        .any(|word| word.starts_with(token))
    {
        return Some(120);
    }
    if index.group.contains(token) {
        return Some(90);
    }
    if token.chars().count() >= 4
        && index
            .name_words
            .iter()
            .chain(index.keywords.iter())
            .chain(index.variant_words.iter())
            .any(|word| within_one_edit(token, word))
    {
        return Some(60);
    }
    None
}

fn normalize(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut pending_space = false;
    for character in text.chars() {
        if character.is_whitespace() || matches!(character, '-' | '_' | '/' | '.' | ':' | ',') {
            pending_space = !result.is_empty();
        } else {
            if pending_space {
                result.push(' ');
                pending_space = false;
            }
            for lower in character.to_lowercase() {
                result.push(lower);
            }
        }
    }
    result
}

/// Bounded Levenshtein check: true when at most one insertion, deletion or
/// substitution turns `left` into `right`.
fn within_one_edit(left: &str, right: &str) -> bool {
    let left: Vec<char> = left.chars().collect();
    let right: Vec<char> = right.chars().collect();
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }
    let (long, short) = if left.len() >= right.len() {
        (left.as_slice(), right.as_slice())
    } else {
        (right.as_slice(), left.as_slice())
    };

    let mut long_index = 0;
    let mut short_index = 0;
    let mut edits = 0_u8;
    while long_index < long.len() && short_index < short.len() {
        if long[long_index] == short[short_index] {
            long_index += 1;
            short_index += 1;
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            if long.len() == short.len() {
                long_index += 1;
                short_index += 1;
            } else {
                long_index += 1;
            }
        }
    }
    let remaining = (long.len() - long_index) + (short.len() - short_index);
    edits as usize + remaining <= 1
}
