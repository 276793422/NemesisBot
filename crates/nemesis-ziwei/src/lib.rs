//! nemesis-ziwei —— 紫微系统门面 crate（总控 §4.1/§4.2）。
//!
//! **窄门面纪律（决议 8①）**：bot 其余模块只准依赖本 crate，禁止直依赖
//! `nemesis-ziwei-profile` / `nemesis-ziwei-rev`——保内部重构自由。
//!
//! 模块规划（按里程碑落地）：
//! - `estop_hook`：急停 → 全会话卸载的回调槽（T0.7；M1 实装卸载序列）
//! - `session`：SessionManager 状态机（Injecting/Handshaking/Ready/Busy/
//!   Unloading/Failed）+ attach 编排 + detach 卸载序列（T1.3）
//! - `dispatch`：PrimitiveDispatcher（read_mem/call_fn/hook_capture 路由 +
//!   结构解码）（T1.4/T4.3）
//! - `poll`：PollScheduler bot 侧轮询调度（DLL 保持无状态）（T2.1）
//!   ——落地为 `scheduler`（含报警事件汇与 `attach_and_schedule` 一站式入口）
//! - `notify`：报警条件 DSL 求值 + bus outbound 出站（T2.3）
//!   ——DSL 求值在本 crate；bus 出站在 gateway 侧（本 crate 不依赖 bus）
//! - `capture`：使用期数据回流落盘（T2.4；7 天 TTL + 容量上限）
//! - `registry`：动态工具注册（profile.tools[] → 注册表注册/注销）（T2.2
//!   ——经 nemesisbot `ziwei_tools` 桥接 agent 注册表，依赖方向所限）
//! - `bridge`：ziwei_bridge 元工具 + setup 流编排（T3.8）
//!
//! ## estop 联动（§9.4）
//!
//! gateway 启动时订阅 `EstopState`（nemesis-agent），engaged 上升沿调用
//! [`estop_hook::run_estop_unload`]：停 poll → 停 hook → 远线程 FreeLibrary
//! → 校验，全程审计。M0 槽在位、无会话时为 no-op。

pub mod estop_hook;
/// 配置期引擎（M3）：SetupManager 六步向导 + rev 分析面转发
/// （bot/web 禁止直依赖 nemesis-ziwei-rev，窄门面纪律决议 8①）。
pub mod setup;
/// profile 数据层转发（同上：bot/web 禁止直依赖 nemesis-ziwei-profile）。
pub mod profile {
    pub use nemesis_ziwei_profile::*;
}
/// rev 分析打包面转发（T3.6：build_prompt/parse_annotations/LlmFn hook；
/// bot 禁止直依赖 nemesis-ziwei-rev，窄门面纪律决议 8①）。
pub mod package {
    pub use nemesis_ziwei_rev::package::*;
}

/// capture 回流存储（T2.4；平台中立）。
pub mod capture;
/// 帧协议 JSON/解码面（平台中立——Linux nightly 全量测试可跑）。
pub mod dispatch;
/// notify 报警条件 DSL（T2.3；平台中立）。
pub mod notify;
/// payload DLL 落盘与版本戳（T1.6）。
pub mod payload_file;
/// 运行时锚扫描（M4/T4.2 支撑：byte_pattern 活体内存扫描 / rva 直算）。
pub mod resolve;
/// M4 运行期调用统一实现（call_fn / hook_capture——工具桥与 WSAPI 同源）。
pub mod runtime;
/// poll 调度器（T2.1/T2.3；bot 侧轮询 + 报警限频 + capture 回流）。
pub mod scheduler;
/// 会话管理器（T1.3；windows 实装 / 非 windows 诚实 stub）。
pub mod session;
/// 附加验证链 + 失效通知 + 授权找回（T5.1/T5.2/T5.3；平台中立核心 +
/// windows VERSIONINFO 抽取）。
pub mod verify;
/// payload 内嵌字节（build.rs 编译期快照；缺席 = 零字节 + 诚实运行期报错）。
mod embed {
    #![allow(dead_code)] // 单架构缺席/未消费时生成常量为空置
    include!(concat!(env!("OUT_DIR"), "/embed.rs"));
}
/// 具名管道 server + 帧协议（T1.5；依赖 tokio windows 扩展——windows-only）。
#[cfg(windows)]
pub mod pipe;

#[cfg(test)]
mod tests;
