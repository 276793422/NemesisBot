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
//! ## U11 用户态沙盒（Linux landlock/bwrap、macOS Seatbelt）
//!
//! 非 Windows 上 `executor.sandbox=true` 时 gateway 以 stdio spawn + env
//! `NEMESISBOT_EXECUTOR_SANDBOX=1`（见 `spawn_and_call`）。子进程在进工具
//! 循环**之前**处理该标记：
//! - **landlock 可用**（SelfApply 形态）→ 对自身装上限制（writable=workspace
//!   子树、全盘读、按 config 禁网），不可逆、后代全继承。
//! - **仅 bwrap / sandbox-exec 可用**（WrapCommand 形态）→ **re-exec 自身进
//!   盒**：本进程退化为 stdio 代理（gateway ↔ 盒内实例），工具全在盒里跑。
//! - **无可用后端** → warn + 无盒继续（降级不崩；`executor.sandbox` 在
//!   Windows Sandboxie 侧仍然生效）。
//!
//! 线程语义（关键正确性）：landlock 只约束**调用线程及其后代线程**。非 mac
//! 入口是 `#[tokio::main]`——多线程 runtime 在本模块之前已建好，worker 线程
//! 不会被之后 apply 的限制覆盖。因此整个 executor 跑在一条**专用线程**上：
//! 线程上先装沙盒（或 re-exec），再用 current_thread runtime 驱动循环——
//! 循环里的一切（含 tokio::spawn 的任务）都落在线程本身，全部受限。
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md` (Layer 1),
//! `docs/PLAN/2026-07-09_sandboxie-integration.md` (Layer 2), and
//! `docs/PLAN/2026-08-23_dsh-remaining-goal.md` W2 (U11).

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
///
/// Immediately delegates to a dedicated thread (see module docs for the
/// landlock thread-semantics rationale); blocks until it finishes.
pub async fn run() -> Result<()> {
    let handle = std::thread::Builder::new()
        .name("nemesis-executor".into())
        .spawn(executor_main)
        .context("spawn executor main thread")?;
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("executor main thread panicked"))?
}

/// Dedicated-thread main (sync): env → 沙盒决策（U11）→ 工具循环。
fn executor_main() -> Result<()> {
    let workspace = std::env::var("NEMESISBOT_EXECUTOR_WORKSPACE")
        .context("NEMESISBOT_EXECUTOR_WORKSPACE not set (executor role requires it)")?;
    // 仅 `sandbox` 构建的 engage() 使用（trim 构建下无消费方）。
    #[cfg_attr(not(feature = "sandbox"), allow(unused_variables))]
    let home = std::env::var_os("NEMESISBOT_EXECUTOR_HOME").map(std::path::PathBuf::from);

    let sandbox_marker = std::env::var("NEMESISBOT_EXECUTOR_SANDBOX").as_deref() == Ok("1");
    let already_boxed = std::env::var("NEMESISBOT_EXECUTOR_REEXEC").as_deref() == Ok("1");
    if sandbox_marker && !already_boxed {
        #[cfg(feature = "sandbox")]
        {
            match userland::engage(&workspace, home.as_deref()) {
                Ok(userland::Outcome::Continue) => {}
                // 盒内实例已完成整个会话（本进程只是 stdio 代理）：按其退出码收尾。
                Ok(userland::Outcome::ReexecDone(status)) => {
                    if status.success() {
                        return Ok(());
                    }
                    anyhow::bail!("wrapped executor exited: {status}");
                }
                // P5-2：沙盒介入失败（严格模式拒绝 / re-exec spawn 失败）。
                // 此时 gateway 的工具调用已在途（per-call 子进程）——先回一行
                // 干净的协议错误再退出，让模型/用户看到拒绝原因，而不是对着
                // 「子进程无响应」猜。
                Err(e) => {
                    emit_error_response(&e.to_string());
                    return Ok(());
                }
            }
        }
        #[cfg(not(feature = "sandbox"))]
        {
            // trim 构建：子进程侧保持 fail-open warn（gateway 侧的严格闸门
            // 已在 spawn 前拒绝该配置下的调用——见 exec_world 的 feature-off
            // 闸门；这里再拒属于重复防线，且本构建读不到 backend 的 strict
            // 解析，不值得为它引入裸 config 解析）。
            tracing::warn!(
                "[executor] sandbox marker set but the 'sandbox' feature is not compiled \
                 into this build — running unsandboxed"
            );
        }
    }

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("build executor runtime")?;
    rt.block_on(run_loop(&workspace))
}

/// 在进工具循环前失败时，向 stdout 回一行协议错误（P5-2）。
///
/// engage 失败发生在任何请求读取之前，但 per-call 子进程被 spawn 的前提就是
/// gateway 有一个调用在途——直接 exit 会让 gateway 收到 EOF、报「子进程无响
/// 应」这种没法排查的错。写一行 `ExecutorResponse`（ok=false）再退出，拒绝
/// 原因就能干净地回到模型/用户面前。同步 IO（专用线程上、runtime 建好之前）。
#[cfg_attr(not(feature = "sandbox"), allow(dead_code))] // 唯一调用点在 feature 门内
fn emit_error_response(error: &str) {
    let resp = ExecutorResponse {
        ok: false,
        result: String::new(),
        error: error.to_string(),
    };
    let mut line = serde_json::to_string(&resp).unwrap_or_else(|_| {
        r#"{"ok":false,"result":"","error":"executor failed before tool loop"}"#.to_string()
    });
    line.push('\n');
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    let _ = out.write_all(line.as_bytes());
    let _ = out.flush();
}

/// 工具循环（原 run() 主体）：注册共享工具集 + 选传输层。
async fn run_loop(workspace: &str) -> Result<()> {
    // Same registry the gateway builds — zero drift between local and remote
    // tool impls. Minimal config: only `workspace`; everything else None →
    // STAY tools (memory/cron/cluster_rpc/...) register as inert stubs that the
    // gateway never invokes (it only sends MOVE tool names over the wire).
    let cfg = SharedToolConfig {
        workspace: Some(workspace.to_string()),
        ..Default::default()
    };
    let tools: HashMap<String, Box<dyn Tool>> = register_shared_tools(&cfg);
    debug!("[executor] registered {} tools", tools.len());

    // Transport: named pipe if the gateway gave us one, else stdio (Layer 1).
    #[cfg(windows)]
    if let Ok(pipe_name) = std::env::var("NEMESISBOT_EXECUTOR_PIPE") {
        let stream = nemesis_agent::executor_pipe::connect_client(&pipe_name)
            .await
            .context("connect executor pipe")?;
        debug!("[executor] connected to pipe {pipe_name}");
        return pipe_loop(stream, &tools).await;
    }

    stdio_loop(&tools).await
}

// ---------------------------------------------------------------------------
// U11 用户态沙盒（feature 门控 —— trim 构建回退无盒 + warn）
// ---------------------------------------------------------------------------
#[cfg(feature = "sandbox")]
mod userland {
    use std::path::Path;
    use std::sync::Arc;

    use anyhow::{Context, Result};
    use nemesis_sandbox::backend::{self, BackendForm, Enforcement, SandboxBackend, SandboxConf};

    /// engage() 的结果。`Debug`：测试里 `expect_err` 需要 Ok 侧 Debug。
    #[derive(Debug)]
    pub enum Outcome {
        /// 继续（Plain 路径 / 自装完成 / 降级完成）。
        Continue,
        /// re-exec 代理已完成整个会话（盒内实例退出）；携带其退出状态。
        ReexecDone(std::process::ExitStatus),
    }

    /// 纯决策（单测覆盖）：标记与后端形态 → 执行路径。
    pub fn plan(sandbox_marker: bool, already_boxed: bool, form: Option<BackendForm>) -> Plan {
        if already_boxed || !sandbox_marker {
            return Plan::Plain;
        }
        match form {
            Some(BackendForm::SelfApply) => Plan::SelfApply,
            Some(BackendForm::WrapCommand) => Plan::WrapReexec,
            None => Plan::Plain,
        }
    }

    /// 三条路径：直接循环 / 自装 / re-exec 进盒。
    #[derive(Debug, PartialEq, Eq)]
    pub enum Plan {
        Plain,
        SelfApply,
        WrapReexec,
    }

    /// 沙盒介入点（executor 专用线程上调用）。默认降级语义 = warn + 无盒
    /// 继续、永不因沙盒失败而 Err（U11 验收「Landlock 不可用降级无盒+warn
    /// 不崩」）。Err 出口有二：
    /// 1. re-exec 的进程层失败（spawn 不起来 = 代理模式根本没法跑）；
    /// 2. **P5-2 严格模式**（`executor.strict=true`）：无可用后端 / 自装失败
    ///    时改判为 Err（fail-closed 拒绝）——gateway 侧闸门已在 spawn 前拒
    ///    绝过一遍，这里是子进程侧的第二道防线（防两边探测结果分叉，如
    ///    bwrap 在闸门过后、子进程启动前的窗口里被卸载）。
    ///    注意 **Partial 强制不算失败**（规则已装、有能力缺口如 landlock 不
    ///    覆盖网络）——严格模式保证「有盒」，不保证「盒无能力缺口」，缺口
    ///    照旧 warn + 状态页如实展示。
    pub fn engage(workspace: &str, home: Option<&Path>) -> Result<Outcome> {
        let strict = home.map(backend::read_executor_strict).unwrap_or(false);
        let detected = backend::detect_backend();
        let form = detected
            .as_ref()
            .map(|b: &Arc<dyn SandboxBackend>| b.form());
        match plan(true, false, form) {
            Plan::Plain => {
                if strict {
                    anyhow::bail!(
                        "strict mode (fail-closed): executor.sandbox is on but no \
                         userland sandbox backend is available on this system — \
                         refusing to run unsandboxed"
                    );
                }
                tracing::warn!(
                    "[executor] no userland sandbox backend on this system — running \
                     unsandboxed (executor.sandbox stays honoured for Windows Sandboxie)"
                );
                Ok(Outcome::Continue)
            }
            Plan::SelfApply => {
                let backend = detected.expect("form Some implies backend Some");
                let allow_network = home.map(backend::read_executor_allow_network);
                let conf =
                    SandboxConf::for_executor(Path::new(workspace), allow_network.unwrap_or(false));
                match backend.apply_to_self(&conf) {
                    Ok(Enforcement::Full) => tracing::info!(
                        "[executor] userland sandbox '{}' fully enforced (writable: {})",
                        backend.name(),
                        workspace
                    ),
                    Ok(Enforcement::Partial(gaps)) => tracing::warn!(
                        "[executor] userland sandbox '{}' PARTIAL (rules applied with \
                         gaps): {gaps:?}",
                        backend.name()
                    ),
                    Err(err) => {
                        if strict {
                            anyhow::bail!(
                                "strict mode (fail-closed): userland sandbox '{}' apply \
                                 failed: {err} — refusing to run unsandboxed",
                                backend.name()
                            );
                        }
                        tracing::warn!(
                            "[executor] userland sandbox '{}' apply failed: {err} — running \
                             unsandboxed",
                            backend.name()
                        );
                    }
                }
                Ok(Outcome::Continue)
            }
            Plan::WrapReexec => {
                let backend = detected.expect("form Some implies backend Some");
                let allow_network = home.map(backend::read_executor_allow_network);
                let conf =
                    SandboxConf::for_executor(Path::new(workspace), allow_network.unwrap_or(false));
                reexec_wrapped(backend, conf)
            }
        }
    }

    /// re-exec 自身进盒（bwrap / sandbox-exec）：外层进程退化为 stdio 代理，
    /// 工具全在盒内实例里跑。gateway 的 stdio 协议原样透传。
    fn reexec_wrapped(backend: Arc<dyn SandboxBackend>, conf: SandboxConf) -> Result<Outcome> {
        let exe = std::env::current_exe().context("resolve current exe for re-exec")?;
        let mut inner = std::process::Command::new(&exe);
        // env 继承自本进程（gateway 给的 ROLE/WORKSPACE/SANDBOX 都在）；
        // REEXEC 防环（盒内实例见到它就跳过沙盒介入）。
        inner.env("NEMESISBOT_EXECUTOR_REEXEC", "1");
        let mut wrapped = backend
            .wrap_command(&conf, &inner)
            .map_err(|e| anyhow::anyhow!("wrap executor with {}: {e}", backend.name()))?;
        wrapped
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());
        let mut child = wrapped
            .spawn()
            .with_context(|| format!("spawn {}-wrapped executor", backend.name()))?;
        tracing::info!(
            "[executor] re-exec'd into {} sandbox (stdin/stdout proxied)",
            backend.name()
        );

        let mut inner_stdin = child.stdin.take().expect("piped inner stdin");
        let mut inner_stdout = child.stdout.take().expect("piped inner stdout");
        let t_in = std::thread::spawn(move || {
            let _ = std::io::copy(&mut std::io::stdin(), &mut inner_stdin);
        });
        let t_out = std::thread::spawn(move || {
            let _ = std::io::copy(&mut inner_stdout, &mut std::io::stdout());
            let _ = std::io::Write::flush(&mut std::io::stdout());
        });

        let _ = t_in.join();
        let status = child.wait().context("wait wrapped executor")?;
        let _ = t_out.join();
        Ok(Outcome::ReexecDone(status))
    }
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

#[cfg(test)]
mod tests;
