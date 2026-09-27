//! nemesis-tools 内嵌子代理 loop 的 system prompt（字节保留的历史文案）。
//!
//! 说明：agent 侧的 spawn 子代理人格体系已升级为 [`crate::subagents`] 的
//! 六角色模板（SubagentRole，经 `DetachedOpts.role` 注入）；本模块的两条
//! 单句是 nemesis-tools 自身 toolloop 子代理（spawn.rs / subagent.rs 工具
//! 实现）仍在消费的独立路径，迁入本 crate 仅做集中化，**文本逐字节保留**
//! （含英文原文），升级需连同消费方回归测试一起评审。

/// spawn.rs 工具内嵌 toolloop 的 system prompt。
pub const SPAWN_SUBAGENT_SYSTEM_PROMPT: &str = "You are a subagent. Complete the given task independently and report the result.\n\
     You have access to tools - use them as needed to complete your task.\n\
     After completing the task, provide a clear summary of what was done.";

/// subagent.rs 工具内嵌 toolloop 的 system prompt。
pub const SUBAGENT_TOOL_SYSTEM_PROMPT: &str = "You are a subagent. Complete the given task independently and provide a clear, concise result.";
