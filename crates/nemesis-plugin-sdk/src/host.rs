//! 宿主能力面（WIT `nemesis:plugin/host` 导入）的封装。
//!
//! **guest 插件请勿使用本模块实现 Guest trait**：Guest 生成 trait 的签名
//! 类型来自插件 crate 自身的 generate! 绑定（`nemesis::plugin::host`），
//! 与本模块的镜像类型是不同名义类型（Rust 类型系统不认跨 crate 同形）。
//! 正确写法见 crate 根文档示例：`use nemesis::plugin::host;`。
//!
//! 本模块的定位：SDK 自身绑定的独立组件场景（只调宿主、无导出的辅助
//! 组件）与 SDK 内部测试；底层绑定由 SDK crate 内的 bindgen 生成（组件级
//! 同名导入合并，与插件 crate 自身生成的导入互不冲突）；本模块提供稳定
//! 签名 + 文档。原始形态在 `crate::__host_wire`（`nemesis::plugin::host`）。

#![allow(clippy::module_name_repetitions)]

// SDK 自身的合同绑定（host 导入侧）。非 wasm 目标编译时 wit-bindgen 生成
// 会 panic 的桩——workspace `cargo check` 仍可通过（真实语义只在 wasm32-wasip2
// 组件内生效）。生成物不带文档注释，missing_docs 在此模块内豁免。
#[allow(missing_docs)]
mod wire {
    wit_bindgen::generate!({
        path: concat!(env!("CARGO_MANIFEST_DIR"), "/wit"),
        world: "plugin-tool",
    });
    pub use nemesis::plugin::host as raw;
}

pub use wire::raw::{HostError, HttpRequest, HttpResponse, LogLevel};

/// 结构化日志（宿主背压：队列满丢弃并计数，永不反压 guest）。
pub fn log(level: LogLevel, message: &str) {
    wire::raw::log(level, message);
}

/// 当前 Unix 毫秒时间。
#[must_use]
pub fn now_millis() -> u64 {
    wire::raw::now_millis()
}

/// 实例配置读取（manifest config-schema 声明的普通键；`Ok(None)` = 键未
/// 设置。x-secret 键走此通道返回 `HostError::PolicyDenied`——配置面与凭据
/// 面分离）。
pub fn config_get(key: &str) -> Result<Option<String>, HostError> {
    wire::raw::config_get(key)
}

/// 凭据读取（仅 manifest `x-secret` 声明过的名字；宿主经
/// `vault:plugin/<slug>/<name>` 别名取值）。
pub fn secret_get(name: &str) -> Result<String, HostError> {
    wire::raw::secret_get(name)
}

/// 工作区只读（相对路径锚定工作区根；越界/绝对路径/8.3 短名拒绝）。
pub fn workspace_read(rel_path: &str) -> Result<Vec<u8>, HostError> {
    wire::raw::workspace_read(rel_path)
}

/// 插件私有数据目录的 WASI 挂载点（仅本插件可读写；持久化写这里）。
pub fn data_dir_path() -> Result<String, HostError> {
    wire::raw::data_dir_path()
}

/// 出站 HTTP（deny-by-default：manifest egress 无 allowlist 即
/// `HostError::NoPermission`；重定向禁用、DNS 钉地址、响应体超限截断）。
pub fn http_send(req: HttpRequest) -> Result<HttpResponse, HostError> {
    wire::raw::http_send(&req)
}

/// 调用其它宿主工具（深度 1：`plugin.` 前缀目标拒绝——插件不可互相调用）。
pub fn tool_invoke(tool: &str, args_json: &str) -> Result<String, HostError> {
    wire::raw::tool_invoke(tool, args_json)
}
