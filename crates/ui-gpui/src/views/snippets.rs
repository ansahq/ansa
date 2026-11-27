//! Snippets view scaffold.
//!
//! Snippets will search the generic `feature.snippet` record namespace and
//! insert text through the shared clipboard `PasteTarget` trait. No snippet is
//! stored or rendered yet.

use super::FeatureView;
use ansa_alpha::SNIPPET_RECORD_KIND;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "snippets",
    title: "Snippets",
    record_kind: Some(SNIPPET_RECORD_KIND),
    state: "Scaffold only — no snippet library or insertion path is enabled",
    owner: "views/snippets.rs",
};
