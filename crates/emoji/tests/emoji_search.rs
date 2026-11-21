//! Public-API tests for the emoji dataset, search, activation and collections.

use ansa_emoji::{
    ActivationOutcome, Collections, CopyFallbackReason, DatasetError, MAX_DATASET_BYTES,
    MAX_FAVORITES, MAX_QUERY_CHARS, MAX_RECENTS, MAX_RESULTS, PINNED_CHECKSUM, activation,
    compute_checksum_for_json, dataset, diagnostics, is_single_grapheme, load_from_json,
    load_from_json_with_checksum, search,
};

const CORPUS: &str = include_str!("../data/emoji.json");

fn tampered_with_recomputed_checksum() -> (String, String) {
    let mutated = CORPUS
        .replace("Grinning Face", "Grinning Face Tampered")
        .replace("emoji-1.0.0", "evil-9.9.9");
    let blank = mutated.replace(PINNED_CHECKSUM, "");
    let recomputed =
        compute_checksum_for_json(&blank).expect("a blank-checksum document must be computable");
    let tampered = blank.replace(
        "\"checksum\": \"\"",
        &format!("\"checksum\": \"{recomputed}\""),
    );
    (tampered, recomputed)
}

#[test]
fn diagnostics_expose_version_source_license_and_checksum() {
    let diagnostics = diagnostics().expect("embedded dataset must load");
    assert_eq!(diagnostics.version, "emoji-1.0.0");
    assert_eq!(diagnostics.unicode_version, "16.0");
    assert_eq!(diagnostics.license, "Unicode License v3");
    assert!(diagnostics.source.contains("Unicode 16.0"));
    assert!(diagnostics.license_url.contains("unicode.org"));
    assert_eq!(diagnostics.checksum, PINNED_CHECKSUM);
    assert!(diagnostics.entry_count >= 100);
}

#[test]
fn checked_in_dataset_loads_through_the_public_verifier() {
    let loaded = load_from_json(CORPUS).expect("checked-in corpus must verify");
    assert_eq!(loaded.checksum(), PINNED_CHECKSUM);
    assert_eq!(loaded.entry_count(), dataset().unwrap().entry_count());
}

#[test]
fn tampered_copy_reports_checksum_mismatch() {
    let tampered = CORPUS.replace("\"Grinning Face\"", "\"Grinning Face Tampered\"");
    assert_ne!(CORPUS, tampered, "mutation must actually change the data");
    match load_from_json(&tampered) {
        Err(DatasetError::ChecksumMismatch { expected, actual }) => {
            assert_eq!(expected, PINNED_CHECKSUM);
            assert_ne!(actual, expected);
        }
        other => panic!("expected checksum mismatch, got {other:?}"),
    }
}

#[test]
fn recomputed_checksum_does_not_bypass_the_pin() {
    let (tampered, recomputed) = tampered_with_recomputed_checksum();
    assert_ne!(recomputed, PINNED_CHECKSUM);
    match load_from_json(&tampered) {
        Err(DatasetError::ChecksumMismatch { expected, actual }) => {
            assert_eq!(expected, PINNED_CHECKSUM);
            assert_eq!(actual, recomputed);
        }
        other => panic!("expected pinned checksum rejection, got {other:?}"),
    }

    let explicit = load_from_json_with_checksum(&tampered, &recomputed)
        .expect("explicit-checksum loader should accept an internally consistent fixture");
    assert_eq!(explicit.checksum(), recomputed);
    assert_eq!(explicit.version(), "evil-9.9.9");
    assert!(
        explicit
            .find("grinning-face")
            .unwrap()
            .name()
            .contains("Tampered")
    );
    assert_eq!(dataset().unwrap().checksum(), PINNED_CHECKSUM);
}

#[test]
fn caller_supplied_dataset_input_is_bounded() {
    let oversized = " ".repeat(MAX_DATASET_BYTES + 1);
    assert!(matches!(
        load_from_json(&oversized),
        Err(DatasetError::InputTooLarge { .. })
    ));
}

#[test]
fn punctuation_only_queries_return_no_hits() {
    assert!(search("-", 10).unwrap().is_empty());
    assert!(search("...", 10).unwrap().is_empty());
    assert!(!search("", 10).unwrap().is_empty());
}

#[test]
fn search_matches_names_keywords_and_unicode_graphemes() {
    let smile = search("smile", 20).expect("search");
    assert!(!smile.is_empty());
    assert!(smile.iter().any(|hit| hit.entry.id() == "grinning-face"));

    let exact = search("😀", 5).expect("search");
    assert_eq!(exact[0].entry.id(), "grinning-face");

    let heart = search("❤️", 5).expect("search");
    assert_eq!(heart[0].entry.id(), "red-heart");

    let text_heart = search("❤", 5).expect("search");
    assert_eq!(text_heart[0].entry.id(), "red-heart");

    let family = search("family", 10).expect("search");
    assert!(
        family
            .iter()
            .any(|hit| hit.entry.id() == "family-man-woman-girl-boy")
    );
}

#[test]
fn search_tolerates_a_single_typo() {
    let hits = search("smily", 10).expect("search");
    assert!(hits.iter().any(|hit| hit.entry.id() == "grinning-face"));
}

#[test]
fn search_surfaces_variant_words() {
    let hits = search("dark skin tone", MAX_RESULTS).expect("search");
    assert!(hits.iter().any(|hit| hit.entry.id() == "waving-hand"));
    assert!(hits.iter().any(|hit| hit.entry.id() == "thumbs-up"));

    let presentation = search("text presentation", MAX_RESULTS).expect("search");
    assert!(presentation.iter().any(|hit| hit.entry.id() == "airplane"));
}

#[test]
fn search_is_deterministic_and_bounded() {
    let first: Vec<&str> = search("flag", MAX_RESULTS)
        .unwrap()
        .into_iter()
        .map(|hit| hit.entry.id())
        .collect();
    let second: Vec<&str> = search("flag", MAX_RESULTS)
        .unwrap()
        .into_iter()
        .map(|hit| hit.entry.id())
        .collect();
    assert_eq!(first, second);

    assert!(search("a", 1_000).unwrap().len() <= MAX_RESULTS);
    assert!(search("a", 0).unwrap().is_empty());

    let long_query = "x".repeat(MAX_QUERY_CHARS + 500);
    assert!(search(&long_query, 5).is_ok());

    let browsed: Vec<&str> = search("", 10)
        .unwrap()
        .into_iter()
        .map(|hit| hit.entry.id())
        .collect();
    assert_eq!(browsed.len(), 10);
    let browsed_again: Vec<&str> = search("", 10)
        .unwrap()
        .into_iter()
        .map(|hit| hit.entry.id())
        .collect();
    assert_eq!(browsed, browsed_again);
}

#[test]
fn grapheme_corpus_is_integrity_checked() {
    let dataset = dataset().expect("embedded dataset must load");
    for entry in dataset.entries() {
        assert!(is_single_grapheme(entry.base()), "{}", entry.id());
        for variant in entry.variants() {
            assert!(
                is_single_grapheme(variant.grapheme()),
                "{} {}",
                entry.id(),
                variant.name()
            );
        }
    }

    for id in [
        "family-man-woman-girl-boy",
        "couple-with-heart-woman-man",
        "rainbow-flag",
        "pirate-flag",
        "flag-united-states",
        "waving-hand",
        "keycap-one",
        "airplane",
        "red-heart",
    ] {
        let entry = dataset.find(id).unwrap_or_else(|| panic!("missing {id}"));
        assert!(is_single_grapheme(entry.base()), "{id}");
        for variant in entry.variants() {
            assert!(is_single_grapheme(variant.grapheme()), "{id}");
        }
    }
}

#[test]
fn variants_have_distinguishable_accessible_names() {
    let dataset = dataset().expect("embedded dataset must load");
    for entry in dataset.entries() {
        let mut names: Vec<&str> = entry
            .variants()
            .iter()
            .map(|variant| variant.accessible_name())
            .collect();
        let original = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), original, "{}", entry.id());
    }

    let waving = dataset.find("waving-hand").expect("waving-hand");
    assert_eq!(waving.variants().len(), 5);
    assert!(
        waving
            .variants()
            .iter()
            .all(|variant| variant.name().contains("skin tone"))
    );
}

#[test]
fn activation_falls_back_without_verification() {
    assert_eq!(activation(true, true), ActivationOutcome::Inserted);
    assert_eq!(
        activation(true, false),
        ActivationOutcome::CopyFallback {
            reason: CopyFallbackReason::TargetUnverified
        }
    );
    assert_eq!(activation(false, true), ActivationOutcome::Copied);
    assert_eq!(activation(false, false), ActivationOutcome::Copied);
}

#[test]
fn recents_and_favorites_clear_independently() {
    let mut collections = Collections::new();
    for index in 0..(MAX_RECENTS + 8) {
        collections.record_recent(format!("recent-{index}"));
    }
    assert_eq!(collections.recents().len(), MAX_RECENTS);

    for index in 0..(MAX_FAVORITES + 3) {
        assert!(collections.toggle_favorite(&format!("favorite-{index}")));
    }
    assert_eq!(collections.favorites().len(), MAX_FAVORITES);

    collections.record_recent("favorite-10");
    collections.clear_recents();
    assert!(collections.recents().is_empty());
    assert!(collections.is_favorite("favorite-10"));

    collections.clear_favorites();
    assert!(collections.favorites().is_empty());

    collections.record_recent("grinning-face");
    collections.toggle_favorite("grinning-face");
    collections.clear_recents();
    assert!(collections.is_favorite("grinning-face"));
    collections.clear_favorites();
    assert!(!collections.is_favorite("grinning-face"));
}

#[test]
fn empty_identities_are_ignored_by_favorites() {
    let mut collections = Collections::new();
    assert!(!collections.toggle_favorite(""));
    assert!(!collections.is_favorite(""));
    assert!(collections.favorites().is_empty());
    assert!(collections.recents().is_empty());
}
