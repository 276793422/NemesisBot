//! 目标（goals）handler — `goals.list` / `goals.get` / `goals.create` /
//! `goals.update` / `goals.enable` / `goals.disable` / `goals.delete` /
//! `goals.evals`（目标系统，2026-11-17 计划）。
//!
//! 状态真相源 `<workspace>/config/goals.json` 注册表 + `goals/{id}.goal.md`
//! 目标文件（nemesis-goals store；gateway 侧评估闸引擎与 agent goal 工具
//! 同读同写）。本面是**人**的管理面（Dashboard「目标」页）：create/update/
//! enable/disable/delete——`paused` 仅人可置/解，`done` 唯一出口是评估闸
//! 判定（agent 无 done 能力，防自锁），blocked 解锁走 enable。
//!
//! estop 语义：人控面不在 estop 冻结范围（急停冻结 agent 活动与评估闸
//! 注入，人的管理操作是带外通道）。
//!
//! create 绑定校验：session_id（sanitize）或 session_key 原文二选一；
//! `session_occupied` 不占用 = 拒绝（不静默绑死会话）；同会话已有未完成
//! goal（非 done）= 拒绝（一对一绑定；done 旧条目保留作历史，新建不删）。

use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_goals::{
    GoalCreatedBy, GoalEntry, GoalStatus, alloc_id, load_registry, read_evals_tail, read_goal_md,
    remove_goal, update_registry, write_goal_md,
};

pub struct GoalsHandler;

/// workspace 根。
fn ws_of(ctx: &RequestContext) -> Result<std::path::PathBuf, String> {
    ctx.state
        .workspace
        .as_ref()
        .map(std::path::PathBuf::from)
        .ok_or("workspace not available".to_string())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// data 指定会话 → session_key（session_id sanitize 形态优先，session_key
/// 原文兜底；两者都缺 = Err）。loops.rs 同款公式。
fn resolve_session_key(data: &serde_json::Value) -> Result<String, String> {
    if let Some(sid) = data
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(format!(
            "agent:main:session:{}",
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

fn parse_id(data: &serde_json::Value) -> Result<String, String> {
    data.get("id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .ok_or("missing id".to_string())
}

/// 列表行视图。
fn summary_view(e: &GoalEntry) -> serde_json::Value {
    serde_json::json!({
        "id": e.id,
        "session_key": e.session_key,
        "status": e.status.as_str(),
        "created_by": e.created_by.as_str(),
        "max_rounds": e.max_rounds,
        "rounds_used": e.rounds_used,
        "eval_model": e.eval_model,
        "blocked_code": e.blocked_code,
        "blocked_message": e.blocked_message,
        "created_at": e.created_at_ms,
        "updated_at": e.updated_at_ms,
    })
}

/// 详情视图（含 goal.md 内容与评估台账尾部）。
fn detail_view(ws: &std::path::Path, e: &GoalEntry, goal_md: Option<String>) -> serde_json::Value {
    let evals = read_evals_tail(ws, &e.id, 100)
        .into_iter()
        .map(|r| {
            serde_json::json!({
                "at": r.at_ms,
                "round": r.round,
                "verdict": r.verdict,
                "gap": r.gap,
                "model": r.model,
                "duration_ms": r.duration_ms,
            })
        })
        .collect::<Vec<_>>();
    serde_json::json!({
        "id": e.id,
        "session_key": e.session_key,
        "status": e.status.as_str(),
        "created_by": e.created_by.as_str(),
        "max_rounds": e.max_rounds,
        "rounds_used": e.rounds_used,
        "eval_model": e.eval_model,
        "blocked_code": e.blocked_code,
        "blocked_message": e.blocked_message,
        "created_at": e.created_at_ms,
        "updated_at": e.updated_at_ms,
        "content": goal_md,
        "content_missing": goal_md.is_none(),
        "evals": evals,
    })
}

/// eval_model 入参解析（缺省/空串 = None 全局默认；"small" = 杂务槽）。
fn parse_eval_model(data: &serde_json::Value) -> Result<Option<String>, String> {
    match data.get("eval_model") {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => {
            let s = v
                .as_str()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .ok_or("eval_model must be a non-empty string")?;
            Ok(Some(s.to_string()))
        }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for GoalsHandler {
    fn module_name(&self) -> &str {
        "goals"
    }

    fn commands(&self) -> &'static [&'static str] {
        &[
            "list", "get", "create", "update", "enable", "disable", "delete", "evals",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let ws = ws_of(ctx)?;
        match cmd {
            "list" => {
                let entries = load_registry(&ws)?;
                Ok(Some(serde_json::json!({
                    "goals": entries.iter().map(summary_view).collect::<Vec<_>>(),
                    "total": entries.len(),
                })))
            }
            "get" => {
                let data = data.ok_or("missing data")?;
                let id = parse_id(&data)?;
                let entries = load_registry(&ws)?;
                let e = entries
                    .iter()
                    .find(|e| e.id == id)
                    .ok_or_else(|| format!("goal {id} 不存在"))?;
                let goal_md = read_goal_md(&ws, &id)?;
                Ok(Some(detail_view(&ws, e, goal_md)))
            }
            "create" => {
                let data = data.ok_or("missing data")?;
                let content = data
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or("missing content")?;
                let session_key = resolve_session_key(&data)?;
                let eval_model = parse_eval_model(&data)?;
                let max_rounds = match data.get("max_rounds") {
                    None | Some(serde_json::Value::Null) => None,
                    Some(v) => Some(v.as_u64().ok_or("max_rounds must be u64")?),
                };
                // 在场校验：不绑死会话（sessions.create 产 meta sidecar，
                // 未发言会话也算在场——session_occupied 双形态判定）。
                if !nemesis_agent::chat_log::session_occupied(&session_key) {
                    return Err(format!(
                        "会话 {session_key} 不存在（先建会话再绑定目标；不静默创建）"
                    ));
                }
                let ts = now_ms();
                let id = update_registry(&ws, |entries| {
                    // 一对一绑定：同会话已有未完成 goal = 拒（done 旧条目
                    // 保留作历史，新建不删——id 单调不复用）。
                    if let Some(existing) = entries
                        .iter()
                        .find(|e| e.session_key == session_key && e.status != GoalStatus::Done)
                    {
                        return Err(format!(
                            "会话已有未完成目标 {}（{}）；先 delete 或等评估闸判 done，done 的历史目标可直接新建替换",
                            existing.id,
                            existing.status.as_str()
                        ));
                    }
                    let id = alloc_id(&ws);
                    entries.push(GoalEntry::new(
                        id.clone(),
                        session_key.clone(),
                        GoalCreatedBy::Human,
                        max_rounds,
                        eval_model.clone(),
                        ts,
                    ));
                    Ok(id)
                })?;
                write_goal_md(&ws, &id, content)?;
                tracing::info!("[Web/WSAPI] goal {id} created for session {session_key}");
                Ok(Some(serde_json::json!({ "created": true, "id": id })))
            }
            "update" => {
                let data = data.ok_or("missing data")?;
                let id = parse_id(&data)?;
                let content = data
                    .get("content")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or("missing content")?;
                update_registry(&ws, |entries| {
                    let e = entries
                        .iter_mut()
                        .find(|e| e.id == id)
                        .ok_or_else(|| format!("goal {id} 不存在"))?;
                    if e.status == GoalStatus::Done {
                        return Err(format!("goal {id} 已 done（终态）；如需新目标请新建替换"));
                    }
                    e.updated_at_ms = now_ms();
                    Ok(())
                })?;
                write_goal_md(&ws, &id, content)?;
                tracing::info!("[Web/WSAPI] goal {id} content updated");
                Ok(Some(serde_json::json!({ "updated": true, "id": id })))
            }
            "enable" | "disable" => {
                let data = data.ok_or("missing data")?;
                let id = parse_id(&data)?;
                let target = if cmd == "enable" {
                    GoalStatus::Active
                } else {
                    GoalStatus::Paused
                };
                let status = update_registry(&ws, |entries| {
                    let e = entries
                        .iter_mut()
                        .find(|e| e.id == id)
                        .ok_or_else(|| format!("goal {id} 不存在"))?;
                    if !nemesis_goals::transition_allowed(e.status, target) {
                        return Err(format!(
                            "goal {id} 当前状态 {}，不允许转移到 {}（done 终态不可转出；blocked 解锁走 enable）",
                            e.status.as_str(),
                            target.as_str()
                        ));
                    }
                    e.status = target;
                    // 解锁时清停车注记（重新进入运行态）。
                    if target == GoalStatus::Active {
                        e.blocked_code = None;
                        e.blocked_message = None;
                    }
                    e.updated_at_ms = now_ms();
                    Ok(e.status.as_str())
                })?;
                tracing::info!("[Web/WSAPI] goal {id} {} -> {}", cmd, status);
                Ok(Some(
                    serde_json::json!({ cmd: true, "id": id, "status": status }),
                ))
            }
            "delete" => {
                let data = data.ok_or("missing data")?;
                let id = parse_id(&data)?;
                // 先校验在场（remove_goal 对注册表缺行 Err，此处提前给
                // 人读错误）。
                let exists = load_registry(&ws)?.iter().any(|e| e.id == id);
                if !exists {
                    return Err(format!("goal {id} 不存在"));
                }
                remove_goal(&ws, &id)?;
                tracing::info!("[Web/WSAPI] goal {id} removed");
                Ok(Some(serde_json::json!({ "removed": true, "id": id })))
            }
            "evals" => {
                let data = data.ok_or("missing data")?;
                let id = parse_id(&data)?;
                let exists = load_registry(&ws)?.iter().any(|e| e.id == id);
                if !exists {
                    return Err(format!("goal {id} 不存在"));
                }
                let limit = data
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize)
                    .unwrap_or(100)
                    .min(1000);
                let evals = read_evals_tail(&ws, &id, limit)
                    .into_iter()
                    .map(|r| {
                        serde_json::json!({
                            "at": r.at_ms,
                            "round": r.round,
                            "verdict": r.verdict,
                            "gap": r.gap,
                            "model": r.model,
                            "duration_ms": r.duration_ms,
                            "raw_excerpt": r.raw_excerpt,
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(Some(serde_json::json!({ "id": id, "evals": evals })))
            }
            _ => Err(format!("unknown command: goals.{}", cmd)),
        }
    }
}

#[cfg(all(test, feature = "goals"))]
mod tests;
