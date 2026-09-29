//! WASM 插件框架宿主运行时。
//!
//! 分层（依赖方向自上而下）：
//! - [`registry`]：已装插件注册表 + 实例配置/凭据/调用注入面（`PluginManager`）
//! - [`install`]：装配准入九步（manifest→验签→sha→扫描→审批→编译→落位→注册→lockfile）
//! - [`runtime`]：fresh-store-per-call 执行器（fuel/epoch/信号量/超时双闸）
//! - [`host_impl`]：宿主能力面实现（log/now/config/secret/read/data-dir/http/tool-invoke）
//! - [`egress`]：deny-by-default 出站 HTTP 代理
//! - [`observer`]：观察者事件泵（fire-and-forget 脱敏投影投递）
//! - [`manifest`] / [`trust`]：manifest 解析校验 + 签名信任四态
//! - [`engine`]：wasmtime Engine 单例（fuel+epoch+编译缓存）
//!
//! 合同（WIT）权威源在仓库根 `wit/v0/plugin.wit`；本 crate `wit/` 为宿主
//! bindgen 副本，字节相等由 drift 测试钉死。

pub mod bindings;
pub mod egress;
pub mod engine;
pub mod error;
pub mod host_impl;
pub mod install;
pub mod limits;
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
