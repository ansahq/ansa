//! Renderer-independent first-run onboarding for Ansa.
//!
//! The crate models the Welcome → Shortcut → TryLaunch → Integrations → Done
//! journey without GPUI, platform calls, accounts or network access:
//!
//! * [`Onboarding`] owns progress, skip warnings and persistence.
//! * [`Shortcut`] normalizes chords; [`ShortcutProbe`] reports whether the host
//!   can actually register one.
//! * [`LaunchProbe`] performs a real catalog launch and reports
//!   [`LaunchOutcome`]; onboarding records only a confirmed launch.
//! * [`PermissionRequest`] names the feature, data touched and denial
//!   consequence, and exists only after the user enables that feature.
//! * [`CapabilityNotice`] keeps unsupported and degraded states visible.
//! * [`OnboardingStore`] has in-memory and atomic JSON-file implementations.
//!
//! A fresh profile can reach a verified application launch with no account and
//! no optional permissions. Every optional step can be skipped, but a skip is
//! persisted as a warning rather than a fabricated success.
//!
//! ```
//! use ansa_domain::CommandId;
//! use ansa_onboarding::{
//!     LaunchOutcome, LaunchProbe, MemoryStore, Onboarding, ProbeOutcome, Shortcut,
//!     ShortcutProbe,
//! };
//!
//! struct Available;
//! impl ShortcutProbe for Available {
//!     fn probe(&self, _shortcut: &Shortcut) -> ProbeOutcome {
//!         ProbeOutcome::Available
//!     }
//! }
//!
//! struct Launched;
//! impl LaunchProbe for Launched {
//!     fn launch(&self, _command_id: &CommandId) -> LaunchOutcome {
//!         LaunchOutcome::Launched
//!     }
//! }
//!
//! let mut onboarding = Onboarding::load(MemoryStore::new());
//! onboarding.next().unwrap();
//! onboarding
//!     .select_shortcut(&Shortcut::documented_default(), &Available)
//!     .unwrap();
//! onboarding.next().unwrap();
//! onboarding
//!     .try_launch(&CommandId::parse("app:demo").unwrap(), &Launched)
//!     .unwrap();
//! onboarding.next().unwrap();
//! onboarding.next().unwrap();
//! onboarding.complete().unwrap();
//! assert!(onboarding.status().is_completed());
//! ```

#![forbid(unsafe_code)]

mod capability;
mod launch;
mod model;
mod permissions;
mod shortcut;
mod store;

pub use capability::{CapabilityNotice, CapabilityStatus, render_notices};
pub use launch::{LaunchOutcome, LaunchProbe, LaunchVerification};
pub use model::{FlowError, Onboarding, OnboardingStatus, STATE_VERSION, Step};
pub use permissions::{
    IntegrationChoice, IntegrationFeature, PermissionDecision, PermissionRequest,
};
pub use shortcut::{
    Key, Modifier, ProbeOutcome, Shortcut, ShortcutChoice, ShortcutParseError, ShortcutProbe,
    ShortcutVerification,
};
pub use store::{JsonFileStore, MemoryStore, OnboardingStore, StoreError};
