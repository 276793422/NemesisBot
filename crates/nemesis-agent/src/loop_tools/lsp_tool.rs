//! L1 (U19): the agent-facing `lsp` tool — four read-only semantic code
//! queries (definition / references / implementation / hover) driven
//! through real language servers by `nemesis-lsp`.
//!
//! Registration follows the CC/Codex probe pattern (loop_tools.rs): the
//! tool exists for the model only when (a) config opted in via
//! `agents.lsp_tool.enabled` AND (b) at least one language server was
//! found on PATH at registration. `registration_plan` encodes that
//! decision as a pure function so acceptance ② (missing server ⇒ not
//! registered) is unit-testable without touching PATH.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nemesis_lsp::{LspManager, LspOp};

use crate::context::RequestContext;
use crate::loop_tools::Tool;

pub struct LspTool {
    manager: Arc<LspManager>,
}

impl LspTool {
    /// `timeout_secs`: per-request LSP budget (None = manager's 120s
    /// default). `idle_secs`: idle session reap threshold (None = 600s).
    pub fn new(timeout_secs: Option<u64>, idle_secs: Option<u64>) -> Self {
        Self {
            manager: Arc::new(LspManager::new(
                timeout_secs.map(Duration::from_secs),
                idle_secs.map(Duration::from_secs),
            )),
        }
    }

    /// Pure registration decision (acceptance ②): enabled in config AND
    /// at least one language server available. The caller probes PATH and
    /// passes the count; this fn owns only the policy.
    pub fn registration_plan(enabled: bool, available_langs: usize) -> bool {
        enabled && available_langs > 0
    }
}

#[async_trait]
impl Tool for LspTool {
    fn description(&self) -> String {
        "对代码做只读语义查询：查找定义/引用/实现、查看悬停信息。由真实语言服务器（rust-analyzer/gopls 等）驱动，比文本 grep 更精确（跨文件、按符号）。".to_string()
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["definition", "references", "implementation", "hover"],
                    "description": "Query type: definition (where is this symbol defined), references (all usages), implementation (impls of a trait/type), hover (type/doc at this position)."
                },
                "path": {
                    "type": "string",
                    "description": "Absolute file path (relative resolved against the working directory)."
                },
                "line": {
                    "type": "integer",
                    "description": "0-based line number."
                },
                "character": {
                    "type": "integer",
                    "description": "0-based column in UTF-16 code units (LSP convention)."
                }
            },
            "required": ["op", "path", "line", "character"]
        })
    }

    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        let v: serde_json::Value =
            serde_json::from_str(args).map_err(|e| format!("Invalid arguments: {}", e))?;

        let op_str = v
            .get("op")
            .and_then(|o| o.as_str())
            .ok_or("Missing 'op' argument")?;
        let op = LspOp::parse(op_str).ok_or_else(|| {
            format!(
                "Invalid 'op' {:?} — valid values: definition | references | implementation | hover",
                op_str
            )
        })?;

        let path_str = v
            .get("path")
            .and_then(|p| p.as_str())
            .ok_or("Missing 'path' argument")?;
        let path = PathBuf::from(path_str);
        // Relative paths resolve against the process working directory
        // (the gateway's cwd, same convention as exec/file tools).
        let path = if path.is_relative() {
            std::env::current_dir()
                .map_err(|e| format!("cannot resolve relative path {path_str:?}: {e}"))?
                .join(path)
        } else {
            path
        };

        let line = v
            .get("line")
            .and_then(|l| l.as_u64())
            .ok_or("Missing or non-integer 'line' argument")?;
        let character = v
            .get("character")
            .and_then(|c| c.as_u64())
            .ok_or("Missing or non-integer 'character' argument")?;
        // LSP positions are u32; reject absurd values early rather than
        // truncating a u64 silently.
        let line = u32::try_from(line).map_err(|_| "'line' out of range")?;
        let character = u32::try_from(character).map_err(|_| "'character' out of range")?;

        self.manager.query(op, &path, line, character).await
    }
}

#[cfg(test)]
mod tests;
