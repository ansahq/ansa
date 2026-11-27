//! Scripts view scaffold.
//!
//! Script definitions will use the generic `feature.script` record namespace.
//! Execution is deliberately absent until a reviewed command handler and tool
//! approval path exist.

use super::FeatureView;
use ansa_alpha::SCRIPT_RECORD_KIND;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "scripts",
    title: "Scripts",
    record_kind: Some(SCRIPT_RECORD_KIND),
    state: "Scaffold only — no script authoring or execution is enabled",
    owner: "views/scripts.rs",
};
