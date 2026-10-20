//! VM 生命周期原语（P5 第 2 步）：探针裁决 → QEMU spawn → QMP 就绪等待 →
//! 停止（QMP quit 优雅路径 → kill 兜底）→ guest agent 连接（首帧 token
//! 挑战鉴权）+ 工具调用。
//!
//! 诚实边界（文档见实施计划 §5）：
//! - 探针只做静态必要条件（/dev/kvm、WinHvPlatform.dll、qemu --version），
//!   最终裁决在真启动（QMP 握手失败 = Launch 错误带 stderr 日志路径）；
//! - QMP 无鉴权、guest agent NAT 端口无网络层防护——两者**恒绑
//!   127.0.0.1**（装配层钉死，见 `assembly`），宿主其他本地进程可达 =
//!   与 hostfwd 同级的本地威胁模型，首帧 token 是 guest agent 唯一入口
//!   鉴权（本地进程枚举 token 的威胁不设防，声明的威胁模型之内）；
//! - 临时端口 bind-0-drop-reserve 有竞窗（别的进程抢先绑走）→ QEMU
//!   bind 失败诚实 Launch 错误（stderr 日志带现场），不做重试循环。

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use nemesis_exec_protocol::{ExecutorResponseWire, build_v2_request_line, new_call_id};

use super::assembly::{VmLaunchSpec, build_qemu_args, validate_launch_spec};
use super::guest_arch::{GuestArch, QemuAccel};
use super::probe::{AccelProbe, probe_kvm, probe_whpx};
use super::qmp::QmpClient;

/// QEMU 启动后等待 QMP 就绪的总预算（QEMU 进程起 + 机器初始化通常 <5s；
/// 60s 覆盖慢机与 WHPX 冷启动）。
pub const QMP_READY_TIMEOUT: Duration = Duration::from_secs(60);

/// wake（`-incoming file:`）后的额外就绪预算：迁移状态回放完成前 QMP
/// 状态停在 inmigrate/paused——不在冷启预算内硬卡，单独给 180s（2GB
/// RAM 状态文件回放覆盖慢盘）。
pub const WAKE_READY_TIMEOUT: Duration = Duration::from_secs(180);

/// savevm 快照 tag（固定内部常量——无用户输入通道，HMP 注入面为零；
/// 重复 sleep 同 tag 覆盖旧快照 = 幂等语义，qcow2 内不累积）。
pub const SLEEP_SNAPSHOT_TAG: &str = "p5sleep";

/// migrate-to-file 状态文件名（落 run_dir）。
pub const SLEEP_STATE_FILENAME: &str = "vm.state";

/// 睡眠状态载体（P5 第 3 步；线选择随宿主 accel 定，见
/// [`SleepMode::default_for_accel`]）。两条线的机制面都实装、fake-qmp
/// 测试全覆盖（CI 锚 + 接口预留），但**生产默认线**由早验定谳钉死：
/// QEMU 11.1.0 WHPX 的迁移机器整体挂死（savevm 180s+300s 无响应、
/// migrate-to-file 三种 URI 形态各 30s 连 error 对象都无应答，2026-10-18
/// 三探针定谳）——WHPX 当前置 None = 诚实不支持，详见实施计划附录。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepMode {
    /// savevm qcow2 内部快照（KVM 默认线；wake = `-loadvm <tag>`）。
    Snapshot,
    /// migrate-to-file 外部状态文件（备选线，接口预留；wake =
    /// `-incoming file:`。当前无 accel 默认落到它——QEMU 侧 WHPX 迁移
    /// 修复后可按同门重新早验）。
    MigrateFile,
}

impl SleepMode {
    /// 按加速后端选默认睡眠线；`None` = 该 accel 眼下没有可用睡眠线
    /// （调用方诚实报「不支持」，不做假睡——退而求其次的 stop 冷停是
    /// 调用方的显式决定，RAM 状态会丢）。
    pub fn default_for_accel(accel: QemuAccel) -> Option<Self> {
        match accel {
            QemuAccel::Kvm => Some(SleepMode::Snapshot),
            // WHPX：savevm 与 migrate 双双挂死（三探针定谳，2026-10-18），
            // 编码成 None 而非硬选一条必挂的线烧预算。
            QemuAccel::Whpx => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            SleepMode::Snapshot => "snapshot",
            SleepMode::MigrateFile => "migrate-file",
        }
    }
}

/// sleep 成功产物（bot 侧 registry 落 instance.json 的凭据——wake 重挂
/// 只认这两者之一；同一次 sleep 只填一个）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SleepArtifact {
    /// Snapshot 线：savevm tag（wake 用 `-loadvm <tag>`）。
    pub snapshot_tag: Option<String>,
    /// MigrateFile 线：状态文件绝对路径（wake 用 `-incoming file:<path>`）。
    pub state_path: Option<PathBuf>,
}

/// VM 生命周期错误。`Unavailable` 与 `StrictReject` 的分工：两者都拒绝
/// 执行（**绝不 TCG 降级**——性能不可用级的假跑违反诚实降级铁律），区别
/// 在调用方语义——`Unavailable` 允许上层回落到别的执行后端（Sandboxie /
/// 宿主直跑），`StrictReject` 是 `executor.strict` fail-closed 闸门，要求
/// 沙盒但本机不可用 → 拒绝执行而非降级。
#[derive(Debug)]
pub enum VmError {
    /// 探针缺席（非 strict——调用方可回落到其他执行后端）。
    Unavailable(String),
    /// strict 模式 fail-closed（要求 VM 但宿主不可用 → 拒绝执行）。
    StrictReject(String),
    /// 启动失败（spawn 失败 / QMP 就绪超时 / 状态异常）。
    Launch(String),
    /// QMP 交互错误。
    Qmp(super::qmp::QmpError),
    /// 操作超时（连接 / 鉴权 / 工具调用预算耗尽）。
    Timeout,
    /// 底层 IO 错。
    Io(std::io::Error),
}

impl std::fmt::Display for VmError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmError::Unavailable(m) => write!(f, "VM 后端不可用（可回落）: {m}"),
            VmError::StrictReject(m) => write!(f, "VM 后端不可用且 strict 模式拒绝执行: {m}"),
            VmError::Launch(m) => write!(f, "VM 启动失败: {m}"),
            VmError::Qmp(e) => write!(f, "{e}"),
            VmError::Timeout => write!(f, "VM 操作超时"),
            VmError::Io(e) => write!(f, "VM io error: {e}"),
        }
    }
}

impl From<super::qmp::QmpError> for VmError {
    fn from(e: super::qmp::QmpError) -> Self {
        VmError::Qmp(e)
    }
}

impl From<std::io::Error> for VmError {
    fn from(e: std::io::Error) -> Self {
        VmError::Io(e)
    }
}

/// accel 裁决（**纯函数**——探测结果作参数传入，CI 全组合可测；运行期
/// 包装 [`start_vm`] 只负责真实探测后调它）。
///
/// - `arch` 未产出（aarch64 预留档）→ `Unavailable`（诚实「当前不产出」）；
/// - 显式 `hypervisor`：该 accel 探针缺席 → strict ? `StrictReject` :
///   `Unavailable`；
/// - `auto`：按宿主 OS 选首选序（Windows = WHPX 优先；Linux = KVM 优先；
///   非首选留作显式覆盖口），全部缺席 → strict ? `StrictReject` :
///   `Unavailable`（原因聚合两路探针明细）。
pub fn adjudicate_accel(
    strict: bool,
    explicit: Option<QemuAccel>,
    kvm_probe: &AccelProbe,
    whpx_probe: &AccelProbe,
    host_is_windows: bool,
    arch: GuestArch,
) -> Result<QemuAccel, VmError> {
    if !arch.is_produced() {
        return Err(VmError::Unavailable(format!(
            "guest 架构 {} 为接口预留档，当前不产出（不产镜像、不启动运行）",
            arch.as_str()
        )));
    }
    let probe_of = |a: QemuAccel| match a {
        QemuAccel::Kvm => kvm_probe,
        QemuAccel::Whpx => whpx_probe,
    };
    if let Some(acc) = explicit {
        return match probe_of(acc) {
            p if p.available => Ok(acc),
            p => {
                let msg = format!("accel {} 探针缺席: {}", acc.as_str(), p.reason);
                Err(if strict {
                    VmError::StrictReject(msg)
                } else {
                    VmError::Unavailable(msg)
                })
            }
        };
    }
    let order: &[QemuAccel] = if host_is_windows {
        &[QemuAccel::Whpx, QemuAccel::Kvm]
    } else {
        &[QemuAccel::Kvm, QemuAccel::Whpx]
    };
    for acc in order {
        let p = probe_of(*acc);
        if p.available {
            return Ok(*acc);
        }
    }
    let msg = format!(
        "无可用硬件加速（kvm: {}；whpx: {}）——不做 TCG 纯模拟降级",
        kvm_probe.reason, whpx_probe.reason
    );
    Err(if strict {
        VmError::StrictReject(msg)
    } else {
        VmError::Unavailable(msg)
    })
}

/// 运行期真实探测 + 裁决（[`adjudicate_accel`] 的 IO 包装——探测本机
/// KVM/WHPX 静态层后裁决）。
pub fn adjudicate_accel_real(
    strict: bool,
    explicit: Option<QemuAccel>,
    arch: GuestArch,
) -> Result<QemuAccel, VmError> {
    let kvm = probe_kvm();
    let whpx = probe_whpx();
    adjudicate_accel(strict, explicit, &kvm, &whpx, cfg!(windows), arch)
}

/// 生成 per-instance 鉴权 token：SHA-256 折叠（time_ns + pid + 进程内
/// 计数器）取前 16 字节 = 32 hex。
///
/// 威胁模型声明：**本地回环进程**（与宿主直跑形态同级）——熵源非 CSPRNG
/// 是刻意的诚实边界；防的是别的 agent 实例/误绑进程的误连，不防有意的
/// 本地枚举攻击。常量时间比较在 guest 侧（`ct_eq`）。
pub fn generate_token() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut hasher = Sha256::new();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update(n.to_le_bytes());
    let digest = hasher.finalize();
    let mut token = String::with_capacity(32);
    for b in &digest[..16] {
        token.push_str(&format!("{b:02x}"));
    }
    token
}

/// 分配一个 127.0.0.1 临时端口（bind-0-drop-reserve；竞窗见模块文档）。
pub fn alloc_loopback_port() -> std::io::Result<u16> {
    let l = TcpListener::bind(("127.0.0.1", 0))?;
    let port = l.local_addr()?.port();
    drop(l);
    Ok(port)
}

/// 一次 VM 启动的完整输入（caller 从 `ExecutorVmConfig` + 镜像管线产出；
/// accel 已经裁决）。
#[derive(Debug, Clone)]
pub struct VmStartParams {
    /// 实例 id（run_dir 命名与日志关联）。
    pub id: String,
    /// 运行期目录（stderr 日志 / pidfile 落这里）。
    pub run_dir: PathBuf,
    /// qemu-system 二进制（[`super::assembly::resolve_qemu_binary`] 产出）。
    pub qemu_bin: PathBuf,
    /// 已裁决的加速后端（[`adjudicate_accel`]）。
    pub accel: QemuAccel,
    pub arch: GuestArch,
    pub vcpu: u32,
    pub ram_mb: u32,
    /// 本实例 overlay（backing = base 镜像，见 `overlay` 模块）。
    pub overlay_path: PathBuf,
    /// per-instance 鉴权 token（[`generate_token`]）。
    pub token: String,
    /// 9p 共享子树（None = 不挂，Windows 宿主诚实边界）。
    pub shared_host_path: Option<PathBuf>,
    /// wake 线 A：savevm 快照 tag（睡眠实例重挂用；冷启 None）。
    pub loadvm_tag: Option<String>,
    /// wake 线 B：migrate-to-file 状态文件（睡眠实例重挂用；冷启 None）。
    pub incoming_file: Option<PathBuf>,
}

/// 运行中的 VM 实例（QEMU 子进程句柄 + 控制面/数据面地址）。
#[derive(Debug)]
pub struct VmInstance {
    /// 实例 id（与 `VmStartParams.id` 同源）。
    pub id: String,
    child: tokio::process::Child,
    /// QMP 控制面地址（127.0.0.1 临时端口）。
    pub qmp_addr: SocketAddr,
    /// guest agent 数据面地址（hostfwd 宿主侧，127.0.0.1 临时端口）。
    pub agent_addr: SocketAddr,
    /// pidfile 路径（孤儿 sweep 通道锚点，第 3 步消费）。
    pub pidfile: PathBuf,
    /// per-instance 鉴权 token。
    pub token: String,
    run_dir: PathBuf,
}

impl VmInstance {
    /// spawn QEMU + 等 QMP 就绪 + 状态核验（真启动裁决——静态探针只是
    /// 必要条件，这里才是最终闸）。
    pub async fn start(p: VmStartParams) -> Result<Self, VmError> {
        std::fs::create_dir_all(&p.run_dir).map_err(|e| {
            VmError::Launch(format!("run_dir 创建失败 {}: {e}", p.run_dir.display()))
        })?;
        let qmp_port = alloc_loopback_port().map_err(VmError::Io)?;
        let host_port = alloc_loopback_port().map_err(VmError::Io)?;
        let spec = VmLaunchSpec {
            qemu_bin: p.qemu_bin.clone(),
            pidfile: p.run_dir.join("qemu.pid"),
            accel: p.accel,
            arch: p.arch,
            vcpu: p.vcpu,
            ram_mb: p.ram_mb,
            overlay_path: p.overlay_path.clone(),
            qmp_port,
            host_forward_port: host_port,
            token: p.token.clone(),
            shared_host_path: p.shared_host_path.clone(),
            loadvm_tag: p.loadvm_tag.clone(),
            incoming_file: p.incoming_file.clone(),
        };
        // 装配闸（逗号/wake 互斥）：spawn 前诚实拒绝，不带病上岗。
        if let Err(reason) = validate_launch_spec(&spec) {
            return Err(VmError::Launch(reason));
        }
        let args = build_qemu_args(&spec);
        let stderr_path = p.run_dir.join("qemu.stderr.log");
        let stderr_file = std::fs::File::create(&stderr_path).map_err(|e| {
            VmError::Launch(format!(
                "stderr 日志创建失败 {}: {e}",
                stderr_path.display()
            ))
        })?;

        let mut cmd = tokio::process::Command::new(&p.qemu_bin);
        cmd.args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::from(stderr_file));
        // 后台形态：不弹窗（Windows CREATE_NO_WINDOW；对齐仓库后台进程纪律）。
        #[cfg(target_os = "windows")]
        {
            cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
        }
        let mut child = cmd.spawn().map_err(|e| {
            VmError::Launch(format!("QEMU spawn 失败 {}: {e}", p.qemu_bin.display()))
        })?;

        let qmp_addr: SocketAddr = format!("127.0.0.1:{qmp_port}").parse().expect("qmp addr");
        let agent_addr: SocketAddr = format!("127.0.0.1:{host_port}")
            .parse()
            .expect("agent addr");
        // wake 重挂（-incoming/-loadvm）= 状态回放完成前 QMP 停在过渡态，
        // 就绪判定走过渡态容忍分支（独立预算，见 WAKE_READY_TIMEOUT）。
        let is_wake = p.loadvm_tag.is_some() || p.incoming_file.is_some();
        Self::wait_qmp_ready(&mut child, qmp_addr, &stderr_path, is_wake).await?;

        tracing::info!(
            id = %p.id,
            %qmp_port,
            %host_port,
            accel = p.accel.as_str(),
            "[vm] QEMU 实例已启动（QMP 就绪）"
        );
        Ok(Self {
            id: p.id,
            child,
            qmp_addr,
            agent_addr,
            pidfile: spec.pidfile,
            token: p.token,
            run_dir: p.run_dir,
        })
    }

    /// 轮询 QMP 直至就绪（失败现场落 stderr 日志路径——诊断自包含）。
    /// `wake=true`：状态回放完成前容忍过渡态（inmigrate/prelaunch/paused/
    /// restore-*），预算切 [`WAKE_READY_TIMEOUT`]；冷启仍要求直接 running。
    async fn wait_qmp_ready(
        child: &mut tokio::process::Child,
        qmp_addr: SocketAddr,
        stderr_path: &Path,
        wake: bool,
    ) -> Result<(), VmError> {
        let total = if wake {
            WAKE_READY_TIMEOUT
        } else {
            QMP_READY_TIMEOUT
        };
        Self::wait_qmp_ready_with_budget(child, qmp_addr, stderr_path, wake, total).await
    }

    /// 轮询主体（预算参数化——单测以短预算驱动全部失败路径，不烧 60s/180s）。
    ///
    /// 收尸纪律：spawn 成功后的**每一条**失败路径都必须 kill+wait 子进程再
    /// 返回（deadline 与坏状态两处）——泄漏一个还在跑的 QEMU 会占住 overlay
    /// 和端口，下轮 ensure_running 再 spawn 第二个 QEMU 抢同一 overlay =
    /// qcow2 并发写损坏。查询瞬断不算失败：与连接失败同性质（进程活着、
    /// 这一轮没读上），继续轮询。
    async fn wait_qmp_ready_with_budget(
        child: &mut tokio::process::Child,
        qmp_addr: SocketAddr,
        stderr_path: &Path,
        wake: bool,
        total: Duration,
    ) -> Result<(), VmError> {
        let deadline = tokio::time::Instant::now() + total;
        loop {
            // QEMU 已退出 = 启动失败，立即报（不等满预算）。
            if let Ok(Some(status)) = child.try_wait() {
                return Err(VmError::Launch(format!(
                    "QEMU 提前退出（{status}）——stderr 现场: {}",
                    stderr_path.display()
                )));
            }
            if let Ok(mut client) = QmpClient::connect(qmp_addr, Duration::from_secs(1)).await {
                match client.query_status(Duration::from_secs(5)).await {
                    Ok(status) => {
                        let transitional = wake
                            && matches!(
                                status.as_str(),
                                "inmigrate" | "prelaunch" | "paused" | "restore-vm"
                            );
                        if status != "running" && !transitional {
                            // 收尸后再报（见函数级收尸纪律）。
                            let _ = child.kill().await;
                            let _ = child.wait().await;
                            return Err(VmError::Launch(format!(
                                "QEMU 起了但状态非 running（{status}），已终止——stderr 现场: {}",
                                stderr_path.display()
                            )));
                        }
                        if status == "running" {
                            return Ok(());
                        }
                        // wake 过渡态：继续轮询到 running（预算由外层 deadline 管）。
                    }
                    Err(e) => {
                        tracing::debug!(error = ?e, "[vm] QMP 状态查询未读上，继续轮询");
                    }
                }
            }
            if tokio::time::Instant::now() >= deadline {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(VmError::Launch(format!(
                    "QMP {}s 内未就绪——stderr 现场: {}",
                    total.as_secs(),
                    stderr_path.display()
                )));
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    /// QEMU 进程是否仍在（不阻塞）。
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// QEMU 进程 pid（诊断 / pidfile 交叉核验）。
    pub fn pid(&self) -> Option<u32> {
        self.child.id()
    }

    /// stderr 日志路径（启动失败现场自包含）。
    pub fn stderr_log_path(&self) -> PathBuf {
        self.run_dir.join("qemu.stderr.log")
    }

    /// 查询客机状态（每次新起 QMP 连接——QMP 每连接独立会话）。
    pub async fn status(&self, timeout: Duration) -> Result<String, VmError> {
        let mut client = QmpClient::connect(self.qmp_addr, timeout).await?;
        Ok(client.query_status(timeout).await?)
    }

    /// 强杀（不走 QMP 优雅路径）：estop 语义 / 失控 VM / kill 探针用。
    /// kill 后等 wait 收割子进程（不留僵尸）。
    pub async fn kill(&mut self) -> Result<(), VmError> {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        tracing::warn!(id = %self.id, "[vm] QEMU 已强杀");
        Ok(())
    }

    /// 停止：QMP `quit` 优雅路径 → 预算内等退出 → 超时 kill 兜底。
    /// 优雅路径失败不阻断（QEMU 已死/卡死时 quit 连不上是正常面），kill
    /// 兜底保证收敛。
    pub async fn stop(&mut self, grace: Duration) -> Result<(), VmError> {
        match QmpClient::connect(self.qmp_addr, Duration::from_secs(5)).await {
            Ok(mut client) => {
                if let Err(e) = client.quit(Duration::from_secs(5)).await {
                    tracing::debug!(id = %self.id, "[vm] QMP quit 未成功（kill 兜底接管）: {e}");
                }
            }
            Err(e) => {
                tracing::debug!(id = %self.id, "[vm] QMP 停止连接未成（kill 兜底接管）: {e}");
            }
        }
        match tokio::time::timeout(grace, self.child.wait()).await {
            Ok(status) => {
                tracing::info!(id = %self.id, ?status, "[vm] QEMU 已退出（优雅路径）");
                Ok(())
            }
            Err(_) => {
                let _ = self.child.kill().await;
                let _ = self.child.wait().await;
                tracing::warn!(id = %self.id, "[vm] 优雅退出超预算，已 kill 兜底");
                Ok(())
            }
        }
    }

    /// 睡眠（P5 第 3 步）：[`sleep_via_qmp`] 状态保存 + 优雅退出 + 收割。
    /// Ok = VM 已退出且状态载体在手（caller 把产物落 registry 供 wake）；
    /// Err = 状态保存失败，**VM 保持运行**（无 quit——失败不毁现场，调用
    /// 方可重试 sleep 或改走 stop 丢弃状态）。
    pub async fn sleep(
        &mut self,
        mode: SleepMode,
        budget: Duration,
    ) -> Result<SleepArtifact, VmError> {
        let artifact = sleep_via_qmp(self.qmp_addr, mode, &self.run_dir, budget).await?;
        // sleep_via_qmp 已发 quit——等进程退出收割（预算内；超时 kill 兜底）。
        match tokio::time::timeout(Duration::from_secs(15), self.child.wait()).await {
            Ok(_status) => {}
            Err(_) => {
                let _ = self.child.kill().await;
                let _ = self.child.wait().await;
            }
        }
        tracing::info!(id = %self.id, mode = mode.as_str(), "[vm] 已睡眠（状态保存 + 退出）");
        Ok(artifact)
    }

    /// 连接 guest agent（数据面）：TCP → **首帧 token 挑战鉴权**（`{"v":2,
    /// "auth":"<token>"}`）→ 校验 ok → 返回流供 [`guest_call`] 复用。
    pub async fn connect_guest_agent(&self, timeout: Duration) -> Result<TcpStream, VmError> {
        auth_handshake(self.agent_addr, &self.token, timeout).await
    }
}

/// 睡眠执行（独立自由函数——[`VmInstance::sleep`] 与 fake-qmp 状态机 CI
/// 测试同源消费；不持有实例句柄，纯 QMP 地址驱动）。
///
/// 分支语义：
/// - Snapshot：HMP `savevm <tag>`（savevm 只有 HMP 形态）→ `quit`；
/// - MigrateFile：`migrate file:<run_dir>/vm.state` → 轮询
///   `query-migrate` 至 completed → `quit`；
/// - **失败不 quit**——VM 保持运行（状态没保存成功就退出 = 丢状态，
///   诚实失败让调用方裁决重试或丢弃）。
pub async fn sleep_via_qmp(
    qmp_addr: SocketAddr,
    mode: SleepMode,
    run_dir: &Path,
    budget: Duration,
) -> Result<SleepArtifact, VmError> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut client = QmpClient::connect(qmp_addr, Duration::from_secs(5)).await?;
    let artifact = match mode {
        SleepMode::Snapshot => {
            // savevm 预算 = 总预算余额直用（大内存 savevm 需要分钟级，5s
            // 硬顶会假超时；预算耗尽即诚实 Timeout）。
            let remaining = deadline
                .saturating_duration_since(tokio::time::Instant::now())
                .max(Duration::from_secs(1));
            let output = client
                .human_monitor(&format!("savevm {SLEEP_SNAPSHOT_TAG}"), remaining)
                .await?;
            tracing::debug!("[vm] savevm 完成（HMP 输出 {output:?}）");
            SleepArtifact {
                snapshot_tag: Some(SLEEP_SNAPSHOT_TAG.to_string()),
                state_path: None,
            }
        }
        SleepMode::MigrateFile => {
            let state_path = run_dir.join(SLEEP_STATE_FILENAME);
            let uri = format!(
                "file:{}",
                state_path.display().to_string().replace('\\', "/")
            );
            client.migrate(&uri, Duration::from_secs(10)).await?;
            // 轮询到 completed（active 迭代中；failed = 诚实错误）。
            loop {
                if tokio::time::Instant::now() >= deadline {
                    return Err(VmError::Timeout);
                }
                let status = client
                    .query_migrate(Duration::from_secs(5))
                    .await
                    .map_err(VmError::Qmp)?;
                match status.as_str() {
                    "completed" => break,
                    "failed" | "cancelled" => {
                        return Err(VmError::Launch(format!(
                            "migrate-to-file 未完成（status={status}）——VM 保持运行"
                        )));
                    }
                    _ => tokio::time::sleep(Duration::from_millis(300)).await,
                }
            }
            SleepArtifact {
                snapshot_tag: None,
                state_path: Some(state_path),
            }
        }
    };
    // quit 的成功判据是「QEMU 退出」（由 [`VmInstance::sleep`] 的 child.wait
    // 核验），不是「读到 quit 回包」。QEMU 收到 quit 即停机拆除：JSON 解析在
    // `}` 处停止，命令尾部 `\n` 常留在内核接收队列——close() 遇接收队列非空
    // 走内核 abort 路径（RST + 丢弃发送队列中的 quit 回包），本端读回包拿到
    // ECONNRESET / connection-closed。这是预期拆除形态不是错误（2026-10-18
    // 远程 KVM pcap+strace 实证：savevm 715ms 完成 → SHUTDOWN 事件 138B →
    // RST 取代 16B 回包 → 误报失败）。状态载体已在手，quit 回包未达不阻断；
    // QEMU 若真未退出由 15s 收割 + kill 兜底收敛。
    if let Err(e) = client.quit(Duration::from_secs(10)).await {
        tracing::debug!("[vm] quit 回包未达（QEMU 停机拆除竞速，预期形态）: {e}");
    }
    Ok(artifact)
}

/// 首帧鉴权握手（独立函数——[`VmInstance::connect_guest_agent`] 与 CI 内
/// 进程内 guest 回归测试同源消费）。
pub async fn auth_handshake(
    addr: SocketAddr,
    token: &str,
    timeout: Duration,
) -> Result<TcpStream, VmError> {
    let connect = tokio::time::timeout(timeout, TcpStream::connect(addr)).await;
    let mut stream = match connect {
        Err(_) => return Err(VmError::Timeout),
        Ok(Err(e)) => return Err(VmError::Io(e)),
        Ok(Ok(s)) => s,
    };
    let auth = format!("{{\"v\":2,\"auth\":\"{token}\"}}\n");
    write_all_budgeted(&mut stream, auth.as_bytes(), timeout).await?;
    let resp = read_line_budgeted(&mut stream, timeout).await?;
    let parsed: ExecutorResponseWire = serde_json::from_str(resp.trim())
        .map_err(|e| VmError::Launch(format!("鉴权响应非协议形态: {e} ({resp})")))?;
    if !parsed.ok {
        // 拒绝后 guest 关连接——这里主动断开（诚实错误带原因）。
        let _ = stream.shutdown().await;
        return Err(VmError::Launch(format!(
            "guest agent 鉴权拒绝: {}",
            parsed.error
        )));
    }
    Ok(stream)
}

/// 工具调用（复用已鉴权流）：v2 请求行 → 读一行响应。每次调用独立
/// call_id；**单行假设**——guest 在两条响应之间不主动发数据（协议纪律），
/// 因此每次临时 BufReader 不丢字节。
pub async fn guest_call(
    stream: &mut TcpStream,
    tool: &str,
    args: &str,
    context: &serde_json::Value,
    timeout: Duration,
) -> Result<ExecutorResponseWire, VmError> {
    let call_id = new_call_id();
    let req = build_v2_request_line(tool, args, context, &call_id).map_err(VmError::Launch)?;
    write_all_budgeted(stream, req.as_bytes(), timeout).await?;
    let resp = read_line_budgeted(stream, timeout).await?;
    serde_json::from_str(resp.trim())
        .map_err(|e| VmError::Launch(format!("工具响应非协议形态: {e} ({resp})")))
}

async fn write_all_budgeted(
    s: &mut TcpStream,
    buf: &[u8],
    timeout: Duration,
) -> Result<(), VmError> {
    tokio::time::timeout(timeout, s.write_all(buf))
        .await
        .map_err(|_| VmError::Timeout)?
        .map_err(VmError::Io)
}

async fn read_line_budgeted(s: &mut TcpStream, timeout: Duration) -> Result<String, VmError> {
    let mut line = String::new();
    let mut reader = BufReader::new(&mut *s);
    let n = tokio::time::timeout(timeout, reader.read_line(&mut line))
        .await
        .map_err(|_| VmError::Timeout)?
        .map_err(VmError::Io)?;
    if n == 0 {
        return Err(VmError::Io(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "guest agent 连接被对端关闭",
        )));
    }
    Ok(line)
}

#[cfg(test)]
mod tests;
