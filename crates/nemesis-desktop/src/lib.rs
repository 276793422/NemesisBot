//! NemesisBot - Desktop Integration
//!
//! System tray management, icon handling, subprocess management,
//! WebSocket communication, and desktop UI abstractions.

pub mod icons;
pub mod process;
pub mod websocket;
pub mod windows;
pub mod child_mode;

#[cfg(not(target_os = "android"))]
pub mod systray;

#[cfg(not(target_os = "android"))]
pub use systray::PlatformTray;

#[cfg(not(target_os = "android"))]
pub use systray::{enable_cluster_menu_items, disable_cluster_menu_items};

pub use child_mode::{has_child_mode_flag, run_child_mode};
