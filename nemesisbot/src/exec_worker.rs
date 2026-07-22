//! Executor role entrypoint.
//!
//! Activated when the binary is spawned with `NEMESISBOT_ROLE=executor` (set by
//! the gateway's [`ExecutorChannel`](../../nemesis_agent/remote_executor_tool/)
//! when it spawns a child per tool call). `main()` short-circuits here BEFORE
//! clap parsing — the child is spawned with no subcommand.
//!
//! Two transports (mirroring the gateway side), selected by env:
//! - `NEMESISBOT_EXECUTOR_PIPE` set → **named-pipe** transport (sandbox mode):
//!   connect to the gateway's `\\.\pipe\NemesisBox_<id>`.
//! - otherwise → **stdio** transport (Layer 1): read stdin, write stdout.
//!
//! Both exchange the same newline-delimited JSON protocol and dispatch via the
//! same `register_shared_tools` registry (zero implementation drift). Workspace
//! is passed via `NEMESISBOT_EXECUTOR_WORKSPACE` so the child does not re-run
//! path resolution.
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md` (Layer 1) and
//! `docs/PLAN/2026-07-09_sandboxie-integration.md` (Layer 2).

use std::collections::HashMap;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::debug;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool;
use nemesis_agent::{SharedToolConfig, register_shared_tools};

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

/// Executor entrypoint. Reads stdin OR a named pipe, dispatches one tool per
/// line, writes responses, exits on EOF (gateway closed the channel).
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

    // Transport: named pipe if the gateway gave us one, else stdio (Layer 1).
    #[cfg(windows)]
    if let Some(pipe_name) = std::env::var("NEMESISBOT_EXECUTOR_PIPE").ok() {
        let stream = nemesis_agent::executor_pipe::connect_client(&pipe_name)
            .await
            .context("connect executor pipe")?;
        debug!("[executor] connected to pipe {pipe_name}");
        return pipe_loop(stream, &tools).await;
    }

    stdio_loop(&tools).await
}

/// Named-pipe transport loop (sandbox mode).
#[cfg(windows)]
async fn pipe_loop(
    mut stream: nemesis_agent::executor_pipe::NamedPipeClient,
    tools: &HashMap<String, Box<dyn Tool>>,
) -> Result<()> {
    loop {
        // Read one request line (block scopes the BufReader borrow so the write
        // below can borrow `stream` after).
        let line = {
            let mut reader = BufReader::new(&mut stream).lines();
            match reader.next_line().await {
                Ok(Some(l)) => l,
                Ok(None) => return Ok(()), // gateway closed → exit cleanly
                Err(e) => return Err(anyhow::anyhow!("pipe read: {e}")),
            }
        };
        let resp = dispatch(tools, &line).await;
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"ok":false,"result":"","error":"response serialize failed"}"#.to_string()
        });
        out.push('\n');
        stream
            .write_all(out.as_bytes())
            .await
            .context("pipe write")?;
        stream.flush().await.context("pipe flush")?;
    }
}

/// stdio transport loop (Layer 1).
async fn stdio_loop(tools: &HashMap<String, Box<dyn Tool>>) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();

    while let Ok(Some(line)) = reader.next_line().await {
        let resp = dispatch(tools, &line).await;
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"ok":false,"result":"","error":"response serialize failed"}"#.to_string()
        });
        out.push('\n');
        let _ = stdout.write_all(out.as_bytes()).await;
        let _ = stdout.flush().await;
    }
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
            };
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
            };
        }
    };

    let tool = match tools.get(&req.tool) {
        Some(t) => t,
        None => {
            return ExecutorResponse {
                ok: false,
                result: String::new(),
                error: format!("unknown tool: {}", req.tool),
            };
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
