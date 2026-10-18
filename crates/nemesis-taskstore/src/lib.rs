//! 统一任务编排层（tasks.db 单库）。
//!
//! 承载 runs / attempts / approvals / deliveries 四表与统一状态机
//! （`pending → running → succeeded | failed | cancelled`），是持久
//! Agent 实体与统一任务编排（P1）的地基：
//!
//! - **集群任务真相源**：`SqliteTaskStore` 以 nemesis-types 的
//!   `Task`/`TaskStatus` 为载体落 runs 表，语义与
//!   `nemesis-cluster::task_manager::InMemoryTaskStore` 逐条对齐
//!   （错误文案、completed_at 盖章规则、条件更新先到先得）；
//!   `TaskStore` trait 的适配 impl 住在 trait 所有方 nemesis-cluster
//!   （orphan rule），本 crate 只暴露固有方法。
//! - **审批台账**：`SqliteApprovalJournal` 实现
//!   `nemesis_types::approval_journal::ApprovalJournal`——pending 落库
//!   幂等、裁决条件更新幂等（重启后未裁决审批可补裁且只生效一次）。
//! - **交付账本**：deliveries 表（幂等键 UNIQUE；P4 交付门泛化——拒绝
//!   语义 `reject_delivery`（rejected 终态带原因留痕）+ run 维/近期查询
//!   面，分支交付/评审结论/dossier 均入库）。
//! - **损坏熔断**：`TaskDb::open` 做完整性快检，损坏 = Err（调用方
//!   fail-closed 处置，ChatSecretStore poisoned 同款纪律）——不静默
//!   退回内存假装持久化。
//!
//! schema 纪律：`PRAGMA user_version` 顺序迁移，每步在
//! `BEGIN IMMEDIATE` 事务内（双连接并发 open 竞态安全）；P2/P4 增表
//! 走同一框架（追加迁移步 + 抬 LATEST_SCHEMA_VERSION），不改历史步。

pub mod agents;
pub mod approvals;
pub mod db;
pub mod deliveries;
pub mod ledger;
pub mod runs;
pub mod tasks;

pub use agents::{AgentRecord, SqliteAgentsStore};
pub use approvals::SqliteApprovalJournal;
pub use db::TaskDb;
pub use runs::{NewRun, RunRecord};
pub use tasks::SqliteTaskStore;
