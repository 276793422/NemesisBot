//! Tools handler — get/save TOOLS.md, and list the host bot's tool registry.
//!
//! `tools.get`/`tools.save` read/write the local `TOOLS.md` notes file.
//! `tools.list` enumerates the agent's registered tools (name + description +
//! JSON Schema parameters) so the workflow canvas can render a tool picker and
//! a schema-driven parameter form for the `tool` node. The list mirrors the
//! set bridged into the workflow tool registry (see `AgentToolAdapter`).

use crate::handlers::{read_workspace_file, require_workspace, write_workspace_file};
use crate::ws_router::{ModuleHandler, RequestContext};

const TOOLS_FILE: &str = "TOOLS.md";

pub struct ToolsHandler;

#[async_trait::async_trait]
impl ModuleHandler for ToolsHandler {
    fn module_name(&self) -> &str {
        "tools"
    }

    fn commands(&self) -> &'static [&'static str] {
        &["list", "get", "save"]
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // List the host bot's registered tools (name + description + schema).
            // Does not need the workspace; reads the agent loop's tool map.
            // L6++ G4（2026-09-08）：可选 session_id——项目会话列项目 loop 的
            // 工具表（工厂裁剪后的集合），不带 = 主 loop（现状不变）。
            "list" => self.list(ctx, data.as_ref()),
            "get" => {
                let workspace = require_workspace(ctx)?;
                self.get(workspace)
            }
            "save" => {
                let workspace = require_workspace(ctx)?;
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.save(workspace, &content)
            }
            _ => Err(format!("unknown command: tools.{}", cmd)),
        }
    }
}

impl ToolsHandler {
    /// `tools.list` — enumerate agent tools with their parameter schemas.
    ///
    /// Returns `{ tools: [{name, description, parameters}], count }`. The
    /// `parameters` field is an OpenAI-compatible JSON Schema object, ready to
    /// drive a dynamic form. Returns an error if the agent loop isn't running
    /// (the dashboard normally has it running).
    fn list(
        &self,
        ctx: &RequestContext,
        data: Option<&serde_json::Value>,
    ) -> Result<Option<serde_json::Value>, String> {
        // L6++ G4：显式 session_id 契约——带则按归属解析（项目 loop / 诚实
        // 报错），不带 = 主 loop。
        let al: std::sync::Arc<nemesis_agent::r#loop::AgentLoop> = match data
            .and_then(|d| d.get("session_id"))
            .and_then(|v| v.as_str())
        {
            Some(sid) => {
                let session_key = format!(
                    "agent:main:session:{}",
                    nemesis_agent::session::SessionStore::sanitize_session_id(sid)
                );
                crate::handlers::projects::resolve_session_loop(ctx, &session_key)?
            }
            None => ctx
                .state
                .agent_loop
                .read()
                .clone()
                .ok_or("agent not running")?,
        };
        let tools = al.tools();
        let rows: Vec<serde_json::Value> = tools
            .iter()
            .map(|(name, tool)| {
                serde_json::json!({
                    "name": name,
                    "description": tool.description(),
                    "parameters": tool.parameters(),
                })
            })
            .collect();
        let count = rows.len();
        Ok(Some(serde_json::json!({ "tools": rows, "count": count })))
    }

    fn get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, TOOLS_FILE)?;
        Ok(Some(serde_json::json!({ "content": content })))
    }

    fn save(&self, workspace: &str, content: &str) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, TOOLS_FILE, content)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}

// Phase 3 覆盖率（2026-08-25）：list 的真 AgentLoop map 体
// （name/description/parameters 三字段组装）+ agent not running bail。
#[cfg(test)]
mod tests;
