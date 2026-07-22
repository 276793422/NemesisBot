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
                let guard = ctx.state.agent_loop.read();
                if let Some(al) = guard.as_ref() {
                    if let Some(store) = al.session_store() {
                        store.delete_session(&session_key);
                    }
                }
                Ok(Some(serde_json::json!({ "deleted": session_id })))
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
                let guard = ctx.state.agent_loop.read();
                if let Some(al) = guard.as_ref() {
                    if let Some(store) = al.session_store() {
                        store.clear_session(&session_key);
                    }
                }
                nemesis_agent::chat_log::clear_chat_log(&session_key);
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
