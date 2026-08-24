//! U11 用户态沙盒后端（Linux landlock/bwrap、macOS Seatbelt）。
//!
//! Windows 不注册任何后端——Sandboxie（内核态驱动 + 盒虚拟化）是 Windows 上
//! 更强的方案，见 crate 根文档与本 goal 的 U11 条目「Windows 不动」。
//!
//! ## 两种后端形态
//!
//! - **进程内自装**（landlock）：`apply_to_self` 把限制装到**当前进程**上，
//!   之后所有后代进程继承，**不可逆**。Linux executor 子进程（exec_worker）
//!   启动时调用——无需外部监督者，也无 Start.exe 式包装。
//! - **包装式启动**（bwrap / Seatbelt `sandbox-exec`）：`wrap_command` 返回
//!   一个包好沙盒参数的 `Command`，由调用方 spawn。landlock 不可用（老内核）
//!   或需要网络隔离（landlock ABI<4 不管网络；bwrap `--unshare-net` 能）时用。
//!
//! ## enforcement full|partial 语义
//!
//! [`Enforcement::Partial`] = 规则**已装上**但能力集合有缺口（例如 landlock
//! ABI 3 内核缺 `Refer` 权限、或 `allow_network=false` 在 FS-only 后端上不可
//! 强制）。调用方拿到 Partial 应 **warn 并继续**（有盒好过没盒），缺口明细
//! 在 `gaps: Vec<String>` 供日志/报告——「降级不崩」验收的语义载体。
//! 完全装不上 = `Err`（调用方 warn + 无盒继续，见 exec_worker 装配点）。
//!
//! ## 诚实边界
//!
//! - landlock 是**文件系统** LSM：读/写/执行粒度，不管 socket/net（ABI 4+
//!   的 TCP 规则本 crate 未接）。`allow_network=false` 在 LandlockBackend 上
//!   恒进 Partial 缺口清单。
//! - landlock 自装**不可逆**且只对本进程树生效——gateway 自身永不在列，只有
//!   executor 子进程装（与 Sandboxie 只盒 executor 的边界一致）。
//! - Seatbelt（macOS）路径无真机验证：`wrap_command` 的 SBPL profile 生成
//!   是纯函数有单测，`sandbox-exec` 调用形态按 Apple 文档实现——**交付代码 +
//!   诚实标注，B7 的 mac 半边保留欠账**（goal 拍板 2026-08-23）。
//! - bwrap 需要发行版安装（Ubuntu 24.04 自带）；缺二进制 = Unavailable，
//!   链条降级终止（warn + 无盒），不阻断执行。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 沙盒配置（后端中立）。由调用方（exec_worker / 测试）从 workspace + config
/// 构造，后端只消费。
#[derive(Debug, Clone)]
pub struct SandboxConf {
    /// 允许**写**的根（executor 语义 = workspace 子树）。默认全拒写。
    pub writable_roots: Vec<PathBuf>,
    /// 允许**读+执行**的根（landlock deny-by-default 下工具进程要能跑
    /// /usr/bin、读 /etc）。默认 `[/]`（全放读，写仍全拒）。
    pub read_exec_roots: Vec<PathBuf>,
    /// false = 期望禁网。FS-only 后端强制不了 → Partial 缺口（见模块文档）。
    pub allow_network: bool,
    /// 诊断标签（日志用）。
    pub label: String,
}

impl SandboxConf {
    /// executor 子进程的标准配置：workspace 可写、全盘可读、按 config 禁网。
    pub fn for_executor(workspace: &Path, allow_network: bool) -> Self {
        Self {
            writable_roots: vec![workspace.to_path_buf()],
            read_exec_roots: vec![PathBuf::from("/")],
            allow_network,
            label: "executor".to_string(),
        }
    }
}

/// 沙盒装上后的强制程度。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enforcement {
    /// 全部规则生效。
    Full,
    /// 规则已装，但能力集合有缺口（明细在 gaps，如「network: landlock ABI3
    /// 不覆盖」）。调用方 warn + 继续。
    Partial(Vec<String>),
}

/// 后端在**本机**的可用性（探测结果，不装规则）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// 可用（或内核能力足以 BestEffort 装）。
    Full,
    /// 可用但已知缺口（如内核 ABI 老于编译目标）。
    Partial(Vec<String>),
    /// 不可用（原因字符串：内核无 landlock / bwrap 二进制缺失 / ...）。
    Unavailable(String),
}

/// 后端形态（U11 链条的分派依据）。
///
/// - [`BackendForm::SelfApply`]：进程内自装（landlock）——executor 子进程启动
///   时对自身 `apply_to_self`，后代全继承。
/// - [`BackendForm::WrapCommand`]：包装式（bwrap / sandbox-exec）——必须由
///   **父进程**（或子进程 re-exec 自身）组好包装参数再 spawn，`apply_to_self`
///   对本进程无意义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendForm {
    SelfApply,
    WrapCommand,
}

/// 用户态沙盒后端统一接口（U11「统一 SandboxBackend trait」）。
pub trait SandboxBackend: Send + Sync {
    /// 后端名（日志/诊断）。
    fn name(&self) -> &str;

    /// 后端形态（决定链路上谁来执行：自装 vs 包装）。
    fn form(&self) -> BackendForm;

    /// 本机可用性探测（不装规则、可反复调用）。
    fn availability(&self) -> Availability;

    /// 进程内自装（landlock 形态）。装上后**不可逆**，只影响本进程树。
    /// 包装式后端默认不支持（返回 Err）。
    fn apply_to_self(&self, _conf: &SandboxConf) -> Result<Enforcement, String> {
        Err(format!("backend '{}' does not support self-apply", self.name()))
    }

    /// 包装式启动（bwrap / sandbox-exec 形态）：返回带沙盒参数的 `Command`
    /// （program 已替换为包装器），调用方继续补 args/env 并 spawn。
    /// 自装式后端默认不支持（返回 Err）。
    fn wrap_command(&self, _conf: &SandboxConf, _cmd: &Command) -> Result<Command, String> {
        Err(format!("backend '{}' does not wrap commands", self.name()))
    }
}

/// 探测并返回本机最优后端（U11 链条：Linux landlock 优先 → bwrap 次之；
/// macOS Seatbelt；Windows None——Sandboxie 承担）。无可用后端 = None
/// （调用方 warn + 无盒降级，不崩）。
pub fn detect_backend() -> Option<std::sync::Arc<dyn SandboxBackend>> {
    detect_platform_backend()
}

// ---------------------------------------------------------------------------
// 平台实现挂载点
// ---------------------------------------------------------------------------

// 平台实现模块（cfg 与 detect_platform_backend 的引用严格对齐——Windows 裁掉
// 全部三个：无引用也无编译）。
#[cfg(target_os = "linux")]
mod bwrap_impl;
#[cfg(target_os = "linux")]
mod landlock_impl;
#[cfg(target_os = "macos")]
mod seatbelt_impl;

#[cfg(target_os = "linux")]
fn detect_platform_backend() -> Option<std::sync::Arc<dyn SandboxBackend>> {
    let landlock = super::backend::landlock_impl::LandlockBackend::new();
    match landlock.availability() {
        Availability::Unavailable(_) => {
            tracing::warn!(
                "[UserlandSandbox] landlock unavailable on this kernel — falling back to \
                 bubblewrap (install bwrap for a stronger chain)"
            );
            let bwrap = super::backend::bwrap_impl::BwrapBackend::new();
            match bwrap.availability() {
                Availability::Unavailable(reason) => {
                    tracing::warn!(
                        "[UserlandSandbox] no userland sandbox backend (landlock + bwrap both \
                         unavailable: {reason}) — executor runs unsandboxed (config \
                         executor.sandbox stays honoured for Windows Sandboxie)"
                    );
                    None
                }
                _ => Some(std::sync::Arc::new(bwrap)),
            }
        }
        _ => Some(std::sync::Arc::new(landlock)),
    }
}

#[cfg(target_os = "macos")]
fn detect_platform_backend() -> Option<std::sync::Arc<dyn SandboxBackend>> {
    let seatbelt = super::backend::seatbelt_impl::SeatbeltBackend::new();
    match seatbelt.availability() {
        Availability::Unavailable(reason) => {
            tracing::warn!(
                "[UserlandSandbox] Seatbelt unavailable ({reason}) — executor runs unsandboxed"
            );
            None
        }
        _ => Some(std::sync::Arc::new(seatbelt)),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn detect_platform_backend() -> Option<std::sync::Arc<dyn SandboxBackend>> {
    // Windows: Sandboxie owns sandboxing (kernel driver + box); no userland
    // backend registered here by design (U11: Windows 不动).
    None
}

// ---------------------------------------------------------------------------
// 纯函数：命令行/profile 构造（跨平台可单测）
// ---------------------------------------------------------------------------

/// 读 `<home>/config.json` 的 `executor.allow_network`（缺失/损坏默认 false——
/// 与 Sandboxie ini 的 `AllowNetworkAccess=n` 默认一致；install.rs 的
/// `read_allow_network` 是同一语义的 Windows 侧消费，已委托到此）。
///
/// 用户态沙盒用它决定 bwrap 是否加 `--unshare-net`（landlock 不碰网络，
/// 只影响 Partial 缺口文案）。
pub fn read_executor_allow_network(home: &Path) -> bool {
    let raw = match std::fs::read_to_string(home.join("config.json")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let val: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return false,
    };
    val.get("executor")
        .and_then(|e| e.get("allow_network"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

/// 读 `<home>/config.json` 的 `executor.strict`（P5-2 严格模式开关；缺失/
/// 损坏默认 false = fail-open 现状，与 [`read_executor_allow_network`] 同款
/// 语义）。executor 子进程（`exec_worker` 的 `engage`）用它决定「无后端 /
/// 自装失败」时是降级 warn 继续还是拒绝执行（fail-closed）。
pub fn read_executor_strict(home: &Path) -> bool {
    let raw = match std::fs::read_to_string(home.join("config.json")) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let val: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(_) => return false,
    };
    val.get("executor")
        .and_then(|e| e.get("strict"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// 逐后端探测（诊断/状态展示，P5-1 沙盒页「后端探测状态」）
// ---------------------------------------------------------------------------

/// 单个用户态后端的探测结果（不参与 [`detect_backend`] 的优先级选择——
/// 状态页要把 landlock / bwrap 各自的可用性**并列展示**，选了 landlock 也
/// 要告诉用户 bwrap 装没装）。
#[derive(Debug, Clone)]
pub struct UserlandBackendProbe {
    /// 后端名（landlock / bwrap / sandbox-exec）。
    pub name: String,
    /// 形态（自装 vs 包装）。
    pub form: BackendForm,
    /// 本机可用性（探测，不装规则）。
    pub availability: Availability,
}

/// 逐个探测本机**全部**用户态后端（不排序、不选择）。Windows 返回空 vec
/// （设计上 Sandboxie 承担沙盒，见 [`detect_platform_backend`] 的 Windows 注释）。
pub fn probe_userland_backends() -> Vec<UserlandBackendProbe> {
    probe_platform_userland_backends()
}

#[cfg(target_os = "linux")]
fn probe_platform_userland_backends() -> Vec<UserlandBackendProbe> {
    let landlock = super::backend::landlock_impl::LandlockBackend::new();
    let bwrap = super::backend::bwrap_impl::BwrapBackend::new();
    vec![
        UserlandBackendProbe {
            name: landlock.name().to_string(),
            form: landlock.form(),
            availability: landlock.availability(),
        },
        UserlandBackendProbe {
            name: bwrap.name().to_string(),
            form: bwrap.form(),
            availability: bwrap.availability(),
        },
    ]
}

#[cfg(target_os = "macos")]
fn probe_platform_userland_backends() -> Vec<UserlandBackendProbe> {
    let seatbelt = super::backend::seatbelt_impl::SeatbeltBackend::new();
    vec![UserlandBackendProbe {
        name: seatbelt.name().to_string(),
        form: seatbelt.form(),
        availability: seatbelt.availability(),
    }]
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn probe_platform_userland_backends() -> Vec<UserlandBackendProbe> {
    Vec::new()
}

/// bubblewrap 参数构造（纯函数）。写 = `--bind`（读写挂载），读+执行 =
/// `--ro-bind`。根挂载 `--ro-bind / /` 后对 writable_roots 再 `--bind`
/// （后项覆盖先项）。`--unshare-net` 只在 `allow_network=false` 时加（会断
/// 工具的网能力——executor 禁网语义）。
pub fn bwrap_args(conf: &SandboxConf) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "--ro-bind".into(),
        "/".into(),
        "/".into(),
        "--dev".into(),
        "/dev".into(),
        "--proc".into(),
        "/proc".into(),
        "--tmpfs".into(),
        "/tmp".into(),
        "--die-with-parent".into(),
        "--unshare-pid".into(),
    ];
    for root in &conf.read_exec_roots {
        // "/" is already ro-bound above; skip duplicates (component-wise).
        if root != Path::new("/") {
            let p = root.to_string_lossy().to_string();
            args.push("--ro-bind".into());
            args.push(p.clone());
            args.push(p);
        }
    }
    for root in &conf.writable_roots {
        let p = root.to_string_lossy().to_string();
        args.push("--bind".into());
        args.push(p.clone());
        args.push(p);
    }
    if !conf.allow_network {
        args.push("--unshare-net".into());
    }
    args
}

/// macOS Seatbelt SBPL profile 构造（纯函数）。策略：全默认放行，**拒绝写**
/// 整个文件系统，再对 writable_roots 逐个放行写。网络拒绝按 allow_network
/// 追加 `(deny network*)`。
///
/// ⚠️ 无真机验证（goal 拍板：mac 半边保留欠账）——SBPL 语法按 Apple
/// sandbox-exec(1) 手册写，profile 字符串有单测钉形状。
pub fn seatbelt_profile(conf: &SandboxConf) -> String {
    let mut allow_write = String::new();
    for root in &conf.writable_roots {
        allow_write.push_str(&format!(
            "(allow file-write* (subpath (literal \"{}\")))\n",
            root.to_string_lossy()
        ));
    }
    let deny_net = if conf.allow_network {
        String::new()
    } else {
        "(deny network*)\n".to_string()
    };
    format!(
        "(version 1)\n(deny default)\n(allow process-exec*)\n(allow process-fork)\n\
         (allow file-read*)\n{allow_write}{deny_net}\
         ; generated by nemesis-sandbox seatbelt_profile (label: {})\n",
        conf.label
    )
}

#[cfg(test)]
mod tests;
