//! Claude Code subagent delegation tool (H7 / U13 half; T5 refactored to the
//! shared CLI-delegation layer).
//!
//! Delegates a self-contained task to the local Claude Code CLI:
//! `claude --print -p "<prompt>" --output-format text --permission-mode <tier>`,
//! stdout collected as the result. The spawn/timeout/tree-kill plumbing lives
//! in [`super::cli_delegation`] (T5 / A9 — this file is now the
//! "argument construction + config" shell).
//!
//! T5 (U13 original item): the permission mode is a FIXED config tier
//! (`agents.claude_code_tool.permission_mode`, CLI camelCase enum
//! acceptEdits/auto/bypassPermissions/manual/dontAsk/plan, default
//! `acceptEdits` — non-interactive-safe). It is deliberately NOT in the tool
//! schema: the model cannot choose it; the deployment config governs the
//! child.
//!
//! Registration is OPT-IN: `claude_code_tool.enabled = true` in config
//! (default false), AND the CLI must be locatable at registration time —
//! absent CLI ⇒ tool simply not registered (graceful degradation).

use crate::context::RequestContext;
use crate::loop_tools::Tool;
use crate::loop_tools::cli_delegation::{self, CliDelegationSpec};
use async_trait::async_trait;

/// Default wall-clock budget for one delegation.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Locate the Claude Code CLI: `where claude` (Windows) / `which claude`.
/// Returns the resolved path, or None when not installed.
pub fn find_claude_cli() -> Option<String> {
    cli_delegation::find_cli_on_path("claude")
}

/// The delegation tool. Constructed only when the CLI was found AND the
/// config enabled it.
pub struct ClaudeCodeTool {
    cli_path: String,
    timeout_secs: u64,
    /// T5: fixed permission tier (already normalized; default acceptEdits).
    permission_mode: &'static str,
}

impl ClaudeCodeTool {
    /// `permission_mode: None` = unset in config → `acceptEdits` (default).
    /// Unknown values fall back to the default (fail-safe).
    pub fn new(cli_path: String, timeout_secs: Option<u64>, permission_mode: Option<&str>) -> Self {
        Self {
            cli_path,
            timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            permission_mode: cli_delegation::resolve_cc_permission_mode(
                permission_mode.unwrap_or(""),
            ),
        }
    }

    /// The normalized permission tier this tool spawns with (test/debug
    /// visibility).
    pub fn permission_mode(&self) -> &str {
        self.permission_mode
    }
}

#[async_trait]
impl Tool for ClaudeCodeTool {
    fn description(&self) -> String {
        "将任务委派给本机的 Claude Code CLI 执行并返回其最终答复。适合借用 Claude 的编码/工具能力处理自包含的子任务。输入应为无需额外上下文即可执行的完整任务描述。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // T5: permission_mode is intentionally ABSENT — fixed config tier,
        // not model-selectable.
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task description for Claude Code (include all context it needs: paths, goals, constraints)."
                }
            },
            "required": ["prompt"]
        })
    }

    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;
        let prompt = v
            .get("prompt")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'prompt' argument")?;
        if prompt.trim().is_empty() {
            return Err("'prompt' must not be empty".to_string());
        }

        let cwd: std::path::PathBuf = cli_delegation::delegation_cwd(&context.session_key);

        // T5 (U13): `--permission-mode <tier>` keeps the child deterministic
        // per deployment config. (H7 originally passed no permission flag and
        // let the CLI's own settings govern the child; the original U13
        // acceptance requires a fixed non-interactive tier instead.)
        let args = vec![
            "--print".to_string(),
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "text".to_string(),
            "--permission-mode".to_string(),
            self.permission_mode.to_string(),
        ];

        cli_delegation::run_cli_delegation(CliDelegationSpec {
            cli: &self.cli_path,
            cli_label: "claude CLI",
            args,
            timeout_secs: self.timeout_secs,
            cwd: &cwd,
        })
        .await
    }
}

#[cfg(test)]
mod tests;
