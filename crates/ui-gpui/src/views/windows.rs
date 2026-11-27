//! Window actions view scaffold.
//!
//! Owns window enumeration and focus actions as a feature surface. The launcher
//! shell keeps its own summon/focus lifecycle; this view must call reviewed
//! platform adapters rather than reaching into `shell.rs`.

use super::FeatureView;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "windows",
    title: "Window actions",
    record_kind: None,
    state: "Scaffold only — no window picker or focus action is enabled",
    owner: "views/windows.rs",
};
