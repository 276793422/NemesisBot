//! Remote executor: run execution-class tools in a separate child process.
//!
//! Gateway-side bridge. Each MOVE tool (exec/file/grep/git/...) is wrapped in a
//! [`RemoteExecutorTool`] that delegates metadata (`description` / `parameters`
//! / `preview`) to the local tool impl — so the LLM sees byte-identical schemas
//! and the checkpoint (edit safety net) still snapshots file writes — but routes
//! `execute()` to a freshly-spawned `nemesisbot` child running in executor role.
//!
//! Two transports, picked by `sandbox`:
//! - **stdio** (sandbox=false, Layer 1): spawn child, exchange JSON over its
//!   stdin/stdout.
//! - **named pipe** (sandbox=true, Layer 2): `Start.exe` does not forward stdio
//!   across the box boundary, so the sandboxed path uses a Windows named pipe
//!   `\\.\pipe\NemesisBox_<id>` instead.
//!
//! The spawn command is controlled INDEPENDENTLY by `start_exe`:
//! - `None` → spawn the executor directly (Layer 1, or L2.1 transport testing).
//! - `Some` → wrap with `Start.exe /box:<box>` (L2.2 real Sandboxie containment).
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md` (Layer 1) and
//! `docs/PLAN/2026-07-09_sandboxie-integration.md` (Layer 2).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

use crate::context::RequestContext;
use crate::r#loop::{FileChange, Tool};

/// Tools that run in the executor child (the MOVE set): execution-class tools
/// with side effects that don't need gateway in-memory state — exactly the
/// LLM-driven dangerous-ops surface the sandbox (Layer 2) will wrap.
///
/// Kept local (STAY) because they need gateway objects (bus / vector store /
/// scheduler / engine / registries): `message`, `cluster_rpc`, `cron`,
/// `memory_*`, `find_skills` / `install_skill` / `skill_manage`, `workflow_run`,
/// `forge_bridge`, `mcp_*`.
///
/// Note: `exec_async` (background processes) is intentionally NOT here — a
/// per-call child exits after one tool call, so a background-process handle it
/// returned would be orphaned. It stays local until a long-lived executor model
/// is introduced. `sleep` likewise stays local (no isolation value). The B4
/// `background_start`/`background_output`/`background_kill` trio is local for
/// the same reason, more strictly: the job registry lives in the gateway
/// process (`SharedResources.background_registry`) and the exec_worker child
/// doesn't even register the tools (`SharedToolConfig.background_registry`
/// is `None` there).
pub const MOVE_TOOLS: &[&str] = &[
    "exec",
    "run_script",
    // C8 (2026-09-06): run_checks spawns cargo/npm/go builds — same write
    // surface as exec (target/, node_modules/), so executor separation and
    // sandbox contain it identically. It waits in-call, so a per-call
    // subprocess is safe.
    "run_checks",
    "read_file",
    "write_file",
    "list_dir",
    "edit_file",
    // A7 (2026-09-06): multiedit is a batch edit_file — same write surface,
    // contained identically (in-memory apply + write-all happens in the child).
    "multiedit",
    "append_file",
    "delete_file",
    "create_dir",
    "delete_dir",
    "grep",
    "git",
];

// ---------------------------------------------------------------------------
// Wire protocol — one JSON line per direction (newline-delimited)
// ---------------------------------------------------------------------------

// P3 T1：wire 结构收编到 [`crate::executor_transport`]（gateway / exec_worker
// 两侧单一真相源；per-call 路径产出 v1 字节形态不变——`ExecutorRequestRef`
// 空 `v`/`call_id` 跳序列化）。
use crate::executor_transport::{ExecutorRequestRef, ExecutorResponseWire, StderrTail};

// ---------------------------------------------------------------------------
// ExecutorChannel — spawns a fresh child per call, one request → one response
// ---------------------------------------------------------------------------

/// Type alias for the P5-2 strict fail-closed gate (keeps the struct decl and
/// `spawn_and_call` readable).
pub type StrictGate = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// P24：Windows 盒 wrap 缺位时「用户态 ACL 档可否顶上」的选型钩子（gateway
/// 注入；nemesis-agent 刻意不依赖 nemesis-sandbox，决策以闭包传入——与
/// `sandbox_probe` 同款模式）。true = 降级改走 stdio + 用户态标记，子进程
/// engage 按 `executor.backend` 选型自装 ACL 围栏。
pub type UserlandFallback = Arc<dyn Fn() -> bool + Send + Sync>;

// ---------------------------------------------------------------------------
// DACL 定向档（D3，2026-09-27）——受限令牌 spawn 事务的闭包 hook
// ---------------------------------------------------------------------------

/// DACL 定向档 spawn 请求（nemesis-agent 本地类型——刻意不依赖
/// nemesis-sandbox；gateway 注入的闭包把它转译成 `CreateProcessAsUserW`
/// + write-restricted 令牌的同步事务）。
pub struct DaclSpawnRequest {
    pub exe: PathBuf,
    /// executor 子进程无 CLI args（`NEMESISBOT_ROLE` env 检测路由）。
    pub args: Vec<String>,
    pub env_extra: Vec<(String, String)>,
    /// 单行 JSON 请求（与 stdio 路径同一协议行）。
    pub request_line: String,
    /// 事务内 `WaitForSingleObject` 超时窗（与 [`ExecutorChannel::timeout`] 同源）。
    pub timeout: Duration,
}

/// DACL 定向档 spawn 结果（与 nemesis-sandbox `TxnOutcome` 字段同构——
/// agent 侧类型本地定义，gateway 闭包负责逐字段转译）。
pub struct DaclSpawnOutcome {
    /// stdout 首行（executor 协议响应行；`None` = 无响应退出）。
    pub response: Option<String>,
    pub exit_code: Option<u32>,
    pub stderr_tail: String,
}

/// 一次 DACL spawn 事务（同步阻塞 FFI——调用方 `spawn_blocking` 包住）。
pub type DaclSpawnFn =
    Arc<dyn Fn(DaclSpawnRequest) -> Result<DaclSpawnOutcome, String> + Send + Sync>;

/// 每次工具调用询问 gateway：DACL 定向档现在在场吗？（live 读
/// `executor.acl.dacl`，热生效语义与 `sandbox_probe` 一致。）
/// - `Ok(Some(fn))` = 在场（令牌已备）→ 走 DACL spawn 事务；
/// - `Ok(None)` = 未启用，或宽松模式下装配失败已降级 → 走现状路径；
/// - `Err(reason)` = `acl.strict` fail-closed → 拒绝执行（不静默降级）。
pub type DaclSpawnHook = Arc<dyn Fn() -> Result<Option<DaclSpawnFn>, String> + Send + Sync>;

/// Spawn configuration for executor children. Holds no mutable state, so a
/// single `Arc<ExecutorChannel>` is shared by every `RemoteExecutorTool`.
pub struct ExecutorChannel {
    /// Path to the nemesisbot executable (the gateway's own exe).
    pub exe_path: PathBuf,
    /// Resolved workspace path, passed to the child via env so it does not
    /// re-run path resolution (which depends on `--local` / NEMESISBOT_HOME).
    pub workspace: String,
    /// Live probe for "should this call go through the box?" — queried on
    /// EVERY tool call so toggling `executor.sandbox` takes effect without a
    /// gateway restart. Injected by the factory (which reads ConfigStore);
    /// nemesis-agent deliberately does not depend on nemesis-config, so the
    /// decision is passed in as a closure rather than a stored bool.
    pub sandbox_probe: Arc<dyn Fn() -> bool + Send + Sync>,
    /// P5-2 严格模式 fail-closed 闸门（跨平台统一语义）：`Some` 时，凡
    /// `sandbox_probe()` 为 true（本次调用**要求**沙盒）的调用先过闸门——
    /// 闸门 `Err` 则**拒绝执行**并返回明确错误（fail-closed），不再走
    /// "warn + 无盒降级"（fail-open，现状）。闸门内部自行 live 读
    /// `executor.strict`（经注入的 ConfigStore 闭包）：false 时秒过、行为
    /// 与现状逐字节一致；true 时做平台就绪性复检（Windows=Sandboxie
    /// 引擎；非 Windows=用户态后端可用性；trim 构建=恒拒）。`None` = 未
    /// 注入（测试/裸构造）= 无闸门 = 现状。
    pub strict_gate: Option<StrictGate>,
    /// Sandboxie `Start.exe` path. `Some` → spawn via `Start.exe /box:<box>`
    /// (real containment, L2.2). `None` → spawn the executor directly (Layer 1,
    /// or L2.1 transport testing without the box).
    pub start_exe: Option<PathBuf>,
    /// Sandboxie box name.
    pub box_name: String,
    /// App home（gateway 侧解析好的）。U11：传给子进程读
    /// `<home>/config.json` 的 `executor.allow_network`（用户态沙盒禁网
    /// 开关；`None` = 测试/裸构造，子进程按默认 false 处理）。
    pub home: Option<PathBuf>,
    /// WS9/P22：gateway 侧租约开关透传——true 时 build_command 给子进程
    /// 设 `NEMESISBOT_LEASE=1`，executor 侧据此构造同根租约（写类工具互
    /// 斥跨进程）。子进程不读用户 config，装配语义由父进程钉死。
    pub lease_child: bool,
    /// P24：Windows 盒 wrap 缺位时「用户态 ACL 档可顶」选型钩子（gateway
    /// 注入，见 [`UserlandFallback`]）。`None` = 未注入（测试/裸构造）=
    /// 现状（无盒直接 spawn）字节不变。
    pub userland_fallback: Option<UserlandFallback>,
    /// D3：DACL 定向档 spawn hook（gateway 注入，见 [`DaclSpawnHook`]）。
    /// `None` = 未注入（测试/裸构造/trim 构建）= 现状字节不变。盒 wrap
    /// （`start_exe`）在场时本 hook 不抢（Sandboxie 更强，结构性防线）。
    pub dacl_spawn: Option<DaclSpawnHook>,
    /// Per-call hard timeout (the child must respond within this).
    pub timeout: Duration,
}

impl ExecutorChannel {
    /// Construct a channel in Layer-1 / L2.1 mode (direct spawn, no Start.exe
    /// wrap). L2.2 sets `start_exe` via the `with_start_exe` builder.
    /// `sandbox_probe` is called per tool call to pick stdio vs named-pipe
    /// transport, so it must reflect the live config (the factory wires it to
    /// ConfigStore).
    pub fn new(
        exe_path: PathBuf,
        workspace: String,
        sandbox_probe: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            exe_path,
            workspace,
            sandbox_probe,
            // None = 无严格闸门（fail-open，现状）；gateway 装配侧
            // （exec_world）按平台注入。
            strict_gate: None,
            start_exe: None,
            box_name: "NemesisBox".to_string(),
            home: None,
            lease_child: false,
            userland_fallback: None,
            dacl_spawn: None,
            timeout: Duration::from_secs(24 * 3600),
        }
    }

    /// Set the app home (child reads `executor.allow_network` from its
    /// config.json; U11 userland sandbox network switch).
    pub fn with_home(mut self, home: PathBuf) -> Self {
        self.home = Some(home);
        self
    }

    /// Set the Sandboxie `Start.exe` path (L2.2: wraps the spawn for real box).
    #[allow(dead_code)]
    pub fn with_start_exe(mut self, start_exe: PathBuf) -> Self {
        self.start_exe = Some(start_exe);
        self
    }

    /// Set the P5-2 strict fail-closed gate (see [`ExecutorChannel::strict_gate`]).
    /// Injected by the gateway assembly (`exec_world`); tests use it to prove
    /// refusal happens BEFORE any spawn.
    pub fn with_strict_gate(mut self, gate: StrictGate) -> Self {
        self.strict_gate = Some(gate);
        self
    }

    /// Set the per-call timeout (use a short one in tests).
    #[allow(dead_code)]
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// WS9/P22：透传租约开关给 executor 子进程（见 [`Self::lease_child`]）。
    /// gateway 装配侧（exec_world）按 `agents.lease_enabled` 注入；测试/裸
    /// 构造默认 false（现状不变）。
    pub fn with_lease_child(mut self, lease: bool) -> Self {
        self.lease_child = lease;
        self
    }

    /// P24：注入 Windows 盒缺位时的用户态 ACL 选型钩子（仅 stdio 通道——
    /// 即无 `with_start_exe` 的构造——有意义；盒 wrap 在场时判定短路）。
    pub fn with_userland_fallback(mut self, fallback: UserlandFallback) -> Self {
        self.userland_fallback = Some(fallback);
        self
    }

    /// D3：注入 DACL 定向档 spawn hook（仅 Windows + gateway 装配侧；见
    /// [`ExecutorChannel::dacl_spawn`]）。盒 wrap 在场时 hook 不抢。
    pub fn with_dacl_spawn(mut self, hook: DaclSpawnHook) -> Self {
        self.dacl_spawn = Some(hook);
        self
    }

    /// P24：Windows dispatch 通道判定（纯函数，单测可达）——盒 wrap 缺位
    /// 且用户态 fallback 判定可顶 → stdio + 用户态标记（子进程自装 ACL）；
    /// 其余（盒在场 / 未注入 / 判定 false）→ 管道（现状）。
    #[cfg(windows)]
    pub(crate) fn picks_stdio_userland_fallback(&self) -> bool {
        self.start_exe.is_none() && self.userland_fallback.as_ref().is_some_and(|f| f())
    }

    /// 子进程 env 注入（stdio / 管道 / DACL 三条 spawn 路径同源——单一真
    /// 相源；DACL 路径在此基础上再叠 userland 标记与台账标签，见
    /// [`Self::spawn_and_call_dacl`]）。
    fn env_pairs(&self) -> Vec<(String, String)> {
        let mut env = vec![
            ("NEMESISBOT_ROLE".to_string(), "executor".to_string()),
            (
                "NEMESISBOT_EXECUTOR_WORKSPACE".to_string(),
                self.workspace.clone(),
            ),
        ];
        // WS9/P22：租约透传（true 时才设——省 env 不含语义，子进程缺省 false）。
        if self.lease_child {
            env.push(("NEMESISBOT_LEASE".to_string(), "1".to_string()));
        }
        if let Some(home) = &self.home {
            env.push((
                "NEMESISBOT_EXECUTOR_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            ));
        }
        env
    }

    /// Build the spawn command. The wrap is controlled by `start_exe`:
    /// - `Some` → `Start.exe /box:<box> nemesisbot.exe` (L2.2 real box).
    /// - `None` → `nemesisbot.exe` directly (Layer 1 / L2.1 transport-only).
    fn build_command(&self) -> Command {
        let mut cmd = if let Some(start) = &self.start_exe {
            let mut c = Command::new(start);
            c.arg(format!("/box:{}", self.box_name));
            // /hide_window → Start.exe passes SW_HIDE to the boxed child's
            // STARTUPINFO (start.cpp:754-759), preventing the console window
            // from flashing on each per-call sandboxed spawn.
            c.arg("/hide_window");
            c.arg(&self.exe_path);
            c
        } else {
            Command::new(&self.exe_path)
        };
        cmd.envs(self.env_pairs());
        // Prevent a console window from flashing on each per-call spawn (every
        // tool call spawns a fresh child; without this, Windows pops a black
        // console window that disappears when the child exits).
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }

    /// Spawn a child, send one request, read one response, reap.
    pub async fn spawn_and_call(
        &self,
        tool: &str,
        args: &str,
        ctx: &RequestContext,
    ) -> Result<String, String> {
        let request_line = self.build_request_line(tool, args, ctx)?;
        // D3：DACL 定向档 hook 最先询问（kernel 强制写围栏 > 子进程自装完
        // 整性档）。盒 wrap（start_exe）在场时不抢——Sandboxie 是更强档，
        // 这里是结构性防线，选型归 gateway 闭包语义管辖。
        // Ok(None) = 未启用/宽松降级 → 走现状分支（字节不变）；Err =
        // `acl.strict` fail-closed 拒绝。
        if self.start_exe.is_none()
            && let Some(hook) = &self.dacl_spawn
        {
            match hook() {
                Ok(Some(spawn_fn)) => {
                    return self
                        .spawn_and_call_dacl(spawn_fn, &request_line, tool)
                        .await;
                }
                Ok(None) => {}
                Err(reason) => {
                    tracing::warn!("[Executor] acl.strict refusing '{tool}': {reason}");
                    return Err(format!(
                        "executor.acl strict (fail-closed): {reason} — refusing to \
                         run '{tool}' without the workspace-dacl fence"
                    ));
                }
            }
        }
        if (self.sandbox_probe)() {
            // P5-2 严格模式（fail-closed）：本次调用要求沙盒（sandbox_probe
            // 为 true），先过严格闸门——不过则拒绝执行，绝不静默降级成无盒。
            // 闸门内部 live 读 executor.strict：false 时秒过（现状字节不变）。
            if let Some(gate) = &self.strict_gate
                && let Err(reason) = gate()
            {
                tracing::warn!(
                    "[Executor] strict mode refusing '{tool}': sandbox required \
                         but unavailable: {reason}"
                );
                return Err(format!(
                    "strict mode (fail-closed): executor.sandbox=true but the \
                         sandbox backend is unavailable ({reason}) — refusing to run \
                         '{tool}' unsandboxed"
                ));
            }
            #[cfg(windows)]
            {
                // P24（2026-09-26）：盒 wrap 缺位且 gateway 选型判定「用户态
                // ACL 档可顶」→ 降级不走裸 spawn（L2.1 无盒），改走 stdio +
                // 用户态标记——子进程 engage 按 executor.backend 选型自装
                // ACL 完整性围栏（恒 Partial，实验档）。未注入/false = 现状
                // 字节不变。注意 strict 闸门在上面已跑过且仍只认 Sandboxie
                // 引擎（ACL 不算 strict 合格沙盒）。
                if self.picks_stdio_userland_fallback() {
                    return self.spawn_and_call_stdio(tool, &request_line, true).await;
                }
                return self.spawn_and_call_pipe(tool, &request_line).await;
            }
            #[cfg(not(windows))]
            {
                // U11: non-Windows userland sandbox (landlock 优先 / bwrap 兜底，
                // 见 nemesis_sandbox::backend)。与 Sandboxie 不同，用户态后端不
                // 改变 stdio 通路——子进程（exec_worker）看到 env 标记后在启动时
                // 对自身装上限制（自装式），stdio 传输照常。降级语义在子进程侧：
                // 后端不可用 → warn + 无盒继续（fail-open 默认；strict=true 时
                // gateway 侧闸门已在上面拒绝 + 子进程侧 engage 同样拒绝，双保险）。
                return self.spawn_and_call_stdio(tool, &request_line, true).await;
            }
        }
        self.spawn_and_call_stdio(tool, &request_line, false).await
    }

    fn build_request_line(
        &self,
        tool: &str,
        args: &str,
        ctx: &RequestContext,
    ) -> Result<String, String> {
        let context_value =
            serde_json::to_value(ctx).map_err(|e| format!("serialize context: {e}"))?;
        // per-call = v1 线形态（v/call_id 空值跳序列化，字节与历史一致）。
        let request = ExecutorRequestRef {
            v: 0,
            call_id: "",
            tool,
            args,
            context: context_value,
        };
        let mut line = serde_json::to_string(&request)
            .map_err(|e| format!("serialize executor request: {e}"))?;
        line.push('\n');
        Ok(line)
    }

    fn parse_response(resp_line: &str) -> Result<String, String> {
        let resp: ExecutorResponseWire =
            serde_json::from_str(resp_line).map_err(|e| format!("parse executor response: {e}"))?;
        if resp.ok {
            Ok(resp.result)
        } else if resp.error.is_empty() {
            Err("executor returned an error".to_string())
        } else {
            Err(resp.error)
        }
    }

    /// Drain child stderr in the background and retain the tail for failure
    /// diagnostics. 实现收编到 [`StderrTail`]（P3 T1：per-call 与 persistent
    /// 会话共用单一真相源）。
    fn drain_stderr(child: &mut tokio::process::Child) -> StderrTail {
        StderrTail::spawn(child.stderr.take())
    }

    /// stdio transport (sandbox=false, or non-Windows userland sandbox): write
    /// stdin, read stdout. `userland_sandbox` sets the env marker the child
    /// (`exec_worker`) reads at startup to self-apply landlock/bwrap (U11).
    async fn spawn_and_call_stdio(
        &self,
        tool: &str,
        request_line: &str,
        userland_sandbox: bool,
    ) -> Result<String, String> {
        let mut cmd = self.build_command();
        if userland_sandbox {
            cmd.env("NEMESISBOT_EXECUTOR_SANDBOX", "1");
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // 异常路径兜底：future 被取消（estop 中断/调用方 drop）时 Child 随
        // 之 drop——kill_on_drop 保证不留活孤儿进程（有序路径已在收尾
        // start_kill + wait，此开关不生效，无副作用）。
        cmd.kill_on_drop(true);

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn executor child: {e}"))?;
        let stderr_tail = Self::drain_stderr(&mut child);

        // Write the single request line, then drop stdin to signal EOF.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(request_line.as_bytes()).await;
            let _ = stdin.flush().await;
            drop(stdin);
        }

        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "executor child has no stdout".to_string())?;
        let mut reader = BufReader::new(stdout).lines();
        let resp_line = match tokio::time::timeout(self.timeout, reader.next_line()).await {
            Err(_) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor timed out after {:?} (tool={tool})",
                    self.timeout
                ));
            }
            Ok(Err(e)) => {
                let _ = child.start_kill();
                return Err(format!("read executor response: {e}"));
            }
            Ok(Ok(None)) => {
                let _ = child.start_kill();
                // 带退出状态 + stderr tail（静默死亡=panic/OOM/启动失败的
                // 唯一诊断面；DACL 路径同款格式）。
                let status = child.wait().await;
                return Err(format!(
                    "executor child exited without a response (tool={tool}, \
                     exit={:?}); stderr: {}",
                    status.map(|s| s.to_string()),
                    stderr_tail.snapshot()
                ));
            }
            Ok(Ok(Some(line))) => line,
        };

        let _ = child.wait().await;
        Self::parse_response(&resp_line)
    }

    /// DACL 定向档 spawn 事务（D3）：gateway 注入的 [`DaclSpawnFn`] 内部
    /// 完成 write-restricted 令牌铸造 + `CreateProcessAsUserW` + stdio 往返
    /// （同步 FFI——这里 `spawn_blocking` 包住，不占 async 线程）。请求行与
    /// env 走 [`Self::env_pairs`] 同源，额外注入：
    /// - `NEMESISBOT_EXECUTOR_SANDBOX=1`：两层叠加（推荐档，设计 §3.4）——
    ///   子进程照常自装完整性标签，父进程再罩受限令牌写围栏；
    /// - `NEMESISBOT_SANDBOX_BACKEND=workspace-dacl`：拒绝台账 backend 标签
    ///   证据化（与 Start.exe wrap 注入 "sandboxie" 同一约定）。
    ///
    /// spawn 形态为 `DETACHED_PROCESS`（闭包层职责）：受限令牌下**新**
    /// console 分配必死 0xC0000142（nemesis-sandbox token.rs 模块文档实证），
    /// executor 协议 stdio 全管道无需 console。
    async fn spawn_and_call_dacl(
        &self,
        spawn_fn: DaclSpawnFn,
        request_line: &str,
        tool: &str,
    ) -> Result<String, String> {
        let mut env_extra = self.env_pairs();
        env_extra.push(("NEMESISBOT_EXECUTOR_SANDBOX".to_string(), "1".to_string()));
        env_extra.push((
            "NEMESISBOT_SANDBOX_BACKEND".to_string(),
            "workspace-dacl".to_string(),
        ));
        let req = DaclSpawnRequest {
            exe: self.exe_path.clone(),
            args: Vec::new(),
            env_extra,
            request_line: request_line.to_string(),
            timeout: self.timeout,
        };
        let outcome = tokio::task::spawn_blocking(move || spawn_fn(req))
            .await
            .map_err(|e| format!("dacl spawn task join error (tool={tool}): {e}"))?
            .map_err(|e| format!("dacl spawn transaction failed (tool={tool}): {e}"))?;
        match outcome.response {
            Some(line) => Self::parse_response(&line),
            None => Err(format!(
                "executor (workspace-dacl) exited without a response \
                 (exit={:?}, tool={tool}); stderr: {}",
                outcome.exit_code, outcome.stderr_tail
            )),
        }
    }

    /// Named-pipe transport (sandbox=true). L2.1: works with or without the box
    /// — `start_exe=None` spawns directly (transport test); `start_exe=Some`
    /// wraps with Start.exe (real box, L2.2).
    #[cfg(windows)]
    async fn spawn_and_call_pipe(&self, tool: &str, request_line: &str) -> Result<String, String> {
        use crate::executor_pipe;

        let id = executor_pipe::unique_pipe_id();
        let pipe = executor_pipe::pipe_name(&id);

        // 1. Create the named pipe BEFORE spawn so the child can connect to it.
        let server = executor_pipe::create_server(&pipe)
            .map_err(|e| format!("create executor pipe: {e}"))?;

        // 2. Spawn the child with the pipe env. stdio is unused for transport
        //    (the pipe carries the JSON); null stdin/stdout, piped stderr for
        //    diagnosis.
        let mut cmd = self.build_command();
        cmd.env("NEMESISBOT_EXECUTOR_PIPE", &pipe)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        // 台账标签证据化：PIPE 传输 ≠ 盒内。只有真盒（Start.exe wrap）才注入
        // sandboxie 后端名，exec_worker 据此记拒绝台账——无盒 PIPE 传输
        // （transport test / 降级装配）不冒标（标签与 `NEMESISBOT_SANDBOX_
        // BACKEND` 既有语义同源：bwrap reexec 盒内实例同样用 env 注入后端名）。
        if self.start_exe.is_some() {
            cmd.env("NEMESISBOT_SANDBOX_BACKEND", "sandboxie");
        }
        // 同上：异常 drop 兜底杀子进程（有序路径显式 kill+wait 不受影响）。
        cmd.kill_on_drop(true);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to spawn executor child: {e}"))?;
        let stderr_tail = Self::drain_stderr(&mut child);

        // 3. Wait for the child to connect — A4 装配纪律：等待循环耦合子进程
        //    存活断言（child 启动即崩 = 立即报退出码 + stderr 现场，不傻等
        //    满 30s 超时）。connect 探针幂等可重入（NamedPipeServer::connect
        //    cancel-safe），单次尝试限窗 500ms，超窗下轮重试；connect 的 io
        //    错误按「本轮未就绪」重试（客户端连上又消失等病态形态最终落
        //    Timeout/ChildDied，主失败面 child 早死由存活断言即时收口）。
        //    server 挂 Arc<tokio Mutex>：poll 闭包每次 clone 一个 owned Arc
        //    进 async 块（&mut 借闭包状态的 future 会挂 FnMut 逃逸），等待
        //    结束后写/读路径经锁 guard 使用同一句柄。
        let server = std::sync::Arc::new(tokio::sync::Mutex::new(server));
        let connect_result = {
            let poll_server = server.clone();
            let mut poll = move || {
                let srv = poll_server.clone();
                async move { srv.lock().await.connect().await.ok() }
            };
            let mut aliveness = || {
                let death = match child.try_wait() {
                    Ok(Some(status)) => Some(format!(
                        "exit code: {}, stderr: {}",
                        status
                            .code()
                            .map(|c| c.to_string())
                            .unwrap_or_else(|| status.to_string()),
                        stderr_tail.snapshot()
                    )),
                    Ok(None) => None,
                    Err(e) => Some(format!("status check failed: {e}")),
                };
                std::future::ready(death)
            };
            nemesis_utils::wait_ready::wait_for_ready(
                Duration::from_millis(500),
                Duration::from_secs(30),
                &mut poll,
                &mut aliveness,
            )
            .await
        };
        match connect_result {
            Ok(()) => {}
            Err(nemesis_utils::wait_ready::ReadyError::ChildDied { detail }) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor child exited while connecting to pipe (tool={tool}, {detail})"
                ));
            }
            Err(nemesis_utils::wait_ready::ReadyError::Timeout) => {
                let _ = child.start_kill();
                return Err(format!(
                    "executor child did not connect to pipe within 30s (tool={tool})"
                ));
            }
        }

        // 4. Write request line.
        {
            let mut srv = server.lock().await;
            srv.write_all(request_line.as_bytes())
                .await
                .map_err(|e| format!("write executor pipe: {e}"))?;
            srv.flush()
                .await
                .map_err(|e| format!("flush executor pipe: {e}"))?;
        }

        // 5. Read response line (timeout).
        let resp_line = {
            let mut srv = server.lock().await;
            let mut reader = BufReader::new(&mut *srv).lines();
            match tokio::time::timeout(self.timeout, reader.next_line()).await {
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(format!(
                        "executor timed out after {:?} (tool={tool})",
                        self.timeout
                    ));
                }
                Ok(Err(e)) => {
                    let _ = child.start_kill();
                    return Err(format!("read executor pipe: {e}"));
                }
                Ok(Ok(None)) => {
                    let _ = child.start_kill();
                    // 带退出状态 + stderr tail（静默死亡=panic/OOM/启动失败的
                    // 唯一诊断面；stdio 路径同款格式）。
                    let status = child.wait().await;
                    return Err(format!(
                        "executor child closed pipe without a response (tool={tool}, \
                         exit={:?}); stderr: {}",
                        status.map(|s| s.to_string()),
                        stderr_tail.snapshot()
                    ));
                }
                Ok(Ok(Some(line))) => line,
            }
        };

        // 6. Close our end so the child's loop sees EOF and exits; then reap.
        //    (The per-call child loops waiting for the next request; without
        //    closing the pipe it would block forever and child.wait() hangs.)
        drop(server);
        let _ = child.wait().await;
        Self::parse_response(&resp_line)
    }
}

// ---------------------------------------------------------------------------
// P3 T3：persistent 子进程的 env 白名单 + home 重定向
// ---------------------------------------------------------------------------

/// persistent（常驻）执行体子进程的 env 白名单——常驻 spawn 走
/// `Command::env_clear()` 后只回填本表内的宿主变量 + 显式注入项（workspace /
/// home / agent id / browser profile 等），宿主凭据（`*_API_KEY` / `*_TOKEN`
/// 及其余一切不在表内的变量）对常驻 agent 不可见。per-call 路径不消费本表
/// （env_clear 是 persistent 专属形态，v1 线零变化）。
///
/// 白名单只收「OS 运行时基线」：shell / 编译器 / 解释器能启动的最低需求 +
/// 本机身份定位符。刻意**不收**：
/// - 凭据类（`*_API_KEY` / `*_TOKEN` / `*_SECRET`——白名单语义本身即闸：
///   不在表内即剥，不依赖名字模式猜测）；
/// - `NEMESISBOT_*`（workspace / home / role 等由 spawn 侧显式注入；宿主的
///   `NEMESISBOT_HOME` 语义指向宿主工作区，对 agent home 是错的，必须剥）；
/// - home 族（`USERPROFILE` / `HOME` / `APPDATA` / `LOCALAPPDATA` / `XDG_*`
///   ——persistent 模式重定向到 agent home，见 [`home_redirect_env_pairs`]，
///   不从宿主继承宿主值）。
pub const EXECUTOR_ENV_ALLOWLIST: &[&str] = &[
    // 可执行定位器
    "PATH",
    "PATHEXT",
    // Windows 系统基线（工具链 / 子进程探测最低需求）
    "SYSTEMROOT",
    "SYSTEMDRIVE",
    "WINDIR",
    "COMSPEC",
    "PROGRAMDATA",
    "PROGRAMFILES",
    "PROGRAMFILES(X86)",
    "COMMONPROGRAMFILES",
    "COMMONPROGRAMFILES(X86)",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    "OS",
    // 本机身份定位符（主机名输出 / 工具链探测需要）
    "USERNAME",
    "USERDOMAIN",
    "COMPUTERNAME",
    // 临时目录（OS 惯例跨进程共享，非凭据面；持久模式不重定向）
    "TEMP",
    "TMP",
    // Unix 基线
    "LANG",
    "LC_ALL",
    "TERM",
];

/// 单个 env 键是否在白名单内。Windows env 名大小写不敏感（`Path`/`path`/
/// `PathExt` 都是同一个变量），按忽略大小写匹配；非 Windows 精确匹配。
pub fn env_allowlisted(key: &str) -> bool {
    #[cfg(windows)]
    {
        EXECUTOR_ENV_ALLOWLIST
            .iter()
            .any(|a| a.eq_ignore_ascii_case(key))
    }
    #[cfg(not(windows))]
    {
        EXECUTOR_ENV_ALLOWLIST.contains(&key)
    }
}

/// 从宿主 env 迭代里过白名单（persistent spawn 的 `env_clear()` 回填面）。
/// 纯函数（消费任意迭代器）——测试无需进程级 env 竞态。
pub fn filter_env_allowlist<I>(vars: I) -> Vec<(String, String)>
where
    I: IntoIterator<Item = (String, String)>,
{
    vars.into_iter()
        .filter(|(k, _)| env_allowlisted(k))
        .collect()
}

/// persistent 模式的 home 族重定向对：agent home 就是子进程的「家」——
/// `HOME` / `USERPROFILE` 直指 agent home，应用配置/数据目录（APPDATA、
/// XDG_*）落进 home 的常规子目录。浏览器身份（T6）由此天然隔离在 home 内。
pub fn home_redirect_env_pairs(home: &Path) -> Vec<(String, String)> {
    let home_str = home.to_string_lossy().into_owned();
    let mut v = vec![("HOME".to_string(), home_str.clone())];
    #[cfg(windows)]
    {
        let join = |rel: &str| home.join(rel).to_string_lossy().into_owned();
        v.push(("USERPROFILE".to_string(), home_str));
        v.push(("APPDATA".to_string(), join("AppData/Roaming")));
        v.push(("LOCALAPPDATA".to_string(), join("AppData/Local")));
    }
    #[cfg(not(windows))]
    {
        let join = |rel: &str| home.join(rel).to_string_lossy().into_owned();
        v.push(("XDG_CONFIG_HOME".to_string(), join(".config")));
        v.push(("XDG_DATA_HOME".to_string(), join(".local/share")));
        v.push(("XDG_CACHE_HOME".to_string(), join(".cache")));
    }
    v
}

// ---------------------------------------------------------------------------
// Persistent route (P3 T7) — resident executor attachment point
// ---------------------------------------------------------------------------

/// 常驻执行体调用的分型错误（T7）。语义边界只有一条线：**调用有没有发生**。
/// `Unavailable` = 调用未发生（实体未就绪 / 槽满 / spawn 失败在握手前），调用
/// 方可安全回落 per-call 路径；`ToolFailed` = 调用已送达执行体（可能已有副
/// 作用），回落会造成重复执行——必须原样上抛。这是 supervisor 侧
/// `SupervisorError` 的 agent crate 侧镜像（nemesis-agent 不依赖 nemesisbot，
/// 依赖倒置：trait 定义在本 crate，bot 侧实现）。
#[derive(Debug, Clone)]
pub enum PersistentCallError {
    /// 调用未发生——可安全回落 per-call 执行路径。
    Unavailable(String),
    /// 调用已尝试（可能已有副作用）——绝不回落，原样上抛。
    ToolFailed(String),
}

impl std::fmt::Display for PersistentCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "persistent executor unavailable: {m}"),
            Self::ToolFailed(m) => write!(f, "persistent executor call failed: {m}"),
        }
    }
}

/// 常驻执行体的抽象（T7 依赖倒置）。生产实现 = nemesisbot 侧
/// `AgentRuntimeSupervisor`（按 agent_id 复用常驻子进程 + journal 语义）。
#[async_trait]
pub trait PersistentExecutor: Send + Sync {
    /// 在 `agent_id` 的常驻执行体内执行一次工具调用。`exec_root` 是本次
    /// 调用的工作区锚（main=workspace / project=项目目录 / selfbuilt=home，
    /// 来自 agents 表记录——单一真相源），监督器 spawn 时写进子进程 env。
    async fn execute_persistent(
        &self,
        agent_id: &str,
        exec_root: &std::path::Path,
        tool: &str,
        args: &str,
        context: &RequestContext,
    ) -> Result<String, PersistentCallError>;
}

/// 挂在 [`RemoteExecutorTool`] 上的常驻路由（T7）：agent 身份惰性解析 +
/// 常驻执行体句柄。`resolve` 在**每次工具调用时**执行（loop 建成早于实体
/// seed——首次 boot 时装配完成时 agents 表还没种，静态绑定必空）；返回
/// `None` = 实体未就绪，调用方回落 per-call 路径。
#[derive(Clone)]
pub struct PersistentRoute {
    /// 返回 `(agent_id, exec_root)`；None = 实体未就绪 → per-call 回落。
    pub resolve: std::sync::Arc<dyn Fn() -> Option<(String, std::path::PathBuf)> + Send + Sync>,
    pub executor: std::sync::Arc<dyn PersistentExecutor>,
}

// ---------------------------------------------------------------------------
// RemoteExecutorTool — the Tool the agent loop sees
// ---------------------------------------------------------------------------

/// Gateway-side bridge: a normal `Tool` to the agent loop, but `execute()` is
/// proxied to an executor child. Metadata + `preview` delegate to the wrapped
/// local impl, so the LLM sees identical schemas and the checkpoint safety net
/// still snapshots file writes.
///
/// P3 T7：`persistent` 有值时 execute 先询问常驻路由（身份解析成功 → 常驻
/// 执行体；`Unavailable` → 回落本通道 per-call 链；`ToolFailed` → 原样上抛
/// 不回落）。per-call 路径行为与 T7 前逐字节一致（回落即兼容）。
pub struct RemoteExecutorTool {
    name: String,
    local: Box<dyn Tool>,
    channel: std::sync::Arc<ExecutorChannel>,
    persistent: Option<PersistentRoute>,
}

impl RemoteExecutorTool {
    pub fn new(
        name: String,
        local: Box<dyn Tool>,
        channel: std::sync::Arc<ExecutorChannel>,
    ) -> Self {
        Self {
            name,
            local,
            channel,
            persistent: None,
        }
    }

    /// P3 T7：挂常驻路由（gateway 装配期调用；不挂 = 纯 per-call，行为与
    /// 既有 Layer 1/2 一致）。
    pub fn with_persistent(mut self, route: PersistentRoute) -> Self {
        self.persistent = Some(route);
        self
    }
}

#[async_trait]
impl Tool for RemoteExecutorTool {
    async fn execute(&self, args: &str, context: &RequestContext) -> Result<String, String> {
        // T7 常驻路由前置询问：身份解析成功 → 常驻执行体优先。安全 8 层 /
        // estop / plan 模式闸全部在 loop dispatch 层先于本函数执行（构造性
        // 覆盖），常驻调用不构成旁路。
        if let Some(route) = &self.persistent
            && let Some((agent_id, exec_root)) = (route.resolve)()
        {
            match route
                .executor
                .execute_persistent(&agent_id, &exec_root, &self.name, args, context)
                .await
            {
                Ok(out) => return Ok(out),
                Err(PersistentCallError::ToolFailed(e)) => {
                    // 调用已发生（可能已有副作用）——回落会重复执行，原样上抛。
                    return Err(e);
                }
                Err(PersistentCallError::Unavailable(e)) => {
                    // 调用未发生——诚实降级到 per-call 路径（兼容回退是 PLAN
                    // P3 验收门明文；info 留痕不 warn——回落是设计内形态）。
                    tracing::info!(
                        tool = %self.name,
                        agent_id = %agent_id,
                        reason = %e,
                        "persistent executor unavailable; falling back to per-call"
                    );
                }
            }
        }
        self.channel
            .spawn_and_call(&self.name, args, context)
            .await
            .map_err(|e| format!("executor unavailable: {e}"))
    }

    fn set_context(&self, channel: &str, chat_id: &str) {
        self.local.set_context(channel, chat_id);
    }

    fn description(&self) -> String {
        self.local.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.local.parameters()
    }

    fn preview(&self, args: &str) -> Option<FileChange> {
        self.local.preview(args)
    }
}

#[cfg(test)]
mod tests;
// S9 (quality-hardening goal 冲刺 S9): 独立测试文件挂载（声明式，无内联测试）。
#[cfg(test)]
mod s9_tests;
// 覆盖率补充批次：stdio 成功回路 + stderr drain / userland 标记 / 超时 / 无响应退出。
// 假子进程是 .cmd 批处理（Windows spawn 语义），整文件 Windows 形态。
#[cfg(all(test, windows))]
mod cov_tests;
