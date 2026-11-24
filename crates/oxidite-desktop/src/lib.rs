//! Oxidite desktop terminal emulator library.

#![allow(dead_code, clippy::all)]

pub use oxidite_pty as pty;
pub use oxidite_theme as theme;
pub use oxidite_vt::{grid, handler, parser, snapshot, terminal};

pub mod app;
pub mod changes_panel;
pub mod config;
pub mod context_menu;
pub mod diff_view;
pub mod git;
pub mod global_search;
pub mod input;
pub mod pane;
pub mod renderer;
pub mod search;
pub mod session_state;
pub mod settings_modal;
pub mod shell;
pub mod sidebar;
pub mod split_picker;
pub mod status_bar;
pub mod tab;
pub mod url;
pub mod workspace;
