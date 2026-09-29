//! 观察者事件泵：AgentEvent 广播 → 脱敏投影 → per-plugin 有界队列。
//!
//! v1 事件型 = `tool_start` / `tool_end` 两型（对齐 AgentEvent 广播承载形
//! 态）。投影是**结构化元数据**：不含 args/result 预览与错误正文（内容体
//! 不出宿主——观察者拿到的只有形状）。`turn` / `final` / `error` 三型为
//! schema 预留挂 v2（宿主广播无对应承载形态，v1 不投影）。
//!
//! 投递语义：fire-and-forget。队列满 / 观察者忙 / observe 失败 = 事件丢弃
//! + 计数（`dropped_events`），永不反压 agent 事件路径。

use nemesis_types::agent::AgentEvent;

/// 投影一条 AgentEvent 为观察者事件 JSON（无关型返回 None）。
#[must_use]
pub fn project_event(event: &AgentEvent) -> Option<String> {
    let mut v = serde_json::Map::new();
    match event {
        AgentEvent::ToolStarted {
            session_key,
            call_id,
            tool,
            ..
        } => {
            v.insert("type".into(), "tool_start".into());
            v.insert("ts_ms".into(), serde_json::json!(unix_millis()));
            v.insert("session_key".into(), serde_json::json!(session_key));
            v.insert("tool".into(), serde_json::json!(tool));
            v.insert("call_id".into(), serde_json::json!(call_id));
        }
        AgentEvent::ToolFinished {
            session_key,
            call_id,
            tool,
            duration_ms,
            ok,
            ..
        } => {
            v.insert("type".into(), "tool_end".into());
            v.insert("ts_ms".into(), serde_json::json!(unix_millis()));
            v.insert("session_key".into(), serde_json::json!(session_key));
            v.insert("tool".into(), serde_json::json!(tool));
            v.insert("call_id".into(), serde_json::json!(call_id));
            v.insert("duration_ms".into(), serde_json::json!(duration_ms));
            v.insert("ok".into(), serde_json::json!(ok));
        }
        // 其余变体 v1 不投影（内容体/无广播承载形态；见模块注释）。
        _ => return None,
    }
    serde_json::to_string(&serde_json::Value::Object(v)).ok()
}

/// 事件泵任务：订阅广播，投影后 enqueue 进注册表（观察者 worker 消费）。
///
/// 返回的 JoinHandle 常驻；关停走 `shutdown` watch（gateway estop/退出链）。
pub fn spawn_pump(
    manager: std::sync::Arc<crate::registry::PluginManager>,
    mut events: tokio::sync::broadcast::Receiver<AgentEvent>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.changed() => {
                    if *shutdown.borrow() {
                        break;
                    }
                }
                ev = events.recv() => {
                    match ev {
                        Ok(event) => {
                            if let Some(json) = project_event(&event) {
                                manager.enqueue_observer_event(json);
                            }
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("[WasmPlugin] 事件广播落后 {n} 条（观察者侧丢弃）");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
        tracing::info!("[WasmPlugin] 观察者事件泵已退出");
    })
}

fn unix_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
