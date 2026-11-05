// 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。

use super::*;

/// Executor separation + sandbox switch (Layer 1 / Layer 2). See
/// `executor-separation`.
///
/// - `enabled`: Layer 1 — run execution-class tools (exec/file/grep/git/...) in
///   a separate child process (per-call spawn). `false` = today's in-process
///   behavior (safe fallback).
/// - `sandbox`: Layer 2 — wrap the child spawn with Sandboxie `Start.exe /box:`.
///   Requires `enabled = true` AND Sandboxie installed (next phase). The gateway
///   probes for Start.exe and falls back if absent.
/// - `allow_network`: Sandboxie box-level network switch — writes
///   `AllowNetworkAccess=y/n` into Sandboxie.ini for the NemesisBox box. Applies
///   to every process in the box (the executor child AND any program the user
///   launches from the in-box explorer). Independent of `enabled`/`sandbox`.
///   A flip takes effect for NEWLY started box processes after `Start.exe
///   /reload`; already-running box processes keep their old network state until
///   restarted (Sandboxie's WFP callout reads a per-process `BlockInternet`
///   cache that reload does not refresh).
/// - `strict`（严格模式）: fail-closed 闸门 — `sandbox=true` 被要求但本机
///   沙盒后端不可用（Windows=Sandboxie 未就绪；Linux=landlock/bwrap 均不可用；
///   裁剪构建=`sandbox` feature 被裁掉）时**拒绝执行高危工具**并明确报错，
///   而非默认的 warn + 无盒降级（fail-open）。注意 Partial 强制（后端可用但
///   有能力缺口，如 landlock 不覆盖网络）**不算不可用**——严格模式放行并
///   在日志/状态里如实标注缺口。默认 false = 现状字节不变。开关经
///   ConfigStore 翻转后对后续工具调用实时生效（与 `sandbox` 同链路）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorSeparationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub strict: bool,
    /// 沙盒后端选择（`executor.backend`，
    /// 缺省 `"auto"`）。值域：`auto`（默认——Windows 上 Sandboxie 就绪则
    /// sandboxie，否则退 Windows 用户态 ACL 轻量档）/ `sandboxie`（显式钉
    /// Sandboxie）/ `acl`（显式钉 ACL 档）。未知值诚实拒绝（选型返回
    /// None + warn，不静默改道）。非 Windows 平台无消费方（landlock/bwrap/
    /// Seatbelt 选型不读此键）。
    ///
    /// ⚠ 实验性标注：acl 档是**半档隔离**（强制完整性标签 No-Write-Up
    /// 围栏 + DACL 原语），禁不了网、写围栏依赖令牌降级——详见
    /// `crates/nemesis-sandbox/src/backend.rs` 的 模块文档与诚实边界。
    /// 兼容性：`#[serde(default)]`——老 config.json 缺键 = `"auto"`，行为与
    /// 升级前完全一致（typed save 会把缺省值显式写回，语义不变）。
    #[serde(default = "default_executor_backend")]
    pub backend: String,
    /// DACL 定向档开关组（`executor.acl` 段，缺省全关
    /// = 现状字节不变）。见 [`ExecutorAclConfig`]。
    #[serde(default)]
    pub acl: ExecutorAclConfig,
    /// 常驻执行体开关组（`executor.persistent` 段，缺省全关 =
    /// per-call 现状）。见 [`ExecutorPersistentConfig`]。
    #[serde(default)]
    pub persistent: ExecutorPersistentConfig,
    /// （桌面级 VM）：VM 后端开关组（`executor.vm` 段）。
    /// **铁律：`enabled` 默认 false**——关闭态 VM 路径零激活（零 QEMU 进程、
    /// 零镜像下载、工具走原路径），全链行为字节不变（验收门 专项断言）。
    /// 见 [`ExecutorVmConfig`]。
    #[serde(default)]
    pub vm: ExecutorVmConfig,
}

/// `executor.persistent` 段：常驻执行体（per-agent 长驻子进程）的
/// 细粒度开关。缺省全关 = per-call 形态字节不变——常驻是增强档不是替换
/// 闸，不可用（满员/spawn 失败/传输不可恢复）时全量回落 per-call Layer1/2。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorPersistentConfig {
    /// 常驻执行体总开关（默认 false；true 时高危工具优先路由到常驻子进程）。
    #[serde(default)]
    pub enabled: bool,
    /// 本节点常驻执行体槽位上限（满员时新调用回落 per-call，不排队不拒绝）。
    #[serde(default = "default_persistent_max_slots")]
    pub max_slots: u32,
    /// 空闲自动 sleep 秒数（watchdog 扫描；0 = 不自动睡，由显式 sleep 托管）。
    #[serde(default = "default_persistent_idle_secs")]
    pub idle_shutdown_secs: u64,
    /// 单次工具调用的传输超时秒数（子进程活着但没回话）。
    #[serde(default = "default_persistent_call_timeout_secs")]
    pub call_timeout_secs: u64,
}

fn default_persistent_max_slots() -> u32 {
    4
}

fn default_persistent_idle_secs() -> u64 {
    300
}

fn default_persistent_call_timeout_secs() -> u64 {
    600
}

impl Default for ExecutorPersistentConfig {
    /// 手写而非 derive（ExecutorAclConfig 同款 serde 双路径陷阱——结构体级
    /// Default 不经过字段级 serde 缺省）：缺键 = enabled 关 + 满配额缺省。
    fn default() -> Self {
        Self {
            enabled: false,
            max_slots: default_persistent_max_slots(),
            idle_shutdown_secs: default_persistent_idle_secs(),
            call_timeout_secs: default_persistent_call_timeout_secs(),
        }
    }
}

/// `executor.acl` 段：DACL 定向档（write-restricted 受限令牌 + workspace
/// SID GRANT ACE 树，- 原语）的细粒度开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorAclConfig {
    /// DACL 定向档总开关（默认 false = opt-in；开 = executor 子进程经
    /// `CreateProcessAsUserW` + 受限令牌 spawn——内核强制写围栏，与子进程
    /// 自装完整性标签两层叠加）。关/装配失败（宽松模式）→ 回退现状路径。
    #[serde(default)]
    pub dacl: bool,
    /// 本档自己的 fail-closed 闸（与全局 `executor.strict` 独立——那条只认
    /// Sandboxie）：true 时 DACL 装配失败 = **拒绝执行**（不降级）。
    #[serde(default)]
    pub strict: bool,
    /// 树遍历预算（GRANT ACE 打标的文件数上限，同 `label_tree` 既有语义）。
    #[serde(default = "default_acl_max_files")]
    pub max_files: u32,
}

impl Default for ExecutorAclConfig {
    /// 手写而非 derive（serde 双路径陷阱，hook_flips 真进程测试抓出）：
    /// executor 段**无 acl 键**时 serde 走**结构体级** `Default::default`
    /// 字段级 `#[serde(default = "...")]` 不经过，derive 的 `u32` 默认
    /// 是 0，会让 hook 的铺树预算直接耗尽（写操作全死）。手写让两条构造
    /// 路径收敛到同一组默认值。
    fn default() -> Self {
        Self {
            dacl: false,
            strict: false,
            max_files: default_acl_max_files(),
        }
    }
}

/// `executor.acl.max_files` 缺省值（设计稿：20000）。
fn default_acl_max_files() -> u32 {
    20_000
}

/// `executor.backend` 的 serde 缺省值（`"auto"`）。独立函数而非 `String::default`
/// 空字符串不是合法选择（选型按未知值诚实拒绝），缺键必须落成 `"auto"`。
fn default_executor_backend() -> String {
    "auto".to_string()
}

impl Default for ExecutorSeparationConfig {
    /// 手写而非 derive（与 [`ExecutorAclConfig`] 同款 serde 双路径陷阱）：
    /// `backend` 的 derive 默认是空串，而空串不是合法选型值（诚实拒绝）——
    /// `unwrap_or_default`（sandbox handler 对缺失 executor 段的补齐路径）
    /// 必须落成 `"auto"` 而不是 `""`。
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: false,
            allow_network: false,
            strict: false,
            backend: default_executor_backend(),
            acl: ExecutorAclConfig::default(),
            persistent: ExecutorPersistentConfig::default(),
            vm: ExecutorVmConfig::default(),
        }
    }
}

/// `executor.vm` 段：桌面级 VM 后端（第三类隔离后端——独立内核整机
/// 隔离）。缺省 = VM 路径零激活（铁律：总开关默认关，绝不替用户决定）。
///
/// 枚举型字符串（arch/hypervisor/desktop/gc.over_limit）未知值 loud 拒绝
/// （[`ExecutorVmConfig::validate`]，对齐 `executor.backend` 先例）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorVmConfig {
    /// VM 总开关（默认 false——关闭态零 QEMU 进程、零镜像下载、工具走
    /// 原路径；Dashboard VM 页同款开关，用户可选开、关）。
    #[serde(default)]
    pub enabled: bool,
    /// Guest CPU 架构：`x86_64`（当前唯一产出档）| `aarch64`（接口在位、
    /// 当前不产出——选了诚实报不支持，恢复路径见实施计划）。
    #[serde(default = "default_vm_arch")]
    pub arch: String,
    /// 硬件加速：`auto`（探测裁决）| `kvm`（显式钉 Linux KVM）| `whpx`
    /// （显式钉 Windows WHPX）。加速缺席 = 诚实 Unavailable（不做 TCG
    /// 假跑降级；`executor.strict` 开启时拒绝执行）。
    #[serde(default = "default_vm_hypervisor")]
    pub hypervisor: String,
    /// base 镜像桌面形态：`minimal`（轻量 WM + Chromium，默认）| `full`
    /// （完整 DE 形态的可选项）。两变体 base 镜像分开构建。
    #[serde(default = "default_vm_desktop")]
    pub desktop: String,
    /// base 镜像来源（来源写死在本地配置中，页面可看可选可配
    /// 可改）。
    #[serde(default)]
    pub image: VmImageConfig,
    /// per-VM 资源配额。
    #[serde(default)]
    pub resources: VmResourceConfig,
    /// 本节点并发 VM 实例上限（满员闸，对齐 persistent.max_slots 语义——
    /// 满员诚实拒绝）。
    #[serde(default = "default_vm_max_instances")]
    pub max_instances: u32,
    /// 空闲自动 sleep 秒数（0 = 不自动睡；对齐 persistent.idle_shutdown_secs）。
    #[serde(default = "default_vm_idle_sleep_secs")]
    pub idle_sleep_secs: u64,
    /// guest agent 就绪等待预算秒数（冷启/唤醒统一）：QMP running ≠ guest
    /// 侧 agent 已监听——cloud-init/systemd 还在走（微型镜像典型 20-60s，
    /// 缺省 240 给慢盘慢机余量）。就绪前交槽 = 首次 guest 调用 connect
    /// refused → 宿主回落混两套文件视图——引导墙钟必须在交槽前吃掉。
    /// 缺省 240 = 原 vm_runtime 编译期常量行为字节不变（配置化）。
    /// 0 拒绝（无预算 = 启动可无限挂起，不是合法形态）。
    #[serde(default = "default_vm_ready_budget_secs")]
    pub ready_budget_secs: u64,
    /// 显示面（带界面 VM）：`none`（默认，`-display none` 无头形态
    /// 字节不变）| `vnc`（QEMU `-vnc 127.0.0.1:<display>` 回环 VNC——端口
    /// 走临时端口段分配、真实 TCP 端口进 registry，QEMU 参数按 display=
    /// port-5900 反向换算；经 plugin-vm 插件中继到 Dashboard）。vnc 态要求
    /// desktop variant 镜像（headless 镜像 + vnc = 诚实报错，黑屏不是合法形态）。
    #[serde(default = "default_vm_display")]
    pub display: String,
    /// vnc 态分辨率预设（guest 侧 xinit/xrandr 消费；`宽x高` 形态，缺省
    /// 1280x720）。display=none 时该键无效（不校验与显示面的耦合，键独立
    /// 合法即可——无头形态下改分辨率无消费方）。
    #[serde(default = "default_vm_display_resolution")]
    pub display_resolution: String,
    /// 磁盘 GC（总磁盘预算）。
    #[serde(default)]
    pub gc: VmGcConfig,
}

/// `executor.vm.image` 段：base 镜像来源（URL + SHA-256）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmImageConfig {
    /// 镜像下载 URL（默认 Debian generic cloud image；可自由编辑——镜像
    /// 来源可换是验收面）。
    #[serde(default = "default_vm_image_source")]
    pub source: String,
    /// 镜像 SHA-256（hex）。**空 = 构建时从 `{source}` 同目录的 SHA256SUMS
    /// sidecar 按文件名查表**（Debian 目录惯例）——模板不带易过期的硬编码
    /// 哈希；显式钉住优先于 sidecar。
    #[serde(default)]
    pub sha256: String,
}

/// `executor.vm.resources` 段：per-VM 资源配额。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmResourceConfig {
    #[serde(default = "default_vm_vcpu")]
    pub vcpu: u32,
    #[serde(default = "default_vm_ram_mb")]
    pub ram_mb: u32,
    #[serde(default = "default_vm_disk_gb")]
    pub disk_gb: u32,
}

/// `executor.vm.gc` 段：磁盘 GC（总磁盘预算）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmGcConfig {
    /// 全部 VM 磁盘产物（base + overlay）总预算 GB；超限动作按 `over_limit`。
    /// 0 = 不设预算。
    #[serde(default = "default_vm_gc_max_disk_gb")]
    pub max_total_disk_gb: u32,
    /// 超限动作：`warn`（默认，日志横幅可）| `stop`（拒绝新实例创建，
    /// 既有实例不受影响）。
    #[serde(default = "default_vm_gc_over_limit")]
    pub over_limit: String,
}

fn default_vm_arch() -> String {
    "x86_64".to_string()
}

fn default_vm_hypervisor() -> String {
    "auto".to_string()
}

fn default_vm_desktop() -> String {
    "minimal".to_string()
}

fn default_vm_display() -> String {
    "none".to_string()
}

fn default_vm_display_resolution() -> String {
    "1280x720".to_string()
}

fn default_vm_max_instances() -> u32 {
    2
}

fn default_vm_idle_sleep_secs() -> u64 {
    300
}

fn default_vm_ready_budget_secs() -> u64 {
    240
}

fn default_vm_vcpu() -> u32 {
    2
}

fn default_vm_ram_mb() -> u32 {
    2048
}

fn default_vm_disk_gb() -> u32 {
    20
}

fn default_vm_gc_max_disk_gb() -> u32 {
    50
}

fn default_vm_gc_over_limit() -> String {
    "warn".to_string()
}

/// Debian generic cloud image（trixie/latest 的 genericcloud 变体——无
/// 云初始化代理依赖的最小 cloud-init 面）。可自由编辑。
/// 与 build-vm-image.sh（plugins/plugin-vm/，随 plugin-vm 插件包分发）的
/// DEFAULT_BASE_URL 必须同源：config 缺省
/// sha256 为空时，脚本只对**自己的默认 URL** 自动走上游 SHA256SUMS→
/// SHA512SUMS 校验链；两处默认一旦分叉，出盒链路会被脚本按「换源」诚实
/// 拒绝（缺显式哈希）—— 全新环境实跑。
fn default_vm_image_source() -> String {
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"
        .to_string()
}

impl Default for ExecutorVmConfig {
    /// 手写而非 derive（serde 双路径陷阱，ExecutorPersistentConfig 同款）：
    /// executor 段**无 vm 键**时 serde 走结构体级 Default，字段级
    /// `#[serde(default = "...")]` 不经过——两条构造路径必须收敛到同一组
    /// 默认值（enabled=false 铁律在两条路径都成立）。
    fn default() -> Self {
        Self {
            enabled: false,
            arch: default_vm_arch(),
            hypervisor: default_vm_hypervisor(),
            desktop: default_vm_desktop(),
            display: default_vm_display(),
            display_resolution: default_vm_display_resolution(),
            image: VmImageConfig::default(),
            resources: VmResourceConfig::default(),
            max_instances: default_vm_max_instances(),
            idle_sleep_secs: default_vm_idle_sleep_secs(),
            ready_budget_secs: default_vm_ready_budget_secs(),
            gc: VmGcConfig::default(),
        }
    }
}

impl Default for VmImageConfig {
    fn default() -> Self {
        Self {
            source: default_vm_image_source(),
            sha256: String::new(),
        }
    }
}

impl Default for VmResourceConfig {
    fn default() -> Self {
        Self {
            vcpu: default_vm_vcpu(),
            ram_mb: default_vm_ram_mb(),
            disk_gb: default_vm_disk_gb(),
        }
    }
}

impl Default for VmGcConfig {
    fn default() -> Self {
        Self {
            max_total_disk_gb: default_vm_gc_max_disk_gb(),
            over_limit: default_vm_gc_over_limit(),
        }
    }
}

impl ExecutorVmConfig {
    /// 枚举型字符串字段校验（未知值 loud 拒绝——选型错误不静默改道）。
    /// 配置装载/`vm.config.set` 两面同源消费。
    pub fn validate(&self) -> Result<()> {
        // arch：与 nemesis-sandbox vm::guest_arch 同值域（crate 分层上
        // nemesis-config 不依赖 sandbox——值域在此重复一份，单测钉同步）。
        match self.arch.as_str() {
            "x86_64" | "aarch64" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.arch 未知值 {other:?}（合法值：x86_64 | aarch64）"
                )));
            }
        }
        match self.hypervisor.as_str() {
            "auto" | "kvm" | "whpx" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.hypervisor 未知值 {other:?}（合法值：auto | kvm | whpx）"
                )));
            }
        }
        match self.desktop.as_str() {
            "minimal" | "full" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.desktop 未知值 {other:?}（合法值：minimal | full）"
                )));
            }
        }
        match self.display.as_str() {
            "none" | "vnc" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.display 未知值 {other:?}（合法值：none | vnc；\
                     vnc 需 plugin-vm 插件 + desktop variant 镜像）"
                )));
            }
        }
        // display_resolution：`宽x高` 形态（正整数），guest xrandr 消费面。
        if let Err(msg) = validate_display_resolution(&self.display_resolution) {
            return Err(ConfigError::Validation(format!(
                "executor.vm.display_resolution {msg}（合法形态：宽x高，如 1280x720）"
            )));
        }
        match self.gc.over_limit.as_str() {
            "warn" | "stop" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.gc.over_limit 未知值 {other:?}（合法值：warn | stop）"
                )));
            }
        }
        if self.max_instances == 0 {
            return Err(ConfigError::Validation(
                "executor.vm.max_instances 必须 ≥ 1（0 无法创建任何实例；\
                 要彻底关闭 VM 请用 executor.vm.enabled=false）"
                    .to_string(),
            ));
        }
        if self.ready_budget_secs == 0 {
            return Err(ConfigError::Validation(
                "executor.vm.ready_budget_secs 必须 ≥ 1（0 = guest 就绪等待无上限，\
                 启动可无限挂起不是合法形态；要更多余量请直接调大数值）"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// `executor.vm.display_resolution` 形态校验：`宽x高`（正整数）。
/// 单独函数（validate 与未来消费方同源复用），错误消息只含值态不含键名
/// （键名由调用方补——文案单一出口）。
pub fn validate_display_resolution(v: &str) -> std::result::Result<(), String> {
    let (w, h) = v
        .split_once('x')
        .ok_or_else(|| format!("未知值 {v:?}，缺 'x' 分隔"))?;
    let w: u32 = w
        .parse()
        .map_err(|_| format!("未知值 {v:?}，宽度非正整数"))?;
    let h: u32 = h
        .parse()
        .map_err(|_| format!("未知值 {v:?}，高度非正整数"))?;
    if w == 0 || h == 0 {
        return Err(format!("未知值 {v:?}，宽高必须 > 0"));
    }
    Ok(())
}

// ============================================================================
// Debug Config (diagnostic capture)
// ============================================================================
