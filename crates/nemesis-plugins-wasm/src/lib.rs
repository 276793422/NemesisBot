//! WASM 插件框架宿主运行时。
//!
//! 分层（依赖方向自上而下）：
//! - [`registry`]：已装插件注册表 + 实例配置/凭据/调用注入面（`PluginManager`）
//! - [`install`]：装配准入九步（manifest→验签→sha→扫描→审批→编译→落位→注册→lockfile）
//! - [`runtime`]：fresh-store-per-call 执行器（fuel/epoch/信号量/超时双闸）
//! - [`host_impl`] + `callbacks`：宿主能力面策略核心（log/now/config/secret/
//!   read/data-dir/http/tool-invoke；经 C-ABI 回调表分发给壳）
//! - [`egress`]：deny-by-default 出站 HTTP 代理
//! - [`observer`]：观察者事件泵（fire-and-forget 脱敏投影投递）
//! - [`manifest`] / [`trust`]：manifest 解析校验 + 签名信任四态
//! - `loader` + `abi`：宿主壳 DLL（`plugin_wasm.dll`，纯 wasmtime 通用装配
//!   壳）的装载与 C-ABI 合同（句柄 + 字节串 + 回调表；壳缺位 = 子系统诚实
//!   不可用，`PluginManager::new` 上抛 `NotAvailable`）
//!
//! 合同（WIT）权威源在本 crate `wit/plugin.wit`（合同由宿主定义）；SDK 绑定
//! 直接引用该目录，已入库示例的随包副本字节相等由 drift 测试钉死；合同
//! 变更经 build.rs rerun-if-changed 自动触发重编。

mod abi;
pub mod bindings;
mod callbacks;
pub mod egress;
pub mod error;
pub mod host_impl;
pub mod install;
pub mod limits;
mod loader;
pub mod manifest;
pub mod observer;
pub mod registry;
pub mod runtime;
pub mod trust;

pub use error::PluginError;
pub use manifest::{PluginKind, PluginManifest};
pub use registry::{InstanceConfigFile, PluginManager, RegisteredPlugin, ToolMetaSnapshot};
pub use trust::{PluginTrustState, VerificationOutcome};

/// 合同代际（与 WIT package 版本对应；manifest api-version 必须一致）。
pub const CONTRACT_API_VERSION: u32 = 1;

/// 宿主注册的工具名前缀（`plugin.<slug>.<name>`）。
pub const PLUGIN_TOOL_PREFIX: &str = "plugin.";

/// 拼接宿主注册工具全名。
#[must_use]
pub fn plugin_tool_full_name(slug: &str, base: &str) -> String {
    format!("{PLUGIN_TOOL_PREFIX}{slug}.{base}")
}
