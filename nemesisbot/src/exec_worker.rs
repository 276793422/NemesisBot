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
//! the 2026-08-23 remaining-goal plan doc (W2 / U11, local archive).

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
            // D4（三轮复查根修）：workspace-dacl 受限令牌 spawn 时 fence 已由
            // 内核强制——engage 的 Plain 臂不再因「无用户态后端」触发 strict
            // 拒绝或「unsandboxed」warn（engage 内见 spawn_fenced 注释）。
            let spawn_fenced =
                std::env::var("NEMESISBOT_SANDBOX_BACKEND").as_deref() == Ok("workspace-dacl");
            match userland::engage(&workspace, home.as_deref(), spawn_fenced) {
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

    // D4（DACL 定向档，2026-09-27）：受限令牌树下 console 分配面治理。
    // gateway 经 workspace-dacl hook 注入的 env 标记在本进程可见 → 先尽力
    // **附着**父进程 console（附着=打开既有 condrv 非写类，白名单下放行；
    // 新分配才会死，见 nemesis-sandbox token.rs 实证）。附着成功 = 本树内
    // 默认 flags spawn 继承 console，第三方链式工具链（cargo→rustc 类）可
    // 用；失败（gateway console-less，服务化启动）= 全链 DETACHED 降级。
    // 结果钉进进程 env（NEMESISBOT_CONSOLE=1/0）供 exec 工具选 creation
    // flags——本线程此刻在 tokio runtime 构建前，进程内无并发 env 读者。
    #[cfg(all(target_os = "windows", feature = "sandbox"))]
    if std::env::var("NEMESISBOT_SANDBOX_BACKEND").as_deref() == Ok("workspace-dacl") {
        let attached = nemesis_sandbox::backend::attach_parent_console();
        // SAFETY: 单一 executor 线程、runtime 未建、主线程阻塞在 join——
        // 进程内无并发 env 访问者。
        unsafe {
            std::env::set_var("NEMESISBOT_CONSOLE", if attached { "1" } else { "0" });
        }
        tracing::info!("[executor] workspace-dacl console attach: {attached}");
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
        // A5（2026-09-04）：子进程侧同样设界——executor 工作区即边界根
        // （restrict 恒 true：能被剥离执行的操作本就该只碰工作区），与
        // gateway 侧纵深防御一致（8 层管线已在 dispatch 前跑过）。
        workspace_boundary: Some(std::sync::Arc::new(
            nemesis_agent::loop_tools::WorkspaceBoundary {
                root: std::path::PathBuf::from(workspace),
                restrict: true,
            },
        )),
        // WS9/P22：executor 侧租约——gateway 在 spawn 前经
        // `NEMESISBOT_LEASE=1` 透传「主进程开了租约」（executor 子进程不
        // 读用户 config，装配语义由父进程钉死）。持有者名带本子进程 PID。
        workspace_lease: if std::env::var("NEMESISBOT_LEASE").as_deref() == Ok("1") {
            Some(std::sync::Arc::new(
                nemesis_agent::workspace_lease::WorkspaceLease::new(
                    std::path::Path::new(workspace),
                    &format!("executor:pid:{}", std::process::id()),
                ),
            ))
        } else {
            None
        },
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

    // P21（2026-09-25）：拒绝台账钩子是 exec_worker 顶层的兄弟模块，这里
    // 引入后在 engage / reexec 各失败臂直接记账。
    use crate::exec_worker::sandbox_denial;

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
    ///
    /// `spawn_fenced`（三轮复查根修）：`NEMESISBOT_SANDBOX_BACKEND=workspace-
    /// dacl` 时为 true——gateway 已以 write-restricted 受限令牌 spawn 本进程，
    /// 内核写围栏在 spawn 时即成立（nemesis-sandbox token.rs）。此时
    /// Plan::Plain（无用户态后端可选，auto 不回落实验档）**不再触发 strict
    /// 拒绝也不报「unsandboxed」**：strict 要的是「不在无盒状态跑命令」，
    /// 该实质已满足；否则 dacl+strict 组合会被「无用户态后端」虚假全拒
    /// （用户视角围栏明明活着）。backend="acl" 显式选装的叠加层（SelfApply）
    /// 不受影响——那是另一条 arm，其 strict fail-closed 语义保留。
    pub fn engage(
        workspace: &str,
        home: Option<&Path>,
        spawn_fenced: bool,
    ) -> Result<Outcome> {
        let strict = home.map(backend::read_executor_strict).unwrap_or(false);
        // P1（2026-09-25）：先读网络要求再选后端——禁网 + bwrap 可用 → 选
        // bwrap（--unshare-net 真禁网）；landlock 仅在允许网络或无 bwrap 时
        // 上岗（降级时 apply_to_self 的 gaps 仍诚实标注网络缺口）。
        let allow_network = home
            .map(backend::read_executor_allow_network)
            .unwrap_or(false);
        let detected = backend::detect_backend(allow_network);
        // P24（2026-09-26）：Windows 无平台默认后端（Sandboxie 盒路径不经
        // userland engage）——**显式 `executor.backend = "acl"`** 且本机可用
        // 时 AclBackend（用户态完整性围栏，恒 Partial）opt-in 上岗。auto/
        // sandboxie/未知一律维持 None：auto 档不回落用户态实验档（默认行为
        // 字节不变——2026-09-26 全量回归实证 auto 回落会让既有 executor
        // 子进程测试的「无盒 warn」静默变成「真实装围栏」，strict 语义也被
        // 改写），显式钉 acl 才启用。
        #[cfg(all(target_os = "windows", feature = "sandbox"))]
        let detected = detected.or_else(|| {
            let choice = home
                .as_deref()
                .map(backend::read_executor_backend)
                .unwrap_or(backend::ExecutorBackendChoice::Auto);
            if !matches!(choice, backend::ExecutorBackendChoice::Acl) {
                return None;
            }
            let acl = backend::AclBackend::new();
            match acl.availability() {
                backend::Availability::Unavailable(_) => None,
                _ => Some(Arc::new(acl) as Arc<dyn SandboxBackend>),
            }
        });
        let form = detected
            .as_ref()
            .map(|b: &Arc<dyn SandboxBackend>| b.form());
        match plan(true, false, form) {
            Plan::Plain => {
                if spawn_fenced {
                    // workspace-dacl：write-restricted 令牌在 spawn 时已把内核
                    // 写围栏装上（gateway 铸造 + CreateProcessAsUserW，本进程
                    // 无法自证但 env 标签由可信父进程注入）——不是
                    // 「unsandboxed」。用户态层未选装不构成 strict 拒绝理由，
                    // 也不再打「running unsandboxed」误导 warn。
                    tracing::info!(
                        "[executor] spawn-time workspace-dacl fence active (write-restricted \
                         token); no userland layer selected — continuing"
                    );
                    return Ok(Outcome::Continue);
                }
                if strict {
                    let reason = "no userland sandbox backend is available on this system";
                    sandbox_denial::record(
                        "none",
                        "sandbox_engage_refused",
                        workspace,
                        reason,
                        true,
                        workspace,
                    );
                    anyhow::bail!(
                        "strict mode (fail-closed): executor.sandbox is on but {} — \
                         refusing to run unsandboxed",
                        reason
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
                let conf = SandboxConf::for_executor(Path::new(workspace), allow_network);
                match backend.apply_to_self(&conf) {
                    Ok(Enforcement::Full) => {
                        sandbox_denial::mark_backend_engaged(backend.name());
                        tracing::info!(
                            "[executor] userland sandbox '{}' fully enforced (writable: {})",
                            backend.name(),
                            workspace
                        )
                    }
                    Ok(Enforcement::Partial(gaps)) => {
                        // Partial = 规则已装上（缺口如禁网不可强制）——沙盒
                        // engaged，dispatch 侧拒绝分类照常记台账。
                        sandbox_denial::mark_backend_engaged(backend.name());
                        tracing::warn!(
                            "[executor] userland sandbox '{}' PARTIAL (rules applied with \
                             gaps): {gaps:?}",
                            backend.name()
                        )
                    }
                    Err(err) => {
                        if strict {
                            sandbox_denial::record(
                                backend.name(),
                                "sandbox_engage_refused",
                                workspace,
                                &err,
                                true,
                                workspace,
                            );
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
                let conf = SandboxConf::for_executor(Path::new(workspace), allow_network);
                reexec_wrapped(backend, conf, workspace)
            }
        }
    }

    /// re-exec 自身进盒（bwrap / sandbox-exec）：外层进程退化为 stdio 代理，
    /// 工具全在盒内实例里跑。gateway 的 stdio 协议原样透传。
    fn reexec_wrapped(
        backend: Arc<dyn SandboxBackend>,
        conf: SandboxConf,
        workspace: &str,
    ) -> Result<Outcome> {
        let exe = std::env::current_exe().context("resolve current exe for re-exec")?;
        let mut inner = std::process::Command::new(&exe);
        // env 继承自本进程（gateway 给的 ROLE/WORKSPACE/SANDBOX 都在）；
        // REEXEC 防环（盒内实例见到它就跳过沙盒介入）。P21：盒内实例的
        // dispatch 靠这个键知道「自己在哪个后端里」（台账 backend 字段）。
        inner.env("NEMESISBOT_EXECUTOR_REEXEC", "1");
        inner.env("NEMESISBOT_SANDBOX_BACKEND", backend.name());
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
        if !status.success() {
            // P21：盒内实例非零退出（bwrap 包装层拒绝/失败）记台账。此时本
            // 进程只是 stdio 代理、无法把结构化错误回注 gateway → 模型不可见。
            sandbox_denial::record(
                backend.name(),
                "executor_reexec_failed",
                workspace,
                &format!("wrapped executor exit: {status}"),
                false,
                workspace,
            );
        }
        Ok(Outcome::ReexecDone(status))
    }
}

// ---------------------------------------------------------------------------
// P21（2026-09-25）：沙盒拒绝台账钩子（sandbox feature 门控——trim 构建无
// 沙盒 → 无台账面）。核心语义见 nemesis_sandbox::denial 模块文档。
// ---------------------------------------------------------------------------
#[cfg(feature = "sandbox")]
mod sandbox_denial {
    use nemesis_sandbox::denial;

    /// engage 成功装上后端时标记（SelfApply 同进程路径专用；盒内实例一律走
    /// env 注入后端名——bwrap reexec / Windows 真盒都是，见
    /// [`active_backend_label`]）。
    static ENGAGED_BACKEND: std::sync::OnceLock<String> = std::sync::OnceLock::new();

    pub(super) fn mark_backend_engaged(name: &str) {
        let _ = ENGAGED_BACKEND.set(name.to_string());
    }

    /// 当前沙盒后端标签（engaged 才有）：landlock 自装 = engage 标记；盒内
    /// 实例 = env 注入的后端名（bwrap reexec / Windows 真盒 Start.exe wrap
    /// 时由 gateway 注入 `sandboxie`——标签证据化，见 remote_executor_tool
    /// 的 spawn_and_call_pipe）。**不再从 `NEMESISBOT_EXECUTOR_PIPE` 推断**：
    /// PIPE 只是传输通道选择，无盒 PIPE 传输（transport test / 降级装配）下
    /// 推断出的 "sandboxie" 是冒标——会把普通错误误记进沙盒拒绝台账。
    /// None = 无沙盒 → 工具错误与沙盒无关，不记台账不改写文案。
    pub(crate) fn active_backend_label() -> Option<String> {
        if let Some(b) = ENGAGED_BACKEND.get() {
            return Some(b.clone());
        }
        std::env::var("NEMESISBOT_SANDBOX_BACKEND")
            .ok()
            .filter(|s| !s.is_empty() && s != "none")
    }

    /// 记一条到台账（append 失败 = warn 放行，永不阻断工具执行/退出路径）。
    pub(super) fn record(
        backend: &str,
        op: &str,
        target: &str,
        reason: &str,
        model_visible: bool,
        workspace: &str,
    ) {
        let rec = denial::new_record(backend, op, target, reason, model_visible);
        if let Err(e) = denial::append_denial(std::path::Path::new(workspace), &rec) {
            tracing::warn!("[executor] sandbox denial ledger append failed (ignored): {e}");
        }
    }

    /// 工具错误出口（dispatch Err 臂调用）：沙盒 engaged 且错误长得像沙盒
    /// 拒绝 → 记台账 + 改写为面向模型的可自纠文案；否则原样返回（普通
    /// 工具错误不记台账、不换文案）。
    pub(crate) fn on_tool_error(tool: &str, args: &str, error: &str) -> String {
        let Some(backend) = active_backend_label() else {
            return error.to_string();
        };
        if !denial::looks_like_denial(error) {
            return error.to_string();
        }
        let workspace = std::env::var("NEMESISBOT_EXECUTOR_WORKSPACE").unwrap_or_default();
        let target = denial::preview_target(args);
        record(&backend, tool, &target, error, true, &workspace);
        denial::model_facing_text(&backend, tool, &target, error, &workspace)
    }
}

/// Named-pipe transport loop (sandbox mode).
#[cfg(windows)]
async fn pipe_loop(
    stream: nemesis_agent::executor_pipe::NamedPipeClient,
    tools: &HashMap<String, Box<dyn Tool>>,
) -> Result<()> {
    // BufReader 必须活过整个循环：内部缓冲跨请求保留（每请求新建 reader 会
    // 把缓冲里已收到的后续请求字节随旧 reader 一起丢弃——多行一次到达时
    // 请求被吞）。读走 read_line（AsyncBufReadExt，共享同一缓冲），写经
    // get_mut 穿透借用同一 stream。
    let mut reader = BufReader::new(stream);
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line).await {
            Ok(0) => return Ok(()), // gateway closed → exit cleanly
            Ok(_) => {}
            Err(e) => return Err(anyhow::anyhow!("pipe read: {e}")),
        };
        let line = line.trim_end_matches(['\n', '\r']);
        let resp = dispatch(tools, line).await;
        let mut out = serde_json::to_string(&resp).unwrap_or_else(|_| {
            r#"{"ok":false,"result":"","error":"response serialize failed"}"#.to_string()
        });
        out.push('\n');
        let stream = reader.get_mut();
        stream.write_all(out.as_bytes()).await.context("pipe write")?;
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
        Err(error) => {
            // P21（2026-09-25）：沙盒 engaged 且错误长得像沙盒拒绝 → 记台账 +
            // 改写为面向模型的可自纠文案；普通错误原样透传（trim 构建无沙盒
            // → 无台账面，feature 门控）。
            #[cfg(feature = "sandbox")]
            let error = sandbox_denial::on_tool_error(&req.tool, &req.args, &error);
            ExecutorResponse {
                ok: false,
                result: String::new(),
                error,
            }
        }
    }
}

#[cfg(test)]
mod tests;
