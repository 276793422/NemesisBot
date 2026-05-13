//! NemesisBot - Desktop Integration
//!
//! System tray management, icon handling, subprocess management,
//! WebSocket communication, and desktop UI abstractions.

pub mod icons;
pub mod systray;
pub mod process;
pub mod websocket;
pub mod windows;
pub mod child_mode;

pub use child_mode::{has_child_mode_flag, run_child_mode};

pub use systray::PlatformTray;
