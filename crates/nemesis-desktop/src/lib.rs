//! NemesisBot - Desktop Integration
//!
//! System tray management, icon handling, subprocess management,
//! WebSocket communication, and desktop UI abstractions.

pub mod child_mode;
pub mod icons;
pub mod process;
pub mod websocket;
pub mod windows;

#[cfg(not(target_os = "android"))]
pub mod systray;

#[cfg(not(target_os = "android"))]
pub use systray::PlatformTray;

#[cfg(not(target_os = "android"))]
pub use systray::{disable_cluster_menu_items, enable_cluster_menu_items};

// macOS main-thread tray handoff (see systray::main_thread_handoff docs).
#[cfg(target_os = "macos")]
pub use systray::main_thread_handoff;

pub use child_mode::{has_child_mode_flag, run_child_mode};
