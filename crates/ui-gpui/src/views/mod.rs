//! Phase 0 feature view scaffolds.
//!
//! Each module owns one feature surface and no shared shell state. The shell
//! renders a summary line from [`FEATURE_VIEWS`]; feature work should replace
//! the placeholder state without changing the shell or another view.
//!
//! Ownership is one file per feature:
//!
//! | View | Owner |
//! |---|---|
//! | Clipboard history | `views/clipboard.rs` |
//! | Snippets | `views/snippets.rs` |
//! | Scripts | `views/scripts.rs` |
//! | Agents | `views/agents.rs` |
//! | MCP servers | `views/mcp.rs` |
//! | Window actions | `views/windows.rs` |
//! | File search | `views/files.rs` |
//!
//! No view reads storage, the vault or the clipboard on the render path.

pub(crate) mod agents;
pub(crate) mod clipboard;
pub(crate) mod files;
pub(crate) mod mcp;
pub(crate) mod scripts;
pub(crate) mod snippets;
pub(crate) mod windows;

/// Static metadata for a feature surface scaffold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FeatureView {
    /// Stable feature identity.
    pub(crate) id: &'static str,
    /// Display title.
    pub(crate) title: &'static str,
    /// Generic record kind the feature will own, once implemented.
    pub(crate) record_kind: Option<&'static str>,
    /// Truthful current state.
    pub(crate) state: &'static str,
    /// File that owns this view.
    pub(crate) owner: &'static str,
}

/// Every scaffolded feature surface, in display order.
pub(crate) const FEATURE_VIEWS: &[FeatureView] = &[
    clipboard::VIEW,
    snippets::VIEW,
    scripts::VIEW,
    agents::VIEW,
    mcp::VIEW,
    windows::VIEW,
    files::VIEW,
];

/// One-line, truthful summary for Integration Status.
pub(crate) fn scaffold_status() -> String {
    let titles = FEATURE_VIEWS
        .iter()
        .map(|view| view.title)
        .collect::<Vec<_>>()
        .join(", ");
    let record_backed = FEATURE_VIEWS
        .iter()
        .filter(|view| view.record_kind.is_some())
        .count();
    format!(
        "Feature views: {titles} are scaffolded ({record_backed} with generic record namespaces). \
         No feature data, clipboard history, MCP connection or agent conversation is enabled."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scaffold_names_a_unique_owner_and_id() {
        let mut ids = std::collections::BTreeSet::new();
        let mut owners = std::collections::BTreeSet::new();
        for view in FEATURE_VIEWS {
            assert!(ids.insert(view.id), "duplicate feature id: {}", view.id);
            assert!(
                owners.insert(view.owner),
                "duplicate feature owner: {}",
                view.owner
            );
            assert!(!view.state.is_empty());
            assert!(view.owner.ends_with(".rs"));
        }
        assert_eq!(FEATURE_VIEWS.len(), 7);
        assert!(scaffold_status().contains("scaffolded"));
    }
}
