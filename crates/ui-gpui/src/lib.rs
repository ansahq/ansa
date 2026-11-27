//! The Ansa GPUI desktop shell.
//!
//! Phase 0 splits the former single-file Launcher into owned surfaces:
//!
//! * `shell` — window lifecycle, focus, appearance and the root render tree.
//! * `input` — text changes, IME gating and emoji mode.
//! * `navigation` — selection, actions, Escape and Tab movement.
//! * `dispatch` — command-to-handler routing and execution.
//! * `views` — one module per future feature surface.

use ansa_core::{
    Engine,
    action_panel::{ActionEntry, ActionId, ActionPanel, PanelTarget},
    query::{QueryRoute, route_query},
};
use ansa_domain::{BuiltinCommand, Command, CommandId, CommandKind};
use ansa_platform_api::{AppEntry, LaunchOutcome, Platform};
use ansa_ui_components::{Changed, EditRejected, TextInput, Theme, install_keybindings};
#[cfg(feature = "upstream-candidate")]
use gpui::Role;
use gpui::{
    App, Application, Bounds, ClipboardItem, Context, Entity, FocusHandle, Focusable, KeyBinding,
    ScrollStrategy, SharedString, Subscription, UniformListScrollHandle, Window,
    WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions, actions, div, prelude::*,
    px, size, uniform_list,
};
use std::path::PathBuf;

use launcher_view::{Caption, control, empty_state, icon_control};
use visual::{Icon, Palette, icon, keys};

#[cfg(any(not(windows), test))]
mod catalog_dispatch;
mod compat;
mod daily;
mod dispatch;
mod input;
mod launcher_view;
mod navigation;
mod shell;
mod views;
mod visual;

pub(crate) use input::copy_status;
pub(crate) use navigation::action_slug;
pub(crate) use shell::Launcher;
pub use shell::{run, run_with_application};

#[cfg(feature = "upstream-candidate")]
mod live_status;
#[cfg(not(windows))]
mod native_host;

actions!(
    ansa,
    [
        Next,
        Previous,
        PageDown,
        PageUp,
        Execute,
        Actions,
        Escape,
        Quit,
        CopyTarget,
        TabNext,
        TabPrevious,
        SetupActivate,
        ToggleGlass,
        ToggleTheme
    ]
);

#[cfg(debug_assertions)]
mod smoke;
