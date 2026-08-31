//! Sessions handler — Dashboard multi-session management (list/create/delete).
//!
//! Each conversation is identified by a client-chosen `session_id`; the
//! backend turns it into session_key `agent:main:session:{sid}`
//! (see `server.rs` process_messages + `loop.rs` handle_history_request).
//! List source = `session_logs/*.jsonl` (reuses `logs::scan_session_logs`),
//! filtered to web conversations. Delete clears SessionStore + session_logs.

use crate::handlers::logs::scan_session_logs;
use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};

pub struct SessionsHandler;

#[async_trait::async_trait]
impl ModuleHandler for SessionsHandler {
    fn module_name(&self) -> &str {
        "sessions"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "list" => {
                let workspace = require_workspace(ctx)?;
                let all = scan_session_logs(workspace);
                // Web multi-session conversations only: files are named
                // `agent_main_session_{sid}.jsonl` (session_key
                // `agent:main:session:{sid}` with `:`→`_`). Strip the prefix
                // so the client gets the bare `sid` — exactly what it sends
                // back as moduleData.session_id. (Legacy `agent_main_main`
                // migration is Phase 2.)
                let web: Vec<_> = all
                    .into_iter()
                    .filter_map(|mut s| {
                        let id = s["id"].as_str()?.to_string();
                        let sid = id.strip_prefix("agent_main_session_")?.to_string();
                        s["id"] = serde_json::Value::String(sid);
                        Some(s)
                    })
                    .collect();
                Ok(Some(serde_json::json!({ "sessions": web })))
            }
            "create" => {
                // Backend generates the id; the conversation lazily
                // materializes in session_logs on the first message. Title is
                // written to a sidecar meta file immediately.
                let session_id = uuid::Uuid::new_v4().to_string();
                let title = data
                    .as_ref()
                    .and_then(|d| d.get("title"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("新对话")
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                nemesis_agent::chat_log::write_session_meta(&session_key, &title);
                Ok(Some(
                    serde_json::json!({ "session_id": session_id, "title": title }),
                ))
            }
            "rename" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let title = data
                    .as_ref()
                    .and_then(|d| d.get("title"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing title".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                nemesis_agent::chat_log::write_session_meta(&session_key, &title);
                Ok(Some(
                    serde_json::json!({ "session_id": session_id, "title": title }),
                ))
            }
            "delete" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // Clear SessionStore (in-memory + sessions/*.json) +
                // session_logs/*.jsonl. Best-effort; absence is not an error.
                {
                    let guard = ctx.state.agent_loop.read();
                    if let Some(al) = guard.as_ref()
                        && let Some(store) = al.session_store() {
                            store.delete_session(&session_key);
                        }
                }
                // CC SessionEnd（观察型，2026-08-29 T3）：显式删除也触发
                // （桥经 AgentLoop 的 cc_hooks_bridge 访问；未装配 = 跳过）。
                // 先把桥 Arc 克隆出 guard 作用域，再 await（guard 不跨 await）。
                let session_end_bridge = {
                    let guard = ctx.state.agent_loop.read();
                    guard.as_ref().and_then(|al| al.cc_hooks_bridge())
                };
                if let Some(bridge) = session_end_bridge {
                    bridge.on_session_end(&session_key, "deleted").await;
                }
                // Cron cascade (2026-08-25): a scheduled job pinned to this
                // session_key would otherwise FIRE on a deleted conversation
                // and resurrect it — an empty jsonl re-created + cron rows
                // appended, a session the user explicitly deleted coming
                // back as a zombie. Disable (NOT remove: the job definition
                // stays on the Tasks page for re-pointing/re-enabling) every
                // ENABLED job whose payload targets this session, and report
                // what was paused. Disabled jobs are left untouched.
                // Guard released above so we never hold agent_loop + cron
                // mutexes together.
                let mut paused: Vec<serde_json::Value> = Vec::new();
                if let Some(svc) = ctx.state.cron.as_ref()
                    && let Ok(svc) = svc.lock() {
                        for job in svc.list_jobs(true) {
                            if !job.enabled {
                                continue;
                            }
                            if job.payload.session_key.as_deref() != Some(session_key.as_str()) {
                                continue;
                            }
                            match svc.enable_job(&job.id, false) {
                                Ok(j) => paused.push(serde_json::json!({
                                    "id": j.id,
                                    "name": j.name,
                                })),
                                Err(e) => tracing::warn!(
                                    "[sessions] delete cascade: failed to pause cron job {} ({}): {}",
                                    job.id, job.name, e
                                ),
                            }
                        }
                    }
                Ok(Some(serde_json::json!({
                    "deleted": session_id,
                    "paused_cron_jobs": paused,
                })))
            }
            "clear" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                // Clear SessionStore messages + session_logs jsonl (keep meta/key).
                // Order matters (2026-08-25 self-heal note): truncate the
                // chat_log FIRST, then drop the store json. If we crashed
                // between the two steps the other way round, the surviving
                // jsonl would let `rebuild_from_chat_log` resurrect the
                // content the user just asked to clear. jsonl-first keeps the
                // rebuild path unable to revive cleared data.
                nemesis_agent::chat_log::clear_chat_log(&session_key);
                let guard = ctx.state.agent_loop.read();
                if let Some(al) = guard.as_ref()
                    && let Some(store) = al.session_store() {
                        store.clear_session(&session_key);
                    }
                Ok(Some(serde_json::json!({ "cleared": session_id })))
            }
            "export" => {
                let session_id = data
                    .as_ref()
                    .and_then(|d| d.get("session_id"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "missing session_id".to_string())?
                    .to_string();
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(&session_id)
                );
                let (messages, total, _, _) =
                    nemesis_agent::chat_log::read_chat_log(&session_key, 100_000, None);
                Ok(Some(serde_json::json!({
                    "session_id": session_id,
                    "messages": messages,
                    "count": total,
                })))
            }
            _ => Err(format!("unknown sessions cmd: {}", cmd)),
        }
    }
}
