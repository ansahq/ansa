//! Golden relevance-corpus runner.
//!
//! The committed corpus is split into a tuning set and a validation set. This
//! runner reports top-1 accuracy, recall@10 and MRR per category, per split and
//! overall, and fails when any pinned baseline regresses.
//!
//! Definitions used here (recorded in docs/ranking/):
//!
//! - top-1 accuracy: the first ranked id equals the first expected id.
//! - recall@10: expected ids found in the first ten ranked ids divided by the
//!   number of expected ids.
//! - MRR: reciprocal rank of the first ranked id that appears anywhere in the
//!   expected list, averaged over queries (0 when no expected id is returned).

use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_search::{MatchTier, SearchSignals, normalize, rank};
use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};

/// Bump this together with `corpus_version` in the JSON data file.
const EXPECTED_CORPUS_VERSION: &str = "2026-11-19.3";
const EXPECTED_COMMANDS: usize = 31;
const EXPECTED_TUNING_QUERIES: usize = 23;
const EXPECTED_VALIDATION_QUERIES: usize = 39;
const EPSILON: f64 = 1e-9;

#[derive(Debug, Deserialize)]
struct Corpus {
    corpus_version: String,
    notes: String,
    commands: Vec<CommandSpec>,
    splits: Splits,
}

#[derive(Debug, Deserialize)]
struct CommandSpec {
    id: String,
    title: String,
    #[serde(default)]
    alias: Option<String>,
    #[serde(default)]
    keywords: Vec<String>,
    #[serde(default)]
    subtitle: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Splits {
    tuning: Split,
    validation: Split,
}

#[derive(Debug, Deserialize)]
struct Split {
    notes: String,
    #[serde(default)]
    signals: SignalSpec,
    categories: BTreeMap<String, Vec<QuerySpec>>,
}

#[derive(Debug, Default, Deserialize)]
struct SignalSpec {
    #[serde(default)]
    favorites: Vec<String>,
    #[serde(default)]
    usage: BTreeMap<String, u32>,
}

#[derive(Debug, Deserialize)]
struct QuerySpec {
    query: String,
    expected_top10: Vec<String>,
    #[serde(default)]
    known_failure: bool,
    notes: String,
}

#[derive(Debug, Default, Clone, Copy)]
struct Metrics {
    queries: usize,
    top1: usize,
    recall_hits: usize,
    recall_total: usize,
    reciprocal_rank_sum: f64,
}

impl Metrics {
    fn add(&mut self, other: Metrics) {
        self.queries += other.queries;
        self.top1 += other.top1;
        self.recall_hits += other.recall_hits;
        self.recall_total += other.recall_total;
        self.reciprocal_rank_sum += other.reciprocal_rank_sum;
    }

    fn top1_accuracy(self) -> f64 {
        ratio(self.top1, self.queries)
    }

    fn recall_at_10(self) -> f64 {
        ratio(self.recall_hits, self.recall_total)
    }

    fn mrr(self) -> f64 {
        ratio_f64(self.reciprocal_rank_sum, self.queries)
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator as f64 / denominator as f64
    }
}

fn ratio_f64(numerator: f64, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        numerator / denominator as f64
    }
}

#[derive(Debug, Clone, Copy)]
struct Threshold {
    top1: f64,
    recall_at_10: f64,
    mrr: f64,
}

impl Threshold {
    const fn new(top1: f64, recall_at_10: f64, mrr: f64) -> Self {
        Self {
            top1,
            recall_at_10,
            mrr,
        }
    }
}

/// Tuning floor measured from `relevance_corpus.json` version 2026-11-19.3.
/// The tuning split is unchanged from the earlier policy-derived corpus.
const TUNING_THRESHOLDS: &[(&str, Threshold)] = &[
    ("exact", Threshold::new(1.0, 1.0, 1.0)),
    ("prefix", Threshold::new(1.0, 1.0, 1.0)),
    ("token", Threshold::new(1.0, 1.0, 1.0)),
    ("fuzzy", Threshold::new(1.0, 1.0, 1.0)),
    ("typo", Threshold::new(1.0, 1.0, 1.0)),
    ("unicode", Threshold::new(1.0, 1.0, 1.0)),
    ("browse", Threshold::new(1.0, 1.0, 1.0)),
];

const TUNING_OVERALL_THRESHOLD: Threshold = Threshold::new(1.0, 1.0, 1.0);

/// Held-out validation floor measured from version 2026-11-19.3. These are the
/// real measured values, including known failures. They are not predictions or
/// targets, and the ranker was not tuned against this split.
const VALIDATION_THRESHOLDS: &[(&str, Threshold)] = &[
    ("exact", Threshold::new(1.0, 1.0, 1.0)),
    ("prefix", Threshold::new(1.0, 1.0, 1.0)),
    ("token", Threshold::new(1.0, 1.0, 1.0)),
    ("fuzzy", Threshold::new(1.0, 1.0, 1.0)),
    // 5/7 top-1 and MRR, 6/9 recall@10.
    ("typo", Threshold::new(5.0 / 7.0, 6.0 / 9.0, 5.0 / 7.0)),
    // 3/5 for all three metrics.
    ("unicode", Threshold::new(3.0 / 5.0, 3.0 / 5.0, 3.0 / 5.0)),
    ("browse", Threshold::new(1.0, 1.0, 1.0)),
];

// 35/39 top-1 and MRR, 60/65 recall@10.
const VALIDATION_OVERALL_THRESHOLD: Threshold =
    Threshold::new(35.0 / 39.0, 60.0 / 65.0, 35.0 / 39.0);

fn load_corpus() -> Corpus {
    serde_json::from_str(include_str!("../data/relevance_corpus.json"))
        .expect("relevance_corpus.json must be valid JSON")
}

fn to_command(spec: &CommandSpec) -> Command {
    let mut command = Command::new(
        CommandId::parse(spec.id.clone()).expect("corpus command id must be valid"),
        spec.title.clone(),
        CommandKind::Builtin(BuiltinCommand::About),
    );
    if let Some(alias) = &spec.alias {
        command = command.with_alias(alias.clone());
    }
    if !spec.keywords.is_empty() {
        command = command.with_keywords(spec.keywords.clone());
    }
    if let Some(subtitle) = &spec.subtitle {
        command = command.with_subtitle(subtitle.clone());
    }
    command
}

fn signals(spec: &SignalSpec, commands: &[Command]) -> SearchSignals {
    let mut signals = SearchSignals::new();
    let known: HashSet<&str> = commands.iter().map(|command| command.id.as_str()).collect();
    for favorite in &spec.favorites {
        assert!(
            known.contains(favorite.as_str()),
            "corpus favorite {favorite} is not a corpus command"
        );
        signals.set_favorite(
            &CommandId::parse(favorite.clone()).expect("favorite id must be valid"),
            true,
        );
    }
    for (id, count) in &spec.usage {
        assert!(
            known.contains(id.as_str()),
            "corpus usage id {id} is not a corpus command"
        );
        let command_id = CommandId::parse(id.clone()).expect("usage id must be valid");
        for _ in 0..*count {
            signals.record_use(&command_id);
        }
    }
    signals
}

fn category_accepts_tier(category: &str, tier: MatchTier) -> bool {
    match category {
        "exact" => matches!(tier, MatchTier::ExactAlias | MatchTier::ExactTitle),
        "prefix" => tier == MatchTier::TitlePrefix,
        "token" => tier == MatchTier::Token,
        "fuzzy" | "typo" => tier == MatchTier::Fuzzy,
        "unicode" => matches!(
            tier,
            MatchTier::ExactTitle | MatchTier::TitlePrefix | MatchTier::Token | MatchTier::Fuzzy
        ),
        "browse" => tier == MatchTier::Fallback,
        _ => false,
    }
}

fn ordered_prefix_problem(actual: &[String], expected: &[String]) -> Option<String> {
    let actual_prefix: Vec<&str> = actual
        .iter()
        .take(expected.len())
        .map(String::as_str)
        .collect();
    let expected_prefix: Vec<&str> = expected.iter().map(String::as_str).collect();
    if actual_prefix == expected_prefix {
        None
    } else {
        Some(format!(
            "ordered expectation mismatch: expected {expected_prefix:?}, actual prefix {actual_prefix:?}"
        ))
    }
}

fn evaluate(
    commands: &[Command],
    signals: &SearchSignals,
    category: &str,
    query: &QuerySpec,
) -> (Metrics, Vec<String>) {
    assert!(
        !query.expected_top10.is_empty(),
        "query {:?} must have expectations: {}",
        query.query,
        query.notes
    );
    let expected: HashSet<&str> = query.expected_top10.iter().map(String::as_str).collect();
    let ranked_results = rank(commands, &query.query, signals);
    let ranked: Vec<String> = ranked_results
        .iter()
        .map(|result| result.command.id.as_str().to_owned())
        .collect();

    let mut metrics = Metrics {
        queries: 1,
        ..Metrics::default()
    };
    if ranked.first().map(String::as_str) == query.expected_top10.first().map(String::as_str) {
        metrics.top1 = 1;
    }

    let retrieved: HashSet<&str> = ranked.iter().take(10).map(String::as_str).collect();
    metrics.recall_hits = expected.intersection(&retrieved).count();
    metrics.recall_total = expected.len();

    if let Some(position) = ranked.iter().position(|id| expected.contains(id.as_str())) {
        metrics.reciprocal_rank_sum = 1.0 / (position as f64 + 1.0);
    }

    let mut problems = Vec::new();
    if !query.known_failure {
        if metrics.top1 == 0 {
            problems.push(format!(
                "top1 mismatch: expected first {:?}, got {:?}",
                query.expected_top10.first(),
                ranked.first()
            ));
        }
        let missing: Vec<&str> = expected
            .iter()
            .copied()
            .filter(|id| !retrieved.contains(id))
            .collect();
        if !missing.is_empty() {
            problems.push(format!("missing from top10: {missing:?}"));
        }
        if let Some(problem) = ordered_prefix_problem(&ranked, &query.expected_top10) {
            problems.push(problem);
        }
        if let Some(result) = ranked_results.first()
            && !category_accepts_tier(category, result.tier)
        {
            problems.push(format!(
                "category {category:?} expects a different tier, got {:?}",
                result.tier
            ));
        }
    }
    (metrics, problems)
}

fn evaluate_split(
    split_name: &str,
    split: &Split,
    commands: &[Command],
    combined: &mut BTreeMap<String, Metrics>,
) -> BTreeMap<String, Metrics> {
    let signals = signals(&split.signals, commands);
    let mut metrics_by_category = BTreeMap::new();
    let mut overall = Metrics::default();
    let mut problems = Vec::new();

    for (category, queries) in &split.categories {
        let mut category_metrics = Metrics::default();
        for query in queries {
            let (query_metrics, query_problems) = evaluate(commands, &signals, category, query);
            category_metrics.add(query_metrics);
            overall.add(query_metrics);
            for problem in query_problems {
                problems.push(format!(
                    "{split_name}/{category} query {:?}: {problem}",
                    query.query
                ));
            }
        }
        combined
            .entry(category.clone())
            .or_default()
            .add(category_metrics);
        metrics_by_category.insert(category.clone(), category_metrics);
    }

    println!("split {split_name}: {}", split.notes);
    for (category, metrics) in &metrics_by_category {
        println!(
            "  {category:<8} n={:<2} top1={:.3} recall@10={:.3} mrr={:.3}",
            metrics.queries,
            metrics.top1_accuracy(),
            metrics.recall_at_10(),
            metrics.mrr()
        );
    }
    println!(
        "  overall  n={:<2} top1={:.3} recall@10={:.3} mrr={:.3}",
        overall.queries,
        overall.top1_accuracy(),
        overall.recall_at_10(),
        overall.mrr()
    );
    for problem in &problems {
        println!("  PROBLEM {problem}");
    }
    assert!(
        problems.is_empty(),
        "corpus relevance or integrity problems:\n{}",
        problems.join("\n")
    );
    metrics_by_category
}

fn assert_threshold(label: &str, metrics: Metrics, threshold: Threshold) {
    let top1 = metrics.top1_accuracy();
    let recall = metrics.recall_at_10();
    let mrr = metrics.mrr();
    assert!(
        top1 + EPSILON >= threshold.top1,
        "{label} top-1 regression: {top1:.6} < {:.6}",
        threshold.top1
    );
    assert!(
        recall + EPSILON >= threshold.recall_at_10,
        "{label} recall@10 regression: {recall:.6} < {:.6}",
        threshold.recall_at_10
    );
    assert!(
        mrr + EPSILON >= threshold.mrr,
        "{label} MRR regression: {mrr:.6} < {:.6}",
        threshold.mrr
    );
}

fn sum_metrics(metrics: &BTreeMap<String, Metrics>) -> Metrics {
    let mut total = Metrics::default();
    for metrics in metrics.values() {
        total.add(*metrics);
    }
    total
}

fn validate_corpus(corpus: &Corpus, commands: &[Command]) {
    assert_eq!(commands.len(), EXPECTED_COMMANDS, "command count changed");
    let known: HashSet<&str> = commands.iter().map(|command| command.id.as_str()).collect();
    let mut global_queries: BTreeMap<String, (String, String)> = BTreeMap::new();

    for (split_name, split, expected_count) in [
        ("tuning", &corpus.splits.tuning, EXPECTED_TUNING_QUERIES),
        (
            "validation",
            &corpus.splits.validation,
            EXPECTED_VALIDATION_QUERIES,
        ),
    ] {
        let mut split_queries = HashSet::new();
        let mut count = 0usize;
        for (category, queries) in &split.categories {
            for query in queries {
                count += 1;
                assert!(
                    !query.expected_top10.is_empty() && query.expected_top10.len() <= 10,
                    "{split_name}/{category} query {:?} must have 1..=10 expectations",
                    query.query
                );
                let unique_expected: HashSet<&str> =
                    query.expected_top10.iter().map(String::as_str).collect();
                assert_eq!(
                    unique_expected.len(),
                    query.expected_top10.len(),
                    "{split_name}/{category} query {:?} has duplicate expected ids",
                    query.query
                );
                for expected in &query.expected_top10 {
                    assert!(
                        known.contains(expected.as_str()),
                        "{split_name}/{category} query {:?} expects unknown id {expected}",
                        query.query
                    );
                }

                let normalized = normalize(&query.query);
                if !normalized.is_empty() {
                    assert!(
                        split_queries.insert(normalized.clone()),
                        "duplicate non-empty query {:?} within {split_name}",
                        query.query
                    );
                    if let Some((other_split, other_category)) = global_queries.insert(
                        normalized.clone(),
                        (split_name.to_owned(), category.clone()),
                    ) {
                        panic!(
                            "non-empty query {:?} appears in {other_split}/{other_category} and {split_name}/{category}",
                            query.query
                        );
                    }
                }
            }
        }
        assert_eq!(
            count, expected_count,
            "{split_name} query count changed; bump the pinned count deliberately"
        );
    }
}

#[test]
fn relevance_corpus_baseline_holds() {
    let corpus = load_corpus();
    assert_eq!(
        corpus.corpus_version, EXPECTED_CORPUS_VERSION,
        "corpus version and runner baseline must be bumped together"
    );
    assert!(!corpus.notes.is_empty());
    assert!(!corpus.commands.is_empty());

    let commands: Vec<Command> = corpus.commands.iter().map(to_command).collect();
    let unique_ids: HashSet<&str> = corpus
        .commands
        .iter()
        .map(|spec| spec.id.as_str())
        .collect();
    assert_eq!(
        unique_ids.len(),
        corpus.commands.len(),
        "corpus command ids must be unique"
    );
    validate_corpus(&corpus, &commands);

    println!("corpus version: {}", corpus.corpus_version);
    println!("corpus commands: {}", commands.len());

    let mut combined = BTreeMap::new();
    let tuning = evaluate_split("tuning", &corpus.splits.tuning, &commands, &mut combined);
    let validation = evaluate_split(
        "validation",
        &corpus.splits.validation,
        &commands,
        &mut combined,
    );
    let tuning_overall = sum_metrics(&tuning);
    let validation_overall = sum_metrics(&validation);

    println!("combined categories:");
    let mut overall = Metrics::default();
    for (category, metrics) in &combined {
        println!(
            "  {category:<8} n={:<2} top1={:.3} recall@10={:.3} mrr={:.3}",
            metrics.queries,
            metrics.top1_accuracy(),
            metrics.recall_at_10(),
            metrics.mrr()
        );
        overall.add(*metrics);
    }
    println!(
        "overall    n={:<2} top1={:.3} recall@10={:.3} mrr={:.3}",
        overall.queries,
        overall.top1_accuracy(),
        overall.recall_at_10(),
        overall.mrr()
    );

    for (category, threshold) in TUNING_THRESHOLDS {
        let metrics = tuning
            .get(*category)
            .unwrap_or_else(|| panic!("missing tuning corpus category {category}"));
        assert_threshold(&format!("tuning/{category}"), *metrics, *threshold);
    }
    assert_eq!(
        tuning.len(),
        TUNING_THRESHOLDS.len(),
        "tuning corpus has an unthresholded category"
    );
    assert_threshold("tuning overall", tuning_overall, TUNING_OVERALL_THRESHOLD);

    for (category, threshold) in VALIDATION_THRESHOLDS {
        let metrics = validation
            .get(*category)
            .unwrap_or_else(|| panic!("missing validation corpus category {category}"));
        assert_threshold(&format!("validation/{category}"), *metrics, *threshold);
    }
    assert_eq!(
        validation.len(),
        VALIDATION_THRESHOLDS.len(),
        "validation corpus has an unthresholded category"
    );
    assert_threshold(
        "validation overall",
        validation_overall,
        VALIDATION_OVERALL_THRESHOLD,
    );
}

#[test]
fn expected_order_regression_is_detected() {
    let actual = vec!["app:a".to_owned(), "app:b".to_owned(), "app:c".to_owned()];
    let expected_ok = vec!["app:a".to_owned(), "app:b".to_owned(), "app:c".to_owned()];
    let swapped = vec!["app:a".to_owned(), "app:c".to_owned(), "app:b".to_owned()];
    assert!(ordered_prefix_problem(&actual, &expected_ok).is_none());
    assert!(ordered_prefix_problem(&actual, &swapped).is_some());
}
