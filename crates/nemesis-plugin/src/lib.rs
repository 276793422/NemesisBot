//! Plugin system for extending NemesisBot.

pub mod host_services;
pub mod plugin;
pub mod tool_wrapper;
// P5-R2 ④：plugin-vm 显示通道 vtable（C-ABI 解冻首例 + 双向版本闸）。
pub mod vm_display;
pub mod wrapper;

pub use host_services::{HOST_SERVICES_VERSION, HostServices, build_host_services};
pub use plugin::{BasePlugin, Plugin, PluginManager, ToolInvocation};
pub use tool_wrapper::{PluginableTool, ToolExecutor, ToolWrapper};
pub use vm_display::{HOST_VM_DISPLAY_ABI_VERSION, HostVmDisplayVTable, PluginVmInfo};
pub use wrapper::PluginWrapper;
