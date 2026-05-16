//! Plugin system for extending NemesisBot.

pub mod host_services;
pub mod plugin;
pub mod tool_wrapper;
pub mod wrapper;

pub use host_services::{HostServices, build_host_services, HOST_SERVICES_VERSION};
pub use plugin::{Plugin, BasePlugin, PluginManager, ToolInvocation};
pub use wrapper::PluginWrapper;
pub use tool_wrapper::{ToolExecutor, ToolWrapper, PluginableTool};
