//! Plugin system for extending NemesisBot.

pub mod host_services;
pub mod plugin;
pub mod tool_wrapper;
// ④：plugin-vm 显示通道 vtable（C-ABI 解冻首例 + 双向版本闸）。
pub mod vm_display;
// QUIC 漫游插件契约（C-ABI 解冻第二例：新式交互插件样板，分组函数表 + 双向回调）。
pub mod roaming;
// 附加资源插件契约（C-ABI 解冻第三例：纯数据资源载体，无 vtable 无回调）。
pub mod resources;
// 邮件基础设施插件契约（C-ABI 解冻第四例：SMTP/POP3 服务端 + 客户端发信 +
// maildir 分库存储 + 地址注册制；agent 面可发可收可读不可删）。
pub mod mail;
pub mod wrapper;

pub use host_services::{HOST_SERVICES_VERSION, HostServices, build_host_services};
pub use mail::{HOST_MAIL_ABI_VERSION, HostMailVTable, MailPluginHandle};
pub use plugin::{BasePlugin, Plugin, PluginManager, ToolInvocation};
pub use resources::{
    HOST_RESOURCES_ABI_VERSION, PrsAbiVersionFn, PrsFreeFn, PrsGetFn, PrsStrFn, ResourceEntry,
    ResourcesManifest, ResourcesPluginHandle,
};
pub use roaming::{HOST_ROAMING_ABI_VERSION, HostRoamingVTable, RoamingPluginHandle};
pub use tool_wrapper::{PluginableTool, ToolExecutor, ToolWrapper};
pub use vm_display::{HOST_VM_DISPLAY_ABI_VERSION, HostVmDisplayVTable, PluginVmInfo};
pub use wrapper::PluginWrapper;
