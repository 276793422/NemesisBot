//! Outbox handler — worker 侧发件箱死信面（CD6，2026-09-17）。
//!
//! 死信 = 回传条目过闸（age 7 天 / attempts 1000）后的诚实停车态。本
//! handler 提供 Dashboard 可见性（列表）与手动重放口；状态全在磁盘
//! （entry.json），操作只需要 workspace 根——不需要活实例，重放后推送
//! 循环下一 tick（≤15s）自然拾取。零丢失红线不回归：重放只是重新入队，
//! master 落盘 ACK 才删本地。

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};

pub struct OutboxHandler;

/// outbox 根目录（nemesis-path 唯一拼接点，与 TransferOutbox 同源）。
fn outbox_root(ctx: &RequestContext) -> Result<std::path::PathBuf, String> {
    let workspace = require_workspace(ctx)?;
    Ok(nemesis_path::cluster_dir_in_workspace(std::path::Path::new(workspace)).join("outbox"))
}

fn entry_json(e: &nemesis_cluster::outbox::OutboxEntry) -> serde_json::Value {
    serde_json::json!({
        "task_id": e.task_id,
        "source_node": e.source_node,
        "state": e.state,
        "created_at": e.created_at,
        "attempts": e.attempts,
        "last_error": e.last_error,
        "next_retry_at": e.next_retry_at,
    })
}

#[async_trait::async_trait]
impl ModuleHandler for OutboxHandler {
    fn module_name(&self) -> &str {
        "outbox"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["dead_list", "dead_replay"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "dead_list" => {
                let root = outbox_root(ctx)?;
                let entries: Vec<serde_json::Value> =
                    nemesis_cluster::outbox::list_dead_letter_entries(&root)
                        .iter()
                        .map(entry_json)
                        .collect();
                Ok(Some(serde_json::json!({ "dead_letters": entries })))
            }
            "dead_replay" => {
                let task_id = data
                    .as_ref()
                    .and_then(|d| d.get("task_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if task_id.is_empty() {
                    return Err("missing 'task_id' field".to_string());
                }
                let root = outbox_root(ctx)?;
                let entry = nemesis_cluster::outbox::replay_dead_letter_entry(&root, &task_id)?;
                Ok(Some(serde_json::json!({ "replayed": entry_json(&entry) })))
            }
            _ => Err(format!("outbox: unknown command '{cmd}'")),
        }
    }
}
