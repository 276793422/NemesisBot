//! Tool registration: default tools for the agent loop executor.
//!
//! Provides `register_default_tools()` which creates and returns a HashMap
//! of built-in tools (message, read_file, write_file, list_dir,
//! edit_file, append_file, delete_file, create_dir, delete_dir, sleep)
//! that can be registered with an `AgentLoopExecutor`.
//!
//! Also provides additional tools:
//! - Web search (Brave, DuckDuckGo, Perplexity)
//! - Web fetch
//! - Cluster RPC
//! - Spawn (sub-agent management)
//! - Memory tools
//! - Skills tools

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use nemesis_path::paths::canonicalize_for_compare;

use async_trait::async_trait;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, info};

use crate::background_registry::{
    BackgroundKillTool, BackgroundOutputTool, BackgroundProcessRegistry, BackgroundStartTool,
    BackgroundSteerTool,
};
use crate::context::RequestContext;
use crate::r#loop::{FileChange, FileChangeKind, McpToolSnapshotEntry, OperationOverride, Tool};
// ===========================================================================
// Basic file/message tools
// ===========================================================================

// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：按工具族分文件。纯代码搬家，行为零变更；
// 各子模块 pub 项经 pub use 保持原 loop_tools:: 路径不变。
// ---------------------------------------------------------------------------
mod cluster_rpc;
mod exec;
mod fs;
mod interactive;
mod mcp;
mod memory_skills;
mod messaging;
mod misc;
mod registry;
mod search_code;
mod spawn;
mod web;
mod workflow;

pub use cluster_rpc::*;
pub use exec::*;
pub use fs::*;
pub use interactive::*;
pub use mcp::*;
pub use memory_skills::*;
pub use messaging::*;
pub use misc::*;
pub use registry::*;
pub use search_code::*;
pub use spawn::*;
pub use web::*;
#[cfg(feature = "workflow")]
pub use workflow::*;

/// A1/A2 (2026-09-04): edit_file 失败修复指令 + unified diff 辅助。
pub(crate) mod edit_hint;

/// A4 (2026-09-04): edit_file 五级模糊替换级联。
pub(crate) mod edit_replacers;

pub mod claude_code_tool;
pub mod cli_delegation;
pub mod codex_tool;
#[cfg(feature = "lsp")]
pub mod lsp_tool;

#[cfg(test)]
mod coverage_boost_tests;
// D1 (2026-09-04): GitTool 写操作（add/commit/branch_create/checkout/restore/stash）
// 枚举白名单 + push/reset 不可达 + 参数走私拒绝测试。
#[cfg(test)]
mod git_tool_tests;
// J2a (2026-09-04): web_fetch 重定向循环 + SSRF 逐跳复查测试。
#[cfg(test)]
mod exec_timeout_tests;
#[cfg(test)]
mod loop_tools_extra_tests;
#[cfg(test)]
mod skill_manage_tests;
// BUG-20261101-295：install_skill 工具 WS4 漏斗接线回归（lockfile/origin
// 记账凭证 + P13 门晚填槽时序 + slug/exists/force 语义）。
#[cfg(test)]
mod install_skill_funnel_tests;
// CronTool schema↔impl 一致性回归（2026-10-19）：schema 形态调用是
// 一等公民，死参（command/deliver）不再声明，schedule 字符串仅作回退。
#[cfg(test)]
mod cron_tool_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod web_fetch_tests;

// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
#[cfg(test)]
mod s9c_tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9d_tests;
// A5 (2026-09-04): WorkspaceBoundary 校验矩阵 + write/edit/append 带界形态
// + register_shared_tools 注入接线测试。
#[cfg(test)]
mod workspace_boundary_tests;
// H1 (2026-09-05): todowrite 全量提交语义 + 原子落盘 + TodoUpdated 广播 +
// 安全管线放行 + tier 供给测试。
#[cfg(test)]
mod todowrite_tests;

// C8 (2026-09-06): run_checks 生态探测/命令映射/统计解析/失败聚焦纯函数层 +
// 真实 cargo fixture 端到端（成功 build / 故意编译失败回灌 error 聚焦 + 存档）。
#[cfg(test)]
mod run_checks_tests;

// F7 (2026-09-06): question 工具——注册门控（有 broker 才注册）/ args 校验
// / 回灌格式（选中项 / 超时按最佳判断继续）/ 槽未装配诚实报错 / id 自增。
#[cfg(test)]
mod question_tool_tests;

// N2 (2026-10-13 机会项): present_plan 计划审批——注册门控 / schema /
// 批准翻转 build+事件 / 驳回与超时落 verdict / 无 broker 诚实报错 /
// build 模式拒绝 / plan 必填。
#[cfg(test)]
mod present_plan_tests;

// N5 (2026-10-13 机会项): present_files 交付物登记——注册门控 / schema /
// 解析（title 缺省文件名）/ 存在性 all-or-nothing / 相对路径锚定工作区 /
// session_key 分桶 / 重复登记去重 / 校验路径。
#[cfg(test)]
mod present_files_tests;

// A7 (2026-09-06): multiedit 批量编辑——args 解析 / 原子性（一败全不落盘）
// / 同文件累积 / 级联与 replace_all / preview_all 多文件预检 / 边界拦截。
#[cfg(test)]
mod multiedit_tests;

// A8 (2026-09-07): read_file 二进制/PDF 分支——非 UTF-8 按 magic byte 分类
// 诚实摘要（png vision 提示 / pdf 诚实说明 / exe·无 magic 报字节数），
// 文本路径字节不变。
#[cfg(test)]
mod read_binary_tests;

// Wave4 覆盖率补充批次：exec 判定原语 / magic 分派 / run_one_stage /
// run_checks 解析助手 / WebSearch 无 key 早退（零网络）。
#[cfg(test)]
mod cov_more_tests;

// Wave6B 覆盖率收尾批次：checks_commands 全生态 / Default 实现 / 空 ctx 回退
// / 级联两臂 / 文件锁错误注入 / SkillManage 审批 Err+JoinError / 工作流创建
// 决策表 / register_shared_tools 各注册臂。
#[cfg(all(test, feature = "lsp"))]
mod cov_wave6_tests;
