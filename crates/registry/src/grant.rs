//! Capability catalog, permission snapshots and confirmation state.

use std::collections::BTreeSet;

/// Capability names known to the built-in registry catalog.
///
/// The catalog is intentionally closed: a command that requires an unknown
/// capability fails registration instead of silently running without a policy
/// entry. Hosts that expose additional capabilities construct a
/// [`super::CapabilityCatalog`] explicitly.
pub mod capability_names {
    /// Launch a desktop application.
    pub const LAUNCH_APPLICATION: &str = "host.application.launch";
    /// Open a URL through the host opener.
    pub const OPEN_URL: &str = "host.url.open";
    /// Run a reviewed script through the host runner.
    pub const RUN_SCRIPT: &str = "host.script.run";
    /// Invoke an action on a trusted extension instance.
    pub const INVOKE_EXTENSION: &str = "host.extension.invoke";
    /// Write to the system clipboard.
    pub const WRITE_CLIPBOARD: &str = "host.clipboard.write";
    /// Invoke a reviewed first-party built-in behavior.
    pub const INVOKE_BUILTIN: &str = "host.builtin.invoke";
}

/// The set of capability names a registry accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityCatalog {
    names: BTreeSet<String>,
}

impl CapabilityCatalog {
    /// The built-in catalog covering the standard host adapters.
    pub fn builtin() -> Self {
        Self::from_names([
            capability_names::LAUNCH_APPLICATION,
            capability_names::OPEN_URL,
            capability_names::RUN_SCRIPT,
            capability_names::INVOKE_EXTENSION,
            capability_names::WRITE_CLIPBOARD,
            capability_names::INVOKE_BUILTIN,
        ])
    }

    /// A catalog with no known capability names.
    pub fn empty() -> Self {
        Self {
            names: BTreeSet::new(),
        }
    }

    /// Builds a catalog from arbitrary names. Blank names are ignored.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let names = names
            .into_iter()
            .map(Into::into)
            .map(|name: String| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        Self { names }
    }

    /// Whether `name` is a known capability.
    pub fn contains(&self, name: &str) -> bool {
        self.names.contains(name)
    }

    /// Adds a capability name. Returns `false` when the name was blank or
    /// already present.
    pub fn insert(&mut self, name: impl Into<String>) -> bool {
        let name = name.into().trim().to_owned();
        if name.is_empty() {
            return false;
        }
        self.names.insert(name)
    }

    /// Iterates known names in stable order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.names.iter().map(String::as_str)
    }

    /// Number of known capability names.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether the catalog is empty.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

impl Default for CapabilityCatalog {
    fn default() -> Self {
        Self::builtin()
    }
}

/// The grant snapshot revision cannot advance because it is already at
/// [`u64::MAX`].
///
/// Permission changes fail closed instead of reusing a revision, which would
/// let an older authorization remain valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantRevisionExhausted;

impl std::fmt::Display for GrantRevisionExhausted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("grant snapshot revision space is exhausted")
    }
}

impl std::error::Error for GrantRevisionExhausted {}

/// A point-in-time snapshot of granted capability names.
///
/// The snapshot must be produced by trusted host permission state. The
/// registry is *not* a permission store and does not verify the snapshot's
/// origin; an in-process caller that can fabricate a snapshot can also
/// fabricate grants. What the registry does guarantee is that an
/// [`super::Authorization`] is bound to the snapshot's
/// [`revision`](Self::revision), so an accidentally stale snapshot is rejected
/// instead of silently reused.
///
/// Revisions must increase whenever the underlying permission state changes.
/// [`grant`](Self::grant) and [`revoke`](Self::revoke) bump it with a checked
/// increment. A host replacing the snapshot wholesale supplies its own
/// revision with [`with_revision`](Self::with_revision).
///
/// **Revision zero provides no staleness detection.** [`none`](Self::none) and
/// [`from_capabilities`](Self::from_capabilities) start at revision zero, so
/// two independently constructed snapshots look identical to the registry.
/// A trusted host must supply a monotonic revision when staleness matters.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrantSnapshot {
    granted: BTreeSet<String>,
    revision: u64,
}

impl GrantSnapshot {
    /// A snapshot with no grants at revision zero.
    ///
    /// Revision zero provides no staleness detection; prefer
    /// [`with_revision`](Self::with_revision) with trusted host state.
    pub fn none() -> Self {
        Self::default()
    }

    /// Builds a snapshot from capability names at revision zero. Blank names
    /// are ignored.
    ///
    /// Revision zero provides no staleness detection; prefer
    /// [`with_revision`](Self::with_revision) with trusted host state.
    pub fn from_capabilities<I, S>(capabilities: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let granted = capabilities
            .into_iter()
            .map(Into::into)
            .map(|capability: String| capability.trim().to_owned())
            .filter(|capability| !capability.is_empty())
            .collect();
        Self {
            granted,
            revision: 0,
        }
    }

    /// Sets the trusted host revision for this snapshot.
    ///
    /// Use this when the host can provide a monotonic permission-store
    /// revision. The registry compares this value between authorization and
    /// execution.
    pub fn with_revision(mut self, revision: u64) -> Self {
        self.revision = revision;
        self
    }

    /// The trusted host revision associated with this snapshot.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Grants a capability for this snapshot. Blank or already-present names
    /// are no-ops.
    ///
    /// A real change bumps the revision so older authorizations are rejected.
    ///
    /// # Panics
    ///
    /// Panics if the revision is already [`u64::MAX`]. Use
    /// [`try_grant`](Self::try_grant) to handle that condition explicitly.
    pub fn grant(&mut self, capability: impl Into<String>) {
        self.try_grant(capability)
            .expect("grant snapshot revision space is exhausted");
    }

    /// Fallible variant of [`grant`](Self::grant).
    ///
    /// Returns `true` when the capability was added. A revision at
    /// [`u64::MAX`] fails closed without changing the snapshot.
    pub fn try_grant(
        &mut self,
        capability: impl Into<String>,
    ) -> Result<bool, GrantRevisionExhausted> {
        let capability = capability.into().trim().to_owned();
        if capability.is_empty() || self.granted.contains(&capability) {
            return Ok(false);
        }
        let revision = self.revision.checked_add(1).ok_or(GrantRevisionExhausted)?;
        self.granted.insert(capability);
        self.revision = revision;
        Ok(true)
    }

    /// Revokes a capability from this snapshot. Returns whether it was present.
    /// A real change bumps the revision so older authorizations are rejected.
    ///
    /// # Panics
    ///
    /// Panics if the revision is already [`u64::MAX`]. Use
    /// [`try_revoke`](Self::try_revoke) to handle that condition explicitly.
    pub fn revoke(&mut self, capability: &str) -> bool {
        self.try_revoke(capability)
            .expect("grant snapshot revision space is exhausted")
    }

    /// Fallible variant of [`revoke`](Self::revoke).
    ///
    /// Returns `true` when the capability was removed. A revision at
    /// [`u64::MAX`] fails closed without changing the snapshot.
    pub fn try_revoke(&mut self, capability: &str) -> Result<bool, GrantRevisionExhausted> {
        if !self.granted.contains(capability) {
            return Ok(false);
        }
        let revision = self.revision.checked_add(1).ok_or(GrantRevisionExhausted)?;
        self.granted.remove(capability);
        self.revision = revision;
        Ok(true)
    }

    /// Whether this snapshot contains `capability`.
    pub fn contains(&self, capability: &str) -> bool {
        self.granted.contains(capability)
    }

    /// Iterates granted names in stable order.
    pub fn granted(&self) -> impl Iterator<Item = &str> {
        self.granted.iter().map(String::as_str)
    }

    /// Number of granted capabilities.
    pub fn len(&self) -> usize {
        self.granted.len()
    }

    /// Whether no capabilities are granted.
    pub fn is_empty(&self) -> bool {
        self.granted.is_empty()
    }
}

/// Whether the user explicitly confirmed a consequential action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Confirmation {
    /// No confirmation was supplied.
    #[default]
    Unconfirmed,
    /// The user explicitly confirmed the displayed effect.
    Confirmed,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshots_grant_and_revoke_without_leaking_state() {
        let mut snapshot = GrantSnapshot::none();
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.revision(), 0);
        snapshot.grant(capability_names::OPEN_URL);
        assert!(snapshot.contains(capability_names::OPEN_URL));
        assert_eq!(snapshot.revision(), 1);
        assert_eq!(
            snapshot.granted().collect::<Vec<_>>(),
            vec![capability_names::OPEN_URL]
        );

        // Re-granting an existing capability is not a state change.
        snapshot.grant(capability_names::OPEN_URL);
        assert_eq!(snapshot.revision(), 1);

        let revoked = snapshot.clone();
        assert!(snapshot.revoke(capability_names::OPEN_URL));
        assert!(!snapshot.contains(capability_names::OPEN_URL));
        assert!(revoked.contains(capability_names::OPEN_URL));
        assert_eq!(snapshot.revision(), 2);
        assert!(!snapshot.revoke(capability_names::OPEN_URL));
        assert_eq!(snapshot.revision(), 2);
    }

    #[test]
    fn explicit_revisions_are_preserved() {
        let snapshot =
            GrantSnapshot::from_capabilities([capability_names::OPEN_URL]).with_revision(41);
        assert_eq!(snapshot.revision(), 41);
        assert_eq!(
            GrantSnapshot::from_capabilities([capability_names::OPEN_URL]).revision(),
            0
        );
    }

    #[test]
    fn revision_exhaustion_fails_closed() {
        let mut grant = GrantSnapshot::none().with_revision(u64::MAX);
        assert!(grant.try_grant("vendor.capability").is_err());
        assert!(!grant.contains("vendor.capability"));
        assert_eq!(grant.revision(), u64::MAX);

        let mut revoke =
            GrantSnapshot::from_capabilities(["vendor.capability"]).with_revision(u64::MAX);
        assert!(revoke.try_revoke("vendor.capability").is_err());
        assert!(revoke.contains("vendor.capability"));
        assert_eq!(revoke.revision(), u64::MAX);

        let mut absent = GrantSnapshot::none().with_revision(u64::MAX);
        assert_eq!(absent.try_revoke("absent"), Ok(false));
    }

    #[test]
    fn catalog_rejects_unknown_names_but_is_configurable() {
        let builtin = CapabilityCatalog::builtin();
        assert!(builtin.contains(capability_names::RUN_SCRIPT));
        assert!(!builtin.contains("vendor.secret"));
        assert!(!builtin.is_empty());

        let mut custom = CapabilityCatalog::empty();
        assert!(custom.is_empty());
        assert!(custom.insert("vendor.secret"));
        assert!(custom.contains("vendor.secret"));
        assert!(!custom.insert("   "));
        assert_eq!(custom.len(), 1);

        let from_names = CapabilityCatalog::from_names(["", " a ", "b"]);
        assert_eq!(from_names.len(), 2);
    }

    #[test]
    fn confirmation_defaults_to_unconfirmed() {
        assert_eq!(Confirmation::default(), Confirmation::Unconfirmed);
        assert_ne!(Confirmation::Confirmed, Confirmation::Unconfirmed);
    }
}
