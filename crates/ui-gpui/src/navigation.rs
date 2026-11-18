//! Typed child-view bodies shown above the root launcher.
//!
//! The body text is static content; the identity and captured parent frame live
//! in [`ansa_core::navigation`]. Nothing here is used as an identity.

use ansa_core::navigation::ViewId;

pub(crate) struct ChildView {
    pub(crate) id: ViewId,
    pub(crate) body: &'static str,
}

impl ChildView {
    pub(crate) fn about() -> Self {
        Self {
            id: ViewId::new("ansa:about").expect("static view identity"),
            body: ABOUT_BODY,
        }
    }

    pub(crate) fn settings() -> Self {
        Self {
            id: ViewId::new("ansa:settings").expect("static view identity"),
            body: SETTINGS_BODY,
        }
    }
}

pub(crate) const ABOUT_BODY: &str = "Ansa\n\nA little less friction. A little more flow.\n\nA free, open-source keyboard launcher built in Rust. An original interface, an offline core, and no account required.\n\nFoundation prototype · 0.1.0\nApache-2.0 · GPUI 0.2.2\n\nThis build starts the feasibility phase; it does not claim Raycast extension compatibility or production readiness.";

pub(crate) const SETTINGS_BODY: &str = "Integration status\n\nApplications: Windows Start Menu shortcuts (.lnk). Packaged apps and switching to running windows are not supported yet.\n\nSummon: Ctrl+Space when registration succeeds. Shortcut editing is not implemented. Dismissal attempts checked, best-effort focus return only if Ansa still owns the foreground. Windows can deny activation.\n\nActions: Ctrl+K, type to filter, arrows to select, Enter to act. Pins and usage signals last only for this process; no settings are saved yet. Tab reaches the footer control.\n\nCalculator: bounded approximate f64 arithmetic; Enter copies. Start with = to explicitly request arithmetic. No units, currencies, or dates yet.\n\nClipboard history, automatic paste, file indexing, extensions, AI and sync are not enabled. No account or network service is required.\n\nAccessibility and multilingual input still require real-device validation.";
