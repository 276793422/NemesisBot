//! Coding handler — P2-1 (2026-08-24 UI entry gap) 「代码开发」页后端。
//!
//! Read-only status commands for the three delegation/semantic-code tool
//! configs; writes deliberately go through the existing generic
//! `config.set_field` WSAPI (dot-path into config.json via ConfigStore) —
//! this handler only adds what config files cannot express: runtime PATH
//! probing of the five LSP language servers (§九.6 状态显示原则：能力状态
//! 必须问后端，禁止前端硬编码).
//!
//! All three toggles are read at AgentLoop build time (agent_factory PATH
//! probe registration), so the UI card tells the user to restart the Agent
//! (`agent.stop` → `agent.start`) after saving.

use crate::ws_router::{ModuleHandler, RequestContext};

pub struct CodingHandler;

#[async_trait::async_trait]
impl ModuleHandler for CodingHandler {
    fn module_name(&self) -> &str {
        "coding"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "lsp_status" => self.lsp_status(),
            "config" => self.config(ctx),
            _ => Err(format!("unknown command: coding.{}", cmd)),
        }
    }
}

impl CodingHandler {
    /// Live PATH probe of every language server in the nemesis-lsp registry
    /// table (`SERVERS`). Availability is machine-dependent — this command is
    /// exactly why the page does not hard-code anything.
    fn lsp_status(&self) -> Result<Option<serde_json::Value>, String> {
        let languages: Vec<serde_json::Value> = nemesis_lsp::registry::SERVERS
            .iter()
            .map(|spec| {
                serde_json::json!({
                    "lang": format!("{:?}", spec.lang),
                    "label": spec.lang.label(),
                    "command": spec.command,
                    "available": nemesis_lsp::registry::server_available(spec.lang),
                })
            })
            .collect();
        let available_count = nemesis_lsp::registry::probe_available().len();
        Ok(Some(serde_json::json!({
            "languages": languages,
            "available_count": available_count,
            // The lsp tool only registers when at least one server exists
            // (probe_available empty ⇒ tool not registered at all).
            "tool_would_register": available_count > 0,
        })))
    }

    /// The three tool config sections for the page (read side; writes go via
    /// `config.set_field` with paths like `agents.lsp_tool.enabled`).
    fn config(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let home = crate::handlers::require_home(ctx)?;
        let cfg = load_config(&home)?;
        Ok(Some(serde_json::json!({
            "lsp": {
                "enabled": cfg.agents.lsp_tool.enabled,
                "timeout_secs": cfg.agents.lsp_tool.timeout_secs,
                "idle_secs": cfg.agents.lsp_tool.idle_secs,
            },
            "claude_code": {
                "enabled": cfg.agents.claude_code_tool.enabled,
                "timeout_secs": cfg.agents.claude_code_tool.timeout_secs,
                // Valid: default | accept_edits | plan | bypass_permissions
                // (empty → accept_edits fail-safe at spawn; NOT model-selectable).
                "permission_mode": cfg.agents.claude_code_tool.permission_mode,
            },
            "codex": {
                "enabled": cfg.agents.codex_tool.enabled,
                "timeout_secs": cfg.agents.codex_tool.timeout_secs,
                // Valid: read_only | workspace_write | danger_full_access
                // (empty → read_only fail-safe at spawn; NOT model-selectable).
                "sandbox": cfg.agents.codex_tool.sandbox,
            },
        })))
    }
}

fn config_path(home: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(home).join("config.json")
}

/// Same load strategy as handlers/config.rs: prefer the runtime ConfigStore,
/// fall back to disk. Keeps the read consistent with what set_field writes.
fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    nemesis_config::load_config(&config_path(home))
        .map_err(|e| format!("failed to load config: {}", e))
}

#[cfg(test)]
mod tests;
