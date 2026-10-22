//! P5 第 4 步 — [`VmBridge`](nemesis_web::handlers::vm::VmBridge) 的
//! [`VmSupervisor`] 实现（bot 侧桥——nemesis-web 不反向依赖本 crate，
//! trait 在 web 侧定义、实现在这里，gateway 装配期装槽）。
//!
//! 职责边界：本模块只做**投影与转发**——typed config 解析/校验/持久化、
//! 加速器探针包装、镜像构建后台任务（脚本 spawn + 日志落盘 + 存量
//! overlay 安全 rebase）、生命周期命令错误分类翻译。QEMU 管线细节全在
//! [`crate::agents::vm_runtime`] 与 nemesis-sandbox，不在这里重复。

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nemesis_sandbox::vm::assembly::resolve_qemu_binary;
use nemesis_sandbox::vm::guest_arch::{GuestArch, QemuAccel};
use nemesis_sandbox::vm::install;
use nemesis_sandbox::vm::lifecycle::SleepMode;
use nemesis_sandbox::vm::probe;
use nemesis_sandbox::vm::probe::AccelProbe;
#[cfg(target_os = "linux")]
use nemesis_sandbox::vm::probe::probe_kvm;
#[cfg(target_os = "windows")]
use nemesis_sandbox::vm::probe::probe_whpx;

use crate::agents::vm_runtime::{
    GIB, GcDecision, VmSupervisor, VmSupervisorError, WakeKind, disk_usage_bytes, gc_decision,
    load_record, now_ms, resolve_qemu_img, vm_images_root, vm_overlays_root, wake_kind_of,
};

use nemesis_web::handlers::vm::VmBridge;

/// 镜像构建日志（`<workspace>/tools/vm/image-build.log`）——每次构建
/// 截断重写；页面进度 = 轮询 `vm.image.list` 的 log.tail。
pub(crate) fn image_build_log_path(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("image-build.log")
}

/// 构建暂存目录（`<workspace>/tools/vm/images/.staging`）——构建产物
/// 先落这里，rebase/换装成功后才进正式位（半途失败不碰现有镜像）。
fn staging_dir(workspace: &Path) -> PathBuf {
    vm_images_root(workspace).join(".staging")
}

/// 进行中的构建任务（进程级单飞闸——并发构建互斥）。
struct ImageJob {
    started_ms: u64,
    update: bool,
}

static IMAGE_JOB: Mutex<Option<ImageJob>> = Mutex::new(None);

/// 进行中的 QEMU 安装任务（B3 自举族——与 IMAGE_JOB 同款单飞闸；两闸独立：
/// 装 QEMU 与建镜像互不阻塞）。
struct BootstrapJob {
    started_ms: u64,
}

static BOOTSTRAP_JOB: Mutex<Option<BootstrapJob>> = Mutex::new(None);

/// QEMU 安装日志（`<workspace>/tools/vm/bootstrap.log`）——每次安装
/// 截断重写；安装卡进度 = 轮询 `vm.bootstrap.status` 的 log.tail。
pub(crate) fn bootstrap_log_path(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("bootstrap.log")
}

/// 镜像构建成功收尾日志行（run_image_build 唯一成功出口；CLI 前台臂以
/// 本行判定成败——单一真相源。「换装失败」分支不含本串，不误判）。
pub(crate) const IMAGE_BUILD_DONE_LINE: &str = "换装完成";

/// 安装/构建任务是否在飞（CLI 前台臂轮询用；WSAPI 面看各 list 的 job 槽）。
pub(crate) fn image_job_running() -> bool {
    IMAGE_JOB.lock().expect("image job lock").is_some()
}

// ===========================================================================
// B7 —— Windows 宿主镜像构建 WSL 自动路由
//
// 探针四态 + 解析/分类纯函数 + 路径翻译。纯函数跨平台（解析/分类可在
// 任何宿主上单测）；真 spawn 探测 cfg(windows)。构建管线本体
// （plugins/plugin-vm/build-vm-image.sh，随 plugin-vm 插件包分发）零改动
// ——本模块只回答「怎么把它 spawn 起来」。
// ===========================================================================

/// WSL 探针结论（`bootstrap_status` 的 image_route 段与 `image_build`
/// 启动裁决共用一份事实）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WslProbe {
    pub state: WslState,
    /// MissingTools 时缺的工具（人读清单，前端直接展示）。
    pub missing: Vec<String>,
    /// 发行版内有 apt-get（修复命令用 apt 形态）。
    pub has_apt: bool,
    /// Unavailable 时的人读原因。
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Ready/NoDistro 的构造点全在 cfg(windows) 探针与 cfg(test) 单测里——Linux
// 非 test 构建天然无构造（spawn 探测只在 Windows 跑），条件 allow 消
// dead_code（CI Linux clippy 实录 2026-10-19）；Windows/test 侧 lint 照常在场。
// 变体本身不可 cfg 裁剪：classify_image_route 跨平台读全四态。
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub(crate) enum WslState {
    /// 管线工具齐备——可直接经 wsl.exe 构建。
    Ready,
    /// 无任何发行版（含 WSL 功能未启用——`wsl --install` 一并修复）。
    NoDistro,
    /// 有发行版但缺管线工具（ISO 工具 / file(1) / 下载器）。
    MissingTools,
    /// wsl.exe 不在场 / 探测超时等探测本身失败。
    Unavailable,
}

impl WslProbe {
    fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            state: WslState::Unavailable,
            missing: Vec::new(),
            has_apt: false,
            reason: reason.into(),
        }
    }

    /// 唯一消费点 = [`wsl_spawn_probe`]（cfg windows）——随门裁剪消 Linux
    /// dead_code（同 [`WslState`] 注）。
    #[cfg(windows)]
    fn no_distro() -> Self {
        Self {
            state: WslState::NoDistro,
            missing: Vec::new(),
            has_apt: false,
            reason: String::new(),
        }
    }
}

// 下面 5 个解析/分类纯函数的生产消费点全在 [`wsl_spawn_probe`]（cfg
// windows）；Linux 侧只有伴生测试消费——`#[cfg(any(windows, test))]` 让
// Linux bin 构建不编译（无 dead_code），跨平台单测语义保持（模块头注）。
/// wsl.exe 管理命令（`--list` 等）输出解码：带 UTF-16LE BOM 显式认；无 BOM
/// 按「奇数下标 0 字节占比」启发式认 UTF-16LE（纯 ASCII 文本的 UTF-16LE
/// 编码在每个奇数下标放 0x00，UTF-8 文本几乎不会），否则按 UTF-8 lossy。
/// Linux 侧（`wsl -e bash`）输出是 UTF-8，走 lossy 分支原样透传。
#[cfg(any(windows, test))]
pub(crate) fn decode_wsl_output(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let zero_at_odd = bytes.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    if bytes.len() >= 2 && zero_at_odd * 2 >= bytes.len() {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// `wsl --list --quiet` 输出解析：每行一个发行版名；空集 = 无发行版。
/// （容忍行尾 `\r` 与解码残留的 `\0`。）
#[cfg(any(windows, test))]
pub(crate) fn parse_wsl_distro_list(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_end_matches('\u{0}').to_string())
        .filter(|l| !l.is_empty())
        .collect()
}

/// 清单形态判别：`--list --quiet` 成功输出是每行一个发行版注册名
/// （注册名不含空白）；而 wsl.exe 在「WSL 功能未启用 / 无发行版」时
/// 输出的是本地化错误消息——**实测退出码仍为 0**，不能靠 exit code 判别；
/// 消息行（语言相关）必含空白，与注册名形态可判别。非清单形态 = 按
/// 无发行版处理（`wsl --install` 对两态都是正确修复）。
#[cfg(any(windows, test))]
pub(crate) fn is_distro_manifest(lines: &[String]) -> bool {
    !lines.is_empty() && lines.iter().all(|l| !l.contains(' ') && !l.contains('\t'))
}

/// 工具探测输出解析：`<name>=yes|no` 行 → 映射；无法识别的行忽略。
#[cfg(any(windows, test))]
pub(crate) fn parse_tool_probe(text: &str) -> std::collections::BTreeMap<String, bool> {
    let mut tools = std::collections::BTreeMap::new();
    for line in text.lines() {
        if let Some((name, val)) = line.trim().split_once('=') {
            tools.insert(name.trim().to_string(), val.trim() == "yes");
        }
    }
    tools
}

/// 工具映射 →（缺失清单, 是否有 apt-get）。管线硬要求（build-vm-image.sh
/// 的静态检查同口径）：qemu-img（convert 烧录）+ file(1) + 下载器二选一；
/// 烧录步还需 root（loop 挂载），install 提示文案已注明。
#[cfg(any(windows, test))]
pub(crate) fn classify_wsl_tools(
    tools: &std::collections::BTreeMap<String, bool>,
) -> (Vec<String>, bool) {
    let on = |t: &str| tools.get(t).copied().unwrap_or(false);
    let mut missing = Vec::new();
    if !on("qemu-img") {
        missing.push("qemu-img（qemu-utils 包）".to_string());
    }
    if !on("file") {
        missing.push("file".to_string());
    }
    if !on("curl") && !on("wget") {
        missing.push("curl 或 wget".to_string());
    }
    (missing, on("apt-get"))
}

/// 工具探测脚本（在 WSL 发行版内跑；输出 `t=yes|no` 逐行）。唯一消费点
/// = [`wsl_spawn_probe`]（cfg windows）——随门裁剪（同上组注）。
#[cfg(windows)]
const WSL_TOOL_PROBE_SCRIPT: &str = "for t in qemu-img file curl wget apt-get; do command -v \"$t\" >/dev/null 2>&1 && echo \"$t=yes\" || echo \"$t=no\"; done";

/// 真 spawn 探测（Windows-only；阻塞式——调用方持 TTL 缓存）。
/// 单独 pub(crate) 是给伴生测试文件做环境无关断言（只验「落在四态之一」）。
#[cfg(windows)]
pub(crate) fn wsl_spawn_probe() -> WslProbe {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    const TIMEOUT: Duration = Duration::from_secs(3);
    // 后台探测不闪窗（CREATE_NO_WINDOW）。
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn run(program: &str, args: &[&str], timeout: Duration) -> Option<(i32, Vec<u8>, Vec<u8>)> {
        use std::os::windows::process::CommandExt as _;
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .ok()?;
        let mut stdout = child.stdout.take()?;
        let mut stderr = child.stderr.take()?;
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let mut out = Vec::new();
                    let mut err = Vec::new();
                    let _ = stdout.read_to_end(&mut out);
                    let _ = stderr.read_to_end(&mut err);
                    return Some((status.code().unwrap_or(-1), out, err));
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(25)),
                Err(_) => return None,
            }
        }
    }

    // ① 发行版清单（wsl.exe 管理命令 = UTF-16LE 输出）。
    let Some((_, out, _)) = run("wsl.exe", &["--list", "--quiet"], TIMEOUT) else {
        return WslProbe::unavailable("wsl.exe 不在场或探测超时（旧版 Windows / 冷启动）");
    };
    let distros = parse_wsl_distro_list(&decode_wsl_output(&out));
    if !is_distro_manifest(&distros) {
        // 空清单 / 错误消息文本（功能未启用也 exit 0——实测）同样归此态：
        // wsl --install 对「功能未启用」与「无发行版」都是正确修复。
        return WslProbe::no_distro();
    }

    // ② 管线工具探测（Linux 侧输出 = UTF-8）。
    let Some((_, out2, _)) = run(
        "wsl.exe",
        &["-e", "bash", "-c", WSL_TOOL_PROBE_SCRIPT],
        TIMEOUT,
    ) else {
        return WslProbe::unavailable("WSL 工具探测超时（发行版冷启动？）——稍后点复检");
    };
    let tools = parse_tool_probe(&decode_wsl_output(&out2));
    if tools.is_empty() {
        return WslProbe::unavailable("发行版内探测无响应（缺 bash？）——本管线需要 bash");
    }
    let (missing, has_apt) = classify_wsl_tools(&tools);
    if missing.is_empty() {
        WslProbe {
            state: WslState::Ready,
            missing,
            has_apt,
            reason: String::new(),
        }
    } else {
        WslProbe {
            state: WslState::MissingTools,
            missing,
            has_apt,
            reason: String::new(),
        }
    }
}

/// WSL 探针 TTL 缓存——bootstrap.status 前端轮询不能每次都 spawn wsl.exe
/// （冷启动秒级）。就绪/缺工具 30s；无发行版/不可用 5s（装 WSL/装工具是
/// 外部事件，复检要快点看到转绿）。构建启动前走 [`wsl_probe_fresh`] 强刷。
#[cfg(windows)]
static WSL_PROBE_CACHE: std::sync::Mutex<Option<(std::time::Instant, WslProbe)>> =
    std::sync::Mutex::new(None);

/// 缓存读（过期或空槽才真探）。
#[cfg(windows)]
fn wsl_probe_cached() -> WslProbe {
    let mut slot = WSL_PROBE_CACHE.lock().expect("wsl probe cache");
    if let Some((at, probe)) = slot.as_ref() {
        let ttl = match probe.state {
            WslState::Ready | WslState::MissingTools => std::time::Duration::from_secs(30),
            WslState::NoDistro | WslState::Unavailable => std::time::Duration::from_secs(5),
        };
        if at.elapsed() < ttl {
            return probe.clone();
        }
    }
    let fresh = wsl_spawn_probe();
    *slot = Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// 强刷（image_build 启动裁决用——启动瞬间的真实态优先于缓存）。
#[cfg(windows)]
fn wsl_probe_fresh() -> WslProbe {
    let fresh = wsl_spawn_probe();
    *WSL_PROBE_CACHE.lock().expect("wsl probe cache") =
        Some((std::time::Instant::now(), fresh.clone()));
    fresh
}

/// Windows 盘符路径 → WSL /mnt 形态（`C:\a\b` → `/mnt/c/a/b`）。
/// UNC / 非盘符路径诚实 Err（本管线用不到，不猜）。
pub(crate) fn windows_to_wsl_path(p: &Path) -> Result<String, String> {
    let s = p.to_string_lossy();
    let mut chars = s.chars();
    let Some(drive) = chars.next() else {
        return Err("空路径无法翻译为 WSL 形态".to_string());
    };
    if !drive.is_ascii_alphabetic() || chars.next() != Some(':') {
        return Err(format!(
            "非盘符路径（{s}）——WSL 路径翻译只支持「C:\\…」形态"
        ));
    }
    let rest = s[2..].trim_start_matches(['\\', '/']).replace('\\', "/");
    if rest.is_empty() {
        return Ok(format!("/mnt/{}", drive.to_ascii_lowercase()));
    }
    Ok(format!("/mnt/{}/{}", drive.to_ascii_lowercase(), rest))
}

/// image_route 六态（`bootstrap_status.image_route.state` 的值域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageRouteState {
    /// 非 Windows 宿主——bash 原生直跑。
    Native,
    /// 构建脚本不在场（仓库根约束不满足）。
    NoScript,
    /// Windows + WSL 工具齐备。
    WslReady,
    WslNoDistro,
    WslMissingTools,
    WslUnavailable,
}

/// route 分类纯函数（喂脚本在场性 + 探针结果；探针 None = 非 Windows）。
pub(crate) fn classify_image_route(
    script_present: bool,
    probe: Option<&WslProbe>,
) -> ImageRouteState {
    if !script_present {
        return ImageRouteState::NoScript;
    }
    match probe {
        None => ImageRouteState::Native,
        Some(p) => match p.state {
            WslState::Ready => ImageRouteState::WslReady,
            WslState::NoDistro => ImageRouteState::WslNoDistro,
            WslState::MissingTools => ImageRouteState::WslMissingTools,
            WslState::Unavailable => ImageRouteState::WslUnavailable,
        },
    }
}

// VM 资源查找链（2026-10-22 三拍：**没有资源目录**——镜像构建脚本与
// guest 全部内嵌在 plugin_vm.dll，用时经 `crate::vm_resources` 释放到
// `<workspace>/tools/vm/released/`）。候选链：① **释放态** = released
// 根（构建面先 `vm_resources::ensure_released` 触发释放；状态面只
// exists 探测、不触发写盘）；② 开发形态 = 仓库 plugins/plugin-vm/
// （相对 cwd，即从仓库根运行网关）。下面两个 `*_with` 是注入版纯函数
// （单测钉查找顺序）；运行时包装把 released 根与仓库开发路径喂进来。

/// 镜像构建脚本查找（注入版纯函数，单测钉查找顺序）：
/// ① 释放态 = released 根内 `build-vm-image.sh`；② 开发形态 = 仓库
/// plugins/plugin-vm/。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn vm_image_script_path_with(released: Option<&Path>, dev: &Path) -> Option<PathBuf> {
    if let Some(dir) = released {
        let p = dir.join("build-vm-image.sh");
        if p.is_file() {
            return Some(p);
        }
    }
    dev.is_file().then_some(dev.to_path_buf())
}

/// [`vm_image_script_path_with`] 的运行时包装（released 根 exists 探测 +
/// 仓库开发形态路径——**不触发释放**，状态面用；构建面的释放触发在
/// [`image_build_inputs`]）。
fn vm_image_script_path(workspace: &Path) -> Option<PathBuf> {
    vm_image_script_path_with(
        Some(crate::vm_resources::released_root(workspace).as_path()),
        &PathBuf::from("plugins")
            .join("plugin-vm")
            .join("build-vm-image.sh"),
    )
}

/// guest 产物候选路径（注入版纯函数，单测钉查找顺序与 arch 映射）：
/// ① 释放态 = released 根内 `guest/<短arch>/nemesis-guest`（布局与
/// build-guest.sh 的 `bin/bin_guest/<短arch>/` 一致）；② 开发形态 = 仓库
/// target 交叉编译产物（guest 命名跟 **target** 走不跟宿主走：
/// guest_target 恒为 linux-musl，cargo 产物恒无 .exe——build-guest.sh 的
/// .exe 只用于宿主冒烟件；B5 全新环境实跑抓出的 P5 遗留错配）。
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn guest_bin_candidates_with(
    released: Option<&Path>,
    arch: &str,
) -> Result<Vec<PathBuf>, String> {
    let target = match arch {
        "x86_64" => "x86_64-unknown-linux-musl",
        "aarch64" => "aarch64-unknown-linux-musl",
        other => {
            return Err(format!(
                "guest 架构 {other:?} 非法（validate 应已拦——数据不一致）"
            ));
        }
    };
    let mut v = Vec::new();
    if let Some(dir) = released {
        v.push(dir.join("guest").join(arch).join("nemesis-guest"));
    }
    v.push(
        PathBuf::from("target")
            .join(target)
            .join("release")
            .join("nemesis-guest"),
    );
    Ok(v)
}
/// arch → guest 产物路径（WSAPI/CLI 启动裁决与 image_route 展示共用——
/// 「能不能构建」与「卡上怎么显示」必须同一份事实）。按候选顺序取首个
/// 在场者；全缺席 = 诚实报错并列举候选 + 双形态指路。**不触发释放**
/// （released 根 exists 探测；构建面的释放触发在 [`image_build_inputs`]）。
fn guest_bin_path(workspace: &Path, arch: &str) -> Result<PathBuf, String> {
    let candidates = guest_bin_candidates_with(
        Some(crate::vm_resources::released_root(workspace).as_path()),
        arch,
    )?;
    if let Some(p) = candidates.iter().find(|p| p.exists()) {
        return Ok(p.clone());
    }
    Err(format!(
        "guest 代理二进制不在场（候选：{}）——安装 plugin-vm 插件包（plugins/plugin_vm.dll，guest 双架构 ELF 已内嵌、镜像构建时自动释放），或开发形态先跑 scripts/build-guest.sh 构建 musl guest 工具链产物",
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(" | ")
    ))
}

/// 镜像构建输入裁决（WSAPI 后台臂与 CLI 前台臂共用的**单一真相源**）：
/// 脚本在场 / arch→guest target 映射 / guest 二进制在场 /（Windows）WSL 路由。
pub(crate) struct ImageBuildInputs {
    /// spawn 主体：native = bash；WSL 路由 = wsl.exe。
    pub program: String,
    /// spawn 前置参（WSL = ["-e", "bash"]；native = 空）。
    pub pre_args: Vec<String>,
    /// 构建脚本路径（WSL 路由时为已翻译的 /mnt 形态）。
    pub script: PathBuf,
    /// guest 二进制路径（WSL 路由时为已翻译的 /mnt 形态）。
    pub guest_bin: PathBuf,
    pub arch: String,
    pub base_url: String,
    pub sha256: String,
    /// 镜像 variant（P5-R2 ② 接线）：config `desktop: "full"` →
    /// `--variant desktop`（带桌面镜像——display=vnc 的前置）；`minimal` →
    /// 空（脚本默认 base variant）。与 build-vm-image.sh 的
    /// VARIANT 值域同源。
    pub variant_arg: Vec<String>,
}

/// config `desktop` → 构建脚本 variant 参数（P5-R2 ② 接线；两臂同源消费）。
/// `full` = `["--variant", "desktop"]`；`minimal` = 空（脚本默认 base）。
pub(crate) fn variant_args(desktop: &str) -> Vec<String> {
    match desktop {
        "full" => vec!["--variant".to_string(), "desktop".to_string()],
        _ => Vec::new(),
    }
}

pub(crate) fn image_build_inputs(
    settings: &nemesis_config::ExecutorVmConfig,
    workspace: &Path,
) -> Result<ImageBuildInputs, String> {
    // ---- 资源释放触发（2026-10-22 三拍：内嵌资源用时释放——**只有
    // 这里**写盘；状态面全是 exists 探测）。插件缺席/不可用 = 记下现场，
    // 回落开发链；开发链也缺席时把释放现场一并报出（不许静默吞）。----
    let released = crate::vm_resources::ensure_released(workspace);
    let released_root = released.as_ref().ok().and_then(|o| o.as_deref());
    let released_err = match &released {
        Err(e) => Some(e.as_str()),
        Ok(_) => None,
    };
    let released_hint = || -> String {
        match released_err {
            Some(e) => format!("（插件资源释放失败：{e}）"),
            None => String::new(),
        }
    };

    // 构建脚本 + guest 二进制在场预检（两平台同口径；缺席 = 诚实报错 + 指路。
    // 查找链：released 释放态（<workspace>/tools/vm/released/，plugin_vm.dll
    // 内嵌资源用时释放）优先，仓库 plugins/plugin-vm/ 兜底（开发形态））。
    let script = vm_image_script_path_with(
        released_root,
        &PathBuf::from("plugins")
            .join("plugin-vm")
            .join("build-vm-image.sh"),
    )
    .ok_or_else(|| {
        format!(
            "镜像构建脚本不在场{}——安装 plugin-vm 插件包（plugins/plugin_vm.dll，脚本与 guest 已内嵌、首次构建自动释放），或开发形态从仓库根运行网关",
            released_hint()
        )
    })?;
    // released 根候选可能只有部分架构在场（构建时未内嵌的架构诚实缺席）——
    // 与 dev 候选一起按序找首个在场者。
    let guest_bin = {
        let candidates = guest_bin_candidates_with(released_root, &settings.arch)?;
        match candidates.iter().find(|p| p.exists()) {
            Some(p) => p.clone(),
            None => {
                return Err(format!(
                    "guest 代理二进制不在场（候选：{}）{}——安装 plugin-vm 插件包（plugins/plugin_vm.dll，guest 双架构 ELF 已内嵌、首次构建自动释放），或开发形态先跑 scripts/build-guest.sh 构建 musl guest 工具链产物",
                    candidates
                        .iter()
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    released_hint()
                ));
            }
        }
    };

    #[cfg(windows)]
    {
        // B7：Windows 宿主不再硬拒绝——前置同 Linux 口径，再探 WSL：
        // 就绪 = 自动经 `wsl.exe -e bash` 跑同一管线（产物路径翻译 /mnt 形态，
        // staging→promote 换装链复用）；任何一环缺席 = 诚实报错 + 可复制修复命令。
        // P5 既定边界保留：镜像一次构建双端复用，Linux 构建回传仍是等价出路。
        let probe = wsl_probe_fresh();
        match probe.state {
            WslState::Ready => {
                // 预检路径升绝对（相对形态 = 仓库 cwd join；plugin_vm 包形态
                // 本就绝对，join 幂等）再翻译 /mnt 形态；翻译收在 Ready 臂内
                // ——缺席态优先给指路文案，不被路径错误抢跑。
                let cwd = std::env::current_dir()
                    .map_err(|e| format!("无法定位当前目录（WSL 路径翻译需要绝对路径）：{e}"))?;
                let wsl_script = windows_to_wsl_path(&cwd.join(&script))?;
                let wsl_guest = windows_to_wsl_path(&cwd.join(&guest_bin))?;
                Ok(ImageBuildInputs {
                    program: "wsl.exe".to_string(),
                    pre_args: vec!["-e".to_string(), "bash".to_string()],
                    script: PathBuf::from(wsl_script),
                    guest_bin: PathBuf::from(wsl_guest),
                    arch: settings.arch.clone(),
                    base_url: settings.image.source.clone(),
                    sha256: settings.image.sha256.clone(),
                    variant_arg: variant_args(&settings.desktop),
                })
            }
            WslState::NoDistro => Err(
                "未检测到 WSL 发行版——终端执行 `wsl --install -d Ubuntu`（可能需要管理员与重启，完成后点「复检」），或在 Linux 机器构建后把 base.qcow2 落到镜像目录回传"
                    .to_string(),
            ),
            WslState::MissingTools => Err(if probe.has_apt {
                format!(
                    "WSL 发行版缺镜像构建工具（{}）——执行 `wsl -e bash -c \"sudo apt install -y qemu-utils file curl\"` 后点「复检」（烧录 seed 还需 root 权限）",
                    probe.missing.join("、")
                )
            } else {
                format!(
                    "WSL 发行版缺镜像构建工具（{}）——在发行版内安装齐后点「复检」",
                    probe.missing.join("、")
                )
            }),
            WslState::Unavailable => Err(format!(
                "WSL 不可用（{}）——可执行 `wsl --install` 启用（可能需要管理员与重启），或在 Linux 机器构建后把 base.qcow2 落到镜像目录回传",
                probe.reason
            )),
        }
    }
    #[cfg(not(windows))]
    {
        Ok(ImageBuildInputs {
            program: "bash".to_string(),
            pre_args: Vec::new(),
            script,
            guest_bin,
            arch: settings.arch.clone(),
            base_url: settings.image.source.clone(),
            sha256: settings.image.sha256.clone(),
            variant_arg: variant_args(&settings.desktop),
        })
    }
}

/// [`VmSupervisorError`] → WSAPI 字符串（带分类前缀，页面可区分「档没开」
/// 「身份已终结」「操作失败」三类——Unavailable 可回落、LifecycleRejected
/// 身份已终结禁回落（F1）、Failed 有副作用可能绝不回落）。
fn err_string(e: VmSupervisorError) -> String {
    match e {
        VmSupervisorError::Unavailable(m) => format!("不可用: {m}"),
        VmSupervisorError::LifecycleRejected(m) => format!("生命周期拒绝: {m}"),
        VmSupervisorError::Failed(m) => format!("失败: {m}"),
    }
}

/// 平台加速器探针（本机对应后端；其它平台 = 诚实「平台不支持」）。
fn platform_probe() -> (QemuAccel, AccelProbe) {
    #[cfg(target_os = "linux")]
    {
        (QemuAccel::Kvm, probe_kvm())
    }
    #[cfg(target_os = "windows")]
    {
        (QemuAccel::Whpx, probe_whpx())
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        (
            QemuAccel::Kvm,
            AccelProbe {
                available: false,
                layer: "none",
                reason: "当前平台无受支持加速器（仅 Linux KVM / Windows WHPX）".into(),
            },
        )
    }
}

/// accel 的默认睡眠线（三探针定谳：KVM=Snapshot；WHPX=None——savevm/
/// migrate 双双早验挂死，退路 = 冷停）。
fn sleep_line_note(accel: QemuAccel) -> (Option<&'static str>, String) {
    match SleepMode::default_for_accel(accel) {
        Some(SleepMode::Snapshot) => (
            Some("snapshot"),
            "savevm 快照线可用（睡眠/唤醒 RAM 状态保留）".into(),
        ),
        Some(SleepMode::MigrateFile) => {
            (Some("migrate-file"), "迁移文件线可用（接口预留线）".into())
        }
        None => (
            None,
            "无可用睡眠线（WHPX 三探针定谳）；退路 = 冷停（磁盘状态保留、RAM 丢）".into(),
        ),
    }
}

/// `executor.vm` 段写入（与 web handlers/sandbox.rs `update_executor`
/// 同款双路：gateway 全局 store → CLI 裸 RMW + 原子写。写串行锁罩全程）。
fn update_vm_config(home: &Path, vm: &nemesis_config::ExecutorVmConfig) -> Result<(), String> {
    let _cfg_serial = nemesis_config::config_write_lock();
    if let Some(store) = nemesis_config::global() {
        return store
            .update(|c| {
                c.executor.get_or_insert_with(Default::default).vm = vm.clone();
            })
            .map_err(|e| format!("update vm config: {e}"));
    }
    // CLI / no-store fallback: read-merge-write config.json（typed 解析保
    // 护 executor 段其余键，不整段覆盖）。
    let config_path = home.join("config.json");
    let raw =
        std::fs::read_to_string(&config_path).map_err(|e| format!("read config.json: {e}"))?;
    let mut val: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse config.json: {e}"))?;
    let existing = val.get("executor").and_then(|v| {
        serde_json::from_value::<nemesis_config::ExecutorSeparationConfig>(v.clone()).ok()
    });
    let obj = val
        .as_object_mut()
        .ok_or_else(|| "config.json is not a JSON object".to_string())?;
    let mut e = existing.unwrap_or_default();
    e.vm = vm.clone();
    obj.insert(
        "executor".into(),
        serde_json::to_value(&e).map_err(|e| format!("serialize executor: {e}"))?,
    );
    let out =
        serde_json::to_string_pretty(&val).map_err(|e| format!("serialize config.json: {e}"))?;
    nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), out.as_bytes(), 0o600)
        .map_err(|e| format!("write config.json: {e}"))?;
    Ok(())
}

/// 追加一行进构建日志（逐行 flush——页面轮询 tail 实时可见）。
fn log_line(path: &Path, line: &str) {
    use std::io::Write as _;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// 目录韧性删除（bot 侧本模块内联版：Windows AV/索引器瞬时句柄重试——
/// fsutil 韧性包装在 nemesis-cluster 内部 pub(crate)，不跨 crate）。残留
/// 只影响整洁度（下次构建覆盖同名文件），warn 不阻塞。
fn remove_dir_resilient(dir: &Path) {
    for _ in 0..3 {
        match std::fs::remove_dir_all(dir) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(300)),
        }
    }
    tracing::warn!(
        "[vm-bridge] staging 残留未清（不影响下次构建）: {}",
        dir.display()
    );
}

#[async_trait::async_trait]
impl VmBridge for VmSupervisor {
    fn overview(&self) -> serde_json::Value {
        let config = self.vm_config();
        let enabled = config.as_ref().is_some_and(|c| c.enabled);
        let (accel, probe) = platform_probe();
        let (sleep_line, sleep_note) = sleep_line_note(accel);

        // 档位建议：探针在 → 建议开（睡眠线状态如实附注）；探针缺 → 建议
        // 关（缺席原因明示——绝不替用户决定，建议不是决定）。
        let suggestion = if probe.available {
            serde_json::json!({
                "enabled": true,
                "note": format!(
                    "检测到 {}（{}），可开启 VM 档。睡眠线：{sleep_note}",
                    accel.as_str(),
                    probe.layer
                ),
            })
        } else {
            serde_json::json!({
                "enabled": false,
                "note": format!(
                    "未检测到 {}：{}。可保持关闭；开启后实例启动将诚实报不可用",
                    accel.as_str(),
                    probe.reason
                ),
            })
        };

        let used = disk_usage_bytes(self.workspace());
        let gc = match &config {
            Some(c) => {
                let d = gc_decision(used, &c.gc);
                serde_json::json!({
                    "used_gb": used / GIB,
                    "budget_gb": c.gc.max_total_disk_gb,
                    "decision": match d {
                        GcDecision::Allow => "allow",
                        GcDecision::Warn { .. } => "warn",
                        GcDecision::Deny { .. } => "deny",
                    },
                })
            }
            None => serde_json::Value::Null,
        };

        let instances: Vec<_> = self
            .instance_status_all()
            .iter()
            .map(|(r, running)| record_json(r, *running))
            .collect();

        let arch = config
            .as_ref()
            .map(|c| c.arch.clone())
            .unwrap_or_else(|| "x86_64".to_string());
        let qemu = GuestArch::parse(&arch).map_or_else(
            |_| serde_json::Value::Null,
            |g| {
                serde_json::json!(
                    resolve_qemu_binary(
                        g,
                        crate::agents::vm_runtime::qemu_install_dir(self.workspace()).as_deref(),
                    )
                    .to_string_lossy()
                )
            },
        );

        serde_json::json!({
            "enabled": enabled,
            "config": config.as_ref().map(|c| serde_json::to_value(c).unwrap_or(serde_json::Value::Null)),
            "probe": {
                "platform": std::env::consts::OS,
                "accel": accel.as_str(),
                "available": probe.available,
                "layer": probe.layer,
                "reason": probe.reason,
            },
            "sleep_line": sleep_line,
            "sleep_line_note": sleep_note,
            "suggestion": suggestion,
            "gc": gc,
            "instances": instances,
            "qemu_binary": qemu,
            "images_root": vm_images_root(self.workspace()).to_string_lossy(),
        })
    }

    fn config_get(&self) -> Option<serde_json::Value> {
        self.vm_config().and_then(|c| serde_json::to_value(c).ok())
    }

    fn config_set(&self, vm: serde_json::Value) -> Result<(), String> {
        let typed: nemesis_config::ExecutorVmConfig =
            serde_json::from_value(vm).map_err(|e| format!("executor.vm 配置解析失败: {e}"))?;
        // 值域校验与配置装载同源（ExecutorVmConfig::validate 单一真相源）。
        typed
            .validate()
            .map_err(|e| format!("executor.vm 配置校验失败: {e}"))?;
        update_vm_config(&self.config_home(), &typed)
    }

    fn image_list(&self) -> serde_json::Value {
        let mut images = Vec::new();
        let root = vm_images_root(self.workspace());
        if let Ok(rd) = std::fs::read_dir(&root) {
            let mut entries: Vec<_> = rd.flatten().collect();
            entries.sort_by_key(|e| e.file_name());
            for entry in entries {
                let Ok(meta) = entry.metadata() else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                images.push(serde_json::json!({
                    "name": entry.file_name().to_string_lossy(),
                    "size_bytes": meta.len(),
                    "modified_ms": meta
                        .modified()
                        .ok()
                        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_millis() as u64),
                }));
            }
        }
        let job = IMAGE_JOB.lock().expect("image job lock").as_ref().map(|j| {
            serde_json::json!({ "running": true, "started_ms": j.started_ms, "update": j.update })
        });
        // 日志尾部（页面进度可见性；最多 20 行）。
        let log_path = image_build_log_path(self.workspace());
        let tail: Vec<String> = std::fs::read_to_string(&log_path)
            .map(|raw| {
                raw.lines()
                    .rev()
                    .take(20)
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();
        let log_size = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
        serde_json::json!({
            "images_root": root.to_string_lossy(),
            "images": images,
            "build": job.unwrap_or(serde_json::json!({ "running": false })),
            "log": { "path": log_path.to_string_lossy(), "size_bytes": log_size, "tail": tail },
        })
    }

    async fn image_build(&self, update: bool) -> Result<serde_json::Value, String> {
        let settings = self.vm_settings().ok_or_else(|| {
            "VM 档未开启（executor.vm.enabled=false）：镜像构建不可用".to_string()
        })?;

        // 输入裁决（与 CLI 前台臂共用的单一真相源）——内嵌资源释放触发点。
        let inputs = image_build_inputs(&settings, self.workspace())?;

        // 单飞闸 + 任务登记。
        {
            let mut job = IMAGE_JOB.lock().expect("image job lock");
            if job.is_some() {
                return Err("已有镜像构建任务进行中（进度看 vm.image.list 日志尾部）".to_string());
            }
            *job = Some(ImageJob {
                started_ms: now_ms(),
                update,
            });
        }

        let workspace = self.workspace().to_path_buf();

        tokio::spawn(async move {
            run_image_build(
                workspace,
                inputs.program,
                inputs.pre_args,
                inputs.script,
                inputs.guest_bin,
                inputs.base_url,
                inputs.sha256,
                inputs.arch,
                inputs.variant_arg,
                update,
            )
            .await;
        });

        Ok(serde_json::json!({
            "started": true,
            "update": update,
            "note": "构建任务已后台启动（进度/结果看 vm.image.list 的 log.tail；完成后自动换装，存量 overlay 安全 rebase）",
        }))
    }

    fn instance_list(&self) -> Vec<serde_json::Value> {
        self.instance_status_all()
            .iter()
            .map(|(r, running)| record_json(r, *running))
            .collect()
    }

    async fn instance_sleep(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.sleep_agent(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "slept": true,
            "note": "VM 已按默认睡眠线保存 RAM 状态并退出；下一次启动将从快照/状态文件唤醒",
        }))
    }

    async fn instance_wake(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        // 唤醒线路径预读（记录在 wake 过程中被改写——先取 wake_kind 用于
        // 应答注记；记录缺席 = 冷启语义，与 ensure_running 内部判定一致）。
        let line = load_wake_line(self.workspace(), agent_id);
        self.ensure_running(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "woken": true,
            "wake_line": line,
            "note": match line.as_deref() {
                Some("snapshot") => "已从 qcow2 内部快照唤醒（RAM 状态恢复）",
                Some("migrate-file") => "已从迁移状态文件唤醒（RAM 状态恢复）",
                _ => "冷启动（无睡眠产物可用；磁盘状态保留）",
            },
        }))
    }

    async fn instance_kill(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.stop_agent(agent_id).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": agent_id,
            "stopped": true,
            "note": "VM 已冷停（QEMU 退出，overlay 保留——下次启动同盘冷启）",
        }))
    }

    async fn gc_run(&self) -> serde_json::Value {
        let swept = self.sweep_orphans().await;
        // P5-R4：手动 GC 面同吃磁盘垃圾回收（孤儿 overlay / 换装残留）。
        let disk_gc = self.sweep_disk_garbage();
        let used = disk_usage_bytes(self.workspace());
        let decision = self.vm_config().map(|c| match gc_decision(used, &c.gc) {
            GcDecision::Allow => serde_json::json!({ "verdict": "allow" }),
            GcDecision::Warn { used_gb, budget_gb } => {
                serde_json::json!({ "verdict": "warn", "used_gb": used_gb, "budget_gb": budget_gb })
            }
            GcDecision::Deny { used_gb, budget_gb } => {
                serde_json::json!({ "verdict": "deny", "used_gb": used_gb, "budget_gb": budget_gb })
            }
        });
        serde_json::json!({
            "swept_orphans": swept,
            "swept_disk_garbage": disk_gc,
            "used_gb": used / GIB,
            "decision": decision,
        })
    }

    fn bootstrap_status(&self) -> serde_json::Value {
        let workspace = self.workspace();
        let arch = self
            .vm_config()
            .map(|c| c.arch.clone())
            .unwrap_or_else(|| "x86_64".to_string());
        let guest = GuestArch::parse(&arch).unwrap_or(GuestArch::X86_64);

        // QEMU 在场三态（install-dir 优先，与 resolve_qemu_binary 消费序同源）。
        let presence = probe::classify_qemu_presence(
            guest,
            crate::agents::vm_runtime::qemu_install_dir(workspace).as_deref(),
        );
        let qemu = match &presence {
            probe::QemuPresence::Absent => serde_json::json!({ "state": "absent" }),
            probe::QemuPresence::Partial { missing } => serde_json::json!({
                "state": "partial",
                "missing": missing,
            }),
            probe::QemuPresence::Ready { source } => serde_json::json!({
                "state": "ready",
                "source": source,
            }),
        };

        // 加速器引导八态（事实采集 → 纯函数分类 → 每态可执行下一步）。
        let facts = probe::collect_accel_facts();
        let boot = probe::classify_accel_bootstrap(&facts);
        let accel = match &boot {
            probe::AccelBootstrap::Ok => serde_json::json!({ "state": "ok" }),
            probe::AccelBootstrap::NeedsEnableWindows => serde_json::json!({
                "state": "needs_enable_windows",
                "action": "button",
                "command": format!("DISM {}", install::dism_enable_whpx_args().join(" ")),
                "note": "WHPX 功能未启用——页面一键发起（UAC 提权，用户确认）",
            }),
            probe::AccelBootstrap::NeedsReboot => serde_json::json!({
                "state": "needs_reboot",
                "note": "功能已启用但挂起重启——重启 Windows 后复检",
            }),
            probe::AccelBootstrap::NeedsBios => serde_json::json!({
                "state": "needs_bios",
                "note": "功能已启用但 WHPX 仍缺席——进 BIOS 开启虚拟化（Intel VT-x / AMD-V）",
            }),
            probe::AccelBootstrap::NeedsKmod { module } => serde_json::json!({
                "state": "needs_kmod",
                "command": install::modprobe_kvm_command(module),
                "note": "KVM 内核模块未加载——终端执行后复检",
            }),
            probe::AccelBootstrap::NeedsGroup => serde_json::json!({
                "state": "needs_group",
                "command": install::usermod_kvm_command("$USER"),
                "note": "/dev/kvm 当前用户不可写——加入 kvm 组并重新登录后复检",
            }),
            probe::AccelBootstrap::Unsupported => serde_json::json!({
                "state": "unsupported",
                "note": format!("平台 {} 无受支持加速器（仅 Linux KVM / Windows WHPX）", facts.os),
            }),
            probe::AccelBootstrap::Unknown { reason } => serde_json::json!({
                "state": "unknown",
                "reason": reason,
            }),
        };

        // 基础镜像在场（build 链产出的正式位）+ 构建路由六态（B7：Windows
        // 宿主经 WSL 自动构建的可达面——「能不能点」与「缺什么怎么补」
        // 同一份事实，B2 八态同款每态给可执行下一步）。
        let image_ready = vm_images_root(workspace).join("base.qcow2").exists();
        let image_route = image_route_json(workspace, &arch);

        // 安装任务槽 + 日志尾（同 image_list 形态）。
        let job = BOOTSTRAP_JOB
            .lock()
            .expect("bootstrap job lock")
            .as_ref()
            .map(|j| serde_json::json!({ "running": true, "started_ms": j.started_ms }));
        let log_path = bootstrap_log_path(workspace);
        let tail: Vec<String> = std::fs::read_to_string(&log_path)
            .map(|raw| {
                raw.lines()
                    .rev()
                    .take(20)
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect()
            })
            .unwrap_or_default();

        serde_json::json!({
            "enabled": self.vm_config().as_ref().is_some_and(|c| c.enabled),
            "platform": std::env::consts::OS,
            "qemu": qemu,
            "accel": accel,
            "image_ready": image_ready,
            "image_route": image_route,
            "install": job.unwrap_or(serde_json::json!({ "running": false })),
            "log": { "path": log_path.to_string_lossy(), "tail": tail },
        })
    }

    async fn bootstrap_install_qemu(&self) -> Result<serde_json::Value, String> {
        let enabled = self.vm_config().as_ref().is_some_and(|c| c.enabled);
        if !enabled {
            return Err(
                "VM 档未开启（executor.vm.enabled=false）：环境安装不可达——先在总开关开启后重试"
                    .to_string(),
            );
        }
        let workspace = self.workspace().to_path_buf();
        {
            let mut job = BOOTSTRAP_JOB.lock().expect("bootstrap job lock");
            if job.is_some() {
                return Err(
                    "已有 QEMU 安装任务进行中（进度看 vm.bootstrap.status 日志尾部）".to_string(),
                );
            }
            *job = Some(BootstrapJob {
                started_ms: now_ms(),
            });
        }
        tokio::spawn(async move {
            run_qemu_install(workspace).await;
        });
        Ok(serde_json::json!({
            "started": true,
            "note": "QEMU 安装任务已后台启动（进度/结果看 vm.bootstrap.status 的 log.tail；完成后自动转绿）",
        }))
    }

    async fn bootstrap_enable_accel(&self) -> Result<serde_json::Value, String> {
        let enabled = self.vm_config().as_ref().is_some_and(|c| c.enabled);
        if !enabled {
            return Err(
                "VM 档未开启（executor.vm.enabled=false）：加速器引导不可达——先在总开关开启后重试"
                    .to_string(),
            );
        }

        // Windows：WHPX DISM 提权链（fire-and-forget UAC——网关不自我提权；
        // 完成态恒由探针复检裁决）。Linux：分类 → 引导命令原文返回（不替
        // 用户跑 sudo——半自动诚实纪律）。
        #[cfg(target_os = "windows")]
        {
            let msg = install::enable_accel_windows().map_err(|e| format!("{e:#}"))?;
            return Ok(serde_json::json!({
                "platform": "windows",
                "message": msg,
                "note": "完成后用 vm.bootstrap.status 复检（功能挂起重启则需重启 Windows）",
            }));
        }
        #[cfg(target_os = "linux")]
        {
            let facts = probe::collect_accel_facts();
            let boot = probe::classify_accel_bootstrap(&facts);
            let guidance = match boot {
                probe::AccelBootstrap::Ok => serde_json::json!({
                    "state": "ok",
                    "note": "加速器已就绪（无需引导）",
                }),
                probe::AccelBootstrap::NeedsKmod { module } => serde_json::json!({
                    "state": "needs_kmod",
                    "command": install::modprobe_kvm_command(module),
                    "note": "KVM 内核模块未加载——终端执行后复检",
                }),
                probe::AccelBootstrap::NeedsGroup => serde_json::json!({
                    "state": "needs_group",
                    "command": install::usermod_kvm_command("$USER"),
                    "note": "加入 kvm 组需重新登录生效",
                }),
                probe::AccelBootstrap::NeedsReboot => serde_json::json!({
                    "state": "needs_reboot",
                    "note": "功能已启用但挂起重启——重启后复检",
                }),
                probe::AccelBootstrap::NeedsBios => serde_json::json!({
                    "state": "needs_bios",
                    "note": "进 BIOS 开启虚拟化（Intel VT-x / AMD-V）",
                }),
                other => serde_json::json!({
                    "state": format!("{other:?}"),
                    "note": "当前态无自动引导动作（见 bootstrap.status 全量诊断）",
                }),
            };
            return Ok(serde_json::json!({
                "platform": "linux",
                "guidance": guidance,
            }));
        }
        #[cfg(not(any(target_os = "windows", target_os = "linux")))]
        {
            let _ = enabled;
            Err("当前平台无受支持加速器（仅 Linux KVM / Windows WHPX）".to_string())
        }
    }

    fn try_status(&self) -> serde_json::Value {
        let config = self.vm_config();
        let enabled = config.as_ref().is_some_and(|c| c.enabled);
        let arch = config
            .as_ref()
            .map(|c| c.arch.clone())
            .unwrap_or_else(|| "x86_64".to_string());
        let arch_supported = arch == "x86_64";
        let workspace = self.workspace();
        let images = vm_images_root(workspace);
        let image_ready = images.join("base.qcow2").exists();
        let running = self.running_agents().iter().any(|a| a == TRY_AGENT_ID);
        let (starting, last_error) = self.try_launch_snapshot();
        serde_json::json!({
            "agent_id": TRY_AGENT_ID,
            "enabled": enabled,
            "image_ready": image_ready,
            "arch": arch,
            "arch_supported": arch_supported,
            "running": running,
            "starting": starting,
            "last_error": last_error,
        })
    }

    async fn try_start(&self) -> Result<serde_json::Value, String> {
        try_gate(self)?;
        if self.running_agents().iter().any(|a| a == TRY_AGENT_ID) {
            return Ok(serde_json::json!({
                "agent_id": TRY_AGENT_ID,
                "running": true,
                "starting": false,
                "note": "试玩实例已在运行（复用现有实例）。",
            }));
        }
        match self.try_launch_claim() {
            // 生产装配（gateway 注入自引用）：后台冷启 + 发起即返回——前端
            // 轮询 try.status 的 starting/running 转绿。真机教训（2026-10-19）：
            // 冷启动同步等 30-60s 必撞前端 30s WSAPI 超时（用户看到
            // 「timeout: vm.try.start」弹窗，而 QEMU 实际已成功启动）。
            Some(sup) => {
                tokio::spawn(async move {
                    let outcome = sup.ensure_running(TRY_AGENT_ID).await;
                    let err = outcome.err().map(err_string);
                    sup.try_launch_finish(err);
                    // 启动期间收到停止请求：就绪后立即补停（中止卡在启动的实例）。
                    if sup.try_launch_take_stop_request()
                        && sup.running_agents().iter().any(|a| a == TRY_AGENT_ID)
                        && let Err(e) = sup.stop_agent(TRY_AGENT_ID).await
                    {
                        tracing::warn!("[vm-bridge] 试玩启动后补停失败: {}", err_string(e));
                    }
                });
                Ok(serde_json::json!({
                    "agent_id": TRY_AGENT_ID,
                    "running": false,
                    "starting": true,
                    "note": "冷启动中（首启注入约 1-2 分钟）：状态卡将自动转绿，期间可离开本页。",
                }))
            }
            // 句柄缺席（单测等非常规装配）：同步回落旧语义（阻塞到就绪）。
            None => {
                tracing::warn!(
                    "[vm-bridge] 试玩后台启动句柄缺席（set_self_arc 未接线？）——同步冷启动回落（长请求）"
                );
                self.ensure_running(TRY_AGENT_ID)
                    .await
                    .map_err(err_string)?;
                Ok(serde_json::json!({
                    "agent_id": TRY_AGENT_ID,
                    "running": true,
                    "starting": false,
                    "note": "试玩实例已就绪（冷启动完成，guest agent 在线）。实例受同一套睡眠线管理：闲置后自动睡眠省资源。",
                }))
            }
        }
    }

    async fn try_exec(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        try_gate(self)?;
        // 启动在途诚实拒绝：exec 的自动拉起与后台启动任务并发 ensure 会
        // 双开 QEMU 抢同一 overlay（qcow2 锁冲突）——引导用户等转绿。
        if self.try_launch_snapshot().0 {
            return Err(
                "试玩实例启动中（冷启动约 1-2 分钟）——待状态转「运行中」后再执行命令".to_string(),
            );
        }
        // 自动拉起（幂等）：未运行先 ensure_running——与「启动按钮」同一
        // 通路，失败诚实上抛（镜像缺/加速缺/满员闸各自原样报）。
        let slot = self
            .ensure_running(TRY_AGENT_ID)
            .await
            .map_err(err_string)?;
        // B8c：可选超时透传（缺省与 guest EXEC_DEFAULT_TIMEOUT_SECS 同源
        // 60s——B8e 把 guest 默认 30→60 后 host 若还按 30 预算，60s 长命令
        // 撞 host 墙钟先死；上限 600s 与 guest 硬上限同源钳制，handler 已钳
        // 一次，这里兜底防御）。
        let guest_timeout = timeout_secs.unwrap_or(60).clamp(1, 600);
        // 单调用信封预算 = guest 命令超时 + 握手/协议余量；缺省维持 60s。
        let call_timeout = std::time::Duration::from_secs(guest_timeout + 30).max(TRY_CALL_TIMEOUT);
        // 每调用独立握手（execute_persistent 同款：跨睡眠的常驻流是伪状态）。
        let mut stream = {
            let s = slot.lock().await;
            nemesis_sandbox::vm::lifecycle::auth_handshake(
                s.vm.agent_addr,
                &s.vm.token,
                TRY_CALL_TIMEOUT,
            )
            .await
            .map_err(|e| format!("不可用: VM guest 通道握手失败: {e}"))?
        };
        let args = serde_json::json!({ "command": command, "timeout": guest_timeout }).to_string();
        let resp = nemesis_sandbox::vm::lifecycle::guest_call(
            &mut stream,
            "exec",
            &args,
            &serde_json::json!({ "source": "dashboard-quick-try" }),
            call_timeout,
        )
        .await
        .map_err(|e| format!("失败: VM guest 调用失败: {e}"))?;
        if resp.ok {
            Ok(serde_json::json!({
                "agent_id": TRY_AGENT_ID,
                "command": command,
                "timeout_secs": guest_timeout,
                "output": resp.result,
            }))
        } else {
            Err(format!("失败: VM 内命令执行出错: {}", resp.error))
        }
    }

    async fn try_stop(&self) -> Result<serde_json::Value, String> {
        // 启动在途：登记停止请求，就绪后由启动任务补停（中止卡在启动的
        // 实例——用户「启动卡死了想停」的真实诉求；检查与登记同临界区）。
        if self.try_launch_stop_if_starting() {
            return Ok(serde_json::json!({
                "agent_id": TRY_AGENT_ID,
                "running": false,
                "starting": true,
                "note": "启动中收到停止请求：实例就绪后将立即自动停机。",
            }));
        }
        // 幂等：未运行 = Ok 带「未运行」注记（试玩卡的停止按钮不该报错）。
        if !self.running_agents().iter().any(|a| a == TRY_AGENT_ID) {
            return Ok(serde_json::json!({
                "agent_id": TRY_AGENT_ID,
                "running": false,
                "note": "试玩实例未在运行（无需停止；磁盘状态保留）",
            }));
        }
        self.stop_agent(TRY_AGENT_ID).await.map_err(err_string)?;
        Ok(serde_json::json!({
            "agent_id": TRY_AGENT_ID,
            "running": false,
            "note": "试玩实例已冷停（QEMU 退出，overlay 保留——下次启动同盘冷启）",
        }))
    }

    // -----------------------------------------------------------------
    // B9-3 项目工作副本同步（WSAPI vm.sync.status / vm.commit / vm.resync
    // 同一面；CLI vm commit/resync 走 /api/internal 转发到同一桥）。
    // -----------------------------------------------------------------

    async fn sync_status(&self, project: &str) -> Result<serde_json::Value, String> {
        let entry = resolve_project_entry(self.workspace(), project)?;
        let report =
            crate::vm_sync::sync_status_report(self.workspace(), &entry.id, &entry.path).await?;
        serde_json::to_value(report).map_err(|e| format!("状态序列化失败: {e}"))
    }

    async fn sync_commit(&self, project: &str) -> Result<serde_json::Value, String> {
        let (entry, agent_id) = resolve_sync_target(self.workspace(), project)?;
        match crate::vm_sync::commit_project(
            self.workspace(),
            &entry.id,
            &entry.path,
            self,
            &agent_id,
        )
        .await?
        {
            Some(report) => {
                serde_json::to_value(report).map_err(|e| format!("报告序列化失败: {e}"))
            }
            None => Ok(serde_json::json!({
                "no_baseline": true,
                "note": "项目尚未开工同步（无基线）——先给项目会话发一条消息触发开工同步",
            })),
        }
    }

    async fn sync_resync(&self, project: &str) -> Result<serde_json::Value, String> {
        let (entry, agent_id) = resolve_sync_target(self.workspace(), project)?;
        // force_full=true：跳过基线对账直接全量重建（VM 工作副本清空后
        // 以宿主当前内容整体覆盖——调用方须已向用户披露丢失语义）。
        // 正常路径恒 Fresh；其余臂是防御性穷举（内部锁串行下不会出现）。
        match crate::vm_sync::open_work_gate(
            self.workspace(),
            &entry.id,
            &entry.path,
            self,
            &agent_id,
            true,
        )
        .await?
        {
            crate::vm_sync::GateSync::Fresh(report) => {
                serde_json::to_value(report).map_err(|e| format!("报告序列化失败: {e}"))
            }
            crate::vm_sync::GateSync::Ready { synced_at_ms } => Ok(serde_json::json!({
                "kind": "ready",
                "synced_at_ms": synced_at_ms,
                "note": "内部形态（重同步请求落了增量就绪臂）",
            })),
            crate::vm_sync::GateSync::Aligned { pushed, deleted } => {
                Ok(serde_json::json!({ "kind": "aligned", "pushed": pushed, "deleted": deleted }))
            }
            crate::vm_sync::GateSync::Conflicts(files) => Ok(serde_json::json!({
                "kind": "conflicts",
                "files": files,
            })),
        }
    }
}

/// B6 快速试玩固定 agent_id（骑既有 supervisor 全套机制——registry/槽位/
/// 睡眠线/满员闸与 agent 实例完全同构，`vm.instance.list` 如实可见）。
pub(crate) const TRY_AGENT_ID: &str = "try-dashboard";

/// 试玩单次调用墙钟预算（握手 + exec；预设命令均秒级，30s guest 侧超时
/// 之外留出协议余量——超时 = 诚实失败，不是常态路径）。
const TRY_CALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// 试玩 start/exec 公共前置闸（enabled → arch → 镜像在场），失败文案
/// 带下一步指引（先开总开关 / 换 x86_64 档 / 去环境安装卡补齐）。
fn try_gate(sup: &VmSupervisor) -> Result<(), String> {
    let settings = sup.vm_settings().ok_or_else(|| {
        "VM 档未开启（executor.vm.enabled=false）：试玩不可用——先在配置页开启总开关".to_string()
    })?;
    if settings.arch != "x86_64" {
        return Err(format!(
            "试玩仅支持 x86_64 档（当前 {}）：aarch64 镜像管线尚未产出，属接口预留",
            settings.arch
        ));
    }
    let images = vm_images_root(sup.workspace());
    if !images.join("base.qcow2").exists() {
        return Err(
            "基础镜像未就绪（base.qcow2 缺席）——先在环境安装卡完成 QEMU 安装与镜像构建".to_string(),
        );
    }
    Ok(())
}

/// wake 线路预读（`None` = 无记录/无产物 → 冷启）。
fn load_wake_line(workspace: &Path, agent_id: &str) -> Option<String> {
    let record = load_record(workspace, agent_id)?;
    match wake_kind_of(&record) {
        WakeKind::FromSnapshot(_) => Some("snapshot".to_string()),
        WakeKind::FromState(_) => Some("migrate-file".to_string()),
        WakeKind::Cold => None,
    }
}

/// B9-3 同步目标解析（按项目 id 精确 → 名称回退）。返回注册表条目：
/// `entry.id` 是同步账本的键、`entry.path` 是同步根。
pub(super) fn resolve_project_entry(
    workspace: &Path,
    project: &str,
) -> Result<crate::projects::registry::ProjectEntry, String> {
    let reg = crate::projects::registry::registry_path(workspace);
    if let Some(e) = crate::projects::registry::find_project(&reg, project) {
        return Ok(e);
    }
    crate::projects::registry::list_projects(&reg)
        .into_iter()
        .find(|e| e.name == project)
        .ok_or_else(|| format!("项目不存在: {project}（id 或名称皆未命中注册表）"))
}

/// B9-3 commit/resync 目标全解析：项目条目 + VM 档会话的 agent_id。
/// VM 实例常驻 gateway 进程——agent 记录不在（管理器未装配 / 会话未建）
/// 或非 VM 档 = 诚实报错，不假装同步了。
pub(super) fn resolve_sync_target(
    workspace: &Path,
    project: &str,
) -> Result<(crate::projects::registry::ProjectEntry, String), String> {
    let entry = resolve_project_entry(workspace, project)?;
    let mgr = crate::agents::manager().ok_or_else(|| {
        "Agent 管理器未装配（gateway 未运行或以非 gateway 模式装配）——VM 实例常驻 gateway 进程，手动同步需经 gateway".to_string()
    })?;
    let rec = mgr
        .agent_by_project(&entry.id)
        .ok_or_else(|| format!("项目会话未运行（{}）——先在项目会话发消息建会话", entry.name))?;
    if rec.runtime_backend != nemesis_taskstore::agents::runtime_backend::VM {
        return Err(format!(
            "项目会话不是 VM 档（当前 {} 档）——工作副本同步只服务 VM 档会话",
            if rec.runtime_backend.is_empty() {
                "process"
            } else {
                &rec.runtime_backend
            }
        ));
    }
    Ok((entry, rec.agent_id))
}

/// registry 记录 → web 投影（token 不投影——凭据不出管理面）。
fn record_json(
    r: &crate::agents::vm_runtime::VmInstanceRecord,
    running: bool,
) -> serde_json::Value {
    serde_json::json!({
        "agent_id": r.agent_id,
        "running": running,
        "sleep_mode": r.sleep_mode,
        "asleep_at": r.asleep_at,
        "arch": r.arch,
        "accel": r.accel,
        "vcpu": r.vcpu,
        "ram_mb": r.ram_mb,
        "disk_gb": r.disk_gb,
        "has_snapshot": r.snapshot_tag.is_some(),
        "has_state_file": r.state_path.is_some(),
        // P5-R2 显示面：display_port 在场（运行中实例）= 前端「查看屏幕」
        // 入口的能力依据之一（配合 config display + 插件在位探测）。
        "display_port": r.display_port,
    })
}

/// `bootstrap_status.image_route` 段（B2 八态同款：事实采集 → 纯函数分类
/// → 每态给可执行下一步 + 复制命令）。分类逻辑在
/// [`classify_image_route`]（纯函数，单测钉六态）；本函数只做「探针取事实 +
/// 每态 JSON 形态」。Windows 探针走 TTL 缓存（轮询面不重复 spawn）。
fn image_route_json(workspace: &Path, arch: &str) -> serde_json::Value {
    let script_present = vm_image_script_path(workspace).is_some();
    let guest_ready = guest_bin_path(workspace, arch)
        .map(|p| p.exists())
        .unwrap_or(false);

    #[cfg(windows)]
    let probe: Option<WslProbe> = Some(wsl_probe_cached());
    #[cfg(not(windows))]
    let probe: Option<WslProbe> = None;

    let route = classify_image_route(script_present, probe.as_ref());
    let probe_ref = probe.as_ref();
    // WSL 两态必伴探针（classify 契约，单测钉死）；defensive unwrap 走
    // 诚实形态不 panic。
    let (state, missing, command, note) = match route {
        ImageRouteState::NoScript => (
            "no_script",
            Vec::<String>::new(),
            String::new(),
            "构建脚本不在场——安装 plugin-vm 插件包（plugins/plugin_vm.dll，镜像脚本与 guest 双架构 ELF 已内嵌、首次构建自动释放），或开发形态从仓库根运行网关，或在 Linux 宿主构建后把 base.qcow2 落到镜像目录回传".to_string(),
        ),
        ImageRouteState::Native => (
            "native",
            Vec::<String>::new(),
            String::new(),
            "Linux 宿主原生管线——「首次构建」直接执行".to_string(),
        ),
        ImageRouteState::WslReady => (
            "wsl_ready",
            Vec::<String>::new(),
            String::new(),
            "WSL 环境就绪——「首次构建」自动经 WSL 跑同一管线，产物直落镜像目录"
                .to_string(),
        ),
        ImageRouteState::WslNoDistro => (
            "wsl_no_distro",
            Vec::<String>::new(),
            "wsl --install -d Ubuntu".to_string(),
            "未检测到 WSL 发行版——终端执行后（可能需要管理员与重启）点「复检」"
                .to_string(),
        ),
        ImageRouteState::WslMissingTools => {
            let empty = WslProbe {
                state: WslState::MissingTools,
                missing: Vec::new(),
                has_apt: false,
                reason: String::new(),
            };
            let p = probe_ref.unwrap_or(&empty);
            (
                "wsl_missing_tools",
                p.missing.clone(),
                if p.has_apt {
                    "wsl -e bash -c \"sudo apt install -y qemu-utils file curl\"".to_string()
                } else {
                    String::new()
                },
                "装完后点「复检」，转就绪即可构建".to_string(),
            )
        }
        ImageRouteState::WslUnavailable => {
            let empty = WslProbe::unavailable("探测态缺失（内部状态不一致）");
            let p = probe_ref.unwrap_or(&empty);
            (
                "wsl_unavailable",
                Vec::<String>::new(),
                "wsl --install".to_string(),
                format!("{}；或改在 Linux 宿主构建后把产物落镜像目录回传", p.reason),
            )
        }
    };

    serde_json::json!({
        "state": state,
        "missing": missing,
        "command": command,
        "note": note,
        "guest_ready": guest_ready,
    })
}

/// 镜像构建后台任务：脚本 spawn → 日志逐行落盘 → 成功后 rebase/换装 →
/// 任务槽清位。任何失败只进日志（页面 tail 可见），不 panic。
/// B7：spawn 主体由 inputs 裁决（native bash / 经 `wsl.exe -e bash`——
/// Windows 宿主自动路由）；staging 传给子进程前翻译 /mnt 形态，宿主侧
/// promote 换装链仍用原生形态（字节不变）。
async fn run_image_build(
    workspace: PathBuf,
    program: String,
    pre_args: Vec<String>,
    script: PathBuf,
    guest_bin: PathBuf,
    base_url: String,
    sha256: String,
    arch: String,
    variant_arg: Vec<String>,
    update: bool,
) {
    let log_path = image_build_log_path(&workspace);
    let _ = std::fs::create_dir_all(log_path.parent().unwrap_or_else(|| Path::new(".")));
    let _ = std::fs::remove_file(&log_path); // 截断重写
    let route_note = if program == "wsl.exe" {
        "经 WSL"
    } else {
        "native bash"
    };
    log_line(
        &log_path,
        &format!(
            "[{}] 镜像构建启动（update={update} arch={arch} route={route_note}）",
            now_ms()
        ),
    );

    let staging = staging_dir(&workspace);
    let _ = std::fs::create_dir_all(&staging);
    let staging_arg = if program == "wsl.exe" {
        match windows_to_wsl_path(&staging) {
            Ok(p) => PathBuf::from(p),
            Err(e) => {
                log_line(
                    &log_path,
                    &format!("staging 路径无法翻译为 WSL 形态：{e}；任务结束"),
                );
                IMAGE_JOB.lock().expect("image job lock").take();
                return;
            }
        }
    } else {
        staging.clone()
    };

    let mut cmd = tokio::process::Command::new(&program);
    for a in &pre_args {
        cmd.arg(a);
    }
    cmd.arg(&script)
        .arg("--out")
        .arg(&staging_arg)
        .arg("--guest-bin")
        .arg(&guest_bin)
        .arg("--base-url")
        .arg(&base_url)
        .arg("--arch")
        .arg(&arch);
    // P5-R2 ②：desktop=full → --variant desktop（带桌面镜像——display=vnc
    // 的前置；minimal = 脚本默认 base variant）。
    for a in &variant_arg {
        cmd.arg(a);
    }
    if !sha256.is_empty() {
        cmd.arg("--sha256").arg(&sha256);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    log_line(&log_path, "spawn 构建脚本…");
    let status = match cmd.spawn() {
        Ok(mut child) => {
            let mut stdout = child.stdout.take().expect("piped stdout");
            let mut stderr = child.stderr.take().expect("piped stderr");
            let out_reader = async {
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(&mut stdout).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    log_line(&log_path, &l);
                }
            };
            let err_reader = async {
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(&mut stderr).lines();
                while let Ok(Some(l)) = lines.next_line().await {
                    log_line(&log_path, &format!("[stderr] {l}"));
                }
            };
            let (_, _, status) = tokio::join!(out_reader, err_reader, child.wait());
            status
        }
        Err(e) => {
            log_line(
                &log_path,
                &format!(
                    "spawn {program} 失败: {e}（Windows 宿主需 git-bash/WSL 在 PATH）；任务结束"
                ),
            );
            IMAGE_JOB.lock().expect("image job lock").take();
            return;
        }
    };
    let Ok(status) = status else {
        log_line(&log_path, "构建脚本等待失败；任务结束");
        IMAGE_JOB.lock().expect("image job lock").take();
        return;
    };
    if !status.success() {
        log_line(
            &log_path,
            &format!("构建脚本退出码非零（{status}）；staging 保留供排查，现有镜像未动；任务结束"),
        );
        IMAGE_JOB.lock().expect("image job lock").take();
        return;
    }
    log_line(&log_path, "构建产物就绪（staging）——开始换装/rebase");

    // 换装前置闸（[`IMAGE_SWAP_GATE`] 两相协议的 promote 侧）：持闸期间
    // 无新 spawn（start 路径同闸排队），闸内活体全量扫描拒绝运行中/启动
    // 中的实例（pidfile 身份核验 + spawn 在途标记）——rebase 写 overlay
    // 与运行中 QEMU 并发 = qcow2 损坏；删旧 base 撞运行中实例的打开句柄
    // = Windows 共享违例。拒绝诚实且有出路（睡眠/停机后重试），staging
    // 与现有镜像都原位保留。
    let _swap_gate = crate::agents::vm_runtime::IMAGE_SWAP_GATE.lock().await;
    if crate::agents::vm_runtime::any_live_qemu(&workspace) {
        log_line(
            &log_path,
            "检测到运行中/启动中的 VM 实例——换装拒绝（rebase 不得触碰运行中实例的 overlay/base）：先睡眠或停机全部实例再重试；staging 保留、现有镜像未动；任务结束",
        );
        IMAGE_JOB.lock().expect("image job lock").take();
        return;
    }
    if let Err(e) = promote_staging(&workspace, &staging, &log_path).await {
        log_line(
            &log_path,
            &format!("换装失败：{e}；staging 与现有镜像均保持原位（数据无损），任务结束"),
        );
    } else {
        remove_dir_resilient(&staging);
        log_line(&log_path, &format!("{IMAGE_BUILD_DONE_LINE}；任务结束"));
    }
    IMAGE_JOB.lock().expect("image job lock").take();
}

/// QEMU 安装后台任务（B3 自举族）：平台分派 → 日志逐行落盘 → 任务槽清位。
/// 幂等（已装 = already_installed 早退）；失败只进日志（页面 tail 可见），
/// 不 panic。成功后探针复检由 bootstrap_status 轮询自然转绿。
async fn run_qemu_install(workspace: PathBuf) {
    let log_path = bootstrap_log_path(&workspace);
    let _ = std::fs::create_dir_all(log_path.parent().unwrap_or_else(|| Path::new(".")));
    let _ = std::fs::remove_file(&log_path); // 截断重写
    log_line(&log_path, &format!("[{}] QEMU 安装启动", now_ms()));

    let result = do_qemu_install(&workspace, &log_path).await;
    match result {
        Ok(report) => {
            log_line(
                &log_path,
                &format!(
                    "安装完成：{}（{}）already_installed={}",
                    report.qemu_system.display(),
                    report.version,
                    report.already_installed
                ),
            );
        }
        Err(e) => log_line(&log_path, &format!("安装失败：{e:#}")),
    }
    BOOTSTRAP_JOB.lock().expect("bootstrap job lock").take();
    log_line(&log_path, "任务结束");
}

/// 平台分派的安装链执行体（日志内联进任务函数，这里只产结果）。
async fn do_qemu_install(
    workspace: &Path,
    log_path: &Path,
) -> anyhow::Result<install::QemuInstallReport> {
    #[cfg(target_os = "windows")]
    {
        let runtime = workspace.join("tools").join("vm").join("runtime");
        let opts = install::QemuInstallOptions {
            install_dir: runtime.join("qemu"),
            work_dir: runtime.join("install-work"),
            // 复用 sandbox 的 7z 缓存（同一份 7z 服务两条安装链，零重复下载）。
            seven_zip_dir: workspace.join("tools").join("sandboxie").join("runtime"),
            ..Default::default()
        };
        log_line(
            log_path,
            &format!("Windows 链：下载 {} → 校验 → 7z 直解 → 落位", opts.url),
        );
        install::install_qemu_windows(&opts).await
    }
    #[cfg(target_os = "linux")]
    {
        let _ = workspace; // Linux 链经包管理器装系统路径，不落 workspace
        log_line(
            log_path,
            "Linux 链：包管理器探测 → 免密 sudo 自动装 / 诚实半自动",
        );
        install::install_qemu_linux().await
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        let _ = (workspace, log_path);
        anyhow::bail!(
            "当前平台（{}）无自动安装链（仅 Windows / Linux）",
            std::env::consts::OS
        )
    }
}

/// staging 产物 → 正式位 + 存量 overlay 安全 rebase（数据完好次序：
/// ① 新 base 落临时名 `base.new.qcow2` → ② 每个存量 overlay `qemu-img
/// rebase -b base.new`（safe 模式：旧 backing 经原路径读旧内容、差集
/// 拷进 overlay、改指针）→ ③ 删旧 base、新 base 转正 → ④ 每 overlay
/// `-u` 重定向指针到转正路径（④失败时 rename 已执行、临时名已不在——
/// 自动把转正 base 拷回临时名恢复落点，拷贝失败才留手动修复指引）。
/// 任何一步失败：指针都有落点，全部实例仍可启动（诚实报错不留悬空）。
async fn promote_staging(workspace: &Path, staging: &Path, log_path: &Path) -> Result<(), String> {
    let images = vm_images_root(workspace);
    let new_base_src = staging.join("base.qcow2");
    if !new_base_src.exists() {
        return Err("staging 缺 base.qcow2（构建产物不完整）".to_string());
    }
    let new_base = images.join("base.new.qcow2");
    // GB 级镜像同步拷贝 → spawn_blocking（不占 async executor 线程）。
    let cp_src = new_base_src.clone();
    let cp_dst = new_base.clone();
    tokio::task::spawn_blocking(move || std::fs::copy(&cp_src, &cp_dst))
        .await
        .map_err(|e| format!("新 base 拷贝任务失败: {e}"))?
        .map_err(|e| format!("新 base 落临时名失败: {e}"))?;

    // manifest.json 先就位（不依赖 rebase 成败——留痕面；seed 已烧进
    // base，镜像目录不再有独立 seed 介质）。
    let src = staging.join("manifest.json");
    if src.exists() {
        std::fs::copy(&src, images.join("manifest.json"))
            .map_err(|e| format!("manifest.json 就位失败: {e}"))?;
    }

    // 存量 overlay 清单（旧 base 在位才有 rebase 语义）。显式排序——
    // read_dir 顺序是平台实现细节，rebase 日志序不应随文件系统漂移。
    let overlays: Vec<PathBuf> = std::fs::read_dir(vm_overlays_root(workspace))
        .map(|rd| {
            let mut paths: Vec<PathBuf> = rd
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "qcow2"))
                .collect();
            paths.sort();
            paths
        })
        .unwrap_or_default();
    let qemu_img = resolve_qemu_img(workspace);

    if !overlays.is_empty() && images.join("base.qcow2").exists() {
        log_line(
            log_path,
            &format!(
                "存量 overlay {} 个——安全 rebase（safe 模式，差集拷贝）",
                overlays.len()
            ),
        );
        for ov in &overlays {
            let out = tokio::process::Command::new(&qemu_img)
                .arg("rebase")
                .arg("-b")
                .arg(&new_base)
                // 新版 qemu-img 换 backing 强制显式格式（B7 真机走查实锤：
                // 缺 -F 报 "backing format must be specified"；overlay 全族
                // 均为本管线 create -F qcow2 产物，格式恒 qcow2）。
                .arg("-F")
                .arg("qcow2")
                .arg(ov)
                .output()
                .await
                .map_err(|e| format!("qemu-img rebase 启动失败: {e}"))?;
            if !out.status.success() {
                return Err(format!(
                    "overlay {} rebase 失败（{}）",
                    ov.display(),
                    String::from_utf8_lossy(&out.stderr).trim()
                ));
            }
            log_line(log_path, &format!("rebase 完成: {}", ov.display()));
        }
    }

    // ③ 删旧 base → 新 base 转正 → ④ 指针重定向到转正路径。
    let canonical = images.join("base.qcow2");
    if canonical.exists() {
        std::fs::remove_file(&canonical).map_err(|e| format!("旧 base 移除失败: {e}"))?;
    }
    std::fs::rename(&new_base, &canonical).map_err(|e| format!("新 base 转正失败: {e}"))?;
    for ov in &overlays {
        let out = tokio::process::Command::new(&qemu_img)
            .arg("rebase")
            .arg("-u")
            .arg("-b")
            .arg(&canonical)
            // 同上：-u 只改指针也要求显式 backing 格式。
            .arg("-F")
            .arg("qcow2")
            .arg(ov)
            .output()
            .await
            .map_err(|e| format!("qemu-img rebase -u 启动失败: {e}"))?;
        if !out.status.success() {
            // ④ 失败时 rename 已执行——base.new.qcow2 已转正不复存在，该
            // overlay 指针真实状态 = 悬空。恢复「指针都有落点」不变量：把
            // 转正后的 base 拷回临时名（GB 级 → spawn_blocking；字节相同，
            // 悬空指针重新有效）；拷贝失败则诚实给手动修复命令（此时
            // base.qcow2 在场）。
            let restore = tokio::task::spawn_blocking({
                let canonical = canonical.clone();
                let new_base = new_base.clone();
                move || std::fs::copy(&canonical, &new_base).map(|_| ())
            })
            .await;
            let restore_err = match &restore {
                Ok(Ok(())) => None,
                Ok(Err(e)) => Some(e.to_string()),
                Err(e) => Some(e.to_string()),
            };
            if restore_err.is_none() {
                log_line(
                    log_path,
                    &format!("指针落点已恢复: {} → {}", new_base.display(), ov.display()),
                );
            }
            return Err(format!(
                "overlay {} 指针重定向失败（{}）——{}",
                ov.display(),
                String::from_utf8_lossy(&out.stderr).trim(),
                promote_step4_failure_message(ov, &canonical, restore_err.as_deref())
            ));
        }
    }
    Ok(())
}

/// ④ 臂失败的收尾消息（纯函数，单测钉两态）：恢复成功 = 诚实告知落点
/// 已恢复、实例可启动；恢复失败 = 带手动修复命令（此时转正 base 在场，
/// `rebase -u` 指过去即可修复）。
pub(crate) fn promote_step4_failure_message(
    overlay: &Path,
    canonical: &Path,
    restore_err: Option<&str>,
) -> String {
    let restore_note = match restore_err {
        None => {
            "已自动恢复落点（base.new.qcow2 重新在场，数据与转正 base 相同），该 overlay 可启动"
                .to_string()
        }
        Some(e) => format!(
            "落点自动恢复失败（{e}）——手动修复：qemu-img rebase -u -F qcow2 -b \"{}\" \"{}\"",
            canonical.display(),
            overlay.display()
        ),
    };
    format!("{restore_note}；修复后重跑换装即可继续")
}
