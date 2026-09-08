//! L6++（2026-09-08）：项目注册表 + 项目常驻 AgentLoop（对话/项目双分组）。
//!
//! `registry` = `<workspace>/config/projects.json`（注册不拥有——删项目只
//! 解除分组，不删会话/文件）；`manager`（M2）= 项目 loop 生命周期 + 路由
//! 索引。设计/实施真相源：`docs/PLAN/2026-09-08_project-grouping-l6-impl-plan.md`。

pub mod manager;
pub mod registry;

#[cfg(test)]
mod routing_tests;
