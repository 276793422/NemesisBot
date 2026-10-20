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

use std::collections::{HashMap, HashSet};
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
/// 误判孤儿杀掉。在途启动的精确防线是 supervisor 的 starting 集（sweep
/// 直接跳过在途 agent，不受窗长限制）；本窗兜底「非本 supervisor 在途」
/// 的异常形态（如崩溃残留的半截记录）。
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

/// registry 写盘序号（tmp 文件名唯一性：并发 save 同 agent 不互踩临时文件）。
static REGISTRY_WRITE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// registry 写盘（**原子替换**：临时文件 + 同目录 rename——崩溃/断电中途
/// 只留孤儿 tmp，instance.json 要么旧完整要么新完整，绝不半截 JSON）。
/// registry 是「孤儿 sweep 可见性 + spawn 前落盘锚」的真相源，半截/缺失
/// 记录的后果链：load=None → sweep 看不见孤儿 + ensure 当新记录重开同一
/// overlay = qcow2 并发写损坏。失败如实返回 Err（spawn 前锚位 `?` 消费）。
pub(crate) fn save_record(
    workspace: &Path,
    agent_id: &str,
    record: &VmInstanceRecord,
) -> Result<(), String> {
    let dir = vm_run_dir(workspace, agent_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("registry 目录创建失败 {}: {e}", dir.display()))?;
    let json =
        serde_json::to_string_pretty(record).map_err(|e| format!("registry 序列化失败: {e}"))?;
    let target = registry_path(workspace, agent_id);
    let tmp = dir.join(format!(
        "instance.json.{}.{}.tmp",
        std::process::id(),
        REGISTRY_WRITE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::write(&tmp, json).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("registry 临时文件写入失败 {}: {e}", tmp.display())
    })?;
    // 同目录 rename（NTFS/ext4 都原子；跨盘 rename 才有拷贝窗——tmp 与
    // target 同目录，不存在跨盘）。
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("registry 原子替换失败 {}: {e}", target.display())
    })?;
    Ok(())
}

/// save_record 的 best-effort 包装（非锚定位）：状态转移已发生、落盘失败
/// 只降级可观测性/下次冷启形态，不该把操作面错误顶上去。
pub(crate) fn save_record_warn(
    workspace: &Path,
    agent_id: &str,
    record: &VmInstanceRecord,
    ctx: &str,
) {
    if let Err(e) = save_record(workspace, agent_id, record) {
        tracing::warn!("[vm-runtime] registry 写入失败（{ctx}）: {e}");
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

/// spawn 前防线：该 agent 的 pidfile 指向一个**身份核验通过的活 QEMU** =
/// registry 丢失/损坏后 sweep 看不见的真实孤儿（或另一 gateway 进程共用
/// 同 workspace）——此时再 spawn 就是同一 overlay 双开（qcow2 并发写 =
/// 盘损坏），诚实拒绝而不是撞上去。身份不匹配（pid 复用）不拦——与 sweep
/// 同款保守 doctrine（宁可漏拦不误拦；PATH 回落裸名解析的弱核验窗与
/// sweep 一致）。正常流 pidfile 残留（QEMU 已退出）在这里就是死 pid →
/// 直接放行，零额外成本。
pub(crate) fn ensure_no_live_qemu(pidfile: &Path, qemu_bin: &Path) -> Result<(), String> {
    if let Some(pid) = parse_pidfile(pidfile)
        && pid_matches_qemu_identity(pid, qemu_bin)
    {
        return Err(format!(
            "pidfile 命中身份核验通过的活 QEMU（pid {pid}）——同 overlay 禁止双开：等孤儿 sweep 清理（watchdog ≤{}s 一轮）或显式处理后重试（pidfile: {}）",
            WATCHDOG_TICK.as_secs(),
            pidfile.display()
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 镜像换装互斥（promote vs spawn——桥侧 promote_staging 与本模块 start 共享）
// ---------------------------------------------------------------------------

/// 镜像换装闸：promote（qemu-img rebase / 删旧 base / 新 base 转正）与
/// spawn（overlay 创建 + QEMU 打开 overlay）互斥。两相协议：
/// start 侧在 overlay 首触前取闸、持有到本函数终态（成功=槽即将入 map，
/// 失败=子进程已收尸 overlay 已释放）；promote 侧持闸做 pidfile 全量扫描
/// 后才动镜像——「扫描时无活 QEMU」在闸保护下直到 promote 结束都不会有
/// 新 spawn（spawn 早期 QEMU 尚未写 pidfile，纯扫描有进程创建窗，闸把
/// 窗口钉死）。tokio Mutex（持锁跨 await；任务取消即释放，不卡停机）。
pub(crate) static IMAGE_SWAP_GATE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// spawn 在途标记文件名（run/<agent>/ 下）。
const STARTING_MARKER: &str = "starting";

/// 在途标记的陈旧界：超过视为崩溃残留（进程死在 spawn 相位没来得及
/// 自清理），不再拦换装。正常 spawn 相位（overlay 制备 + QMP 就绪等待）
/// 远短于此；取宽裕值防慢机误伤。
const STARTING_MARKER_STALE: Duration = Duration::from_secs(600);

/// spawn 在途标记守卫：构造即落盘、Drop 即移除（`?` 早退/panic 展开全
/// 覆盖）。跨进程可见性哨兵：QEMU 自身要等 spawn 后才写 pidfile，进程
/// 创建窗内另一进程的镜像换装扫描看不见 pidfile——标记把这段窗补上
/// （与 pidfile 同一「文件即跨进程真相」教义；终态后 pidfile 接管活体
/// 职责，标记可删）。
struct StartingMarkerGuard {
    path: PathBuf,
}

impl StartingMarkerGuard {
    fn arm(run_dir: &Path) -> std::io::Result<Self> {
        let path = run_dir.join(STARTING_MARKER);
        std::fs::write(&path, std::process::id().to_string())?;
        Ok(Self { path })
    }
}

impl Drop for StartingMarkerGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// 在途标记新鲜判定：缺席 = 不在途；mtime 在陈旧界内（或读不出/时钟
/// 倒挂）= 新鲜——拿不准时往「拦换装」的保守方向落。
fn starting_marker_fresh(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    match meta.modified() {
        Ok(mtime) => mtime
            .elapsed()
            .map(|age| age < STARTING_MARKER_STALE)
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// 活 QEMU 扫描核（[`any_live_qemu`] 的可注入形态，单测钉四态表）：
/// run 根下每个 agent 目录的 pidfile 身份核验 + spawn 在途标记（新鲜），
/// 任一命中 = 活。
pub(crate) fn any_live_qemu_scan(workspace: &Path, qemu_bin_of: &dyn Fn(&str) -> PathBuf) -> bool {
    let Ok(rd) = std::fs::read_dir(vm_run_root(workspace)) else {
        return false;
    };
    for entry in rd.flatten() {
        if !entry.path().is_dir() {
            continue;
        }
        // spawn 在途（标记新鲜）= 活：QEMU 尚未写 pidfile 的进程创建窗。
        if starting_marker_fresh(&entry.path().join(STARTING_MARKER)) {
            return true;
        }
        let pidfile = entry.path().join("qemu.pid");
        let agent_id = entry.file_name().to_string_lossy().to_string();
        if let Some(pid) = parse_pidfile(&pidfile)
            && pid_matches_qemu_identity(pid, &qemu_bin_of(&agent_id))
        {
            return true;
        }
    }
    false
}

/// 全 workspace 活 QEMU 扫描（pidfile + 身份核验 + spawn 在途标记，与
/// [`ensure_no_live_qemu`]/sweep 同一判定族）。镜像换装前置闸：任何实例
/// 在跑或在启动中 = 拒绝换装——rebase 写 overlay 与运行中 QEMU 写
/// overlay 并发 = qcow2 损坏，删旧 base 撞运行中实例的打开句柄 = Windows
/// 共享违例。跨进程哨兵：CLI 前台臂 / 另一 gateway 进程的活实例与在途
/// spawn 同样可见（内存槽是进程本地的，pidfile/标记才是跨进程真相）。
pub(crate) fn any_live_qemu(workspace: &Path) -> bool {
    // 身份核验基准与 spawn/sweep 同源：按 registry arch 重解 qemu 二进制。
    // registry 缺失/损坏退 x86_64 解析（spawn 前锚保证本代码产的活 QEMU
    // 必有 registry；盲区只剩「外部手工 spawn 且非 x86_64」——aarch64 是
    // 接口预留档当前不产出，实际不可达）。
    any_live_qemu_scan(workspace, &|agent_id| {
        let arch = load_record(workspace, agent_id)
            .and_then(|r| GuestArch::parse(&r.arch).ok())
            .unwrap_or(GuestArch::X86_64);
        resolve_qemu_binary(arch, qemu_install_dir(workspace).as_deref())
    })
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

/// ensure_running 路由判定（纯函数，单测钉死）。顺序语义：**活槽短路一切
/// 「拒新」闸**——reuse 不消耗新磁盘/名额，GC stop/满员只该拦新启动；把
/// 活实例拒成 Unavailable = 逼上层宿主回落操作另一套文件视图（正确性
/// 事故，不是可用性折损）。无槽时：GC stop 闸先于满员闸（磁盘语义优先）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum EnsureRoute {
    /// 命中活槽：直接复用（GC/满员闸都不该碰它）。
    Reuse,
    /// GC stop 拒新（不删既有数据）。
    GcDeny { used_gb: u64, budget_gb: u32 },
    /// 运行中实例已满额，拒新启动。
    CapFull { max_instances: u32 },
    /// 放行启动。
    Proceed,
}

pub(crate) fn ensure_route(
    has_alive_slot: bool,
    used_bytes: u64,
    gc: &nemesis_config::VmGcConfig,
    running: usize,
    max_instances: u32,
) -> EnsureRoute {
    if has_alive_slot {
        return EnsureRoute::Reuse;
    }
    match gc_decision(used_bytes, gc) {
        GcDecision::Deny { used_gb, budget_gb } => EnsureRoute::GcDeny { used_gb, budget_gb },
        _ if capacity_full(running, max_instances) => EnsureRoute::CapFull { max_instances },
        _ => EnsureRoute::Proceed,
    }
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
    /// 自引用弱句柄（试玩后台启动任务需要 owned `Arc`——装配侧
    /// `set_self_arc` 注入；Weak 不阻止释放。缺席 = try_start 同步回落）。
    self_weak: Mutex<Option<std::sync::Weak<VmSupervisor>>>,
    /// 试玩后台启动状态（`try_launch_*` 族单一真相源；短临界区 std 锁）。
    try_launch: Mutex<TryLaunchState>,
    /// per-agent 启动在途集（ensure_running 同 agent 互斥闸）：spawn 在
    /// map 锁外、全程分钟级——两个并发 ensure_running 都会错过「槽未入
    /// map」窗口而双双 spawn 抢同一 overlay（qcow2 并发写 = 盘损坏风险；
    /// try 面的 starting 旗标只盖 dashboard 一条通路，防不住 instance_wake
    /// ×2 / instance_wake × try_start 等交叉）。在途 = insert 失败 = 诚实
    /// 拒绝。sweep_orphans 同源消费：在途 agent 不是孤儿。
    starting: Mutex<HashSet<String>>,
}

/// 试玩启动在途状态（真机教训 2026-10-19：冷启动同步等 30-60s 撞前端
/// 30s WSAPI 超时——「timeout: vm.try.start」弹窗而 QEMU 实际已成功启动；
/// 改为发起即返回 + 前端轮询 try.status 转绿，本结构记录在途/停止请求/
/// 失败原因）。
#[derive(Default)]
struct TryLaunchState {
    starting: bool,
    /// 启动期间的停止请求登记（try.stop 落点；就绪后由启动任务补停）。
    stop_requested: bool,
    /// 启动失败原因（轮询侧诚实展示；成功清空）。
    last_error: Option<String>,
}

/// 启动在途闸守卫：持集期内 Drop 自动摘除——覆盖 start_or_wake 起所有
/// `?` 早退路径（启动失败/就绪超时/竞态输家），不靠手工逐出口清理。
struct StartGateGuard<'a> {
    set: &'a Mutex<HashSet<String>>,
    id: String,
}

impl Drop for StartGateGuard<'_> {
    fn drop(&mut self) {
        self.set
            .lock()
            .expect("vm supervisor starting lock poisoned")
            .remove(&self.id);
    }
}

impl VmSupervisor {
    pub fn new(workspace: PathBuf, config_path: PathBuf) -> Self {
        Self {
            workspace,
            config_path,
            slots: Mutex::new(HashMap::new()),
            self_weak: Mutex::new(None),
            try_launch: Mutex::new(TryLaunchState::default()),
            starting: Mutex::new(HashSet::new()),
        }
    }

    /// 装配侧注入自引用（gateway install 处调用一次；Weak——释放不受阻）。
    pub fn set_self_arc(self: &std::sync::Arc<Self>) {
        *self
            .self_weak
            .lock()
            .expect("vm supervisor self_weak lock poisoned") =
            Some(std::sync::Arc::downgrade(self));
    }

    /// 试玩启动状态快照（try_status 投影 + try_exec 闸消费）。
    pub(crate) fn try_launch_snapshot(&self) -> (bool, Option<String>) {
        let st = self.try_launch.lock().expect("vm try launch lock poisoned");
        (st.starting, st.last_error.clone())
    }

    /// 认领一次后台启动：置位 starting 并返回自引用句柄。已在途 = None
    /// （幂等——双击/轮询期重复点击不发第二枪）；自引用句柄缺席（单测等
    /// 非常规装配）= 回滚置位后返回 None（调用方同步回落旧语义）。
    pub(crate) fn try_launch_claim(&self) -> Option<std::sync::Arc<VmSupervisor>> {
        {
            let mut st = self.try_launch.lock().expect("vm try launch lock poisoned");
            if st.starting {
                return None;
            }
            st.last_error = None;
            st.stop_requested = false;
            st.starting = true;
        }
        let owned = self
            .self_weak
            .lock()
            .expect("vm supervisor self_weak lock poisoned")
            .clone()
            .and_then(|w| w.upgrade());
        if owned.is_none() {
            self.try_launch
                .lock()
                .expect("vm try launch lock poisoned")
                .starting = false;
        }
        owned
    }

    /// 启动任务收尾：翻牌 + 记错（成功清错）。之后取停止请求须另起一次
    /// 锁（`try_launch_take_stop_request`）——收尾翻牌后 `stop_if_starting`
    /// 不再接受新登记，无「看到在途却补不上停」的窗口。
    pub(crate) fn try_launch_finish(&self, err: Option<String>) {
        let mut st = self.try_launch.lock().expect("vm try launch lock poisoned");
        st.starting = false;
        st.last_error = err;
    }

    /// 读后清停止请求（启动任务就绪后消费）。
    pub(crate) fn try_launch_take_stop_request(&self) -> bool {
        std::mem::take(
            &mut self
                .try_launch
                .lock()
                .expect("vm try launch lock poisoned")
                .stop_requested,
        )
    }

    /// try.stop 在启动在途时的落点：在途 = 登记停止请求并返回 true
    ///（调用方给「就绪后自动停机」注记）；检查与登记同一临界区，与
    /// `try_launch_finish` 的翻牌互斥。
    pub(crate) fn try_launch_stop_if_starting(&self) -> bool {
        let mut st = self.try_launch.lock().expect("vm try launch lock poisoned");
        if st.starting {
            st.stop_requested = true;
            true
        } else {
            false
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

        // 命中槽：摸活跃时间；死槽顺手收敛（QEMU 崩溃后下一次调用自愈）。
        // **在一切「拒新」闸之前**——活槽复用不消耗新磁盘/名额，GC stop/
        // 满员只该拦新启动；把运行中实例拒成 Unavailable = 逼上层宿主回落
        // 操作另一套文件视图（正确性事故）。路由判定走 [`ensure_route`]
        // 纯函数（顺序语义单测钉死）。先绑定再 if-let——scrutinee 临时
        // std 守卫若活到块尾会罩住下面的 `existing.lock().await`，future
        // 非 Send（async_trait 面 require）。
        let mut hit: Option<Arc<tokio::sync::Mutex<VmSlot>>> = None;
        let existing = self
            .slots
            .lock()
            .expect("vm supervisor slots lock poisoned")
            .get(agent_id)
            .cloned();
        if let Some(existing) = existing {
            let mut slot = existing.lock().await;
            slot.last_used = Instant::now();
            let alive = slot.vm.is_alive();
            drop(slot);
            if alive {
                hit = Some(existing);
            } else {
                self.slots
                    .lock()
                    .expect("vm supervisor slots lock poisoned")
                    .remove(agent_id);
                if let Some(mut record) = load_record(&self.workspace, agent_id) {
                    mark_cold(&mut record);
                    save_record_warn(&self.workspace, agent_id, &record, "死槽收敛");
                }
            }
        }

        // GC stop 闸 + 满员闸（经 [`ensure_route`]：活槽已短路到这里之外）。
        let used = disk_usage_bytes(&self.workspace);
        match ensure_route(
            hit.is_some(),
            used,
            &settings.gc,
            self.slots
                .lock()
                .expect("vm supervisor slots lock poisoned")
                .len(),
            settings.max_instances,
        ) {
            EnsureRoute::Reuse => return Ok(hit.expect("route Reuse 必有槽")),
            EnsureRoute::GcDeny { used_gb, budget_gb } => {
                return Err(VmSupervisorError::Unavailable(format!(
                    "VM 磁盘预算闸（over_limit=stop）：已用 {used_gb}GB 超预算 {budget_gb}GB，拒绝新实例（不删既有数据）"
                )));
            }
            EnsureRoute::CapFull { max_instances } => {
                return Err(VmSupervisorError::Unavailable(format!(
                    "VM 满员闸：运行中实例已达 max_instances({max_instances})"
                )));
            }
            EnsureRoute::Proceed => {}
        }

        // 启动在途闸（同 agent 互斥）：insert 成功 = 拿到本 agent 启动权；
        // 已在途 = 诚实拒绝（不排队——spawn 全程分钟级，等它不如让调用方
        // 稍后重试；try 面另有 starting 旗标给前端更友好的引导）。守卫 Drop
        // 覆盖后续所有早退路径。
        if !self
            .starting
            .lock()
            .expect("vm supervisor starting lock poisoned")
            .insert(agent_id.to_string())
        {
            return Err(VmSupervisorError::Unavailable(format!(
                "agent {agent_id} 的 VM 正在启动中（冷启动最长约 {}s）——待就绪后再试",
                GUEST_READY_BUDGET.as_secs()
            )));
        }
        let _start_gate = StartGateGuard {
            set: &self.starting,
            id: agent_id.to_string(),
        };

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
        // spawn 前落盘锚（一）：registry 先于 spawn——写失败 = 锚不成立 =
        // 不 spawn（修前吞错只 warn，锚位形同虚设）。
        save_record(&self.workspace, agent_id, &record).map_err(|e| {
            VmSupervisorError::Failed(format!("registry 落盘失败（spawn 前锚）: {e}"))
        })?;

        let arch = GuestArch::parse(&record.arch)
            .map_err(|e| VmSupervisorError::Failed(format!("registry arch 非法: {e}")))?;
        let explicit =
            QemuAccel::parse_hypervisor(&settings.hypervisor).map_err(VmSupervisorError::Failed)?;
        let accel = adjudicate_accel_real(self.executor_strict(), explicit, arch).map_err(|e| {
            VmSupervisorError::Unavailable(format!("加速裁决拒绝（诚实缺席）: {e}"))
        })?;
        record.accel = accel.as_str().to_string();
        // spawn 前落盘锚（二）：accel 裁决回填后、spawn 前（sweep 身份核验
        // 需要它重解 qemu 二进制）。失败同样不 spawn。
        save_record(&self.workspace, agent_id, &record).map_err(|e| {
            VmSupervisorError::Failed(format!("registry 落盘失败（spawn 前锚）: {e}"))
        })?;

        let qemu_bin = resolve_qemu_binary(arch, qemu_install_dir(&self.workspace).as_deref());

        // spawn 前活 QEMU 防线（[`ensure_no_live_qemu`]）：registry 丢失/
        // 损坏时 sweep 看不见的孤儿——双开同 overlay = qcow2 并发写损坏，
        // 这里诚实拒绝。
        ensure_no_live_qemu(&run_dir.join("qemu.pid"), &qemu_bin)
            .map_err(VmSupervisorError::Failed)?;

        // 镜像换装互斥（[`IMAGE_SWAP_GATE`] 两相协议的 spawn 侧）：从
        // overlay 首触（首启 create_overlay 以 base 作 backing 模板）持到
        // 本函数终态——成功=槽即将入 map（promote 侧 pidfile 扫描可见），
        // 失败=子进程已收尸（overlay 已释放）。promote 持闸期间本路径在
        // 此排队，闸后启动用的已是新 base，一致性自然成立。守卫取消即
        // 释放，不卡停机。
        let _swap_gate = IMAGE_SWAP_GATE.lock().await;

        // 在途标记（[`StartingMarkerGuard`]）：闸后落盘——排队等闸期间不
        // 算在途（否则另一进程的换装扫描会为一个还没开跑的 start 误拒）。
        // 覆盖本进程 spawn 全相位 + 跨进程可见（另一进程的 promote 扫描
        // 看得见）；Drop 即移除，终态后 pidfile 接管活体哨兵。
        let _starting = StartingMarkerGuard::arm(&run_dir)
            .map_err(|e| VmSupervisorError::Failed(format!("spawn 在途标记落盘失败: {e}")))?;

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
                // 唤醒载体的迁移状态文件不删（可能仍有效，手动恢复/排查用），
                // 但记录已转冷停不再指向它——留一声 WARN 指路，避免盘上
                // 孤儿状态文件无从追溯。
                if let Some(p) = &record.state_path {
                    tracing::warn!(
                        "[vm-runtime] agent {agent_id} 启动失败：迁移状态文件保留在 {}（记录已转冷停，可手动恢复或删除）",
                        p.display()
                    );
                }
                mark_cold(&mut record);
                save_record_warn(&self.workspace, agent_id, &record, "启动失败转冷停");
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
                        save_record_warn(&self.workspace, agent_id, &record, "就绪超时转冷停");
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
        save_record_warn(&self.workspace, agent_id, &record, "启动完成回填");
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
            save_record_warn(&self.workspace, agent_id, &record, "睡眠前死槽收敛");
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
        save_record_warn(&self.workspace, agent_id, &record, "睡眠完成");
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
            save_record_warn(&self.workspace, agent_id, &record, "停机转冷停");
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
            if self
                .starting
                .lock()
                .expect("vm supervisor starting lock poisoned")
                .contains(&dir_name)
            {
                continue; // 自己正在启动的实例不碰（启动期记录 asleep_at=None
                // 且槽未入 map——mtime 保护窗短于就绪预算，此处是
                // 不受窗长限制的精确防线）
            }
            let Some(mut record) = load_record(&self.workspace, &dir_name) else {
                // registry 缺失/不可解析（原子写已消除半截形态，剩下是人为
                // 删除/外部损坏）：孤儿可见性盲区——无记录不动任何进程，
                // loud 一声让操作者知道这里有失控风险。
                tracing::warn!(
                    "[vm-runtime] agent {dir_name} 的 registry 缺失/不可解析：孤儿 sweep 本轮跳过（无记录不动任何进程）"
                );
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
            save_record_warn(&self.workspace, &dir_name, &record, "孤儿 sweep");
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
                    save_record_warn(&self.workspace, &id, &record, "watchdog 死槽收敛");
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

        let ctx = serde_json::to_value(context)
            .map_err(|e| PersistentCallError::ToolFailed(format!("序列化请求上下文失败: {e}")))?;
        self.call_guest(agent_id, tool, args, &ctx)
            .await
            .map_err(vm_err_to_persistent)
    }
}

impl VmSupervisor {
    /// 单次 guest 调用共享通路（ensure_running + 握手 + guest_call）。
    /// B9-1 从 execute_persistent 提取——agent 工具调用与 internal_call
    /// 走同一条路，差别只在调用方的闸（子集闸/无闸）。
    ///
    /// 失败分型沿用既有纪律：ensure_running/握手失败 = Unavailable（请求
    /// 从未发出，上层可回落）；请求发出后的失败 = Failed（副作用未知，
    /// 绝不回落）。
    async fn call_guest(
        &self,
        agent_id: &str,
        tool: &str,
        args: &str,
        ctx: &serde_json::Value,
    ) -> Result<String, VmSupervisorError> {
        let slot = self.ensure_running(agent_id).await?;

        // 每调用独立连接（VmInstance 不持常驻流——睡眠/唤醒生命周期以
        // 进程边界为锚，跨睡眠的常驻流是伪状态）。
        let mut stream = {
            let s = slot.lock().await;
            nemesis_sandbox::vm::lifecycle::auth_handshake(
                s.vm.agent_addr,
                &s.vm.token,
                GUEST_CALL_TIMEOUT,
            )
            .await
            .map_err(|e| VmSupervisorError::Unavailable(format!("VM guest 通道握手失败: {e}")))?
        };
        let resp = nemesis_sandbox::vm::lifecycle::guest_call(
            &mut stream,
            tool,
            args,
            ctx,
            GUEST_CALL_TIMEOUT,
        )
        .await
        .map_err(|e| {
            VmSupervisorError::Failed(format!("VM guest 调用失败（副作用未知，不回落）: {e}"))
        })?;
        if resp.ok {
            Ok(resp.result)
        } else {
            Err(VmSupervisorError::Failed(format!(
                "VM guest 工具执行失败: {}",
                resp.error
            )))
        }
    }

    /// B9-1：vm_sync 同步引擎的**内部方法通道**（bulk_write/bulk_read/
    /// bulk_read_chunk/hash_tree）。与 agent 工具面的唯一差别 = **不过
    /// `VM_GUEST_TOOLS` 子集闸**——这四个方法名宿主闸根本不认识，agent
    /// 永远摸不到（vm_sync 地基，B9 实施计划 §4.2 ③「guest 三个内部
    /// 方法」的宿主侧入口）。ctx 钉 `source: "vm_sync"` 供 guest 侧区分。
    pub(crate) async fn internal_call(
        &self,
        agent_id: &str,
        tool: &str,
        args: serde_json::Value,
    ) -> Result<String, VmSupervisorError> {
        let ctx = serde_json::json!({ "source": "vm_sync" });
        self.call_guest(agent_id, tool, &args.to_string(), &ctx)
            .await
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
