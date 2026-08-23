//! Codex CLI delegation tool (I4 / U13 other half; T5 refactored to the
//! shared CLI-delegation layer).
//!
//! Delegates a self-contained task to the local OpenAI Codex CLI:
//! `codex exec --skip-git-repo-check --sandbox <tier> --ask-for-approval never "<prompt>"`
//! (one-shot non-interactive execution; stdout collected as the result).
//! The spawn/timeout/tree-kill plumbing lives in [`super::cli_delegation`]
//! (T5 / A9 — this file is now the "argument construction + config" shell).
//!
//! T5 (U13 original item): the sandbox is a FIXED config tier
//! (`agents.codex_tool.sandbox`, enum read_only/workspace_write/
//! danger_full_access, default `read_only`) mapped to codex's kebab-case CLI
//! value at spawn, plus the implied `--ask-for-approval never` (non-interactive
//! — there is no TTY to answer an approval prompt, a hung child just burns
//! the timeout). Deliberately NOT in the tool schema: the model cannot
//! choose it.
//!
//! VERIFICATION STATUS: the codex CLI was NOT present on this machine when
//! this was written, so the exec sub-command shape follows Codex CLI's public
//! documentation; real-machine verification is the tracked B3 debt — the tool
//! degrades to a structured error, never panics.
//!
//! Registration is OPT-IN (`agents.codex_tool.enabled`, default false) AND
//! requires the CLI on PATH at registration time.

use crate::context::RequestContext;
use crate::loop_tools::cli_delegation::{self, CliDelegationSpec};
use crate::loop_tools::Tool;
use async_trait::async_trait;

/// Default wall-clock budget for one delegation.
const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// Locate the Codex CLI: `where codex` (Windows) / `which codex`.
/// Returns the resolved path, or None when not installed.
pub fn find_codex_cli() -> Option<String> {
    cli_delegation::find_cli_on_path("codex")
}

/// The delegation tool. Constructed only when the CLI was found AND the
/// config enabled it.
pub struct CodexTool {
    cli_path: String,
    timeout_secs: u64,
    /// T5: fixed sandbox tier (snake_case, already normalized; default
    /// read_only).
    sandbox: &'static str,
}

impl CodexTool {
    /// `sandbox: None` = unset in config → `read_only` (default). Unknown
    /// values fall back to the default (fail-safe).
    pub fn new(cli_path: String, timeout_secs: Option<u64>, sandbox: Option<&str>) -> Self {
        Self {
            cli_path,
            timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
            sandbox: cli_delegation::resolve_codex_sandbox(sandbox.unwrap_or("")),
        }
    }

    /// The normalized sandbox tier this tool spawns with (test/debug
    /// visibility).
    pub fn sandbox(&self) -> &str {
        self.sandbox
    }
}

#[async_trait]
impl Tool for CodexTool {
    fn description(&self) -> String {
        "将任务委派给本机的 OpenAI Codex CLI 执行并返回其最终答复。适合借用 Codex 的编码/代理能力处理自包含的子任务。输入应为无需额外上下文即可执行的完整任务描述。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        // T5: sandbox is intentionally ABSENT — fixed config tier, not
        // model-selectable.
        serde_json::json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Self-contained task description for Codex (include all context it needs: paths, goals, constraints)."
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

        // `codex exec` runs the prompt non-interactively and prints the final
        // answer to stdout. `--skip-git-repo-check`: codex exec refuses to
        // run outside a git repository by default, and the gateway's cwd is
        // typically the (non-git) workspace home (same flag as the in-repo
        // codex_cli provider). `--sandbox` + `--ask-for-approval never` are
        // the T5 fixed tiers (see module doc).
        let args = vec![
            "exec".to_string(),
            "--skip-git-repo-check".to_string(),
            "--sandbox".to_string(),
            cli_delegation::codex_sandbox_kebab(self.sandbox).to_string(),
            "--ask-for-approval".to_string(),
            "never".to_string(),
            prompt.to_string(),
        ];

        cli_delegation::run_cli_delegation(CliDelegationSpec {
            cli: &self.cli_path,
            cli_label: "codex CLI",
            args,
            timeout_secs: self.timeout_secs,
            cwd: &cwd,
        })
        .await
    }
}

#[cfg(test)]
mod tests;
