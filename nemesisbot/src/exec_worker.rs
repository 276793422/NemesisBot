//! Executor role entrypoint.
//!
//! Activated when the binary is spawned with `NEMESISBOT_ROLE=executor` (set by
//! the gateway's
//! [`ExecutorChannel`](../../nemesis_agent/remote_executor_tool/) when it spawns
//! a child per tool call). `main()` short-circuits here BEFORE clap parsing —
//! the child is spawned with no subcommand.
//!
//! Protocol: read one newline-delimited JSON request from stdin, dispatch the
//! named tool via the same `register_shared_tools` registry the gateway uses
//! (zero implementation drift between local and remote), write one JSON
//! response to stdout, exit on EOF. Workspace is passed via
//! `NEMESISBOT_EXECUTOR_WORKSPACE` so the child does not re-run path resolution.
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md`.

use std::collections::HashMap;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::debug;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool;
use nemesis_agent::{register_shared_tools, SharedToolConfig};

/// Wire request from the gateway (mirror of the gateway-side `ExecutorRequest`).
#[derive(serde::Deserialize)]
struct ExecutorRequest {
    tool: String,
    args: String,
    context: serde_json::Value,
}

/// Wire response to the gateway (mirror of `ExecutorResponse`).
#[derive(serde::Serialize)]
struct ExecutorResponse {
    ok: bool,
    result: String,
    error: String,
}

/// Executor entrypoint. Reads stdin, dispatches one tool per line, writes
/// responses to stdout, exits on EOF (gateway closed stdin).
pub async fn run() -> Result<()> {
    // Workspace is passed explicitly by the gateway; the child does not resolve.
    let workspace = std::env::var("NEMESISBOT_EXECUTOR_WORKSPACE")
        .context("NEMESISBOT_EXECUTOR_WORKSPACE not set (executor role requires it)")?;

    // Same registry the gateway builds — zero drift between local and remote
    // tool impls. Minimal config: only `workspace`; everything else None →
    // STAY tools (memory/cron/cluster_rpc/...) register as inert stubs that the
    // gateway never invokes (it only sends MOVE tool names over the wire).
    let cfg = SharedToolConfig {
        workspace: Some(workspace),
        ..Default::default()
    };
    let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools(&cfg);
    debug!("[executor] registered {} tools", tools.len());

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let resp = dispatch(&tools, &line).await;
        let resp_line = serde_json::to_string(&resp)
            .unwrap_or_else(|_| r#"{"ok":false,"result":"","error":"response serialize failed"}"#.to_string());
        let mut out = resp_line;
        out.push('\n');
        // Best-effort write + flush before we loop / exit.
        let _ = stdout.write_all(out.as_bytes()).await;
        let _ = stdout.flush().await;
    }
    // EOF → gateway closed stdin → exit cleanly.
    Ok(())
}

/// Dispatch one request line to the tool registry.
async fn dispatch(tools: &HashMap<String, Box<dyn Tool>>, line: &str) -> ExecutorResponse {
    let req: ExecutorRequest = match serde_json::from_str(line) {
        Ok(r) => r,
        Err(e) => {
            return ExecutorResponse {
                ok: false,
                result: String::new(),
                error: format!("bad request line: {e}"),
            }
        }
    };

    // Reconstruct RequestContext (async_callback is `#[serde(skip)]` → None).
    let ctx: RequestContext = match serde_json::from_value(req.context) {
        Ok(c) => c,
        Err(e) => {
            return ExecutorResponse {
                ok: false,
                result: String::new(),
                error: format!("bad context: {e}"),
            }
        }
    };

    let tool = match tools.get(&req.tool) {
        Some(t) => t,
        None => {
            return ExecutorResponse {
                ok: false,
                result: String::new(),
                error: format!("unknown tool: {}", req.tool),
            }
        }
    };

    match tool.execute(&req.args, &ctx).await {
        Ok(result) => ExecutorResponse {
            ok: true,
            result,
            error: String::new(),
        },
        Err(error) => ExecutorResponse {
            ok: false,
            result: String::new(),
            error,
        },
    }
}
