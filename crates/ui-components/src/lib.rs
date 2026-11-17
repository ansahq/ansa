//! Owned GPUI building blocks for the Ansa launcher.
//!
//! The crate is intentionally small: widgets hold their own state, emit typed
//! events and depend only on the pinned GPUI version. Themes are data so a
//! later appearance layer can swap them without changing widget code.

pub mod text_input;
pub mod theme;

pub use text_input::Changed;
#[cfg(feature = "upstream-candidate")]
pub use text_input::DEFAULT_ACCESSIBILITY_LABEL;
pub use text_input::{DEFAULT_PLACEHOLDER, TextInput, install_keybindings};
pub use text_input::{
    EditRejected, EditRejection, MAX_HISTORY_BYTES, MAX_HISTORY_ENTRIES, MAX_TEXT_SCALARS,
};
pub use theme::Theme;
