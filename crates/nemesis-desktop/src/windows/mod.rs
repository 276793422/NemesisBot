//! Window management module - Desktop window abstractions.
//!
//! Provides WindowBase, ApprovalWindow, DashboardWindow, and HeadlessWindow
//! for managing different window types in desktop mode.

pub mod approval;
pub mod dashboard;
pub mod headless;
pub mod window_base;

pub use approval::{ApprovalWindow, ApprovalWindowData};
pub use dashboard::{DashboardWindow, DashboardWindowData};
pub use headless::run_headless_window;
pub use window_base::{WindowBase, WindowData};
