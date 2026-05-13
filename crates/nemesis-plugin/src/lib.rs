//! Plugin system for extending NemesisBot.

pub mod plugin;
pub mod tool_wrapper;
pub mod wrapper;

pub use plugin::{Plugin, BasePlugin, PluginManager, ToolInvocation};
pub use wrapper::PluginWrapper;
pub use tool_wrapper::{ToolExecutor, ToolWrapper, PluginableTool};
