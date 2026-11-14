//! MXC 护栏后端（feature = "mxc"，2026-11-14）。
//!
//! Microsoft eXecution Container（submodule `3rd/mxc`，钉 `4e522623`）的
//! nemesis-sandbox 接入层。**语义定性（用户拍板口径，警告文案的根基）**：
//! MXC 是 ACL 授权模型——授权路径内直接写真盘，无 Sandboxie 写虚拟化盒、
//! 无 pending/commit 事后反悔。它是「护栏」，不是「沙盒」；引爆场景
//! （明知有问题还要跑、事后审查）归 Sandboxie（`executor.backend` 别的值）。
//!
//! ## 平台映射
//!
//! `Containment::Process`（抽象意图）按宿主解析：Windows=ProcessContainer
//! （PSEC + JobObject + WFP，需 Win11 24H2+ 特定 KB）、Linux=bubblewrap
//! （namespace + seccomp）、macOS=seatbelt。一套调用代码三平台通用。
//!
//! ## 检测（单一权威 = MXC 官方 probe，本 crate 不维护 OS 版本表）
//!
//! Windows 走 `mxc_sdk::v1::probe`（只读、不启动容器，`ProbeOutput.error`
//! = 为什么这台机器跑不了）；Linux 走官方 `probe_bwrap`；macOS 检查系统
//! 内置 `sandbox-exec`。结果带进程内缓存（执行路径复用，Dashboard 强刷
//! 走 [`probe_mxc_cached`] 的 `force`）。
//!
//! ## 遥测红线（docs/PLAN/2026-11-14_mxc-sandbox-integration.md §3.1）
//!
//! 构建期：根 `.cargo/config.toml` [env] 强制 `MXC_TELEMETRY_COMPONENT_PATH=""`
//! （无路由构建）。运行期：每次 spawn 显式 `telemetry: Some(TelemetryConfig
//! { enabled: Some(false) })`（双重保险）。删任一 = 红线失效，禁止。
//!
//! ## 消费面
//!
//! nemesisbot `exec_world` 装配（backend="mxc" 时注入 ExecutorChannel 的
//! mxc_spawn hook）+ WSAPI/CLI status。本模块**不读 config**（nemesis-sandbox
//! 刻意不依赖 nemesis-config）——策略入参由调用方解析后传入。

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;

// ---------------------------------------------------------------------------
// 版本与警告（D5 单一真相源）
// ---------------------------------------------------------------------------

/// 钉版 commit 短哈希（submodule `3rd/mxc` 当前指向）。升级 = submodule
/// advance + 本常量 + PLAN §3.2 三点同 commit 更新。
pub const MXC_VERSION: &str = "4e522623";

/// MXC 选择警告（用户拍板原文，版本串随 [`MXC_VERSION`] 走）。出现位置：
/// ① Dashboard 选择器确认框；② MXC 页面常驻横幅。前端文案由 WSAPI 回传
/// 本常量（禁止前端再硬编码一份）。
pub const MXC_WARNING: &str =
    "【当前 MXC 版本是 4e522623，目前功能是护栏功能，非沙盒，无法完整保护系统，可能导致出现异常】";

// ---------------------------------------------------------------------------
// 探测（三态 + 进程内缓存）
// ---------------------------------------------------------------------------

/// MXC 可用性三态报告（serde Serialize → WSAPI/CLI 直接回传）。
#[derive(Debug, Clone, Serialize)]
pub struct MxcProbeReport {
    /// `ready` | `unavailable`。
    pub status: String,
    /// 本平台的 MXC 后端名（processcontainer / bubblewrap / seatbelt）。
    pub platform_backend: String,
    /// 人读结论：ready 时一句话；unavailable 时透传 MXC/官方 probe 的原因。
    pub detail: String,
}

static PROBE_CACHE: Mutex<Option<(MxcProbeReport, std::time::Instant)>> = Mutex::new(None);

/// 进程内缓存的探测报告。`force=false` 命中缓存直接返回（执行路径每调用
/// 询问用）；`force=true` 强制重探（Dashboard「重新检测」按钮 / WSAPI
/// `sandbox.mxc_probe` 用）。
pub fn probe_mxc_cached(force: bool) -> MxcProbeReport {
    let mut cache = PROBE_CACHE.lock().expect("probe cache mutex poisoned");
    if !force && let Some((report, at)) = cache.as_ref() {
        let _ = at; // 缓存无 TTL——环境在网关进程生命周期内不变（重启才重探）
        return report.clone();
    }
    let fresh = probe_mxc_fresh();
    *cache = Some((fresh.clone(), std::time::Instant::now()));
    fresh
}

fn probe_mxc_fresh() -> MxcProbeReport {
    #[cfg(target_os = "windows")]
    {
        // 官方 request-aware probe：只读、不创建容器。probe(None) = 基础
        // 事实面（PSEC API 在场性）；错误 = 透传的机器原因。
        match mxc_sdk::v1::probe(None) {
            Ok(out) => {
                if let Some(err) = out.error {
                    return MxcProbeReport {
                        status: "unavailable".to_string(),
                        platform_backend: "processcontainer".to_string(),
                        detail: format!("MXC 探测不可用：{err}"),
                    };
                }
                let api = out.probes.base_container_api_present;
                let status = if out.tier.is_some() && api {
                    "ready"
                } else {
                    "unavailable"
                };
                let detail = if status == "ready" {
                    "MXC 探测通过（系统隔离组件可用，无需安装）".to_string()
                } else {
                    format!(
                        "MXC 探测不可用：系统隔离组件缺席（PSEC api_present={api}，\
                         tier={:?}）——Windows 需 11 24H2+ 且安装对应累积更新",
                        out.tier
                    )
                };
                MxcProbeReport {
                    status: status.to_string(),
                    platform_backend: "processcontainer".to_string(),
                    detail,
                }
            }
            Err(e) => MxcProbeReport {
                status: "unavailable".to_string(),
                platform_backend: "processcontainer".to_string(),
                detail: format!("MXC 探测执行失败：{e}"),
            },
        }
    }
    #[cfg(target_os = "linux")]
    {
        // 官方 bubblewrap 可用性探测（版本二进制 + 必要 flag 支持）。
        match mxc_sdk::bwrap_common::probe_bwrap() {
            Ok(ver) => MxcProbeReport {
                status: "ready".to_string(),
                platform_backend: "bubblewrap".to_string(),
                detail: format!("MXC 探测通过（bwrap {}）", ver.version),
            },
            Err(e) => MxcProbeReport {
                status: "unavailable".to_string(),
                platform_backend: "bubblewrap".to_string(),
                detail: format!("MXC 探测不可用：{e}（安装 bubblewrap 包后重试）"),
            },
        }
    }
    #[cfg(target_os = "macos")]
    {
        // seatbelt 是 macOS 系统内置设施（/usr/bin/sandbox-exec）——存在性
        // 检查即诚实（MXC 未提供更深的 macOS probe 面）。
        let present = std::path::Path::new("/usr/bin/sandbox-exec").exists();
        MxcProbeReport {
            status: if present { "ready" } else { "unavailable" }.to_string(),
            platform_backend: "seatbelt".to_string(),
            detail: if present {
                "MXC 探测通过（系统内置 seatbelt）".to_string()
            } else {
                "MXC 探测不可用：/usr/bin/sandbox-exec 缺席（非标准 macOS 环境）".to_string()
            },
        }
    }
    // 其他目标（理论上到不了——nemesis-sandbox 只编三桌面平台）：诚实不可用。
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
    {
        MxcProbeReport {
            status: "unavailable".to_string(),
            platform_backend: "unsupported".to_string(),
            detail: "MXC 不支持此平台".to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// spawn 事务（同步阻塞——调用方 spawn_blocking 包住）
// ---------------------------------------------------------------------------

/// MXC 容器内 spawn 事务的入参（nemesis-agent 侧同构类型经
/// gateway 装配闭包转译——本 crate 不依赖 nemesis-agent，与 DACL 档同范式）。
#[derive(Debug, Clone)]
pub struct MxcSpawnSpec {
    /// nemesisbot 可执行文件路径（executor 角色子进程 = 网关自身 exe）。
    pub exe: PathBuf,
    /// 子进程环境对（`ExecutorChannel::env_pairs()` 产出：NEMESISBOT_ROLE /
    /// NEMESISBOT_EXECUTOR_WORKSPACE / lease / deny patterns 等全套）。
    pub env_pairs: Vec<(String, String)>,
    /// 工作目录（executor workspace 根——容器内 working_directory + 文件
    /// 策略 readwrite 根双重来源）。
    pub working_dir: PathBuf,
    /// NEMESISBOT_HOME（子进程读 `<home>/config.json` 的 executor.
    /// allow_network——只读放行）。
    pub home: PathBuf,
    /// 容器额外读写根（项目 agent home 等——`ExecutorChannel::extra_roots`
    /// 同源透传；容器授权模型下 env 围栏放行而容器拒绝 = 自相矛盾，审查
    /// 三轮发现，必须随容器 FilesystemPolicy 一并授予）。
    pub extra_roots: Vec<PathBuf>,
    /// 单行 JSON 请求（与 stdio 路径同一协议行，行尾含 `\n`）。
    pub request_line: String,
    /// 事务超时（MXC 引擎按 request.timeout_ms 执行并回报 TimedOut）。
    pub timeout: Duration,
}

/// 一次 MXC 容器内 spawn 事务的结果（与 nemesis-agent `DaclSpawnOutcome`
/// 字段同构）。
#[derive(Debug, Clone)]
pub struct MxcCallOutcome {
    /// stdout 首行（executor 协议响应行；`None` = 无响应退出/超时）。
    pub response: Option<String>,
    pub exit_code: Option<i32>,
    pub stderr_tail: String,
}

/// 构造 [`ContainerRequest`]（纯函数，单测钉策略映射）。`network` =
/// `"open"`（egress 全放行，与 native 现状一致）| `"locked"`（egress
/// deny-default + `egress_allow` 白名单）；其他值 loud 拒绝（对齐
/// `executor.backend` 未知值先例）。`egress_allow` 每条 `"cidr"` 或
/// `"cidr:port"`（缺协议 = tcp）。
pub fn build_container_request(
    spec: &MxcSpawnSpec,
    network: &str,
    egress_allow: &[String],
) -> Result<mxc_sdk::v1::ContainerRequest, String> {
    let network_policy = match network {
        // open：egress 全放行（能力授予 + WFP 不拦）——与现状零差异；
        // ingress 恒 deny（子进程不开监听面）。
        "open" => mxc_sdk::v1::NetworkPolicy {
            egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Allow),
                allow: None,
                deny: None,
            }),
            ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                default: Some(mxc_sdk::v1::NetworkAction::Deny),
                host_loopback: Some(mxc_sdk::v1::NetworkAction::Deny),
            }),
            runtime_config: None,
        },
        "locked" => {
            let mut rules = Vec::new();
            for entry in egress_allow {
                // 形态解析：`cidr` 或 `cidr:port`。尾段是合法端口（u16 且
                // ≥1——0 被引擎 spawn 期拒绝且 `+443` 这类带符号形态
                // parse 也能过，都归非法；审查三轮发现）→ 拆出端口；尾段
                // 非合法端口时**按冒号数分岔**：单冒号 = 坏端口意图（如
                // `10.0.0.0/8:99999`），loud 拒绝——不能整串当 cidr 塞给
                // 引擎（spawn 期才炸且原因不可读）；多冒号 = IPv6 字面量，
                // 首期不支持端口限定，整串按纯 CIDR 走引擎校验。
                let (cidr, port) = match entry.rsplit_once(':') {
                    Some((c, p))
                        if !p.starts_with('+') && p.parse::<u16>().is_ok_and(|v| v >= 1) =>
                    {
                        (c.to_string(), Some(p.parse::<u16>().expect("已判数字")))
                    }
                    Some((_c, _p)) if entry.matches(':').count() == 1 => {
                        return Err(format!(
                            "executor.mxc.egress_allow 条目 '{entry}' 的端口段非法（期望 `cidr` 或 `cidr:port`，端口须 1..=65535）"
                        ));
                    }
                    // 多冒号（IPv6）或无冒号：整串走引擎校验
                    _ => (entry.clone(), None),
                };
                if cidr.is_empty() || !cidr.contains('/') {
                    return Err(format!(
                        "executor.mxc.egress_allow 条目 '{entry}' 不是合法 CIDR（期望 `cidr` 或 `cidr:port`，例 140.82.112.0/20:443）"
                    ));
                }
                rules.push(mxc_sdk::v1::NetworkRulePolicy {
                    to: Some(vec![mxc_sdk::v1::NetworkPeerPolicy { cidr, except: None }]),
                    ports: Some(vec![mxc_sdk::v1::NetworkPortPolicy {
                        protocol: Some(mxc_sdk::v1::NetworkProtocol::Tcp),
                        port,
                        end_port: None,
                    }]),
                });
            }
            mxc_sdk::v1::NetworkPolicy {
                egress: Some(mxc_sdk::v1::NetworkEgressPolicy {
                    default: Some(mxc_sdk::v1::NetworkAction::Deny),
                    allow: Some(rules),
                    deny: None,
                }),
                ingress: Some(mxc_sdk::v1::NetworkIngressPolicy {
                    default: Some(mxc_sdk::v1::NetworkAction::Deny),
                    host_loopback: Some(mxc_sdk::v1::NetworkAction::Deny),
                }),
                runtime_config: None,
            }
        }
        other => {
            return Err(format!(
                "executor.mxc.network 未知档位 '{other}'（合法 open|locked）"
            ));
        }
    };

    // 文件系统：工作区读写；home 只读（子进程读 config.json 的
    // executor.allow_network）；凭据与认证态定向拒绝（真实路径是
    // `<home>/workspace/config/`——credentials_path_for_home 同源，审查三
    // 轮发现旧条目指向不存在的 `<home>/config/` 且被存在性过滤抹掉、保护
    // 整体落空）。写面 + 读面都压——护栏语义下最小授权，秘密不进容器。
    let ws_config = spec.home.join("workspace").join("config");
    let secrets: Vec<String> = ["credentials.yaml", "auth.json"]
        .iter()
        .map(|f| ws_config.join(f).display().to_string())
        .collect();
    let mut readwrite = vec![spec.working_dir.display().to_string()];
    readwrite.extend(spec.extra_roots.iter().map(|r| r.display().to_string()));
    let filesystem = mxc_sdk::v1::FilesystemPolicy {
        readwrite_paths: readwrite,
        readonly_paths: vec![spec.home.display().to_string()],
        denied_paths: secrets,
        clear_policy_on_exit: None,
    };

    let timeout_ms = u32::try_from(spec.timeout.as_millis()).unwrap_or(u32::MAX);

    Ok(mxc_sdk::v1::ContainerRequest {
        // executor 子进程无 CLI args（NEMESISBOT_ROLE env 路由）；路径带
        // 空格用引号包（与 Start.exe wrap 同形态）。
        command: format!("\"{}\"", spec.exe.display()),
        filesystem: Some(filesystem),
        network: Some(network_policy),
        // UI 细粒度档（真机实证 2026-11-14 + MXC 官方 launch_diagnostics
        // 处方）：disable=true（全禁）会让链了 GUI DLL（user32/comctl32——
        // nemesisbot 全量构建的托盘面静态导入）的子进程死在
        // STATUS_DLL_INIT_FAILED（0xC0000142，Win32k 系统调用被拦）。故
        // disable=false 放开 UI 子系统初始化，**剪贴板 None + 输入注入禁
        // 保留**——控制台工具执行器的加固面不受损。
        ui: Some(mxc_sdk::v1::UiPolicy {
            disable: false,
            clipboard: mxc_sdk::v1::ClipboardPolicy::None,
            allow_input_injection: false,
        }),
        timeout_ms: Some(timeout_ms),
        // 抽象意图按宿主解析：Win=ProcessContainer / Linux=bubblewrap /
        // macOS=seatbelt。实验性后端一律不碰。
        containment: mxc_sdk::v1::Containment::Process,
        container_name: None,
        working_directory: Some(spec.working_dir.display().to_string()),
        // 执行体运输层 env（NEMESISBOT_ROLE 等）逐条注入；叠在后端默认
        // 环境上（PATH 等不丢）。
        environment: Some(spec.env_pairs.clone()),
        inherit_default_environment: Some(true),
    })
}

/// 策略路径存在性判定：自身在场，或父目录在场（父在场 = 「文件稍后创建
/// 仍被拒绝」的声明合法）。两者皆缺 → 该策略条目无意义，调用方过滤。
fn path_self_or_parent_exists(p: &str) -> bool {
    let path = std::path::Path::new(p);
    path.exists() || path.parent().is_some_and(|parent| parent.exists())
}

/// MXC 容器内 spawn + stdio 协议事务。**同步阻塞**——调用方（nemesisbot
/// 装配闭包）负责 `tokio::task::spawn_blocking` 包裹。超时由引擎按
/// request.timeout_ms 执行（`WaitResult::TimedOut` → response=None）。
pub fn spawn_contained(
    spec: MxcSpawnSpec,
    network: &str,
    egress_allow: &[String],
) -> Result<MxcCallOutcome, String> {
    let mut request = build_container_request(&spec, network, egress_allow)?;
    // PSEC 策略路径存在性闸（真机实证 2026-11-14）：fs 策略引用「自身与
    // 父目录都不存在」的路径 → CreateProcessSecurityEnvironment 失败
    // （HRESULT 0x80070003 ERROR_PATH_NOT_FOUND）。典型场景：全新环境的
    // 凭据文件 `config/credentials.yaml` 连同其父目录都不存在——「拒绝一
    // 个不存在的文件」没有策略意义，过滤之（文件或父目录任一在场即保留，
    // 覆盖「文件稍后创建」的合法拒绝声明）。过滤在构造（纯函数）之后、
    // spawn 之前——build_container_request 保持纯函数（单测用假路径）。
    if let Some(fs) = request.filesystem.as_mut() {
        let keep = |p: &String| path_self_or_parent_exists(p);
        fs.denied_paths.retain(keep);
        fs.readonly_paths.retain(keep);
        fs.readwrite_paths.retain(keep);
    }
    // 遥测双保险之运行期闸：显式 Some(false)（构建期红线见模块文档）。
    let options = mxc_sdk::v1::SpawnOptions {
        telemetry: Some(mxc_sdk::v1::TelemetryConfig {
            enabled: Some(false),
        }),
    };
    let mut proc = mxc_sdk::v1::spawn_with_stdin(request, options).map_err(|e| {
        format!(
            "MXC 容器 spawn 失败（后端={}）：{e}",
            probe_mxc_cached(false).platform_backend
        )
    })?;

    let mut stdin = proc
        .take_stdin()
        .ok_or("MXC 子进程 stdin 缺席（后端不支持 stdio 写入）")?;
    stdin
        .write_all(spec.request_line.as_bytes())
        .map_err(|e| format!("MXC 子进程请求行写入失败: {e}"))?;
    stdin
        .flush()
        .map_err(|e| format!("MXC 子进程请求行 flush 失败: {e}"))?;
    // drop(stdin) = EOF 信号（exec_worker 读到行尾即解析，此 drop 亦可）。
    drop(stdin);

    let stdout = proc.take_stdout().ok_or("MXC 子进程 stdout 缺席")?;
    let mut stderr = proc.take_stderr();

    // stderr 尾巴收集线程（协议响应在 stdout；stderr 只进诊断尾巴）。
    // 分块读 + 环形截尾（保留最后 2048B）——**不得** read_to_end 整缓：刷屏
    // 子进程（npm install / verbose build）会把几百 MB 灌进网关 RSS（审查
    // 发现，2026-11-14）；与 executor_transport::StderrTail 同语义。
    let stderr_handle = stderr.take();
    let stderr_thread = std::thread::spawn(move || {
        const TAIL_CAP: usize = 2048;
        let mut tail: Vec<u8> = Vec::with_capacity(TAIL_CAP);
        if let Some(mut s) = stderr_handle {
            let mut chunk = [0u8; 1024];
            loop {
                match s.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        tail.extend_from_slice(&chunk[..n]);
                        if tail.len() > TAIL_CAP {
                            // 从头部丢到只剩上限（环形截尾语义）
                            let excess = tail.len() - TAIL_CAP;
                            tail.drain(..excess);
                        }
                    }
                }
            }
        }
        String::from_utf8_lossy(&tail).into_owned()
    });

    // stdout 首行 = 协议响应行（读到 EOF = 无响应退出/超时被杀）。
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    let read = reader.read_line(&mut line);

    let wait = proc.wait();
    let stderr_tail = stderr_thread.join().unwrap_or_default();

    // exit_code 恒尽力捕获；**TimedOut 不吞**（审查二轮发现）：引擎超时
    // 强杀若折叠成「无响应退出」，用户看到的与 OOM/panic 静默死无异——
    // 违反「失败信息必须自带现场」纪律。TimedOut → 诚实 Err 点名超时。
    let exit_code = match wait {
        Ok(mxc_sdk::v1::WaitResult::Exited(code)) => Some(code),
        Ok(mxc_sdk::v1::WaitResult::TimedOut) => {
            return Err(format!(
                "MXC 容器事务超时（引擎按 timeout_ms={} 强制终止子进程；stderr: {stderr_tail}）",
                spec.timeout.as_millis()
            ));
        }
        Err(e) => {
            return Err(format!(
                "MXC 子进程 wait 失败: {e}（stderr: {stderr_tail}）"
            ));
        }
    };
    match read {
        Err(e) => Err(format!(
            "MXC 子进程 stdout 读取失败: {e}（stderr: {stderr_tail}）"
        )),
        _ => {
            let trimmed = line.trim_end();
            Ok(MxcCallOutcome {
                response: if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                },
                exit_code,
                stderr_tail,
            })
        }
    }
}

#[cfg(test)]
mod tests;
