//! Clipboard history view scaffold.
//!
//! Owns the clipboard list surface only. Capture, sealing and insertion stay in
//! `ansa-clipboard`; paste must go through `PasteTarget` so a snippet and a
//! history row cannot diverge. History is sealed through the vault, not the
//! generic unencrypted record namespace.

use super::FeatureView;

pub(crate) const VIEW: FeatureView = FeatureView {
    id: "clipboard",
    title: "Clipboard history",
    record_kind: None,
    state: "Scaffold only — history capture and rendering are not wired",
    owner: "views/clipboard.rs",
};
