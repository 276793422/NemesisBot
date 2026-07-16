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

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            // List the host bot's registered tools (name + description + schema).
            // Does not need the workspace; reads the agent loop's tool map.
            "list" => self.list(ctx),
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
    fn list(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let agent_loop = ctx.state.agent_loop.read().clone();
        let al = agent_loop.ok_or("agent not running")?;
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
