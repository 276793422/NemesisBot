//! P5 第 3 步：VM 持久实体监督器（`vm` feature 闸——收窄/IoT 形态整模块
//! 不进二进制，`nemesis-sandbox/vm` 同闸）。
//!
//! 每 agent 一台**常驻 VM**（独立内核整机隔离），磁盘即持久状态：overlay
//! 跨睡眠/停机/重启保留，guest 内文件与登录态存活。睡眠 = RAM 状态保存
//! （快照线 `-loadvm` / 迁移线 `-incoming`），唤醒 = 带产物重挂同 overlay。
//!
//! 纪律（镜像 P3 `runtime.rs` 监督器，全部对齐）：
//! - **config 现读**：`executor.vm` 每操作从 config.json 重读（单一真相
//!   源，无快照表）；`enabled=false` / 缺席 / 损坏 = `None` = 诚实不可用
//!   （关闭态零激活铁律）。
//! - **满员闸**：运行中槽数 vs `max_instances`，先查再占 + 插入时竞态复核
//!   （spawn 在 map 锁外，输家诚实停掉）。
//! - **watchdog**：15s tick——死槽收敛（不自动重拉：整机重启成本高且多半
//!   是环境问题，下次 ensure_running 冷启）+ 孤儿 sweep + 磁盘预算告警 +
//!   idle 自动睡眠（`idle_sleep_secs=0` 关）。
//! - **孤儿 sweep = 清理不收编**：registry `asleep_at=None` 的记录应有活
//!   进程；pidfile 指向的 pid 经**身份核验**（可执行镜像 = 我们的 QEMU
//!   二进制）确认后 kill，不匹配（疑似 pid 复用）只转冷停不动进程。
//! - **睡眠线诚实面**：WHPX 无可用睡眠线（savevm/migrate 双双早验挂死，
//!   三探针定谳见实施计划附录）——`SleepMode::default_for_accel` 返回
//!   `None` 时诚实报错，不做假睡；stop 冷停（RAM 状态丢、磁盘保留）是
//!   调用方的显式决定。
//!
//! registry = `<workspace>/tools/vm/run/<agent_id>/instance.json`（端口
//! **不落盘**——临时端口每次启动重分；token/资源/睡眠产物落盘）。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nemesis_config::ExecutorVmConfig;
use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::{GuestArch, QemuAccel};
use nemesis_sandbox::vm::lifecycle::{
    SleepMode, VmInstance, VmStartParams, adjudicate_accel_real, auth_handshake, generate_token,
};
use nemesis_sandbox::vm::overlay;
use nemesis_utils::sanitize::sanitize_path_segment;

/// watchdog tick（对齐 P3 槽位看护节奏）。
const WATCHDOG_TICK: Duration = Duration::from_secs(15);
/// sleep 墙钟预算（savevm/migrate 都是墙钟敏感操作；实机探针同量级）。
const SLEEP_BUDGET: Duration = Duration::from_secs(300);
/// stop 优雅宽限（QMP quit → 超时 kill）。
const STOP_GRACE: Duration = Duration::from_secs(10);
/// registry 写入宽限窗：sweep 跳过 mtime 距今小于该值的记录——ensure_running
/// 先落记录再等 QEMU 起来（冷启秒级到分钟级），不跳过会把正在启动的 VM
/// 误判孤儿杀掉。
const SWEEP_STARTUP_GRACE: Duration = Duration::from_secs(120);
/// guest agent 就绪等待预算（冷启/唤醒统一）：QMP running ≠ guest 侧 agent
/// 已监听——cloud-init/systemd 还在走（微型镜像典型 20-60s，上限给慢盘
/// 慢机余量）。就绪前交槽 = 首次 guest 调用 connect refused → Unavailable →
/// 宿主回落混两套文件视图——引导墙钟必须在交槽前吃掉。
const GUEST_READY_BUDGET: Duration = Duration::from_secs(240);
/// guest agent 就绪轮询间隔。
const GUEST_READY_POLL: Duration = Duration::from_secs(2);
/// GiB（GC 预算单位换算）。
pub(crate) const GIB: u64 = 1024 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 错误分型（对齐 P3 SupervisorError 语义——Step 4 接入面靠它决定可否回落）
// ---------------------------------------------------------------------------

/// VM 监督器错误。两臂语义（P3 同款）：调用方据此决定「回落其他执行后端」
/// 还是「如实上报」。
#[derive(Debug)]
pub enum VmSupervisorError {
    /// 档不可用，**调用从未发生**（config 关 / 镜像缺 / 满员 / GC stop 闸 /
    /// 加速缺席 / 睡眠线不支持）——可安全回落其他执行后端。
    Unavailable(String),
    /// 操作已尝试且**可能已有副作用**（VM 进程状态可能已变）——不回落，
    /// 如实上报。
    Failed(String),
}

impl std::fmt::Display for VmSupervisorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VmSupervisorError::Unavailable(m) => write!(f, "VM 档不可用: {m}"),
            VmSupervisorError::Failed(m) => write!(f, "VM 操作失败: {m}"),
        }
    }
}

impl std::error::Error for VmSupervisorError {}

// ---------------------------------------------------------------------------
// registry 记录（端口不落盘——临时端口每次启动重分）
// ---------------------------------------------------------------------------

/// per-agent VM 实例记录（`<workspace>/tools/vm/run/<agent_id>/instance.json`）。
/// 唤醒产物的权威存放处：`snapshot_tag`（快照线）/ `state_path`（迁移线）
/// 都在 = 快照线优先（[`wake_kind_of`] 路由，单测钉死）；都不在 +
/// `asleep_at` 有值 = 冷停形态（重启 = 冷启同 overlay，磁盘状态保留）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct VmInstanceRecord {
    pub agent_id: String,
    /// overlay 绝对路径（持久盘——睡眠/停机/重启全保留）。
    pub overlay: PathBuf,
    /// guest agent 鉴权 token（每 agent 稳定——跨重启重挂复用同一 token）。
    pub token: String,
    /// 睡眠线：`snapshot` / `migrate-file` / `cold`。
    pub sleep_mode: String,
    /// 快照线产物（QEMU 内部快照 tag，随 overlay 存活，可反复 loadvm）。
    pub snapshot_tag: Option<String>,
    /// 迁移线产物（RAM 状态文件；唤醒成功后消费删除，下一轮睡眠重写）。
    pub state_path: Option<PathBuf>,
    /// 睡眠/冷停时刻（unix ms）；**None = 应有活进程**（孤儿 sweep 的判定键）。
    pub asleep_at: Option<u64>,
    pub arch: String,
    pub accel: String,
    pub vcpu: u32,
    pub ram_mb: u32,
    pub disk_gb: u32,
}

/// 唤醒产物路由（纯函数，单测钉死）：快照线优先（tag 恒随 overlay 在），
/// 迁移线次之（状态文件可能已被消费），都没有 = 冷启。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WakeKind {
    Cold,
    FromSnapshot(String),
    FromState(PathBuf),
}

pub(crate) fn wake_kind_of(record: &VmInstanceRecord) -> WakeKind {
    if let Some(tag) = &record.snapshot_tag {
        return WakeKind::FromSnapshot(tag.clone());
    }
    if let Some(p) = &record.state_path {
        return WakeKind::FromState(p.clone());
    }
    WakeKind::Cold
}

/// 冷停语义（纯函数，单测钉死）：清 wake 产物、`asleep_at` 置 Some（此记录
/// 不再对应任何活进程）、sleep_mode=cold。overlay 保留——磁盘即持久状态。
pub(crate) fn mark_cold(record: &mut VmInstanceRecord) {
    record.sleep_mode = "cold".into();
    record.snapshot_tag = None;
    record.state_path = None;
    record.asleep_at = Some(now_ms());
}

/// 记录里的 accel 字符串解析（与 [`QemuAccel::as_str`] 值域同源）。
pub(crate) fn parse_accel(s: &str) -> Option<QemuAccel> {
    match s {
        "kvm" => Some(QemuAccel::Kvm),
        "whpx" => Some(QemuAccel::Whpx),
        _ => None,
    }
}

/// unix ms（时钟回拨 = 0，仅用于 idle 判定留痕，不影响正确性）。
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 目录布局与 registry 读写（<workspace>/tools/vm/...）
// ---------------------------------------------------------------------------

pub(crate) fn vm_run_root(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("run")
}

pub(crate) fn vm_overlays_root(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("overlays")
}

pub(crate) fn vm_images_root(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("images")
}

/// agent 运行目录（agent_id 过 [`sanitize_path_segment`]——registry 路径
/// 段永远安全，与 cluster 会话键同款纪律）。
pub(crate) fn vm_run_dir(workspace: &Path, agent_id: &str) -> PathBuf {
    vm_run_root(workspace).join(sanitize_path_segment(agent_id))
}

pub(crate) fn registry_path(workspace: &Path, agent_id: &str) -> PathBuf {
    vm_run_dir(workspace, agent_id).join("instance.json")
}

pub(crate) fn overlay_path(workspace: &Path, agent_id: &str) -> PathBuf {
    vm_overlays_root(workspace).join(format!("{}.qcow2", sanitize_path_segment(agent_id)))
}

/// WSAPI 面 agent_id 闸（路径穿越防线）：[`sanitize_path_segment`] 只管
/// 长度（≤80 逐字节不变），不管穿越——`..\x` 原样通过后拼出的路径
/// 文件系统解析时逃出 run 根。生产面 agent_id 全是 uuid 恒过；本闸守
/// 的是 `vm.instance.*` 用户输入面。所有接受外部 agent_id 的操作入口
/// （ensure_running/sleep_agent/stop_agent）必须先过此闸。
pub(crate) fn validate_agent_id(agent_id: &str) -> Result<(), String> {
    if agent_id.is_empty()
        || agent_id.contains('/')
        || agent_id.contains('\\')
        || agent_id == "."
        || agent_id == ".."
    {
        return Err(format!(
            "agent_id 非法（含路径分隔符/空段/穿越段）: {agent_id:?}"
        ));
    }
    Ok(())
}

/// QEMU 安装目录（镜像管线落位形态 `<workspace>/tools/vm/runtime/qemu/`）；
/// 不存在 = None（[`resolve_qemu_binary`] 回落 PATH）。
pub(crate) fn qemu_install_dir(workspace: &Path) -> Option<PathBuf> {
    let dir = workspace
        .join("tools")
        .join("vm")
        .join("runtime")
        .join("qemu");
    dir.is_dir().then_some(dir)
}

/// qemu-img 定位（同款两级：安装目录 → PATH 裸名）。
pub(crate) fn resolve_qemu_img(workspace: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "qemu-img.exe"
    } else {
        "qemu-img"
    };
    if let Some(dir) = qemu_install_dir(workspace) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return candidate;
        }
    }
    PathBuf::from(name)
}

pub(crate) fn load_record(workspace: &Path, agent_id: &str) -> Option<VmInstanceRecord> {
    let raw = std::fs::read_to_string(registry_path(workspace, agent_id)).ok()?;
    match serde_json::from_str(&raw) {
        Ok(r) => Some(r),
        Err(e) => {
            tracing::warn!("[vm-runtime] registry 解析失败（按无记录处理）: {e}");
            None
        }
    }
}

pub(crate) fn save_record(workspace: &Path, agent_id: &str, record: &VmInstanceRecord) {
    let dir = vm_run_dir(workspace, agent_id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("[vm-runtime] registry 目录创建失败 {}: {e}", dir.display());
        return;
    }
    let json = match serde_json::to_string_pretty(record) {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!("[vm-runtime] registry 序列化失败: {e}");
            return;
        }
    };
    if let Err(e) = std::fs::write(registry_path(workspace, agent_id), json) {
        tracing::warn!(
            "[vm-runtime] registry 写入失败 {}: {e}",
            registry_path(workspace, agent_id).display()
        );
    }
}

// ---------------------------------------------------------------------------
// pidfile + 身份核验（清理不收编的安全前提）
// ---------------------------------------------------------------------------

/// QEMU `-pidfile` 产物解析（纯十进制 pid + 换行）。
pub(crate) fn parse_pidfile(pidfile: &Path) -> Option<u32> {
    let txt = std::fs::read_to_string(pidfile).ok()?;
    txt.trim().parse::<u32>().ok()
}

/// 孤儿身份核验：pid 存活**且**其可执行镜像与我们解析出的 QEMU 二进制
/// 一致。pid 复用（pidfile 陈旧 + pid 被无关进程占用）= 不匹配 = 不动。
/// 先例：Windows 走 `QueryFullProcessImageNameW`（nemesis-security clamav
/// ownership 同款），Linux 读 `/proc/<pid>/cmdline` 做字节窗匹配
/// （nemesis-cluster exec_workspace 同款）。
pub(crate) fn pid_matches_qemu_identity(pid: u32, qemu_bin: &Path) -> bool {
    #[cfg(windows)]
    {
        process_image_path(pid).is_some_and(|p| paths_equiv(&p, qemu_bin))
    }
    #[cfg(not(windows))]
    {
        let Some(bytes) = process_cmdline_bytes(pid) else {
            return false;
        };
        // 完整路径窗匹配（我们 spawn 用绝对路径）；PATH 回落形态 argv[0]
        // 是裸名，再比一次文件名窗。
        let full = qemu_bin.to_string_lossy().as_bytes().to_vec();
        if !full.is_empty() && bytes.windows(full.len()).any(|w| w == full) {
            return true;
        }
        match qemu_bin.file_name() {
            Some(name) => {
                let bare = name.to_string_lossy().as_bytes().to_vec();
                !bare.is_empty() && bytes.windows(bare.len()).any(|w| w == bare)
            }
            None => false,
        }
    }
}

/// Windows 路径等价：大小写不敏感的字面比较为快路径；失配时 canonicalize
/// 兜底展开再比。为什么需要兜底：gateway 经 `std::env::temp_dir()` 拿到的
/// spawn 路径可能含 8.3 短名（如 `LOCALA~1`），而 `QueryFullProcessImageNameW`
/// 返回的是 kernel 展开后的长名（如 `localadmin`）——字面比较必失配，会把
/// 真孤儿误判成「pid 复用」而拒绝清理。canonicalize 失败（路径已消失等）仍
/// 按失配处理——身份核验宁可保守漏杀并告警，不可误杀。
#[cfg(windows)]
fn paths_equiv(a: &Path, b: &Path) -> bool {
    if a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => {
            ca.to_string_lossy().to_lowercase() == cb.to_string_lossy().to_lowercase()
        }
        _ => false,
    }
}

/// pid → 可执行镜像绝对路径（kernel32 `QueryFullProcessImageNameW`；先例
/// `crates/nemesis-security/src/clamav/ownership.rs`）。
#[cfg(windows)]
fn process_image_path(pid: u32) -> Option<PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(desiredaccess: u32, inherithandle: i32, processid: u32) -> isize;
        fn CloseHandle(handle: isize) -> i32;
        fn QueryFullProcessImageNameW(
            hprocess: isize,
            dwflags: u32,
            lpexename: *mut u16,
            lpdwsize: *mut u32,
        ) -> i32;
    }

    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if ok == 0 || len == 0 {
            return None;
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(
            &buf[..len as usize],
        )))
    }
}

/// pid → /proc cmdline 原始字节（NUL 分隔；先例
/// `crates/nemesis-cluster/src/exec_workspace.rs`）。
#[cfg(not(windows))]
fn process_cmdline_bytes(pid: u32) -> Option<Vec<u8>> {
    std::fs::read(format!("/proc/{pid}/cmdline")).ok()
}

/// 杀掉孤儿 QEMU（taskkill/kill，10s 墙钟护栏——进程操作延迟不挂 watchdog）。
async fn kill_pid(pid: u32) -> bool {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = tokio::process::Command::new("taskkill");
        c.args(["/F", "/PID", &pid.to_string()]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = tokio::process::Command::new("kill");
        c.args(["-9", &pid.to_string()]);
        c
    };
    match tokio::time::timeout(Duration::from_secs(10), cmd.output()).await {
        Ok(Ok(_)) => true,
        other => {
            tracing::warn!("[vm-runtime] kill pid {pid} 失败: {other:?}");
            false
        }
    }
}

// ---------------------------------------------------------------------------
// GC 与闸（纯函数，单测钉死）
// ---------------------------------------------------------------------------

/// GC 闸决策：`max_total_disk_gb=0` = 不设预算（恒 Allow）；超预算按
/// `over_limit` 分档——`stop` = 拒新不删数据（Deny），其余 = 告警（Warn）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GcDecision {
    Allow,
    Warn { used_gb: u64, budget_gb: u32 },
    Deny { used_gb: u64, budget_gb: u32 },
}

pub(crate) fn gc_decision(used_bytes: u64, cfg: &nemesis_config::VmGcConfig) -> GcDecision {
    if cfg.max_total_disk_gb == 0 {
        return GcDecision::Allow;
    }
    let budget = cfg.max_total_disk_gb as u64 * GIB;
    if used_bytes <= budget {
        return GcDecision::Allow;
    }
    let used_gb = used_bytes / GIB;
    if cfg.over_limit == "stop" {
        GcDecision::Deny {
            used_gb,
            budget_gb: cfg.max_total_disk_gb,
        }
    } else {
        GcDecision::Warn {
            used_gb,
            budget_gb: cfg.max_total_disk_gb,
        }
    }
}

/// VM 磁盘产物总量（images + overlays + run 三棵树；含 base——与
/// `VmGcConfig.max_total_disk_gb` 的「全部 VM 磁盘产物」口径一致）。
pub(crate) fn disk_usage_bytes(workspace: &Path) -> u64 {
    fn dir_size(p: &Path) -> u64 {
        let Ok(rd) = std::fs::read_dir(p) else {
            return 0;
        };
        let mut total = 0u64;
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                total += dir_size(&e.path());
            } else if ft.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
        total
    }
    dir_size(&vm_images_root(workspace))
        + dir_size(&vm_overlays_root(workspace))
        + dir_size(&vm_run_root(workspace))
}

/// 满员闸判定（纯函数，单测钉死；竞态复核共用同一判定）。
pub(crate) fn capacity_full(running: usize, max_instances: u32) -> bool {
    running >= max_instances as usize
}

// ---------------------------------------------------------------------------
// 槽位与监督器
// ---------------------------------------------------------------------------

/// 运行中槽（只有活 VM 在 map 里——睡眠/冷停的实例离开 map，watchdog
/// 看不见它们，与 P3「睡着的槽已离开 map」同语义）。
pub(crate) struct VmSlot {
    pub(crate) vm: VmInstance,
    pub(crate) last_used: Instant,
}

/// VM 持久实体监督器：每 agent 至多一台常驻 VM，睡眠/唤醒/停机/孤儿清理/
/// 磁盘预算统一纪律面。gateway post_agent 装配并注入 web 侧
/// [`nemesis_web::handlers::vm::VmBridge`] 槽（晚注入——与 P3 runtime 同款）。
pub struct VmSupervisor {
    workspace: PathBuf,
    config_path: PathBuf,
    slots: Mutex<HashMap<String, Arc<tokio::sync::Mutex<VmSlot>>>>,
}

impl VmSupervisor {
    pub fn new(workspace: PathBuf, config_path: PathBuf) -> Self {
        Self {
            workspace,
            config_path,
            slots: Mutex::new(HashMap::new()),
        }
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// config 所在目录（`config.json` 的父目录；vm_bridge 的 typed 写回
    /// fallback 用——与 gateway 装配同源的 home 语义）。
    pub(crate) fn config_home(&self) -> PathBuf {
        self.config_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."))
    }

    /// typed 配置段现读（`None` = 缺席/损坏——不问 enabled；web 面渲染
    /// 总开关状态需要看到 `enabled=false` 的段本身）。
    pub(crate) fn vm_config(&self) -> Option<ExecutorVmConfig> {
        let raw = std::fs::read_to_string(&self.config_path).ok()?;
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        serde_json::from_value(v.get("executor")?.get("vm")?.clone()).ok()
    }

    /// config 现读（单一真相源 = config.json；`None` = 缺席 / `enabled=false`
    /// / 损坏——关闭态零激活铁律的判定面，不做任何猜测性回退）。
    pub(crate) fn vm_settings(&self) -> Option<ExecutorVmConfig> {
        self.vm_config().filter(|c| c.enabled)
    }

    /// `executor.strict` 裸读（fail-closed 闸：要求加速但本机不可用 → 拒绝
    /// 而非降级，对齐 U11 语义）。
    fn executor_strict(&self) -> bool {
        std::fs::read_to_string(&self.config_path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
            .and_then(|v| {
                v.get("executor")
                    .and_then(|e| e.get("strict"))
                    .and_then(|s| s.as_bool())
            })
            .unwrap_or(false)
    }

    /// 运行中 agent 清单（观测面；clone 出来不持锁返回）。
    pub fn running_agents(&self) -> Vec<String> {
        self.slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .keys()
            .cloned()
            .collect()
    }

    /// 全量实例状态（web `vm.instance.list` 面）：run 根下每个 registry
    /// 记录 + 运行标志（槽内）。目录名即 agent_id（写盘时已净化——读侧
    /// 无分隔符可能）。无记录的空目录跳过。
    pub fn instance_status_all(&self) -> Vec<(VmInstanceRecord, bool)> {
        let running: std::collections::HashSet<String> =
            self.running_agents().into_iter().collect();
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(vm_run_root(&self.workspace)) else {
            return out;
        };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if !entry.path().is_dir() {
                continue;
            }
            let agent_id = entry.file_name().to_string_lossy().to_string();
            if let Some(record) = load_record(&self.workspace, &agent_id) {
                let is_running = running.contains(&agent_id);
                out.push((record, is_running));
            }
        }
        out
    }

    /// 确保 agent 的 VM 在运行：命中槽 → 摸 `last_used` 直接返回；否则
    /// 唤醒（睡眠产物）或冷启（首启带 seed 制备，否则同盘冷启）。
    ///
    /// spawn 在 map 锁外（P3 同款——冷启秒级到分钟级，不持锁阻塞并发）；
    /// 插入时竞态复核满员 + 同 agent 双赢家（输家诚实停掉）。
    pub async fn ensure_running(
        &self,
        agent_id: &str,
    ) -> Result<Arc<tokio::sync::Mutex<VmSlot>>, VmSupervisorError> {
        // 输入闸先行（路径穿越拒绝在一切 IO 之前——Failed=不回落）。
        if let Err(m) = validate_agent_id(agent_id) {
            return Err(VmSupervisorError::Failed(m));
        }
        let settings = self.vm_settings().ok_or_else(|| {
            VmSupervisorError::Unavailable(
                "executor.vm.enabled=false（或 config 缺席/损坏）：VM 档未开启".into(),
            )
        })?;

        // GC stop 闸：超预算且 over_limit=stop → 拒新（不删既有数据）。
        let used = disk_usage_bytes(&self.workspace);
        if let GcDecision::Deny { used_gb, budget_gb } = gc_decision(used, &settings.gc) {
            return Err(VmSupervisorError::Unavailable(format!(
                "VM 磁盘预算闸（over_limit=stop）：已用 {used_gb}GB 超预算 {budget_gb}GB，拒绝新实例（不删既有数据）"
            )));
        }

        // 命中槽：摸活跃时间；死槽顺手收敛（QEMU 崩溃后下一次调用自愈）。
        // 先绑定再 if-let——scrutinee 临时 std 守卫若活到块尾会罩住下面的
        // `existing.lock().await`，future 非 Send（async_trait 面 require）。
        let existing = self
            .slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .get(agent_id)
            .cloned();
        if let Some(existing) = existing {
            let mut slot = existing.lock().await;
            slot.last_used = Instant::now();
            if slot.vm.is_alive() {
                drop(slot);
                return Ok(existing);
            }
            drop(slot);
            self.slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .remove(agent_id);
            if let Some(mut record) = load_record(&self.workspace, agent_id) {
                mark_cold(&mut record);
                save_record(&self.workspace, agent_id, &record);
            }
        }

        // 满员闸（先查再占；插入时复核）。
        if capacity_full(
            self.slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .len(),
            settings.max_instances,
        ) {
            return Err(VmSupervisorError::Unavailable(format!(
                "VM 满员闸：运行中实例已达 max_instances({})",
                settings.max_instances
            )));
        }

        let vm = self.start_or_wake(agent_id, &settings).await?;

        let slot = Arc::new(tokio::sync::Mutex::new(VmSlot {
            vm,
            last_used: Instant::now(),
        }));
        // 插入/复核（锁内只做决策，await 一律在锁块外——std 守卫跨 await
        // 会让 future 非 Send，async_trait 面直接拒绝）。
        let race_loser = {
            let mut map = self
                .slots
                .lock()
                .expect("vm supervisor slots lock poisoned");
            if capacity_full(map.len(), settings.max_instances) {
                Some("VM 满员闸（并发复核）：另一实例先占满名额".to_string())
            } else if map.insert(agent_id.to_string(), slot.clone()).is_some() {
                // 同 agent 并发 ensure 双赢家：保留先到者，自己（新实例）退出。
                Some("VM 满员闸（并发复核）：同 agent 并发 ensure 双赢家".to_string())
            } else {
                None
            }
        };
        if let Some(reason) = race_loser {
            // 竞态输家：满员窗口/名额被并发赢家占掉 → 刚启动的实例诚实退出。
            let mut loser = slot.lock().await;
            if let Err(e) = loser.vm.stop(STOP_GRACE).await {
                tracing::warn!("[vm-runtime] 竞态输家实例停止失败（残留由孤儿 sweep 收敛）: {e}");
            }
            return Err(VmSupervisorError::Unavailable(reason));
        }
        Ok(slot)
    }

    /// 唤醒或冷启（overlay 缺席 = 首启：走镜像管线产物制备 + seed 播种）。
    /// registry 先于 spawn 落盘（asleep_at=None）——启动中途崩溃也留下
    /// sweep 可见痕迹（SWEEP_STARTUP_GRACE 窗内不误杀）。
    async fn start_or_wake(
        &self,
        agent_id: &str,
        settings: &ExecutorVmConfig,
    ) -> Result<VmInstance, VmSupervisorError> {
        let run_dir = vm_run_dir(&self.workspace, agent_id);
        std::fs::create_dir_all(&run_dir)
            .map_err(|e| VmSupervisorError::Failed(format!("run_dir 创建失败: {e}")))?;
        let overlay_p = overlay_path(&self.workspace, agent_id);
        let first_boot = !overlay_p.exists();

        let mut record = match load_record(&self.workspace, agent_id) {
            Some(r) => r,
            None => VmInstanceRecord {
                agent_id: agent_id.to_string(),
                overlay: overlay_p.clone(),
                token: generate_token(),
                sleep_mode: "cold".into(),
                snapshot_tag: None,
                state_path: None,
                asleep_at: None,
                arch: "x86_64".into(),
                accel: String::new(), // 裁决后回填
                vcpu: settings.resources.vcpu,
                ram_mb: settings.resources.ram_mb,
                disk_gb: settings.resources.disk_gb,
            },
        };
        record.asleep_at = None;
        // 资源以 config 现值为准（跨重启改配 → 下次启动生效，运行中不动）。
        record.vcpu = settings.resources.vcpu;
        record.ram_mb = settings.resources.ram_mb;
        record.disk_gb = settings.resources.disk_gb;
        save_record(&self.workspace, agent_id, &record);

        let arch = GuestArch::parse(&record.arch)
            .map_err(|e| VmSupervisorError::Failed(format!("registry arch 非法: {e}")))?;
        let explicit =
            QemuAccel::parse_hypervisor(&settings.hypervisor).map_err(VmSupervisorError::Failed)?;
        let accel = adjudicate_accel_real(self.executor_strict(), explicit, arch).map_err(|e| {
            VmSupervisorError::Unavailable(format!("加速裁决拒绝（诚实缺席）: {e}"))
        })?;
        record.accel = accel.as_str().to_string();
        save_record(&self.workspace, agent_id, &record);

        let qemu_bin = resolve_qemu_binary(arch, qemu_install_dir(&self.workspace).as_deref());

        // wake-or-cold 路由（[`wake_kind_of`] 纯函数）+ 首启制备。
        let (loadvm_tag, incoming_file, seed_iso) = if first_boot {
            let base = vm_images_root(&self.workspace).join("base.qcow2");
            let seed = vm_images_root(&self.workspace).join("seed.iso");
            if !base.exists() || !seed.exists() {
                return Err(VmSupervisorError::Unavailable(format!(
                    "镜像资产缺失（base={} seed={}）：先跑镜像管线 scripts/build-vm-image.sh",
                    base.display(),
                    seed.display()
                )));
            }
            overlay::create_overlay(
                &resolve_qemu_img(&self.workspace),
                &overlay_p,
                &base,
                record.disk_gb,
            )
            .await
            .map_err(|e| VmSupervisorError::Failed(format!("overlay 创建失败: {e}")))?;
            (None, None, Some(seed))
        } else {
            match wake_kind_of(&record) {
                WakeKind::FromSnapshot(tag) => (Some(tag), None, None),
                WakeKind::FromState(p) if p.exists() => (None, Some(p), None),
                WakeKind::FromState(p) => {
                    tracing::warn!(
                        "[vm-runtime] agent {agent_id} 迁移状态文件缺席（{}）：降级冷启同盘 overlay",
                        p.display()
                    );
                    (None, None, None)
                }
                WakeKind::Cold => (None, None, None), // 已制备盘：不重播 cloud-init
            }
        };

        let params = VmStartParams {
            id: format!("agent-{agent_id}"),
            run_dir,
            qemu_bin,
            accel,
            arch,
            vcpu: record.vcpu,
            ram_mb: record.ram_mb,
            overlay_path: overlay_p,
            seed_iso,
            token: record.token.clone(),
            // Step 3 最小面：宿主共享目录接线归 Step 4 接入面（与工具面
            // 一起定契约），这里诚实 None。
            shared_host_path: None,
            loadvm_tag,
            incoming_file: incoming_file.clone(),
        };
        let mut vm = match VmInstance::start(params).await {
            Ok(vm) => vm,
            Err(e) => {
                mark_cold(&mut record);
                save_record(&self.workspace, agent_id, &record);
                return Err(VmSupervisorError::Failed(format!("VM 启动失败: {e}")));
            }
        };

        // guest agent 就绪等待：QMP running ≠ guest 侧 agent 已监听——
        // cloud-init/systemd 还在走（微型镜像典型 20-60s，上限给慢盘慢机
        // 余量）。就绪前交槽 = 首次 guest 调用 connect refused → Unavailable
        // → 宿主回落混两套文件视图。在交槽前把引导墙钟吃掉：轮询鉴权握手
        //（流即弃——execute_persistent 每次调用独立握手）；超时 = 启动失败
        //（停机 + 回冷），不带着没就绪的 VM 出这个函数。
        let deadline = tokio::time::Instant::now() + GUEST_READY_BUDGET;
        loop {
            match auth_handshake(vm.agent_addr, &record.token, Duration::from_secs(5)).await {
                Ok(_) => break,
                Err(e) => {
                    if tokio::time::Instant::now() >= deadline {
                        let _ = vm.stop(STOP_GRACE).await;
                        mark_cold(&mut record);
                        save_record(&self.workspace, agent_id, &record);
                        return Err(VmSupervisorError::Failed(format!(
                            "guest agent 就绪超时（{GUEST_READY_BUDGET:?}）: {e}"
                        )));
                    }
                    tokio::time::sleep(GUEST_READY_POLL).await;
                }
            }
        }

        // 迁移线唤醒成功：状态文件已消费——删除（下一轮睡眠重写）；
        // 快照线 tag 保留（随 overlay 存活，可反复 loadvm）。
        if incoming_file.is_some() {
            if let Some(p) = &record.state_path
                && let Err(e) = std::fs::remove_file(p)
            {
                tracing::warn!("[vm-runtime] 迁移状态文件删除失败 {}: {e}", p.display());
            }
            record.state_path = None;
        }
        record.asleep_at = None;
        save_record(&self.workspace, agent_id, &record);
        tracing::info!(
            "[vm-runtime] agent {agent_id} VM 已启动（accel={} vcpu={} ram={}MB）",
            record.accel,
            record.vcpu,
            record.ram_mb
        );
        Ok(vm)
    }

    /// 显式睡眠：选默认睡眠线（WHPX = None → 诚实报错不假睡），保存 RAM
    /// 状态后 QEMU 退出，实例出 map（唤醒走下一次 ensure_running）。
    /// 失败路径 VM 保持运行（槽留在 map——睡眠失败不是停机）。
    pub async fn sleep_agent(&self, agent_id: &str) -> Result<(), VmSupervisorError> {
        if let Err(m) = validate_agent_id(agent_id) {
            return Err(VmSupervisorError::Failed(m));
        }
        // config 开着才谈睡眠（关档 = 一切 VM 操作不可用）。
        if self.vm_settings().is_none() {
            return Err(VmSupervisorError::Unavailable(
                "executor.vm.enabled=false（或 config 缺席/损坏）：VM 档未开启".into(),
            ));
        }
        let slot = self
            .slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .get(agent_id)
            .cloned()
            .ok_or_else(|| VmSupervisorError::Failed(format!("agent {agent_id} 无运行中 VM 槽")))?;
        let mut s = slot.lock().await;

        let mut record = load_record(&self.workspace, agent_id).ok_or_else(|| {
            VmSupervisorError::Failed("registry 缺记录（槽与盘上状态失配）".to_string())
        })?;
        let accel = parse_accel(&record.accel).ok_or_else(|| {
            VmSupervisorError::Failed(format!("registry accel 非法: {:?}", record.accel))
        })?;
        let mode = SleepMode::default_for_accel(accel).ok_or_else(|| {
            VmSupervisorError::Unavailable(format!(
                "加速 {} 无可用睡眠线（savevm/migrate 双双早验挂死，三探针定谳）；退路 = stop 冷停（RAM 状态丢、磁盘保留）",
                record.accel
            ))
        })?;

        if !s.vm.is_alive() {
            // 已死：收敛为冷停 + 出 map（watchdog 死槽收敛同语义）。
            drop(s);
            self.slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .remove(agent_id);
            mark_cold(&mut record);
            save_record(&self.workspace, agent_id, &record);
            return Err(VmSupervisorError::Failed(
                "QEMU 进程已死：收敛为冷停记录（未睡眠）".into(),
            ));
        }

        let artifact = s.vm.sleep(mode, SLEEP_BUDGET).await.map_err(|e| {
            VmSupervisorError::Failed(format!("睡眠失败（VM 保持运行，槽不撤）: {e}"))
        })?;

        record.sleep_mode = if artifact.snapshot_tag.is_some() {
            "snapshot"
        } else {
            "migrate-file"
        }
        .into();
        record.snapshot_tag = artifact.snapshot_tag;
        record.state_path = artifact.state_path;
        record.asleep_at = Some(now_ms());
        save_record(&self.workspace, agent_id, &record);
        drop(s);
        self.slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .remove(agent_id);
        tracing::info!(
            "[vm-runtime] agent {agent_id} 已睡眠（line={})",
            record.sleep_mode
        );
        Ok(())
    }

    /// 显式停机（冷停）：QEMU 退出（QMP quit → kill 兜底），记录转冷停，
    /// overlay 保留——下次 ensure_running 冷启同盘（磁盘状态在，RAM 丢）。
    pub async fn stop_agent(&self, agent_id: &str) -> Result<(), VmSupervisorError> {
        if let Err(m) = validate_agent_id(agent_id) {
            return Err(VmSupervisorError::Failed(m));
        }
        let slot = self
            .slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .get(agent_id)
            .cloned()
            .ok_or_else(|| VmSupervisorError::Failed(format!("agent {agent_id} 无运行中 VM 槽")))?;
        let mut s = slot.lock().await;
        let res = s.vm.stop(STOP_GRACE).await;
        drop(s);
        self.slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .remove(agent_id);
        if let Some(mut record) = load_record(&self.workspace, agent_id) {
            mark_cold(&mut record);
            save_record(&self.workspace, agent_id, &record);
        }
        res.map_err(|e| VmSupervisorError::Failed(format!("stop 失败（记录已转冷停）: {e}")))
    }

    /// 孤儿 sweep（gateway 启动 + watchdog 周期共用）：registry
    /// `asleep_at=None` 的记录应有活进程，纪律 = **清理不收编**——pidfile
    /// 指向的 pid 身份核验通过 → kill；pid 已死 / 无 pidfile / 身份不匹配
    /// （疑似 pid 复用）→ 只转冷停记录、不动任何进程。跳过本 supervisor
    /// 槽内实例与 `SWEEP_STARTUP_GRACE` 窗内新写记录（启动保护窗）。
    /// 返回清理的孤儿进程数。
    pub async fn sweep_orphans(&self) -> usize {
        let mut swept = 0usize;
        let Ok(rd) = std::fs::read_dir(vm_run_root(&self.workspace)) else {
            return 0;
        };
        for entry in rd.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if self
                .slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .contains_key(&dir_name)
            {
                continue; // 自己槽内的活实例不碰
            }
            let Some(mut record) = load_record(&self.workspace, &dir_name) else {
                continue;
            };
            if record.asleep_at.is_some() {
                continue; // 已睡眠/冷停：无活进程语义
            }
            // 启动保护窗：registry 刚写过 = 可能正在冷启（先落记录再等
            // QEMU 起来），本 tick 不碰。
            let reg = registry_path(&self.workspace, &dir_name);
            if let Ok(age) = std::fs::metadata(&reg)
                .and_then(|md| md.modified())
                .map(|t| t.elapsed().unwrap_or_default())
                && age < SWEEP_STARTUP_GRACE
            {
                continue;
            }
            // 身份核验基准 = 按当前解析规则重解的 QEMU 二进制（同 workspace
            // 同规则；安装目录迁移过的场景核验会拒绝——诚实告警不误杀）。
            let qemu_bin = GuestArch::parse(&record.arch)
                .ok()
                .map(|a| resolve_qemu_binary(a, qemu_install_dir(&self.workspace).as_deref()));
            let pidfile = vm_run_dir(&self.workspace, &dir_name).join("qemu.pid");
            match (parse_pidfile(&pidfile), qemu_bin.as_ref()) {
                (Some(pid), Some(qemu)) if pid_matches_qemu_identity(pid, qemu) => {
                    tracing::warn!(
                        "[vm-runtime] 孤儿 QEMU（agent {dir_name} pid {pid}）身份确认：清理不收编"
                    );
                    kill_pid(pid).await;
                    swept += 1;
                }
                (Some(pid), _) => {
                    tracing::warn!(
                        "[vm-runtime] agent {dir_name} pidfile 指向 pid {pid} 但身份不匹配（疑似 pid 复用）：不动进程，仅转冷停"
                    );
                }
                (None, _) => {
                    tracing::warn!(
                        "[vm-runtime] agent {dir_name} 记录「应在运行」但无可用 pidfile：仅转冷停"
                    );
                }
            }
            mark_cold(&mut record);
            save_record(&self.workspace, &dir_name, &record);
        }
        swept
    }

    /// watchdog（P3 同款 Arc downgrade 自弱引用循环；supervisor 全体 drop
    /// → 看护自灭）。单 tick：死槽收敛 → 孤儿 sweep → 磁盘预算告警 →
    /// idle 自动睡眠（`idle_sleep_secs=0` 关）。
    pub fn start_watchdog(self: &Arc<Self>) -> tokio::task::JoinHandle<()> {
        let me = Arc::downgrade(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(WATCHDOG_TICK);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(this) = me.upgrade() else { break };
                this.tick().await;
            }
        })
    }

    async fn tick(self: &Arc<Self>) {
        // 1. 死槽收敛：QEMU 意外退出 → 记录转冷停 + 出 map。**不自动重拉**
        //    （整机重启成本高且多半是环境问题；下次 ensure_running 冷启）。
        let ids: Vec<String> = self
            .slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let Some(slot) = self
                .slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .get(&id)
                .cloned()
            else {
                continue;
            };
            // try_lock：忙碌（工具调用/睡眠进行中）= 跳过本 tick（P3 同款）。
            let Ok(mut s) = slot.try_lock() else {
                continue;
            };
            if !s.vm.is_alive() {
                drop(s);
                self.slots
                    .lock()
                    .expect("vm supervisor slots lock poisoned")
                    .remove(&id);
                if let Some(mut record) = load_record(&self.workspace, &id) {
                    mark_cold(&mut record);
                    save_record(&self.workspace, &id, &record);
                }
                tracing::warn!(
                    "[vm-runtime] agent {id} 的 QEMU 进程意外退出：收敛为冷停记录（不自动重拉）"
                );
            }
        }

        // 2. 孤儿 sweep。
        self.sweep_orphans().await;

        // 3 + 4. 预算告警 / idle 自动睡眠（都在 config 现读的臂里）。
        let Some(settings) = self.vm_settings() else {
            return;
        };
        let used = disk_usage_bytes(&self.workspace);
        if let GcDecision::Warn { used_gb, budget_gb } = gc_decision(used, &settings.gc) {
            tracing::warn!(
                "[vm-runtime] VM 磁盘用量 {used_gb}GB 超预算 {budget_gb}GB（over_limit=warn 只告警；stop 档会拒新实例）"
            );
        }
        if settings.idle_sleep_secs > 0 {
            let deadline = Duration::from_secs(settings.idle_sleep_secs);
            let mut idle_ids = Vec::new();
            for id in self.running_agents() {
                let Some(slot) = self
                    .slots
                    .lock()
                    .expect("vm supervisor slots lock poisoned")
                    .get(&id)
                    .cloned()
                else {
                    continue;
                };
                let Ok(s) = slot.try_lock() else {
                    continue; // 忙碌 = 不算 idle
                };
                if s.last_used.elapsed() >= deadline {
                    idle_ids.push(id);
                }
            }
            for id in idle_ids {
                tracing::info!(
                    "[vm-runtime] agent {id} idle 超 {}s，自动睡眠",
                    settings.idle_sleep_secs
                );
                if let Err(e) = self.sleep_agent(&id).await {
                    tracing::warn!("[vm-runtime] agent {id} 自动睡眠失败（保持运行）: {e}");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// P5 第 4 步：执行接入面（PersistentExecutor + VmBackend 桥）
// ---------------------------------------------------------------------------

/// guest agent 工具子集（`nemesis-guest` src/tools.rs dispatch 表同源镜像
/// ——guest 是 VM 内 Linux 目标的独立编译单元，常量不跨 crate 共享；两侧
/// 漂移由 guest tests + 本侧子集闸测试双向钉住）。**grep/git 不在子集**：
/// VM 与宿主是两套文件视图，guest 内 grep/git 操作 VM 盘、宿主回落操作
/// 宿主盘——混用 = 正确性事故，宁可 ToolFailed 诚实报错不回落（与
/// ToolFailed「绝不回落」语义同源）。
pub(crate) const VM_GUEST_TOOLS: [&str; 8] = [
    "exec",
    "read_file",
    "write_file",
    "append_file",
    "list_dir",
    "create_dir",
    "delete_file",
    "delete_dir",
];

/// 单次 guest 调用墙钟预算（连接 + 鉴权 + 工具执行；对齐进程档
/// `FALLBACK_CALL_TIMEOUT_SECS` 的 600s 量级——两者都是「兜底上限不是
/// 常态」：工具自身长短不一，超时 = 诚实失败）。
const GUEST_CALL_TIMEOUT: Duration = Duration::from_secs(600);

/// VmSupervisorError → PersistentCallError 纯映射（T7 同款依赖倒置桥）。
/// 语义对齐是设计好的：Unavailable = 调用从未发生（config 关/镜像缺/满员/
/// GC 闸/加速缺席）→ 上层可安全回落 per-call；Failed = 已可能副作用 →
/// ToolFailed 绝不回落。
fn vm_err_to_persistent(
    e: VmSupervisorError,
) -> nemesis_agent::remote_executor_tool::PersistentCallError {
    use nemesis_agent::remote_executor_tool::PersistentCallError;
    match e {
        VmSupervisorError::Unavailable(m) => PersistentCallError::Unavailable(m),
        VmSupervisorError::Failed(m) => PersistentCallError::ToolFailed(m),
    }
}

#[async_trait::async_trait]
impl nemesis_agent::remote_executor_tool::PersistentExecutor for VmSupervisor {
    async fn execute_persistent(
        &self,
        agent_id: &str,
        // exec_root 是进程档语义（宿主隔离根）。VM 档工作区在 guest 内
        // `/workspace`（Step 3 `shared_host_path: None` 同源——共享宿主
        // 子树接线诚实后置），本实现不消费该路径；签名与进程档同形保证
        // 「同一 RuntimeBackend 接口」。
        _exec_root: &Path,
        tool: &str,
        args: &str,
        context: &nemesis_agent::context::RequestContext,
    ) -> Result<String, nemesis_agent::remote_executor_tool::PersistentCallError> {
        use nemesis_agent::remote_executor_tool::PersistentCallError;

        // 子集闸先行（先于一切 config/IO——超子集调用无论开关态都诚实
        // 失败，绝不回落宿主混用两套文件视图）。
        if !VM_GUEST_TOOLS.contains(&tool) {
            return Err(PersistentCallError::ToolFailed(format!(
                "工具 {tool:?} 不在 VM guest 子集内（允许: {}）；VM 与宿主是两套文件视图，不做宿主回落混用",
                VM_GUEST_TOOLS.join(", ")
            )));
        }

        // ensure_running：config 现读（关=Unavailable）→ GC 闸 → 命中槽/
        // 唤醒/冷启（含满员闸）。失败分型直接透传（Unavailable=可回落，
        // Failed=不回落）。
        let slot = self
            .ensure_running(agent_id)
            .await
            .map_err(vm_err_to_persistent)?;

        // 每调用独立连接（VmInstance 不持常驻流——睡眠/唤醒生命周期以
        // 进程边界为锚，跨睡眠的常驻流是伪状态）。连接 + 鉴权失败 =
        // 请求从未发出 = Unavailable（可回落）；请求发出后的失败 =
        // ToolFailed（副作用未知，绝不回落——重复执行危险）。
        let mut stream = {
            let s = slot.lock().await;
            nemesis_sandbox::vm::lifecycle::auth_handshake(
                s.vm.agent_addr,
                &s.vm.token,
                GUEST_CALL_TIMEOUT,
            )
            .await
            .map_err(|e| PersistentCallError::Unavailable(format!("VM guest 通道握手失败: {e}")))?
        };
        let ctx = serde_json::to_value(context)
            .map_err(|e| PersistentCallError::ToolFailed(format!("序列化请求上下文失败: {e}")))?;
        let resp = nemesis_sandbox::vm::lifecycle::guest_call(
            &mut stream,
            tool,
            args,
            &ctx,
            GUEST_CALL_TIMEOUT,
        )
        .await
        .map_err(|e| {
            PersistentCallError::ToolFailed(format!("VM guest 调用失败（副作用未知，不回落）: {e}"))
        })?;
        if resp.ok {
            Ok(resp.result)
        } else {
            Err(PersistentCallError::ToolFailed(format!(
                "VM guest 工具执行失败: {}",
                resp.error
            )))
        }
    }
}

#[async_trait::async_trait]
impl super::VmBackend for VmSupervisor {
    async fn sleep_agent(&self, agent_id: &str) -> Result<bool, String> {
        // 无运行中实例 = Ok(false)（与进程档「无槽可睡」同义，不算错误）；
        // 其余失败诚实上抛。先查后睡有竞态窗口（查完被 watchdog 收敛），
        // sleep_agent 自身再报「无槽」时如实转 Err——竞态窗极窄，诚实面
        // 优先于把两类失败抹平。
        if !self.running_agents().iter().any(|a| a == agent_id) {
            return Ok(false);
        }
        self.sleep_agent(agent_id)
            .await
            .map(|_| true)
            .map_err(|e| e.to_string())
    }

    async fn wake_agent(&self, agent_id: &str, _exec_root: &Path) -> Result<(), String> {
        // 唤醒落 ensure_running（FromSnapshot/FromState/Cold 路由 + 满员
        // 闸 + 竞态复核都在里面）；宿主 exec_root 不进 guest（同 execute_
        // persistent 注记）。
        self.ensure_running(agent_id)
            .await
            .map(|_| ())
            .map_err(|e| e.to_string())
    }

    fn running_agents(&self) -> Vec<String> {
        self.running_agents()
    }
}

#[cfg(test)]
mod tests;
