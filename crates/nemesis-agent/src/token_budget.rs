//! P8 会话 token 预算闸（集群无人值守硬化 A2）——纯类型 + 会话账本 + 文案单一真相源。
//!
//! 背景：看板重派全部收敛到同一 worker 会话（`cluster_rpc:{source}/board:{n}`）
//! 且每次带全量旧历史重入，既有守卫全部是单请求维度——唯一累计维度闸
//! `board.budget.max_tokens_per_parent` 默认关。本模块提供 per-session
//! 累计账本与用户文案；loop 侧接线（顶检器官/每轮记账）在
//! `loop/token_budget.rs`，重派入口拒接与 fail_class 分类在 nemesisbot
//! cluster_agent，转人工决策在 board_review。
//!
//! config 键 `agents.defaults.session_token_budget`（u64）：**0 = 关（默认，
//! 不改现网行为）**。注意与 `max_tool_iterations` 的「0 = 不限」惯例相反——
//! 本键一族（含 `board.budget.*`）0 = 关，文档与提示词一律写死。
//! 计费口径 = 完整输入（prompt + cache_creation + cache_read，R2 2026-10-22
//! 补全——loop 侧 `loop/token_budget.rs::usage_full_input` 单一真相源）+
//! output，与 `DataStore::aggregate_session_usage` 账本 SQL 同源——累计器
//! 与底账两边公式一致才不会互相打架。

use std::collections::HashMap;

/// 硬停/拒接文案的单一真相源前缀：worker `classify_terminal_failure`
/// 按它识别 `fail_class = token_budget`，board_review 据此转人工不重派。
pub const SESSION_TOKEN_BUDGET_MARKER: &str = "💰 会话 token 预算耗尽";

/// soft 档触发线（占预算百分比）：一次性提示 + 触发一次压缩减负 prefill。
pub(crate) const SESSION_BUDGET_SOFT_RATIO: u64 = 75;

/// 账本条目上限（会话数）。超限全清（同 compact_state
/// `SESSION_STATE_MAX_ENTRIES` 512 的既有手法——预算检查靠 lazy seed
/// 自愈，清账只损失一次 DB 查询）。
const SESSION_BUDGET_MAX_ENTRIES: usize = 512;

/// loop 侧硬停文案（≥100%）：本轮 break 后经 `AgentEvent::Error` 出站，
/// worker `classify_terminal_failure` 按前缀识别为 `token_budget`。
pub fn hard_stop_notice(used: u64, budget: u64) -> String {
    format!(
        "{SESSION_TOKEN_BUDGET_MARKER}（{used}/{budget}）：本会话累计 token \
         用量已达预算上限，已停止处理。已完成的工作已保存。"
    )
}

/// D3 重派入口拒接文案（任务未执行、零 token 消耗；早退于
/// restore_session_history 之前）。
pub fn entry_reject_notice(used: u64, budget: u64) -> String {
    format!(
        "{SESSION_TOKEN_BUDGET_MARKER}（{used}/{budget}）：该会话累计 token \
         用量已达预算上限，任务未执行。请人工裁决（调大 \
         agents.defaults.session_token_budget 或新建会话）。"
    )
}

/// 单会话预算账目：累计 token + soft 档一次性提示闩。
#[derive(Debug, Default)]
struct SessionBudgetEntry {
    total_tokens: u64,
    /// soft ≥75% 提示已发过（一次性，不 nag——仿 `CompactState.soft_noticed`）。
    soft_noticed: bool,
}

/// per-session 累计账本（AgentLoop 字段，仿 `compact_state` 的
/// `Mutex<HashMap>` 形态；无 Arc 外壳——无跨任务借出需求）。
///
/// **seed 防双计**：账目缺失时从 `aggregate_session_usage` 底账种入。
/// 两条路径的时序差异由调用点保证自洽：
/// - `record`（每轮 `record_round_usage` 末尾，`insert_request_log` 之后）：
///   底账已含本轮 → 种入值直接作 total，不再叠加 delta；
/// - `current`（每轮顶检，先于本轮任何 LLM 调用与记账）：底账只含此前
///   轮次 → 种入值即已用（本轮尚未发生）。
///
/// 之后两侧都走内存增量，与 DB 查询解耦。
#[derive(Default)]
pub(crate) struct SessionTokenBudgetLedger {
    map: std::sync::Mutex<HashMap<String, SessionBudgetEntry>>,
}

/// 底账种入值：`Some(total)` = DB 聚合成功（完整输入 = prompt +
/// cache_creation + cache_read，R2 2026-10-22 口径）；`None`
/// = 无 data store 或查询失败（调用方回落内存增量）。
fn seed_total(ds: Option<&nemesis_data::DataStore>, session_key: &str) -> Option<u64> {
    let agg = ds?.aggregate_session_usage(session_key).ok()?;
    let input = u64::try_from(agg.input_tokens.max(0)).unwrap_or(u64::MAX);
    let output = u64::try_from(agg.output_tokens.max(0)).unwrap_or(u64::MAX);
    Some(input.saturating_add(output))
}

impl SessionTokenBudgetLedger {
    /// 每轮记账（预算关闭时调用方已短路，此处不再读 config）。
    /// 账目存在 → `total += delta`；缺失 → 种入底账（已含当轮，见类型注释）。
    pub(crate) fn record(
        &self,
        ds: Option<&nemesis_data::DataStore>,
        session_key: &str,
        delta: u64,
    ) {
        let mut map = self.map.lock().expect("token budget ledger poisoned");
        if let Some(e) = map.get_mut(session_key) {
            e.total_tokens = e.total_tokens.saturating_add(delta);
            return;
        }
        let total = seed_total(ds, session_key).unwrap_or(delta);
        Self::insert_capped(&mut map, session_key, total);
    }

    /// 当前累计（顶检/观测面）。账目缺失 → 种入底账（防 loop 重启丢账）。
    pub(crate) fn current(&self, ds: Option<&nemesis_data::DataStore>, session_key: &str) -> u64 {
        let mut map = self.map.lock().expect("token budget ledger poisoned");
        if let Some(e) = map.get(session_key) {
            return e.total_tokens;
        }
        let total = seed_total(ds, session_key).unwrap_or(0);
        Self::insert_capped(&mut map, session_key, total);
        total
    }

    /// soft 档一次性闩：返回 true = 本会话首次越过 soft 线（调用方发提示
    /// + 触发压缩）；false = 已提示过。
    pub(crate) fn soft_latch(&self, session_key: &str) -> bool {
        let mut map = self.map.lock().expect("token budget ledger poisoned");
        let e = map.entry(session_key.to_string()).or_default();
        let first = !e.soft_noticed;
        e.soft_noticed = true;
        first
    }

    fn insert_capped(map: &mut HashMap<String, SessionBudgetEntry>, session_key: &str, total: u64) {
        if map.len() > SESSION_BUDGET_MAX_ENTRIES {
            map.clear();
        }
        map.insert(
            session_key.to_string(),
            SessionBudgetEntry {
                total_tokens: total,
                soft_noticed: false,
            },
        );
    }
}
