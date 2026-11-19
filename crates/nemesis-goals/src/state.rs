//! Goals 数据对象 schema。
//!
//! 两个落盘面：
//! - 注册表 `<workspace>/config/goals.json`（[`GoalEntry`] 数组，单一真相源：
//!   id → 会话绑定 / 状态 / 轮数 / 创建者 / 评估模型）；
//! - 评估台账 `goals/{id}.evals.jsonl`（[`EvalRecord`] 逐行 append，只增
//!   不改——逐轮判定 + 缺口注记留痕，设置页投影展示）。
//!
//! 目标内容本体 `goals/{id}.goal.md` 是评估的单一真相源（每次评估重读，
//! 人改目标下轮生效），schema 不收录其内容。

use serde::{Deserialize, Serialize};

/// 轮数预算默认值（dsh 同款；0 = 不限）。
pub const DEFAULT_MAX_ROUNDS: u64 = 256;

/// goal 状态四态（D-Q11）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalStatus {
    /// 运行中：绑定会话每个 turn 结束触发评估。
    Active,
    /// 人为关闭（可逆——人 enable 恢复）。评估闸不触发。
    Paused,
    /// 诚实停车（预算耗尽 `round-limit` / 评估失败 `eval-failure`）。
    /// 带 code + 人读 message；人 enable 可解锁。
    Blocked,
    /// 评估闸判定完成（唯一 done 出口，终态）。替换 = 新建 goal。
    Done,
}

impl GoalStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalStatus::Active => "active",
            GoalStatus::Paused => "paused",
            GoalStatus::Blocked => "blocked",
            GoalStatus::Done => "done",
        }
    }

    /// 终态（done）——评估闸不再触发，仅可新建 goal 替换。
    pub fn is_terminal(&self) -> bool {
        matches!(self, GoalStatus::Done)
    }
}

/// 创建者二分（文件保护规则的根基）：
/// - Human：goal.md 对 agent 文件写工具只读（改目标走 Dashboard）；
/// - Agent：完全自有（自由编辑/删除/关闭）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GoalCreatedBy {
    Human,
    Agent,
}

impl GoalCreatedBy {
    pub fn as_str(&self) -> &'static str {
        match self {
            GoalCreatedBy::Human => "human",
            GoalCreatedBy::Agent => "agent",
        }
    }
}

/// 注册表条目（`config/goals.json` 数组元素）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GoalEntry {
    /// `G1`/`G2`…（[`store::alloc_id`] 分配；单调不复用）。
    pub id: String,
    /// 绑定会话键（`agent:main:session:{id}` 全形态）——一对一绑定，
    /// 评估闸按此匹配 outbound。
    pub session_key: String,
    pub status: GoalStatus,
    pub created_by: GoalCreatedBy,
    /// 轮数预算（CONTINUE 注入轮上限；0 = 不限）。
    #[serde(default = "default_max_rounds")]
    pub max_rounds: u64,
    /// 已注入的 goal 轮数（人在场 turn 不计）。
    #[serde(default)]
    pub rounds_used: u64,
    /// 评估模型槽：None = 全局默认模型；Some("small") = `agents.small_model`
    /// 杂务槽（D-Q5）。
    #[serde(default)]
    pub eval_model: Option<String>,
    /// blocked 停车原因稳定 code（`round-limit` / `eval-failure`）。
    #[serde(default)]
    pub blocked_code: Option<String>,
    /// blocked 人读说明。
    #[serde(default)]
    pub blocked_message: Option<String>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

fn default_max_rounds() -> u64 {
    DEFAULT_MAX_ROUNDS
}

impl GoalEntry {
    pub fn new(
        id: String,
        session_key: String,
        created_by: GoalCreatedBy,
        max_rounds: Option<u64>,
        eval_model: Option<String>,
        now_ms: i64,
    ) -> Self {
        Self {
            id,
            session_key,
            status: GoalStatus::Active,
            created_by,
            max_rounds: max_rounds.unwrap_or(DEFAULT_MAX_ROUNDS),
            rounds_used: 0,
            eval_model,
            blocked_code: None,
            blocked_message: None,
            created_at_ms: now_ms,
            updated_at_ms: now_ms,
        }
    }
}

/// 评估判定（gate 解析产物）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvalVerdict {
    /// 目标达成——唯一 done 出口。
    Done,
    /// 未达成——缺口注记注入下一轮。
    Continue,
}

/// 评估台账记录（`goals/{id}.evals.jsonl` 逐行）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRecord {
    pub at_ms: i64,
    /// 触发时的轮数（rounds_used 现值；预算耗尽停车轮 = 耗尽前值）。
    pub round: u64,
    /// `done` / `continue` / `error`（解析失败留痕）。
    pub verdict: String,
    /// 缺口注记（CONTINUE 时评估器给出；error 时为空）。
    #[serde(default)]
    pub gap: Option<String>,
    /// 评估用模型显示名。
    pub model: String,
    pub duration_ms: u64,
    /// 原始输出摘录（截断封顶——解析失败时排查用）。
    #[serde(default)]
    pub raw_excerpt: String,
}
