//! Property-style editing sequences over a curated Unicode corpus.
//!
//! The generator is a small deterministic xorshift PRNG so the tests need no
//! new dependency and failures are reproducible from the printed seed. Every
//! operation asserts the model invariants: valid UTF-8, size bound, grapheme
//! aligned selection, bounded history and consistent composition state.

use super::model::{
    EditRejection, MAX_HISTORY_BYTES, MAX_HISTORY_ENTRIES, MAX_TEXT_SCALARS, TextModel,
};
use unicode_segmentation::UnicodeSegmentation;

const CORPUS: &[&str] = &[
    "a",
    "Z",
    "0",
    " ",
    "_",
    "é",
    "e\u{301}",
    "ñ",
    "क",
    "क्",
    "क्\u{0937}",
    "ि",
    "ष",
    "漢",
    "字",
    "に",
    "ほ",
    "ש",
    "ל",
    "م",
    "ر",
    "€",
    "@",
    "ß",
    "😀",
    "👍🏽",
    "👨‍👩‍👧‍👦",
    "🏳️‍🌈",
    "🇯🇵",
    "🇮🇳",
];

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }

    fn below(&mut self, upper: usize) -> usize {
        if upper == 0 {
            0
        } else {
            (self.next_u64() % upper as u64) as usize
        }
    }
}

fn grapheme_boundary(text: &str, offset: usize) -> bool {
    offset == text.len()
        || text
            .grapheme_indices(true)
            .any(|(index, _)| index == offset)
}

fn utf16_boundaries(text: &str) -> Vec<usize> {
    let mut boundaries = vec![0];
    for (index, grapheme) in text.grapheme_indices(true) {
        boundaries.push(index + grapheme.len());
    }
    boundaries
        .into_iter()
        .map(|byte| text[..byte].encode_utf16().count())
        .collect()
}

fn assert_invariants(model: &TextModel, context: &str) {
    let text = model.content();
    assert!(
        text.chars().count() <= MAX_TEXT_SCALARS,
        "{context}: text exceeded the scalar limit"
    );

    let selection = model.selected_range();
    assert!(
        selection.start <= selection.end,
        "{context}: reversed stored range"
    );
    assert!(
        selection.end <= text.len(),
        "{context}: selection past the end"
    );
    assert!(
        text.is_char_boundary(selection.start) && text.is_char_boundary(selection.end),
        "{context}: selection split a UTF-8 scalar"
    );
    assert!(
        grapheme_boundary(text, selection.start) && grapheme_boundary(text, selection.end),
        "{context}: selection split a grapheme"
    );

    if let Some(marked) = model.marked_range() {
        assert!(
            model.is_composing(),
            "{context}: marked range without state"
        );
        assert!(
            marked.start <= marked.end && marked.end <= text.len(),
            "{context}: marked range out of bounds"
        );
        assert!(
            text.is_char_boundary(marked.start) && text.is_char_boundary(marked.end),
            "{context}: marked range split a UTF-8 scalar"
        );
    } else {
        assert!(
            !model.is_composing(),
            "{context}: composition state without a marked range"
        );
    }

    let (undo, redo, undo_bytes, redo_bytes) = model.history_sizes();
    assert!(
        undo + redo <= MAX_HISTORY_ENTRIES,
        "{context}: undo depth exceeded"
    );
    assert!(
        undo_bytes + redo_bytes <= MAX_HISTORY_BYTES,
        "{context}: saved history bytes exceeded"
    );

    if model.is_composing() {
        assert!(!model.can_undo(), "{context}: undo exposed while composing");
        assert!(!model.can_redo(), "{context}: redo exposed while composing");
    }

    for offset in [selection.start, selection.end] {
        let utf16 = model.offset_to_utf16(offset);
        assert_eq!(
            model.offset_from_utf16(utf16),
            offset,
            "{context}: UTF-16 round trip failed"
        );
    }
}

fn assert_rejection_unchanged(
    model: &TextModel,
    content: &str,
    selection: &std::ops::Range<usize>,
) {
    assert_eq!(model.content(), content, "rejected edit changed text");
    assert_eq!(
        model.selected_range(),
        selection.clone(),
        "rejected edit changed selection"
    );
}

#[test]
fn random_editing_sequences_preserve_model_invariants() {
    for seed in [0x5eed_1234_u64, 0xa11c_e000, 0xdead_beef] {
        let mut rng = Rng::new(seed);
        let mut model = TextModel::default();

        for step in 0..4_000 {
            let context = format!("seed {seed:#x} step {step}");
            let text_len = model.content().len();
            let piece = CORPUS[rng.below(CORPUS.len())];
            let before = model.content().to_owned();
            let before_selection = model.selected_range();

            match rng.below(20) {
                0 => match model.replace_text(None, piece) {
                    Ok(_) => {}
                    Err(rejection) => {
                        assert!(matches!(rejection, EditRejection::TextTooLong { .. }));
                        assert_rejection_unchanged(&model, &before, &before_selection);
                    }
                },
                1 => {
                    let utf16_len = model.utf16_len();
                    let start = rng.below(utf16_len + 3);
                    let end = rng.below(utf16_len + 3);
                    match model.replace_text(Some(start..end), piece) {
                        Ok(_) => {}
                        Err(_) => assert_rejection_unchanged(&model, &before, &before_selection),
                    }
                }
                2 => model.move_to(rng.below(text_len + 3)),
                3 => {
                    let offset = rng.below(text_len + 3);
                    model.select_to(offset);
                }
                4 => model.select_all(),
                5 => {
                    let offset = model.previous_boundary(rng.below(text_len + 3));
                    model.move_to(offset);
                }
                6 => {
                    let offset = model.next_boundary(rng.below(text_len + 3));
                    model.move_to(offset);
                }
                7 => {
                    let offset = model.previous_word_boundary(rng.below(text_len + 3));
                    model.move_to(offset);
                }
                8 => {
                    let offset = model.next_word_boundary(rng.below(text_len + 3));
                    model.move_to(offset);
                }
                9 => {
                    if model.selected_range().is_empty() {
                        let offset = model.previous_boundary(model.cursor_offset());
                        model.select_to(offset);
                    }
                    if !model.selected_range().is_empty() {
                        assert!(model.replace_text(None, "").is_ok());
                    }
                }
                10 => {
                    if model.selected_range().is_empty() {
                        let offset = model.next_boundary(model.cursor_offset());
                        model.select_to(offset);
                    }
                    if !model.selected_range().is_empty() {
                        assert!(model.replace_text(None, "").is_ok());
                    }
                }
                11 => {
                    model.undo();
                }
                12 => {
                    model.redo();
                }
                13 => {
                    let utf16_len = model.utf16_len();
                    let start = rng.below(utf16_len + 2);
                    let end = rng.below(utf16_len + 2);
                    let range = if rng.below(2) == 0 {
                        Some(start..end)
                    } else {
                        None
                    };
                    let piece_boundaries = utf16_boundaries(piece);
                    let relative_start = piece_boundaries[rng.below(piece_boundaries.len())];
                    let relative_end = piece_boundaries[rng.below(piece_boundaries.len())];
                    let relative = if rng.below(2) == 0 {
                        Some(relative_start..relative_end)
                    } else {
                        Some(relative_end..relative_start)
                    };
                    match model.replace_and_mark_text(range, piece, relative) {
                        Ok(_) => {}
                        Err(_) => assert_rejection_unchanged(&model, &before, &before_selection),
                    }
                }
                14 => model.unmark(),
                15 => {
                    model.cancel_composition();
                }
                16 => {
                    model.cut_selection();
                }
                17 => {
                    if let Some(result) = model.paste_text(piece) {
                        if result.is_err() {
                            assert_rejection_unchanged(&model, &before, &before_selection);
                        }
                    } else {
                        assert_eq!(model.content(), before);
                    }
                }
                18 => {
                    model.selected_text();
                }
                _ => {
                    if rng.below(200) == 0 {
                        model.clear();
                    } else {
                        model.select_all();
                    }
                }
            }

            assert_invariants(&model, &context);

            if model.selected_range().is_empty() {
                // The caret is always a grapheme boundary, including after a
                // deletion that merged clusters.
                assert!(grapheme_boundary(model.content(), model.cursor_offset()));
            }
        }
    }
}

#[test]
fn undo_and_redo_are_exact_on_random_history() {
    for seed in [7_u64, 11, 13] {
        let mut rng = Rng::new(seed);
        let mut model = TextModel::default();
        let mut states = vec![(String::new(), 0..0)];
        let mut current = 0;

        for step in 0..600 {
            if rng.below(3) == 0 {
                if current > 0 && model.undo() {
                    current -= 1;
                    let (text, selection) = &states[current];
                    assert_eq!(model.content(), text, "seed {seed} step {step}");
                    assert_eq!(&model.selected_range(), selection);
                }
            } else if rng.below(3) == 0 {
                if current + 1 < states.len() && model.redo() {
                    current += 1;
                    let (text, selection) = &states[current];
                    assert_eq!(model.content(), text, "seed {seed} step {step}");
                    assert_eq!(&model.selected_range(), selection);
                }
            } else {
                let piece = CORPUS[rng.below(CORPUS.len())];
                match model.replace_text(None, piece) {
                    Ok(true) => {
                        states.truncate(current + 1);
                        states.push((model.content().to_owned(), model.selected_range().clone()));
                        current += 1;
                    }
                    Ok(false) => {}
                    Err(_) => {}
                }
            }

            assert_invariants(&model, &format!("undo seed {seed} step {step}"));
        }
    }
}
