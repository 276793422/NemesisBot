//! 集群专业职能框架（Profession Framework）提示词资产层。
//!
//! 六职能（产品经理/UI 设计/架构师/开发/白盒测试开发/黑盒测试）× 专业
//! 参数化（`dev:cpp` 形态）的企业级执行契约、planner 职能化拆解方法论、
//! B 端 worker 行为契约三件套。消费面：board 派发链（matcher 匹配 → B 端
//! 按任务职能渲染执行提示词）；spawn 会话内流程角色（[`crate::subagents`]）
//!
//! 与本模块正交的 spawn 会话内流程角色边界见集群专业职能框架计划文档。
//!
//! - [`meta`]：slug 语法（`family[:spec]`，精确匹配无继承）、内置目录、
//!   min_tier 元数据（tier 口径同 [`crate::subagents::role_tier`]）；
//! - [`render`]：任务职能后缀渲染（诚实注记三臂，无 I/O——用户档案由
//!   消费方从磁盘加载后传入）；
//! - [`CLUSTER_WORKER_CONTRACT`]：B 端稳定前缀的行为契约段（D5）；
//! - [`planner_method`]：planner 系统提示词的方法论段（经
//!   [`crate::board::planner_system_prompt`] 拼装）。

pub mod meta;
pub mod render;

#[cfg(test)]
mod tests;

/// B 端 worker 行为契约段（稳定前缀组成部分，D5：数据非指令注入防御 +
/// 汇报契约 + 工作区纪律）。受 `worker_discipline` 开关控制（缺省 on）。
pub const CLUSTER_WORKER_CONTRACT: &str = include_str!("cluster_worker.md");

/// planner 拆解方法论段（拼进 planner 系统提示词，D10 流水线模式）。
pub const PLANNER_METHOD: &str = include_str!("planner_method.md");
