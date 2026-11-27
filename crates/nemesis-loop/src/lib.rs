//! NemesisBot Loop Module —— 自配速 durable loop（质变计划 项5）。
//!
//! 「任务从跑完即退变 agent 自己规划节奏、跨重启持续自驱」。
//!
//! 结构（三件）：
//! - [`state`]：`<workspace>/loops/{id}.json` 状态对象 schema（goal/status/
//!   plan/progress_log/next_wake/budget/spent）；
//! - [`store`]：状态文件原子读写 + id 分配（扫描 `L<n>` 形态取 max+1）；
//! - [`wake`]：唤醒计算（delay_secs | cron）+ 预算判定 + sweep 纯逻辑
//!   （驱动器的全部裁决，可单测）。
//!
//! 解耦纪律：本 crate 不依赖 bus / estop / agent——驱动器在 gateway 侧
//! （post_agent.rs，项1 注入面同款装配点）：sweep 纯函数裁决 → 宿主闭包
//! 注入（`session_occupied` 在场检查 + bus publish_inbound）。estop 冻结
//! 经 sweep 入参 `estop: bool` 表达（冻结期零动作，wake 留到期，release
//! 后下一个 sweep 补投）。
//!
//! 状态机（单人拍板语义）：`active|paused|parked|done|failed`。
//! `paused`/`parked` 只能由**人**经 WSAPI/CLI 置/解——agent 工具面不含
//! 暂停/恢复（防自锁死循环，计划钦定）。`parked` 由驱动器在预算耗尽或
//! 目标会话缺席时自动置（诚实停车留痕，非静默）。`done` 由 agent
//! `loop_finish` 置。`failed` 预留（P1 无生产写入点）。
//!
//! 预算三维（board.budget 先例，0=不限）：turns（唤醒次数）/ 墙钟
//! （started_at 起算）/ tokens（usage 台账，宿主喂入）。耗尽 → parked。

pub mod resources;
pub mod state;
pub mod store;
pub mod wake;

#[cfg(test)]
mod resources_tests;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod wake_tests;

pub use resources::DurableLoopsResourceProvider;
pub use state::{
    cap_text, LoopBudget, LoopSpent, LoopState, LoopStatus, ProgressEntry, MAX_TEXT_CHARS,
    PROGRESS_LOG_CAP,
};
pub use store::{
    alloc_id, create_loop, list_loops, load_loop, loops_dir, remove_loop, save_loop, update_loop,
};
pub use wake::{
    budget_exhausted, compute_next_wake, format_wake_time, sweep_loop, SweepOutcome, WAKE_MARKER,
};
