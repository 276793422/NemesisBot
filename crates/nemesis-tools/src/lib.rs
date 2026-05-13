//! NemesisBot - Tool Framework
//!
//! Tool trait definitions, registry, executor, and tool implementations.

pub mod types;
pub mod registry;
pub mod executor;
pub mod message;
pub mod filesystem;
pub mod shell;
pub mod web;
pub mod cluster_rpc;
pub mod edit;
pub mod async_shell;
pub mod spawn;
pub mod cron;
pub mod sleep;
pub mod toolloop;
pub mod skills_ops;
pub mod browser;
pub mod hardware;
pub mod bootstrap;
pub mod desktop_automation;
pub mod screen_capture;
pub mod subagent;
