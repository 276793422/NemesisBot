//! Plugin system for extending NemesisBot.

pub mod host_services;
pub mod plugin;
pub mod tool_wrapper;
pub mod wrapper;

pub use host_services::{HOST_SERVICES_VERSION, HostServices, build_host_services};
pub use plugin::{BasePlugin, Plugin, PluginManager, ToolInvocation};
pub use tool_wrapper::{PluginableTool, ToolExecutor, ToolWrapper};
pub use wrapper::PluginWrapper;
