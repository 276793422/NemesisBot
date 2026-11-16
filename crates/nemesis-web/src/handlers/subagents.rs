//! 子代理任务注册表 WSAPI handler（子代理可见性 /，2026-11-03）—
//! `subagents.list` / `subagents.detail` / `subagents.events` /
//! `subagents.cancel` / `subagents.archive` / `subagents.unarchive` /
//! `subagents.remove`（归档三动词，2026-11-15）/ `subagents.steer`
//! （飞行中驾驶，2026-11-16）。
//!
//! 三种子代理形态（内联 detached / `__BG_SPAWN__` 后台 / 集群 delegate
//! 并入）的统一台账读面 + 用户面终止 + 归档生命周期。数据面单一权威 =
//! `nemesis_agent::subagent_registry:global()`（gateway 启动早期注入
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
        &[
            "list",
            "detail",
            "events",
            "cancel",
            "archive",
            "unarchive",
            "remove",
            "steer",
        ]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let reg = nemesis_agent::subagent_registry::global();
        match cmd {
            "list" => {
                let d = data.unwrap_or(serde_json::json!({}));
                let want = |k: &str| d.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
                let status = want("status");
                let origin = want("origin");
                let loop_origin = want("loop_origin");
                // 归档过滤三态：缺省 "no"（只看工作面——既有调用零改动即
                // 获干净列表）/ "only"（归档 TAB）/ "all"（不过滤）。
                let archived_mode = d.get("archived").and_then(|v| v.as_str()).unwrap_or("no");
                let archived_hit =
                    |t: &nemesis_agent::subagent_registry::SubagentTask| match archived_mode {
                        "only" => t.archived_at.is_some(),
                        "all" => true,
                        _ => t.archived_at.is_none(),
                    };
                let all = reg.list();
                // TAB 徽标数据源：全库归档总数（与 status/origin/loop_origin
                // 过滤无关——任何过滤组合下恒定，否则徽标随过滤器漂移）。
                let archived_count = all.iter().filter(|t| t.archived_at.is_some()).count();
                let rows: Vec<serde_json::Value> = all
                    .iter()
                    .filter(|t| archived_hit(t))
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
                Ok(Some(
                    serde_json::json!({ "tasks": rows, "archived_count": archived_count }),
                ))
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
            // 归档三动词（批量）：archive 仅终态行生效（运行中静默跳过、
            // 计数如实）；unarchive 清归档位；remove 物理摘除（delegate 行
            // 删后 B 端回灌无处落 = finish 缺行静默忽略，语义自洽）。
            "archive" | "unarchive" | "remove" => {
                let ids: Vec<String> = data
                    .as_ref()
                    .and_then(|d| d.get("ids"))
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|x| x.as_str().map(str::to_string))
                            .collect()
                    })
                    .filter(|v: &Vec<String>| !v.is_empty())
                    .ok_or_else(|| format!("subagents.{cmd} requires {{ids: string[]}}"))?;
                let changed = match cmd {
                    "archive" => reg.archive(&ids),
                    "unarchive" => reg.unarchive(&ids),
                    _ => reg.remove(&ids),
                };
                Ok(Some(serde_json::json!({ "changed": changed })))
            }
            // 飞行中驾驶（项3）：对运行中子代理中途注入指令。estop 前置闸
            // （cluster.rs 同文案形态）+ 终态前置拒绝（loop 侧
            // `steer_subagent` 是校验链单一裁决点，此处复跑只为错误文案
            // 指向正确——终态任务的 loop 可能已不在，路由错误会误导）。
            // 路由按注册表 `loop_origin` 把请求送到正确的 loop（子代理跑在
            // 哪个引擎，指令就推进哪个引擎的 inbox）；loop_origin 比对、
            // 容量闸、[父代理指令] 前缀全部在 loop 侧单点。
            "steer" => {
                let d =
                    data.ok_or_else(|| "subagents.steer requires {id, instruction}".to_string())?;
                let id = d
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| "subagents.steer requires {id, instruction}".to_string())?;
                let instruction = d
                    .get("instruction")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                    .ok_or_else(|| "subagents.steer requires {id, instruction}".to_string())?;
                if ctx.state.estop.as_ref().is_some_and(|e| e.is_engaged()) {
                    return Err(
                        "⛔ 急停（estop）生效中，子代理驾驶已冻结：`estop --release` 释放后恢复"
                            .to_string(),
                    );
                }
                let t = reg.get(id).ok_or_else(|| format!("task not found: {id}"))?;
                if t.status.is_terminal() {
                    return Err(
                        "子代理已终态，steer 拒绝——飞行中驾驶只对运行中任务生效".to_string()
                    );
                }
                let al = match t.loop_origin.strip_prefix("project:") {
                    Some(pid) => {
                        let bridge = crate::handlers::projects::projects_bridge().ok_or_else(
                            || {
                                "项目会话面未装配（projects bridge 未接线，网关需为装配 gateway 模式）"
                                    .to_string()
                            },
                        )?;
                        bridge.loop_for_session(pid).ok_or_else(|| {
                            format!("项目 loop 当前不可用（project:{pid}），steer 拒绝")
                        })?
                    }
                    _ => match t.loop_origin.strip_prefix("agent:") {
                        Some(agent_id) => {
                            let bridge = crate::handlers::agents::agents_bridge().ok_or_else(
                                || {
                                    "agent 会话面未装配（agents bridge 未接线，网关需为装配 gateway 模式）"
                                        .to_string()
                                },
                            )?;
                            bridge.loop_for_agent(agent_id)?
                        }
                        // main（及未知形态回落主 loop——main 之外的非前缀
                        // 值不存在，回落与主 loop 硬编码口径一致）。
                        _ => ctx
                            .state
                            .agent_loop
                            .read()
                            .clone()
                            .ok_or_else(|| "agent loop not running".to_string())?,
                    },
                };
                let reply = al.steer_subagent(id, instruction)?;
                Ok(Some(
                    serde_json::json!({ "steered": true, "id": id, "reply": reply }),
                ))
            }
            _ => Err(format!("unknown command: subagents.{}", cmd)),
        }
    }
}
