//! Platform-neutral host lifecycle contract for Ansa.
//!
//! This crate owns invocation identity, show/hide/focus outcomes, monitor
//! selection and orderly shutdown state. It contains no native calls; platform
//! adapters implement the traits here and the UI owns the dispatcher thread.
//!
//! The central type is [`HostController`]. It keeps exactly one active
//! invocation, records one [`FocusOutcome`] for every show path, re-places the
//! launcher when its monitor disappears and releases resources in a fixed order
//! on shutdown. Native window handles are not part of this crate's API; the
//! [`MainThreadOnly`] wrapper and [`MainThreadDispatcher`] token model the
//! requirement that they never travel to arbitrary asynchronous work.

#![forbid(unsafe_code)]
#![deny(missing_debug_implementations)]

mod backend;
mod controller;
mod dispatcher;
mod focus;
mod geometry;
mod invocation;
mod monitor;

pub use backend::{
    HostBackend, HostError, RELEASE_PLAN, ReleaseFailure, ReleaseStep, ShutdownReport,
    ThemeAppearance,
};
pub use controller::{
    HideReport, HostController, LifecycleState, MonitorChange, ShowReport, ShowRequest,
    ThemeObservation,
};
pub use dispatcher::{
    DispatchError, MainThreadCall, MainThreadDispatcher, MainThreadOnly, MainThreadToken,
};
pub use focus::{FocusOutcome, FocusOutcomeCounts, ShowOutcome};
pub use geometry::{Point, Rect, Size};
pub use invocation::{InvocationId, InvocationIssuer};
pub use monitor::{
    MonitorId, MonitorInfo, MonitorSelection, MonitorSelectionReason, place_on_monitor,
    select_monitor,
};
