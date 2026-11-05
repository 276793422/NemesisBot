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

/// 循环策略插件一键导出（2026-10-22 批 2；crate 根调用；工程须有
/// `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-loop-policy` world 绑定（`exports::nemesis::
/// plugin::loop_policy` 与 `nemesis::plugin::host` 导入模块）→
/// `export!($ty)`。`runtime_path` 语义同 [`export_tool!`]。
#[macro_export]
macro_rules! export_loop_policy {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-loop-policy",
            runtime_path: "nemesis_plugin_sdk",
        });
        ::nemesis_plugin_sdk::__export_impl!($ty);
    };
}

/// 面板插件一键导出（2026-10-31 面板扩展批；crate 根调用；工程须有
/// `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-panel` world 绑定（`exports::nemesis::plugin::ui`
/// 与 `nemesis::plugin::host` 导入模块）→ `export!($ty)`。
/// `runtime_path` 语义同 [`export_tool!`]。面板 guest 实现
/// `exports::nemesis::plugin::ui::Guest::handle_ui_command(input: String)
/// -> Result<String, host::HostError>`：入参投影
/// `{"command":...,"args":{...},"context":{"slug":...}}`，出参为回复 JSON
/// 串（约定对象；业务失败 `{"error":"..."}` 形态，不 trap）。
#[macro_export]
macro_rules! export_ui {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-panel",
            runtime_path: "nemesis_plugin_sdk",
        });
        ::nemesis_plugin_sdk::__export_impl!($ty);
    };
}

/// flow 步骤插件一键导出（2026-11-03 guest step 批；crate 根调用；工程须有
/// `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-flow-step` world 绑定
/// （`exports::nemesis::plugin::flow_step` 与 `nemesis::plugin::host` 导入
/// 模块）→ `export!($ty)`。`runtime_path` 语义同 [`export_tool!`]。
/// flow-step guest 实现两个出口：
/// - `get_step_metadata() -> Result<StepMetadata, host::HostError>`：
///   自报本插件可执行的步骤 op 词表（`kinds`，不含 `plugin:<slug>:` 前缀，
///   如 `["fetch_issues", "file_bug"]`；安装期对账 + 运行期路由事实源）。
/// - `run_step(input: String) -> Result<String, host::HostError>`：单
///   json-string 进出。入参投影
///   `{"op","args","instance":{...},"steps":{...},"board_issue","attempts",
///   "session_key"}`；出参为四态生命周期 JSON
///   `{"status":"done"|"running"|"failed"|"skipped", ...}`（其余字段 =
///   artifacts 自由载荷；幂等/重试责任在 guest，宿主只管投递与记账）。
#[macro_export]
macro_rules! export_step {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-flow-step",
            runtime_path: "nemesis_plugin_sdk",
        });
        ::nemesis_plugin_sdk::__export_impl!($ty);
    };
}

/// 组合面板+步骤插件一键导出（2026-11-03 组合 world 批；crate 根调用；
/// 工程须有 `wit/plugin.wit` 合同副本）。
///
/// 展开顺序：生成 `plugin-panel-flow-step` world 绑定（`exports::nemesis::
/// plugin::ui` 与 `exports::nemesis::plugin::flow_step` 及
/// `nemesis::plugin::host` 导入模块）→ `export!($ty)`。`runtime_path`
/// 语义同 [`export_tool!`]。组合 guest 必须同时实现两个 Guest trait：
/// `exports::nemesis::plugin::ui::Guest::handle_ui_command`（Panel 帧，
/// 有 secret-set / config-set 写面）与
/// `exports::nemesis::plugin::flow_step::Guest`（`get_step_metadata` /
/// `run_step`，FlowStep 帧——构造期即无写面，写通道只在 ui 帧授予）。
#[macro_export]
macro_rules! export_panel_step {
    ($ty:ident) => {
        ::nemesis_plugin_sdk::generate!({
            path: "wit",
            world: "plugin-panel-flow-step",
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
    /// 循环策略插件 Guest trait（生成后位于插件 crate）。
    pub const LOOP_POLICY_GUEST: &str = "exports::nemesis::plugin::loop_policy::Guest";
    /// 面板插件 Guest trait（生成后位于插件 crate）。
    pub const UI_GUEST: &str = "exports::nemesis::plugin::ui::Guest";
    /// flow 步骤插件 Guest trait（生成后位于插件 crate；2026-11-03 批）。
    pub const FLOW_STEP_GUEST: &str = "exports::nemesis::plugin::flow_step::Guest";
    /// 组合面板+步骤插件 Guest traits（生成后位于插件 crate；2026-11-03
    /// 组合 world 批；两个 trait 都要实现，缺一编译失败）。
    pub const PANEL_STEP_GUESTS: &str =
        "exports::nemesis::plugin::ui::Guest + exports::nemesis::plugin::flow_step::Guest";
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

    /// `policy-metadata`：`compact` / `final_judge` / `validation`（生成名
    /// `PolicyMetadata`；三 bool = 各自策略点接管清单，安装期对账 +
    /// 运行期路由唯一事实源；每策略点仅一个激活组件）。
    pub struct PolicyMetadataDoc;

    /// 循环策略三出口（单 json-string 进出；出参统一支持
    /// `{"verdict":"builtin"}` = 单次显式回退宿主内置逻辑）：
    ///
    /// - `compact-check`：`{"point":"trigger"|"exec", ...}`（入参投影见
    ///   WIT 合同注释；trigger 出 `compact|skip|builtin`，exec 出
    ///   `summary|skip|builtin`）。
    /// - `judge-final`：入 `{session_key, content, turns_used,
    ///   validation_failures}`；出 `accept | retry{nudge} | giveup{notice}
    ///   | builtin`。
    /// - `validate-args`：入 `{tool, schema, args}`；出
    ///   `valid | fixed{args} | reject{message} | builtin`。
    pub struct LoopPolicyDoc;

    /// 面板单出口（2026-10-31 面板扩展批）：`handle-ui-command`，单
    /// json-string 进出——入 `{"command":...,"args":{...},
    /// "context":{"slug":...}}`；出回复 JSON 串（约定对象；业务失败
    /// `{"error":"..."}` 形态，非 trap）。guest 可经 `host::tool_invoke`
    /// 执行动作（过宿主安全 8 层与审批）。
    pub struct PanelDoc;

    /// flow 步骤双出口（2026-11-03 guest step 批）：
    ///
    /// - `get-step-metadata`（无参）：自报 `kinds` 词表（本插件可执行的
    ///   步骤 op 名单，不含 `plugin:<slug>:` 前缀）——安装期对账 +
    ///   运行期路由事实源；插件声明了模板用到但词表外的 op = 安装拒绝
    ///   （MetadataMismatch）。
    /// - `run-step`（单 json-string 进出）：入参投影
    ///   `{"op","args","instance":{template,instance,ext_key,title,
    ///   origin_type,origin_id},"steps":{<id>:{status,error,attempts}},
    ///   "board_issue","attempts","session_key"}`（`args` 已完成
    ///   `{{token}}` 渲染；不含图片字节与凭据）。出参四态生命周期 JSON：
    ///   `{"status":"done"|"running"|"failed"|"skipped", ...}`——其余
    ///   字段 = artifacts 自由载荷（done/skipped 落盘、failed 取
    ///   `error` 字段文本走 on_fail、running 保持 Running 下轮重投）。
    ///   **幂等/重试责任在 guest**：宿主每 tick 重投（`attempts` 计数
    ///   供对账），guest 需凭自身数据目录状态去重已受理的操作。
    pub struct FlowStepDoc;
}
