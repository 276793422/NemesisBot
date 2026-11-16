//! Loop 状态对象 schema（`<workspace>/loops/{id}.json`）。
//!
//! 计划钦定形态：goal / status / plan / progress_log / next_wake /
//! budget / spent。`paused`/`parked` 仅人置（WSAPI/CLI）；agent 工具面
//! 只有 wakeup（排程）/status（读）/finish（收尾）——防自锁。

use serde::{Deserialize, Serialize};

/// 进度条目上限（progress_log 环形截断——超限丢最旧，防无限增长）。
/// 尾部可见性由 [`progress_tail`] 消费（唤醒消息注入最近 3 条）。
pub const PROGRESS_LOG_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoopStatus {
    /// 运行中：驱动器按 next_wake 唤醒。
    Active,
    /// 人为暂停（CLI/WSAPI；agent 无此能力——防自锁）。
    Paused,
    /// 停车场：预算耗尽 / 目标会话缺席时驱动器自动置（诚实停车留痕）；
    /// 人 resume 解除。
    Parked,
    /// agent `loop_finish` 收尾。
    Done,
    /// 预留（P1 无生产写入点）。
    Failed,
}

impl LoopStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            LoopStatus::Active => "active",
            LoopStatus::Paused => "paused",
            LoopStatus::Parked => "parked",
            LoopStatus::Done => "done",
            LoopStatus::Failed => "failed",
        }
    }

    /// 终态（done/failed）——wakeup 拒绝、驱动器不再唤醒。
    pub fn is_terminal(&self) -> bool {
        matches!(self, LoopStatus::Done | LoopStatus::Failed)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoopBudget {
    /// 唤醒次数上限（每次驱动器投递 spent.turns +1）。0 = 不限。
    #[serde(default)]
    pub max_turns_total: u64,
    /// 墙钟预算秒（started_at 起算，跨重启）。0 = 不限。
    #[serde(default)]
    pub wall_clock_budget_secs: u64,
    /// token 上限（usage 台账喂入；0 = 不限；P2 接线）。
    #[serde(default)]
    pub max_tokens: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoopSpent {
    #[serde(default)]
    pub turns: u64,
    #[serde(default)]
    pub wall_clock_secs: u64,
    #[serde(default)]
    pub tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEntry {
    pub at_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopState {
    /// `L1`/`L2`…（[`store::alloc_id`] 分配，文件名 `{id}.json`）。
    pub id: String,
    /// 人设的目标（唤醒消息原文引用）。
    pub goal: String,
    /// 绑定会话键（`agent:main:session:{id}` 形态；唤醒注入目标；工具
    /// 按 session_key 反查绑定）。
    pub session_key: String,
    pub status: LoopStatus,
    /// agent 自维护的计划文本（loop_wakeup 可带 note 更新）。
    #[serde(default)]
    pub plan: String,
    /// 追加式进度日志（环形截断 [`PROGRESS_LOG_CAP`]；唤醒消息带尾部）。
    #[serde(default)]
    pub progress_log: Vec<ProgressEntry>,
    /// 下次唤醒 epoch ms（None = 未排程；驱动器扫到到期即投）。
    #[serde(default)]
    pub next_wake_ms: Option<i64>,
    /// 周期 cron 表达式（Some 时每次唤醒后重算 next_wake；None = 一次性
    /// 排程，唤醒后 agent 须再 loop_wakeup 才有下次——自配速核心）。
    #[serde(default)]
    pub cron: Option<String>,
    pub budget: LoopBudget,
    pub spent: LoopSpent,
    pub started_at_ms: i64,
    pub updated_at_ms: i64,
    /// token 基线（loop 创建时会话已消耗的完整 token 数——driver 按
    /// usage 台账现值 - 基线填 spent.tokens，只算 loop 期间消耗）。
    #[serde(default)]
    pub tokens_base: u64,
    /// loop_finish 收尾总结。
    #[serde(default)]
    pub finish_summary: Option<String>,
}

impl LoopState {
    pub fn new(id: String, goal: String, session_key: String, now_ms: i64) -> Self {
        Self {
            id,
            goal,
            session_key,
            status: LoopStatus::Active,
            plan: String::new(),
            progress_log: Vec::new(),
            next_wake_ms: None,
            cron: None,
            budget: LoopBudget::default(),
            spent: LoopSpent::default(),
            started_at_ms: now_ms,
            updated_at_ms: now_ms,
            tokens_base: 0,
            finish_summary: None,
        }
    }

    /// 追加进度条目（环形截断，丢最旧）。调用方负责落盘。
    pub fn push_progress(&mut self, now_ms: i64, text: impl Into<String>) {
        self.progress_log.push(ProgressEntry {
            at_ms: now_ms,
            text: text.into(),
        });
        if self.progress_log.len() > PROGRESS_LOG_CAP {
            let drop = self.progress_log.len() - PROGRESS_LOG_CAP;
            self.progress_log.drain(..drop);
        }
        self.updated_at_ms = now_ms;
    }

    /// 尾部最近 N 条（原序——旧→新，与完整日志同向；唤醒消息/状态视图
    /// 按时间线自然阅读）。
    pub fn progress_tail(&self, n: usize) -> Vec<&ProgressEntry> {
        let start = self.progress_log.len().saturating_sub(n);
        self.progress_log[start..].iter().collect()
    }
}
