//! 自配速 durable loop handler — `loops.list` / `loops.get` / `loops.create` /
//! `loops.update` / `loops.pause` / `loops.resume` / `loops.remove`
//! （项5，2026-11-16 质变计划）。
//!
//! 状态真相源 `<workspace>/loops/{id}.json`（nemesis-loop store；gateway
//! 驱动器与 agent 三工具同读同写）。本面是**人**的管理面：create/update/
//! pause/resume/remove——`paused`/`parked` 仅人可置/解（agent 工具面无此
//! 能力，防自锁），resume 是停车场预算调整后的解除入口（配合 update 改
//! budget——update 的 budget 按**字段合并**：缺席 = 保留现值，显式 0 =
//! 刻意设不限；create 的 budget 缺席字段 = default 0 = 不限）。
//!
//! estop 语义：人控面不在 estop 冻结范围（急停冻结 agent 活动与注入，
//! 人的管理操作是带外通道）；驱动器/工具侧的冻结在各自入口判定。
//!
//! create 绑定校验：session_id（sanitize）或 session_key 原文二选一；
//! `session_occupied`（jsonl ∥ meta）不占用 = 拒绝——不静默绑死会话。
//! token 基线：max_tokens 显式配置且 usage 台账在场时记录创建时刻会话
//! 已耗 token（spent.tokens = 现值 − 基线，只算 loop 期间）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::cc_hooks::WEB_SESSION_KEY_PREFIX;
use nemesis_loop::{
    LoopBudget, LoopState, LoopStatus, list_loops, load_loop, loops_dir, update_loop,
};

pub struct LoopsHandler;

/// workspace loops 目录。
fn dir_of(ctx: &RequestContext) -> Result<std::path::PathBuf, String> {
    ctx.state
        .workspace
        .as_ref()
        .map(|ws| loops_dir(std::path::Path::new(ws)))
        .ok_or("workspace not available".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// data 指定会话 → session_key（session_id sanitize 形态优先，session_key
/// 原文兜底；两者都缺 = Err）。
fn resolve_session_key(data: &serde_json::Value) -> Result<String, String> {
    if let Some(sid) = data
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(format!(
            "{}{}",
            WEB_SESSION_KEY_PREFIX,
            nemesis_agent::session::SessionStore::sanitize_session_id(sid)
        ));
    }
    if let Some(key) = data
        .get("session_key")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(key.to_string());
    }
    Err("missing session_id (or session_key)".to_string())
}

/// 预算解析（create 面：部分字段可给，缺席 = serde default 0 = 不限——
/// create 的合理缺省，此时还没有「现值」可言）。
fn parse_budget(data: &serde_json::Value) -> Result<LoopBudget, String> {
    let b = data
        .get("budget")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    if b.is_null() {
        return Ok(LoopBudget::default());
    }
    serde_json::from_value(b).map_err(|e| format!("budget schema invalid: {e}"))
}

/// update 面的部分预算（BUG-20261127-020）：三字段全 Option——字段缺席 =
/// None = 保留现值，显式 0 = Some(0) = 刻意设为不限。不可用 LoopBudget
/// 承载：其 `#[serde(default)]` 会把「字段缺席」与「显式 0」都反序列化成
/// 0，配整体替换语义时部分形态（如 LoopsView widenBudget 只发
/// max_turns_total）把未提及维度的预算静默清成 0=不限——保险丝静默拆除。
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
struct PartialBudget {
    max_turns_total: Option<u64>,
    wall_clock_budget_secs: Option<u64>,
    max_tokens: Option<u64>,
}

/// update 面 budget 解析：按字段合并语义。`budget` 整体缺席或显式 null =
/// 不动预算；显式清零用三字段全 0 表达。
fn parse_budget_partial(data: &serde_json::Value) -> Result<Option<PartialBudget>, String> {
    let Some(b) = data.get("budget") else {
        return Ok(None);
    };
    if b.is_null() {
        return Ok(None);
    }
    serde_json::from_value(b.clone())
        .map(Some)
        .map_err(|e| format!("budget schema invalid: {e}"))
}

/// 摘要视图（list 行）。
fn summary_view(st: &LoopState) -> serde_json::Value {
    serde_json::json!({
        "id": st.id,
        "goal": st.goal,
        "status": st.status.as_str(),
        "session_key": st.session_key,
        "next_wake": st.next_wake_ms.map(nemesis_loop::format_wake_time),
        "cron": st.cron,
        "budget": st.budget,
        "spent": st.spent,
        "progress_count": st.progress_log.len(),
        "finish_summary": st.finish_summary,
    })
}

#[async_trait::async_trait]
impl ModuleHandler for LoopsHandler {
    fn module_name(&self) -> &str {
        "loops"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list", "get", "create", "update", "pause", "resume", "remove",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let dir = dir_of(ctx)?;
        match cmd {
            "list" => {
                let all = list_loops(&dir);
                Ok(Some(serde_json::json!({
                    "loops": all.iter().map(summary_view).collect::<Vec<_>>(),
                    "total": all.len(),
                })))
            }
            "get" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing id")?;
                let st = load_loop(&dir, id)?.ok_or_else(|| format!("loop {id} 不存在"))?;
                Ok(Some(serde_json::json!({
                    "id": st.id,
                    "goal": st.goal,
                    "status": st.status.as_str(),
                    "session_key": st.session_key,
                    "plan": st.plan,
                    "progress_log": st.progress_log.iter().map(|e| serde_json::json!({
                        "at": nemesis_loop::format_wake_time(e.at_ms),
                        "text": e.text,
                    })).collect::<Vec<_>>(),
                    "next_wake": st.next_wake_ms.map(nemesis_loop::format_wake_time),
                    "cron": st.cron,
                    "budget": st.budget,
                    "spent": st.spent,
                    "started_at": nemesis_loop::format_wake_time(st.started_at_ms),
                    "finish_summary": st.finish_summary,
                })))
            }
            "create" => {
                let data = data.ok_or("missing data")?;
                let goal = data
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or("missing goal")?;
                let session_key = resolve_session_key(&data)?;
                // 在场校验：不绑死会话（sessions.create 产 meta sidecar，
                // 未发言会话也算在场——session_occupied 双形态判定）。
                if !nemesis_agent::chat_log::session_occupied(&session_key) {
                    return Err(format!(
                        "会话 {session_key} 不存在（先建会话再绑定 loop；不静默创建）"
                    ));
                }
                let budget = parse_budget(&data)?;
                let ts = now_ms();
                // 创建串行化（BUG-20261119-186）：alloc+save 两步收进
                // create_loop 进程级锁——裸 alloc 后在 save 前有并发创建者
                // 扫目录会拿到同一 max+1 → 同 id 互覆（Dashboard 双击连发
                // 即可触发，goals store REGISTRY_LOCK 同款先例）。
                let st = nemesis_loop::create_loop(&dir, |id| {
                    let mut st =
                        LoopState::new(id.clone(), goal.to_string(), session_key.clone(), ts);
                    st.budget = budget;
                    // 初始排程（可选，2026-11-17 会话资源盘点看板 B3「+ 设置目标」
                    // 内联表单前置）：`delay_secs`（一次性）/ `cron`（周期表达式）
                    // 二选一，互斥校验与换算在 nemesis-loop::compute_next_wake
                    // 单一真相源（CLI loops create 同款形态）；都不给 = 等 agent
                    // 自配速（旧行为不变）。
                    {
                        let delay_secs = data
                            .get("delay_secs")
                            .and_then(|v| v.as_u64())
                            .filter(|d| *d > 0);
                        let cron_expr = data
                            .get("cron")
                            .and_then(|v| v.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty());
                        if delay_secs.is_some() || cron_expr.is_some() {
                            let (wake, cron_set) =
                                nemesis_loop::compute_next_wake(ts, delay_secs, cron_expr)
                                    .map_err(|e| format!("排程无效：{e}"))?;
                            st.next_wake_ms = wake;
                            st.cron = cron_set;
                        }
                    }
                    // token 基线：显式配了 max_tokens 且台账在场 → 记创建时刻
                    // 已耗 token（spent.tokens = 现值 − 基线）。
                    if st.budget.max_tokens > 0
                        && let Some(ref ds) = ctx.state.data_store
                        && let Ok(agg) = ds.aggregate_session_usage(&session_key)
                    {
                        st.tokens_base =
                            (agg.input_tokens.max(0) + agg.output_tokens.max(0)) as u64;
                    }
                    st.push_progress(ts, format!("[create] 目标：{goal}（人经 WSAPI 创建）"));
                    Ok(st)
                })?;
                let id = st.id.clone();
                tracing::info!(
                    "[Web/WSAPI] loop {} created for session {}",
                    id,
                    session_key
                );
                Ok(Some(serde_json::json!({ "created": true, "id": id })))
            }
            "update" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing id")?;
                // 字段提取与空检放锁外（提前失败不进临界区）；裁决与落盘
                // 在锁内以盘上最新状态重做（BUG-20261125-003 RMW 串行化——
                // 快照预检只作优化，并发下驱动器/agent 工具可能已改状态）。
                let new_goal = data
                    .get("goal")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from);
                let new_budget = parse_budget_partial(&data)?;
                let ts = now_ms();
                let st = update_loop(&dir, id, |st| {
                    // 存在性由 update_loop 的 load 先裁决（不存在 = Err）；
                    // 字段空检在其后（契约：不存在优先于无可更新字段）。
                    if new_goal.is_none() && new_budget.is_none() {
                        return Err("无可更新字段（goal / budget）".to_string());
                    }
                    if let Some(g) = &new_goal {
                        st.goal = g.clone();
                    }
                    if let Some(b) = new_budget {
                        // 按字段合并（BUG-20261127-020）：缺席 = 保留现值，
                        // 显式 0 = 刻意设不限——部分更新不再静默解除未提及
                        // 维度的预算保险丝。
                        if let Some(v) = b.max_turns_total {
                            st.budget.max_turns_total = v;
                        }
                        if let Some(v) = b.wall_clock_budget_secs {
                            st.budget.wall_clock_budget_secs = v;
                        }
                        if let Some(v) = b.max_tokens {
                            st.budget.max_tokens = v;
                        }
                    }
                    st.updated_at_ms = ts;
                    Ok(true)
                })?;
                Ok(Some(serde_json::json!({ "updated": true, "id": st.id })))
            }
            "pause" | "resume" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing id")?;
                let ts = now_ms();
                let st = update_loop(&dir, id, |st| {
                    // 状态机校验用锁内最新状态（预检快照可能与盘上漂移）。
                    if cmd == "pause" {
                        if st.status != LoopStatus::Active {
                            return Err(format!(
                                "loop {} 当前状态 {}，仅 active 可暂停",
                                st.id,
                                st.status.as_str()
                            ));
                        }
                        st.status = LoopStatus::Paused;
                        st.push_progress(ts, "[paused] 人经 WSAPI 暂停".to_string());
                    } else {
                        if st.status != LoopStatus::Paused && st.status != LoopStatus::Parked {
                            return Err(format!(
                                "loop {} 当前状态 {}，仅 paused/parked 可恢复",
                                st.id,
                                st.status.as_str()
                            ));
                        }
                        st.status = LoopStatus::Active;
                        st.push_progress(ts, "[resumed] 人经 WSAPI 恢复".to_string());
                    }
                    st.updated_at_ms = ts;
                    Ok(true)
                })?;
                Ok(Some(
                    serde_json::json!({ cmd: true, "id": st.id, "status": st.status.as_str() }),
                ))
            }
            "remove" => {
                let data = data.ok_or("missing data")?;
                let id = data
                    .get("id")
                    .and_then(|v| v.as_str())
                    .ok_or("missing id")?;
                let existed = nemesis_loop::remove_loop(&dir, id)?;
                if !existed {
                    return Err(format!("loop {id} 不存在"));
                }
                tracing::info!("[Web/WSAPI] loop {id} removed");
                Ok(Some(serde_json::json!({ "removed": true, "id": id })))
            }
            _ => Err(format!("unknown command: loops.{}", cmd)),
        }
    }
}

#[cfg(all(test, feature = "loop"))]
mod tests;
