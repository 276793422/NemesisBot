//! 子代理任务注册表 WSAPI handler（子代理可见性 /，2026-11-03）—
//! `subagents.list` / `subagents.detail` / `subagents.events` /
//! `subagents.cancel`。
//!
//! 三种子代理形态（内联 detached / `__BG_SPAWN__` 后台 / 集群 delegate
//! 并入）的统一台账读面 + 用户面终止。数据面单一真相源 =
//! `nemesis_agent::subagent_registry::global`（gateway 启动早期注入
//! workspace 落盘路径）。
//!
//! `events` = 详情抽屉的对话环回放：registry id == 子代理会话键
//! （spawn 侧 `session_key_override` 同键），泵侧 tool/RoundText 帧全
//! 按 `record_tool(session_key)` 落 `chat_event_log` 环——直接按 id
//! `replay_after`，不走 `chat.sync`（它硬编码 `agent:main:session:{sid}`
//! 键构，够不着 `subagent:{uuid}`）。响应带 status/running，前端据此
//! 停轮询。环是进程内存态（重启清空）= 诚实边界：重启后只剩注册表
//! 台账行（落盘 tasks.json），对话环不可回放。

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SubagentsHandler;

/// registry 行 → 前端投影（serde Serialize 直转 + 补充派生字段）。
fn task_json(t: &nemesis_agent::subagent_registry::SubagentTask) -> serde_json::Value {
    let mut v = serde_json::to_value(t).unwrap_or_else(|_| serde_json::json!({}));
    // 派生面：运行中的任务给前端「可终止」信号；终态任务给时长（ms）。
    v["running"] = serde_json::Value::Bool(!t.status.is_terminal());
    if let Some(f) = t.finished_at_ms {
        v["duration_ms"] = serde_json::json!(f.saturating_sub(t.started_at_ms));
    }
    v
}

#[async_trait::async_trait]
impl ModuleHandler for SubagentsHandler {
    fn module_name(&self) -> &str {
        "subagents"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "detail", "events", "cancel"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        _ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let reg = nemesis_agent::subagent_registry::global();
        match cmd {
            "list" => {
                let d = data.unwrap_or(serde_json::json!({}));
                let want = |k: &str| d.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                let status = want("status");
                let origin = want("origin");
                let loop_origin = want("loop_origin");
                let rows: Vec<serde_json::Value> = reg
                    .list()
                    .iter()
                    .filter(|t| {
                        status.is_none_or(|s| {
                            serde_json::to_value(t.status)
                                .ok()
                                .and_then(|v| v.as_str().map(str::to_string))
                                .as_deref()
                                == Some(s)
                        })
                    })
                    .filter(|t| origin.is_none_or(|s| t.origin == s))
                    .filter(|t| loop_origin.is_none_or(|s| t.loop_origin == s))
                    .map(task_json)
                    .collect();
                Ok(Some(serde_json::json!({ "tasks": rows })))
            }
            "detail" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "subagents.detail requires {id}".to_string())?;
                let t = reg.get(id).ok_or_else(|| format!("task not found: {id}"))?;
                Ok(Some(task_json(&t)))
            }
            "events" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "subagents.events requires {id}".to_string())?;
                let t = reg.get(id).ok_or_else(|| format!("task not found: {id}"))?;
                let after_seq = data
                    .as_ref()
                    .and_then(|d| d.get("after_seq"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let (events, gap) = crate::chat_event_log::replay_after(id, after_seq);
                Ok(Some(serde_json::json!({
                    "id": id,
                    "after_seq": after_seq,
                    "gap": gap,
                    "status": t.status,
                    "running": !t.status.is_terminal(),
                    "events": events,
                })))
            }
            "cancel" => {
                let id = data
                    .as_ref()
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "subagents.cancel requires {id}".to_string())?;
                if reg.cancel(id) {
                    Ok(Some(serde_json::json!({ "cancelled": true, "id": id })))
                } else {
                    Err(format!("task not found: {id}"))
                }
            }
            _ => Err(format!("unknown command: subagents.{}", cmd)),
        }
    }
}
