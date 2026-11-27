//! File search view scaffold.
//!
//! Owns file search results and reveal/open actions. Indexing is not enabled;
//! no path is read on the render path.

use super::FeatureView;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "files",
    title: "File search",
    record_kind: None,
    state: "Scaffold only — no index or file results are enabled",
    owner: "views/files.rs",
};
