//! Tools handler — get/save TOOLS.md.

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
        let workspace = require_workspace(ctx)?;
        match cmd {
            "get" => self.get(workspace),
            "save" => {
                let data = data.ok_or("missing data")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.save(workspace, &content)
            }
            _ => Err(format!("unknown command: tools.{}", cmd)),
        }
    }
}

impl ToolsHandler {
    fn get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, TOOLS_FILE)?;
        Ok(Some(serde_json::json!({ "content": content })))
    }

    fn save(&self, workspace: &str, content: &str) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, TOOLS_FILE, content)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}
