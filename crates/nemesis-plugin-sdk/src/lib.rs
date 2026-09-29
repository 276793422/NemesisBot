//! NemesisBot WASM 插件三方开发 SDK（guest 侧）。
//!
//! # 工具插件最小样例
//!
//! ```ignore
//! // src/lib.rs（工程根须有 wit/ 目录 = 本 SDK wit/plugin.wit 的同版本副本）
//! use nemesis::plugin::host;
//!
//! struct Hello;
//!
//! impl exports::nemesis::plugin::tool::Guest for Hello {
//!     fn get_metadata() -> Result<exports::nemesis::plugin::tool::ToolMetadata, host::HostError> {
//!         Ok(exports::nemesis::plugin::tool::ToolMetadata {
//!             name: "hello".into(),
//!             title: "Hello".into(),
//!             description: "Greets someone.".into(),
//!             parameters_json: r#"{"type":"object","properties":{"who":{"type":"string"}}}"#.into(),
//!             operation_type: "read".into(),
//!         })
//!     }
//!     fn execute(input: exports::nemesis::plugin::tool::ToolInput)
//!         -> Result<exports::nemesis::plugin::tool::ToolOutput, host::HostError> {
//!         host::log(host::LogLevel::Info, "hello called");
//!         Ok(exports::nemesis::plugin::tool::ToolOutput {
//!             content: format!("hi, {}", host::config_get("who").ok().flatten().unwrap_or_default()),
//!             is_error: false,
//!         })
//!     }
//! }
//!
//! nemesis_plugin_sdk::export_tool!(Hello);
//! ```
//!
//! `export_tool!` 展开 = 以工程 `wit/` 为合同生成绑定 → `export!($ty)`；
//! 生成类型短名可在工程内 `use exports::nemesis::plugin::tool::*`。
//! 观察者插件同理用 [`export_observer!`]（实现
//! `exports::nemesis::plugin::observer::Guest::observe`）。
//!
//! # 诚实边界（v0 合同）
//!
//! - guest 无任意文件系统/网络访问：唯一 IO 面 = 宿主能力面（[`host`]）；
//!   持久化写 [`host::data_dir_path`] 返回的 WASI 挂载点（仅本插件可见）。
//! - 观察者收到的事件为宿主脱敏投影：v1 一律结构化元数据、无内容体。
//! - workspace-write 能力**不在 v0 合同**（挂账记录于实施计划 §十二；
//!   guest 需要写工作区时走 [`host::tool_invoke`] 调宿主写类工具，照常过
//!   宿主安全 8 层与审批）。

#![deny(missing_docs)]

/// 工具插件一键导出（crate 根调用；工程须有 `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-tool` world 绑定（`exports::nemesis::plugin::tool`
/// 与 `nemesis::plugin::host` 导入模块），随后 `export!($ty)` 收尾。
/// 片段用 `ident`：宏链逐层转发，`ty` 片段无法再次匹配内层 `ident`。
/// `runtime_path` 钉到本 crate：生成代码的运行时钩子解析到 SDK 再导出，
/// 插件工程**无需直接依赖 wit-bindgen**。
#[macro_export]
macro_rules! export_tool {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-tool",
            runtime_path: "nemesis_plugin_sdk",
        });
        ::nemesis_plugin_sdk::__export_impl!($ty);
    };
}

/// 观察者插件一键导出（crate 根调用；工程须有 `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-observer` world 绑定 → `export!($ty)`。
/// `runtime_path` 语义同 [`export_tool!`]。
#[macro_export]
macro_rules! export_observer {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-observer",
            runtime_path: "nemesis_plugin_sdk",
        });
        ::nemesis_plugin_sdk::__export_impl!($ty);
    };
}

/// generate!/export! 的顺序衔接（`export!` 宏由 generate! 展开产物定义到
/// 调用 crate 根——必须在其后展开）；不对外稳定。片段全程 `ident`（宏链
/// 逐层转发，`ty` 片段无法再匹配内层 `ident`）。
#[doc(hidden)]
#[macro_export]
macro_rules! __export_impl {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::__export_local!($ty);
    };
}

/// 透传调用 generate! 注入本 crate 的局部 `export!` 宏；不对外稳定。
/// 片段必须 `ident`：`ty` 捕获的片段无法再匹配 wit-bindgen `export!` 的
/// `ident` 规则（宏链全程 ident）。
#[doc(hidden)]
#[macro_export]
macro_rules! __export_local {
    ($ty:ident) => {
        export!($ty);
    };
}

#[cfg(feature = "json")]
pub mod event;
pub mod host;

/// wit-bindgen 的再导出（插件工程无需直接依赖 wit-bindgen）。
///
/// [`export_tool!`] 不够用时的手动通路：
/// `nemesis_plugin_sdk::generate!({ path: "wit", world: "plugin-tool" })`。
pub use wit_bindgen::generate;

/// 生成代码的运行时钩子再导出（`runtime_path` 钉到本 crate——生成代码里的
/// `nemesis_plugin_sdk::run_ctors_once` / `::maybe_link_cabi_realloc` /
/// `::Cleanup` 解析到这里）。仅导出当前合同实际引用的钩子；WIT 演进引入
/// flags/map/async 时需按 wit-bindgen 的路径约定补再导出（`bitflags` /
/// `Map` / `WitMap` / `async_support`）。钩子在 wit-bindgen 侧是
/// wasm32-only，宿主目标编译本 SDK 时随之隐藏（guest 组件只编 wasm）。
#[cfg(target_arch = "wasm32")]
pub use wit_bindgen::rt::{Cleanup, maybe_link_cabi_realloc, run_ctors_once};

#[cfg(feature = "json")]
pub use event::{ObserverEvent, parse_event};

/// 工具插件需要实现的 Guest trait 路径（生成于插件 crate）。
///
/// `exports::nemesis::plugin::tool::Guest`（`get_metadata` / `execute`）。
/// 记录类型同模块：`ToolMetadata` / `ToolInput`（含 `ToolContext`）/
/// `ToolOutput`；宿主错误 `crate::host::HostError`。
pub mod guide {
    /// 工具插件 Guest trait（生成后位于插件 crate）。
    pub const TOOL_GUEST: &str = "exports::nemesis::plugin::tool::Guest";
    /// 观察者插件 Guest trait（生成后位于插件 crate）。
    pub const OBSERVER_GUEST: &str = "exports::nemesis::plugin::observer::Guest";
}

pub mod types {
    //! 生成记录类型的**文档镜像**（真实定义由 bindgen 在插件 crate 内生成，
    //! 字段一一对应；此模块仅为 rustdoc 可发现性，不含运行时代码）。

    /// `tool-metadata`：`name` / `title` / `description` / `parameters_json`
    /// / `operation_type`（生成名 `ToolMetadata`）。
    ///
    /// `operation_type` 取值：`read` / `write` / `exec` / `network` / 空
    /// （宿主对空声明从紧处置：按未注册名默认 CRITICAL 走全闸）。
    pub struct ToolMetadataDoc;

    /// `tool-input`：`args_json` + `context`（`workspace_root` /
    /// `session_key` / `call_id`）（生成名 `ToolInput` / `ToolContext`）。
    pub struct ToolInputDoc;

    /// `tool-output`：`content` / `is_error`（生成名 `ToolOutput`；
    /// `is_error=true` = 业务失败回灌 LLM 自纠，非 trap）。
    pub struct ToolOutputDoc;
}
