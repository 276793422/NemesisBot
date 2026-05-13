//! Window management module - Desktop window abstractions.
//!
//! Provides WindowBase, ApprovalWindow, DashboardWindow, and HeadlessWindow
//! for managing different window types in desktop mode.

pub mod window_base;
pub mod approval;
pub mod dashboard;
pub mod headless;

pub use window_base::{WindowData, WindowBase};
pub use approval::{ApprovalWindowData, ApprovalWindow};
pub use dashboard::{DashboardWindowData, DashboardWindow};
pub use headless::run_headless_window;
