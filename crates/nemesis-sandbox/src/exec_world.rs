//! U10 统一执行世界 —— 「执行环境」一等抽象（writable_roots + env 清洗 + spawn 语义）。
//!
//! ## 为什么需要它
//!
//! Sandboxie 执行分离（Layer 1/2）此前只罩 agent 工具层（MOVE_TOOLS 经
//! `RemoteExecutorTool` → `ExecutorChannel`）。工作流引擎的控制面 IO
//! （定义/执行日志/checkpoint 写盘）与无 registry 的 script 直 spawn 路径
//! 完全绕过该层。U10 把「这段代码跑在哪个环境」抽成一等 trait，让
//! MOVE_TOOLS、workflow script/tool 节点、引擎控制面 IO 共享同一套：
//!
//! - **writable_roots**：控制面写守卫。引擎的写盘点（定义 CRUD、执行
//!   JSONL）先经 [`ExecutionWorld::check_writable`]，越界（如 workflow
//!   `name` 字段带 `..` 拼出的逃逸路径）→ 拒 + 错误信息。
//! - **env 清洗**：本进程内直 spawn（opt-out 车道 / 无 registry 回退）
//!   前剥掉执行体运输层内部变量（`NEMESISBOT_ROLE` /
//!   `NEMESISBOT_EXECUTOR_WORKSPACE` / `NEMESISBOT_EXECUTOR_PIPE`）——
//!   这些只属于 executor 子进程，不应泄给任意脚本。
//! - **spawn 语义**（[`SpawnSemantics`]）：描述轴——当前工具车道是
//!   InProcess（未启用执行分离）/ ExecutorChild（Layer 1 子进程）/
//!   SandboxBoxed（Layer 2 盒内）。日志与状态查询用；不驱动分支
//!   （分支由 config probe 在每次调用时实时决定，见 agent_factory）。
//!
//! ## 两条车道
//!
//! [`ExecOp::Tool`] = 工具调用车道：路由到 executor 通道（子进程/盒内，
//! 与 agent 的 `exec` 同一开关）。需要实现方持有通道（gateway/CLI 的
//! `ExecutorWorld` 桥，在 nemesisbot crate——本 crate 不依赖 nemesis-agent，
//! 同 AgentRunner 桥接范式）。[`DirectWorld`] 没有通道 → 诚实返回 Err。
//! [`ExecOp::Spawn`] = 本进程内直 spawn 车道：受守卫的直跑（per-node
//! `sandbox: false` 显式 opt-out 时走这条），不是裸 spawn——cwd 守卫 +
//! env 清洗 + Windows CREATE_NO_WINDOW + 超时。
//!
//! ## 与 tool-plugin-system op_type 声明的边界（协调记录）
//!
//! 两轴**正交**：
//! - `OperationType`（nemesis-security，`tool_to_operation`）= 操作**风险
//!   分类**，喂 8 层安全管线；未来插件 `ToolInfo.operation_type`
//!   （`docs/PLAN/2026-07-10_tool-plugin-system.md` §3.1/§3.5）声明的
//!   也是这一轴（前置读声明，未声明默认 CRITICAL）。
//! - `ExecutionWorld`（本模块）= 执行**位置/环境**（直跑 vs 子进程 vs
//!   盒内 + 写根 + env）。
//!
//! 插件声明 op_type 不改变它跑在哪个世界；世界选择不改变风险分类。
//! 命名刻意区分（OperationType vs ExecutionWorld）防混淆。
//!
//! ## 消费方（谁在用）
//!
//! - `nemesis-workflow`：引擎写守卫（persist_workflow / delete_workflow_file
//!   / persist_execution）+ script 节点的 world 车道路由 + per-node
//!   `sandbox` 开关。
//! - `nemesisbot`：`ExecutorWorld` 桥（包 ExecutorChannel）+ gateway/CLI
//!   装配。
//!
//! U11（Linux/mac 用户态沙盒）的 `SandboxBackend` trait 将挂载在本 crate，
//! 届时 `SpawnSemantics` 的盒内分支按平台后端解析。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use nemesis_path::paths::canonicalize_for_compare;

/// 工具车道的 spawn 语义（描述轴，见模块文档）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnSemantics {
    /// 未启用执行分离：工具在本进程内执行。
    InProcess,
    /// Layer 1：每次调用 spawn 独立 executor 子进程（stdio 协议）。
    ExecutorChild,
    /// Layer 2：子进程经 Sandboxie Start.exe 起进盒内（命名管道协议）。
    SandboxBoxed,
}

impl std::fmt::Display for SpawnSemantics {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SpawnSemantics::InProcess => write!(f, "in-process"),
            SpawnSemantics::ExecutorChild => write!(f, "executor-child"),
            SpawnSemantics::SandboxBoxed => write!(f, "sandbox-boxed"),
        }
    }
}

/// 工具调用车道请求：`args` 是 JSON 字符串（与 ToolRegistry/executor 协议
/// 的请求行一致）。
#[derive(Debug, Clone)]
pub struct ToolOp {
    pub tool: String,
    pub args: String,
}

/// 本进程内直 spawn 车道请求（受守卫，非裸 spawn）。
#[derive(Debug, Clone)]
pub struct SpawnOp {
    pub program: String,
    pub args: Vec<String>,
    /// 显式 cwd。`Some` 时必须落在世界的 spawn 根内，否则拒。
    pub cwd: Option<PathBuf>,
    /// 可选 stdin 内容（写完后关闭 stdin）。
    pub stdin: Option<String>,
    /// 超时秒数。`None` = 用实现方默认（`DirectWorld::DEFAULT_TIMEOUT_SECS`）。
    pub timeout_secs: Option<u64>,
}

/// 统一执行请求。
#[derive(Debug, Clone)]
pub enum ExecOp {
    Tool(ToolOp),
    Spawn(SpawnOp),
}

/// 统一执行结局。`exit_code: None` = 进程被信号杀死/超时后无法取码。
#[derive(Debug, Clone)]
pub struct ExecOutcome {
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl ExecOutcome {
    /// 非 0 退出码或超时视为失败（spawn 失败直接走 `Err`）。
    pub fn failed(&self) -> bool {
        self.timed_out || self.exit_code.unwrap_or(-1) != 0
    }
}

/// 统一执行世界（U10 一等抽象）。
///
/// 见模块文档的轴定义与两轴正交说明。默认 `check_writable` 用
/// [`path_within_roots`] 语义（component 前缀比较 + best-effort canonicalize）。
#[async_trait]
pub trait ExecutionWorld: Send + Sync {
    /// 世界名（日志/状态展示用）。
    fn name(&self) -> &str;

    /// 控制面写守卫根。引擎写盘点必须落在任一根下。
    fn writable_roots(&self) -> Vec<PathBuf>;

    /// 工具车道当前的 spawn 语义（描述轴）。
    fn spawn_semantics(&self) -> SpawnSemantics;

    /// 控制面写守卫：越界 → `Err(reason)`。
    fn check_writable(&self, path: &Path) -> Result<(), String> {
        let roots = self.writable_roots();
        if path_within_roots(path, &roots) {
            Ok(())
        } else {
            Err(format!(
                "path {:?} is outside the execution world's writable roots {:?}",
                path, roots
            ))
        }
    }

    /// 是否有 executor 工具车道（能路由 [`ExecOp::Tool`]）。
    /// `DirectWorld` 无通道 → false；nemesisbot 的 `ExecutorWorld` → true。
    fn supports_tool_calls(&self) -> bool {
        false
    }

    /// 统一执行入口。
    async fn run(&self, op: ExecOp) -> Result<ExecOutcome, String>;
}

/// `path` 是否落在 `roots` 任一根下。
///
/// 语义 = **component 前缀**（`Path::starts_with`），不是字符串前缀
/// （`C:\ws` 不覆盖 `C:\ws2\...`）。先对双方做 best-effort canonicalize
/// 消掉 `..` / 符号链接 / 8.3 短名 / 大小写等一切表示差异后再比；整体
/// canonicalize 失败（文件不存在等，create 前守卫的常态）时由
/// [`canonicalize_for_compare`] 借最长存在祖先对齐表示、词法尾巴消解。
///
/// Windows 坑（实测）：`canonicalize` 返回 `\\?\C:\...` verbatim 前缀路径，
/// 词法路径没有该前缀 → 直接 starts_with 永假（clamd client.rs 同款坑）。
/// 双方统一走 [`canonicalize_for_compare`]（nemesis-path 唯一真相源）后再比。
pub fn path_within_roots(path: &Path, roots: &[PathBuf]) -> bool {
    let canon = canonicalize_for_compare(path);
    roots.iter().any(|root| {
        let canon_root = canonicalize_for_compare(root);
        canon.starts_with(&canon_root)
    })
}

/// 执行体运输层内部变量——本进程直 spawn 前必须剥掉（见模块文档「env 清洗」）。
pub const EXECUTOR_INTERNAL_ENV: &[&str] = &[
    "NEMESISBOT_ROLE",
    "NEMESISBOT_EXECUTOR_WORKSPACE",
    "NEMESISBOT_EXECUTOR_PIPE",
];

/// env 清洗：从继承环境里剥执行体内部变量，再叠加显式 overrides。
pub fn sanitize_env(
    base: &HashMap<String, String>,
    overrides: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = base
        .iter()
        .filter(|(k, _)| !EXECUTOR_INTERNAL_ENV.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    out.extend(overrides.iter().map(|(k, v)| (k.clone(), v.clone())));
    out
}

/// 直跑世界的默认超时（秒）。防挂死（script 节点裸 spawn 时代能无限挂）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 300;

/// 受守卫的本进程内直 spawn（[`DirectWorld`] 与 nemesisbot 的
/// `ExecutorWorld` 共用：两世界的 Spawn 车道语义必须一致）。
///
/// 守卫：`cwd`（若显式指定）必须落在 `spawn_roots` 内；env 清洗剥
/// [`EXECUTOR_INTERNAL_ENV`]；Windows 加 CREATE_NO_WINDOW（CLAUDE.md 无窗口
/// 纪律）；`kill_on_drop` + 超时兜底。
pub async fn guarded_direct_spawn(
    job: &SpawnOp,
    spawn_roots: &[PathBuf],
) -> Result<ExecOutcome, String> {
    if let Some(cwd) = &job.cwd
        && !path_within_roots(cwd, spawn_roots) {
            return Err(format!(
                "spawn cwd {:?} is outside the execution world's spawn roots {:?}",
                cwd, spawn_roots
            ));
        }

    let timeout = Duration::from_secs(job.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS).max(1));

    let mut cmd = tokio::process::Command::new(&job.program);
    cmd.args(&job.args)
        .env_remove("NEMESISBOT_ROLE")
        .env_remove("NEMESISBOT_EXECUTOR_WORKSPACE")
        .env_remove("NEMESISBOT_EXECUTOR_PIPE")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &job.cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("spawn {:?}: {}", job.program, e))?;

    if let Some(stdin_data) = &job.stdin
        && let Some(mut stdin) = child.stdin.take() {
            use tokio::io::AsyncWriteExt;
            let _ = stdin.write_all(stdin_data.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }
    // stdin drop（take 后离开作用域）= EOF，脚本型子进程正常退出。

    let fut = child.wait_with_output();
    match tokio::time::timeout(timeout, fut).await {
        Ok(Ok(output)) => Ok(ExecOutcome {
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            timed_out: false,
        }),
        Ok(Err(e)) => Err(format!("wait {:?}: {}", job.program, e)),
        // 超时：future 被 drop → child 被 drop → kill_on_drop 杀掉。
        Err(_) => Ok(ExecOutcome {
            exit_code: None,
            stdout: String::new(),
            stderr: format!(
                "timed out after {}s in execution-world spawn",
                timeout.as_secs()
            ),
            timed_out: true,
        }),
    }
}

/// 最小直跑世界：无 executor 通道（工具车道诚实 Err），Spawn 车道 = 受守卫
/// 本进程直 spawn。单测/未启用执行分离的装配用它。
pub struct DirectWorld {
    name: String,
    /// 控制面写守卫根。
    roots: Vec<PathBuf>,
    /// Spawn 车道 cwd 守卫根（通常 = workspace 树；与控制面根不同集）。
    spawn_roots: Vec<PathBuf>,
    semantics: SpawnSemantics,
}

impl DirectWorld {
    pub fn new(
        name: impl Into<String>,
        roots: Vec<PathBuf>,
        spawn_roots: Vec<PathBuf>,
        semantics: SpawnSemantics,
    ) -> Self {
        Self {
            name: name.into(),
            roots,
            spawn_roots,
            semantics,
        }
    }
}

#[async_trait]
impl ExecutionWorld for DirectWorld {
    fn name(&self) -> &str {
        &self.name
    }

    fn writable_roots(&self) -> Vec<PathBuf> {
        self.roots.clone()
    }

    fn spawn_semantics(&self) -> SpawnSemantics {
        self.semantics
    }

    fn supports_tool_calls(&self) -> bool {
        false
    }

    async fn run(&self, op: ExecOp) -> Result<ExecOutcome, String> {
        match op {
            ExecOp::Spawn(job) => guarded_direct_spawn(&job, &self.spawn_roots).await,
            ExecOp::Tool(tool) => Err(format!(
                "execution world '{}' has no executor tool lane (direct world); \
                 cannot run tool '{}'",
                self.name, tool.tool
            )),
        }
    }
}

#[cfg(test)]
mod tests;
