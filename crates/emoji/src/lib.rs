//! Licensed, versioned local emoji and symbol search for Ansa.
//!
//! The crate embeds a curated dataset (`data/emoji.json`) derived from Unicode
//! 16.0 emoji data and CLDR-style annotations. Loading performs no network
//! access. Every base and variant is validated to be exactly one extended
//! grapheme cluster with `unicode-segmentation`.
//!
//! # Dataset pinning
//!
//! `data/emoji.json` records its version, Unicode release, source, license and
//! a canonical FNV-1a 64 checksum. Loading recomputes the checksum over the
//! canonical serialization (the same document with the `checksum` field set to
//! the empty string) and fails with [`DatasetError::ChecksumMismatch`] when the
//! file and pin disagree. [`load_from_json`] always enforces
//! [`PINNED_CHECKSUM`]; a recomputed self-consistent checksum on tampered data
//! is not accepted. [`load_from_json_with_checksum`] exists for fixture and
//! regeneration workflows that pin an explicit non-production checksum.
//! Caller-supplied input is bounded by [`MAX_DATASET_BYTES`] and
//! [`MAX_DATASET_ENTRIES`].
//!
//! FNV-1a 64 is an integrity pin, not a cryptographic signature.
//!
//! # Activation model
//!
//! Copying is always the default. Insertion is only reported as successful
//! when the target explicitly allows insertion and has been verified by the
//! host. Unverified targets produce [`ActivationOutcome::CopyFallback`] with a
//! reason; a target that does not allow insertion is a plain
//! [`ActivationOutcome::Copied`] because insertion was never attempted.

mod collections;
mod dataset;
mod search;

pub use collections::{Collections, MAX_FAVORITES, MAX_RECENTS};
pub use dataset::{
    Dataset, DatasetError, Diagnostics, Entry, MAX_DATASET_BYTES, MAX_DATASET_ENTRIES,
    PINNED_CHECKSUM, Variant, compute_checksum_for_json, dataset, diagnostics, fnv1a_64,
    is_single_grapheme, load_from_json, load_from_json_with_checksum,
};
pub use search::{MAX_QUERY_CHARS, MAX_RESULTS, SearchHit, search};

/// Why verified insertion was not attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyFallbackReason {
    /// The target allows insertion but the host could not verify focus or
    /// selection safety.
    TargetUnverified,
}

/// What an activation gesture should claim to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationOutcome {
    /// The value was copied. No insertion was attempted because the target
    /// does not allow it.
    Copied,
    /// The value was copied and then inserted into a verified, insertion-safe
    /// target.
    Inserted,
    /// The value was copied but insertion was not performed. Keep the
    /// clipboard copy and report this reason rather than claiming success.
    CopyFallback { reason: CopyFallbackReason },
}

/// Decides the activation outcome from host target checks.
///
/// The caller is responsible for actually copying the selected grapheme; this
/// function only models whether insertion may follow the copy.
pub fn activation(target_allows_insert: bool, target_verified: bool) -> ActivationOutcome {
    if !target_allows_insert {
        ActivationOutcome::Copied
    } else if target_verified {
        ActivationOutcome::Inserted
    } else {
        ActivationOutcome::CopyFallback {
            reason: CopyFallbackReason::TargetUnverified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    fn grapheme_count(value: &str) -> usize {
        value.graphemes(true).count()
    }

    #[test]
    fn embedded_dataset_loads_and_is_pinned() {
        let dataset = dataset().expect("embedded dataset must load");
        assert_eq!(dataset.checksum(), PINNED_CHECKSUM);
        assert!(dataset.entry_count() >= 100);
        assert_eq!(dataset.version(), "emoji-1.0.0");
        assert_eq!(dataset.unicode_version(), "16.0");
        assert_eq!(dataset.license(), "Unicode License v3");
    }

    #[test]
    fn every_base_and_variant_is_one_grapheme() {
        let dataset = dataset().expect("embedded dataset must load");
        for entry in dataset.entries() {
            assert_eq!(
                grapheme_count(entry.base()),
                1,
                "base {} {:?}",
                entry.id(),
                entry.base()
            );
            for variant in entry.variants() {
                assert_eq!(
                    grapheme_count(variant.grapheme()),
                    1,
                    "variant {} {} {:?}",
                    entry.id(),
                    variant.name(),
                    variant.grapheme()
                );
            }
        }
    }

    #[test]
    fn known_multi_codepoint_graphemes_are_single_clusters() {
        let dataset = dataset().expect("embedded dataset must load");
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
            assert_eq!(grapheme_count(entry.base()), 1, "{id}");
            for variant in entry.variants() {
                assert_eq!(grapheme_count(variant.grapheme()), 1, "{id}");
            }
        }
    }

    #[test]
    fn variant_names_are_distinguishable() {
        let dataset = dataset().expect("embedded dataset must load");
        for entry in dataset.entries() {
            let mut names: Vec<String> = entry
                .variants()
                .iter()
                .map(|variant| variant.accessible_name().to_lowercase())
                .collect();
            names.sort();
            names.dedup();
            assert_eq!(names.len(), entry.variants().len(), "{}", entry.id());
        }

        let waving = dataset.find("waving-hand").expect("waving hand");
        assert_eq!(waving.variants().len(), 5);
        assert!(
            waving
                .variants()
                .iter()
                .any(|variant| variant.name().contains("dark skin tone"))
        );
    }

    #[test]
    fn activation_never_claims_unverified_insertion() {
        assert_eq!(activation(true, true), ActivationOutcome::Inserted);
        assert_eq!(
            activation(true, false),
            ActivationOutcome::CopyFallback {
                reason: CopyFallbackReason::TargetUnverified
            }
        );
        assert_eq!(activation(false, false), ActivationOutcome::Copied);
        assert_eq!(activation(false, true), ActivationOutcome::Copied);
    }

    #[test]
    fn recents_and_favorites_are_independent_and_bounded() {
        let mut collections = Collections::new();
        for index in 0..(MAX_RECENTS + 10) {
            collections.record_recent(format!("recent-{index}"));
        }
        assert_eq!(collections.recents().len(), MAX_RECENTS);
        assert_eq!(
            collections.recents()[0],
            format!("recent-{}", MAX_RECENTS + 9)
        );

        collections.toggle_favorite("grinning-face");
        collections.record_recent("grinning-face");
        assert_eq!(collections.recents()[0], "grinning-face");
        assert!(collections.is_favorite("grinning-face"));

        collections.clear_recents();
        assert!(collections.recents().is_empty());
        assert_eq!(collections.favorites(), ["grinning-face"]);

        collections.record_recent("grinning-face");
        collections.clear_favorites();
        assert!(!collections.is_favorite("grinning-face"));
        assert_eq!(collections.recents(), ["grinning-face"]);
    }

    #[test]
    fn favorites_are_bounded_and_toggle_off() {
        let mut collections = Collections::new();
        for index in 0..(MAX_FAVORITES + 5) {
            assert!(collections.toggle_favorite(&format!("favorite-{index}")));
        }
        assert_eq!(collections.favorites().len(), MAX_FAVORITES);
        assert!(!collections.toggle_favorite("favorite-10"));
        assert_eq!(collections.favorites().len(), MAX_FAVORITES - 1);
    }
}
