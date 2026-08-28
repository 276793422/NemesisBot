//! Identity handler — list/get/save identity documents (IDENTITY.md, SOUL.md, USER.md, AGENT.md).

use crate::handlers::{read_workspace_file, require_workspace, write_workspace_file};
use crate::ws_router::{ModuleHandler, RequestContext};

/// Known identity document filenames. The trailing two are the H5 (U18)
/// instruction chain roots (workspace root level) — injected per-turn by
/// `nemesis_agent::workspace_instructions`, unlike the persona 四件套.
const IDENTITY_FILES: &[&str] = &[
    "AGENT.md",
    "IDENTITY.md",
    "SOUL.md",
    "USER.md",
    "AGENTS.md",
    "CLAUDE.md",
];

/// Files that belong to the per-turn instruction chain (G5 badge on the
/// identity page distinguishes these from the persona documents).
pub fn is_instruction_chain(name: &str) -> bool {
    name == "AGENTS.md" || name == "CLAUDE.md"
}

pub struct IdentityHandler;

#[async_trait::async_trait]
impl ModuleHandler for IdentityHandler {
    fn module_name(&self) -> &str {
        "identity"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "list" => self.list(workspace),
            "get" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.get(workspace, &name)
            }
            "save" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                let content = crate::handlers::get_str(&data, "content")?;
                self.save(workspace, &name, &content)
            }
            _ => Err(format!("unknown command: identity.{}", cmd)),
        }
    }
}

impl IdentityHandler {
    fn list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let mut docs = Vec::new();
        for &name in IDENTITY_FILES {
            let content = read_workspace_file(workspace, name).ok();
            docs.push(serde_json::json!({
                "name": name,
                "exists": content.is_some(),
                "size": content.as_ref().map(|c| c.len()).unwrap_or(0),
                // G5：前端区分「指令链 · 每轮注入」与人格四件套。
                "instruction_chain": is_instruction_chain(name),
            }));
        }
        Ok(Some(serde_json::json!({ "documents": docs })))
    }

    fn get(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let content = read_workspace_file(workspace, name)?;
        Ok(Some(serde_json::json!({
            "name": name,
            "content": content,
        })))
    }

    fn save(
        &self,
        workspace: &str,
        name: &str,
        content: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        write_workspace_file(workspace, name, content)?;
        Ok(Some(serde_json::json!({ "saved": true, "name": name })))
    }
}

#[cfg(test)]
mod tests;
