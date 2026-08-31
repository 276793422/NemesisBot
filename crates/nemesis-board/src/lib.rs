//! Managed agent board — issue/comment/project data model + SQLite store.
//!
//! Manager 侧单写者权威看板（本地 `board.db`，无复制、无共识）。两级
//! manager+worker 模型里的"看板状态"层：worker 对看板无状态，只管执行 +
//! report；看板读写都发生在 manager 进程内（gateway 注入 [`BoardStore`]）。
//!
//! 设计文档：`docs/PLAN/2026-07-26_managed-agent-board-integration.md`
//! （架构定稿）+ `docs/PLAN/2026-07-26_managed-agent-board-integration_开发计划.md`
//! （P1-P4 任务分解）。crate 自包含 SQLite（不与 nemesis-data 的 usage.db 混），
//! 遵循 `nemesis-data/src/db.rs` 的 WAL + `user_version` 迁移模式。

pub mod assignment;
pub mod db;
pub mod models;
pub mod service;
pub mod state_machine;
pub mod store;

pub use assignment::{Actor, AssignmentType};
pub use models::{
    ActivityLog, Attachment, Autopilot, AutopilotPatch, Comment, CommentType, Issue, IssueFilter,
    IssuePatch, IssueStatus, NewAutopilot, NewComment, NewIssue, NewNotification, Notification,
    Project, ProjectPatch, TaskOrigin, notification_kind,
};
pub use service::BoardService;
pub use state_machine::{can_transition, validate_transition};
pub use store::BoardStore;
