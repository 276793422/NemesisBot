//! 宿主加速器 / QEMU 二进制可用性探测（诚实缺席——缺席要带原因明示）。
//!
//! 探测分层（渐进收紧，假阳性越来越少）：
//! 1. [`kvm_available`] / [`whpx_dll_present`]：纯静态必要条件（文件/模块
//!    在场）——零成本，缺席即终判不可用；
//! 2. [`qemu_version_ok`]：二进制自身实跑 trivial 探针（对齐仓库 diff
//!    纪律「依赖外部工具：二进制在场 ≠ 可用，用前以工具自身实跑」）；
//! 3. WHPX/KVM 的**真启动**验证（QEMU 以目标 accel 实际起一台机器）归
//!    第 2 步 VM 装配路径——DLL/设备在场只是必要条件，最终裁决在真启动。
//!
//! 全部探测**只在 VM 档激活路径与 Dashboard 探针横幅调用**；VM 总开关
//! 关闭态（铁律）零调用、零进程、零 IO。

use std::path::{Path, PathBuf};

/// KVM 可用的静态必要条件：`/dev/kvm` 存在。仅 Linux 有意义；其他平台
/// 恒 false（诚实——非 Linux 不探测）。
pub fn kvm_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/dev/kvm").exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// WHPX 可用的静态必要条件：`WinHvPlatform.dll` 可加载（Windows Hypervisor
/// Platform API 在场）。非 Windows 恒 false。**非充分条件**——DLL 在场但
/// hypervisor 未运行（BIOS 关虚拟化 / Hyper-V 未启用）时仍不可用，真启动
/// 裁决归 QEMU 装配路径（分层探测第 3 层）。
pub fn whpx_dll_present() -> bool {
    #[cfg(target_os = "windows")]
    {
        // LoadLibraryW 而非 GetModuleHandleW：DLL 未随本进程加载过时
        // GetModuleHandle 假阴性。加载后立即释放句柄（无副作用）。
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new("WinHvPlatform.dll")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: 传入 NUL 结尾的宽字符串字面量副本；加载系统 DLL 无副作用。
        let handle =
            unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide.as_ptr()) };
        if !handle.is_null() {
            // SAFETY: handle 来自成功的 LoadLibraryW，FreeLibrary 配对释放
            // （FreeLibrary 挂在 Foundation 模块——windows-sys 0.59 布局）。
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(handle);
            }
            true
        } else {
            false
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// QEMU 二进制实跑 trivial 探针：`<qemu> --version` 退出码 0 = 二进制在场
/// 且可执行（返回版本首行）。错误信息带 stderr 现场（诊断自包含纪律）。
pub async fn qemu_version_ok(qemu: &Path) -> Result<String, String> {
    let out = tokio::process::Command::new(qemu)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("QEMU 二进制不可执行 {}: {e}", qemu.display()))?;
    if !out.status.success() {
        return Err(format!(
            "QEMU --version 退出码非 0（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().next().unwrap_or("").to_string())
}

/// accel 静态探测汇总（Dashboard 探针横幅与启动前置闸共用；`reason` 缺席
/// 带人读原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelProbe {
    pub available: bool,
    /// 探测到哪一层（static-dll / static-dev / none）。
    pub layer: &'static str,
    /// 缺席原因（available=true 时空串）。
    pub reason: String,
}

/// KVM 探测汇总。
pub fn probe_kvm() -> AccelProbe {
    if kvm_available() {
        AccelProbe {
            available: true,
            layer: "static-dev",
            reason: String::new(),
        }
    } else {
        AccelProbe {
            available: false,
            layer: "none",
            reason: "/dev/kvm 不存在（非 Linux 宿主或 KVM 未启用）".to_string(),
        }
    }
}

/// WHPX 探测汇总（静态层）。
pub fn probe_whpx() -> AccelProbe {
    if whpx_dll_present() {
        AccelProbe {
            available: true,
            layer: "static-dll",
            reason: String::new(),
        }
    } else {
        AccelProbe {
            available: false,
            layer: "none",
            reason: "WinHvPlatform.dll 不可加载（Windows Hypervisor Platform 功能未启用）"
                .to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// B2：环境自举状态机（安装卡三步的渲染态——每态带可执行下一步）
// ---------------------------------------------------------------------------

use super::install::{CpuVendor, DismFeatureState};

/// QEMU 二进制在场状态（安装卡第 1 步）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QemuPresence {
    /// 安装目录与 PATH 都没有（→ 一键安装）。
    Absent,
    /// 关键文件缺（半装态/系统装了个瘸的——missing 列缺什么）。
    Partial { missing: Vec<String> },
    /// 就绪（qemu-system + qemu-img 都在场；可执行性仍由
    /// [`qemu_version_ok`] 实跑裁决——在场 ≠ 可用）。
    Ready {
        /// "install-dir"（自举链落位）| "msys2"（MSYS2 CLANGARM64 包落位）
        /// | "path"（用户/包管理器自装）。
        source: &'static str,
    },
}

/// PATH / 常规目录里找一个可执行文件的**所在目录**（对齐 crate 的 backend
/// which 惯例：显式常见路径优先，PATH 扫描兜底；找到 = 目录——安装装后
/// 探针与在场分类同源消费）。
pub fn find_binary_dir_on_path(name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    for candidate in ["/usr/bin", "/bin", "/usr/local/bin"] {
        let p = Path::new(candidate).join(name);
        if p.is_file() {
            return p.parent().map(Path::to_path_buf);
        }
    }
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let p = dir.join(name);
            if p.is_file() {
                return Some(dir);
            }
        }
    }
    None
}

/// QEMU 在场分类（安装卡第 1 步渲染态；二进制名单与探针目录全部出自矩阵
/// 条目 [`super::qemu_config::QemuPlatformEntry`]——安装与探针同源消费，
/// 不再硬编码）。探测顺序：
/// 1) `install_dir`（自举链落位目录，优先——对齐
///    [`super::assembly::resolve_qemu_binary`] 顺序）；
/// 2) pacman 条目（`pacman` 字段非空）→ MSYS2 CLANGARM64 落位目录
///    （`<msys2>/clangarm64/bin`，source=`"msys2"`）；
/// 3) PATH（用户/包管理器自装，source=`"path"`）。
pub fn classify_qemu_presence(
    entry: &super::qemu_config::QemuPlatformEntry,
    install_dir: Option<&Path>,
) -> QemuPresence {
    // binaries 空（kind=none 登记形态）= 无可断言，诚实 Absent。
    let Some(sys_name) = entry.binaries.first() else {
        return QemuPresence::Absent;
    };
    // 单目录齐套检查：[0] 在场 + 其余成员齐 = Ready；缺 = Partial（目录不
    // 命中 = None 交给下一探针位）。
    let check_dir = |dir: &Path, source: &'static str| -> Option<QemuPresence> {
        if !dir.join(sys_name).exists() {
            return None;
        }
        let missing: Vec<String> = entry
            .binaries
            .iter()
            .skip(1)
            .filter(|n| !dir.join(n).exists())
            .cloned()
            .collect();
        Some(if missing.is_empty() {
            QemuPresence::Ready { source }
        } else {
            QemuPresence::Partial { missing }
        })
    };

    // 1) 自举链落位目录优先。
    if let Some(p) = install_dir.and_then(|dir| check_dir(dir, "install-dir")) {
        return p;
    }

    // 2) pacman 条目：MSYS2 CLANGARM64 落位目录（windows 臂；MSYS2 缺席 =
    //    诚实跳过不伪装）。
    #[cfg(target_os = "windows")]
    if !entry.pacman.trim().is_empty()
        && let Some(p) = super::install::locate_msys2_root()
            .and_then(|msys2| check_dir(&msys2.join("clangarm64").join("bin"), "msys2"))
    {
        return p;
    }

    // 3) PATH 命中（用户/包管理器自装）——img 缺席同诚实 Partial。
    if let Some(p) = find_binary_dir_on_path(sys_name).and_then(|dir| check_dir(&dir, "path")) {
        return p;
    }
    QemuPresence::Absent
}

/// 加速器引导态（安装卡第 2 步——每态 = 一个可执行下一步）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccelBootstrap {
    /// 加速可用（静态层；真启动裁决仍归装配路径——分层探测纪律不变）。
    Ok,
    /// Windows：WHPX 功能未启用 → DISM 一键（runas UAC）。
    NeedsEnableWindows,
    /// Windows：启用已挂起 → 需重启 Windows 生效。
    NeedsReboot,
    /// Windows：功能已启用但 WHPX DLL 仍缺席 → BIOS 虚拟化或 hypervisor
    /// 冲突，软件无解，指路 BIOS（Intel VT-x / AMD-V）。
    NeedsBios,
    /// Linux：`/dev/kvm` 缺席且模块未载 → modprobe 引导（按 CPU vendor
    /// 选 kvm_intel / kvm_amd）。
    NeedsKmod { module: &'static str },
    /// Linux：`/dev/kvm` 在场但当前进程不可写 → kvm 组成员引导。
    NeedsGroup,
    /// 平台不支持（非 Windows 非 Linux）。
    Unsupported,
    /// 事实采集不完整 / 矛盾（模块已载但设备缺席等）——诚实未知，带原因。
    Unknown { reason: String },
}

/// 加速器引导态的事实面（采集器产出；分类器纯消费——单测钉死每态）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostAccelFacts {
    /// `std::env::consts::OS`（"windows" / "linux" / 其他）。
    pub os: &'static str,
    /// `/dev/kvm` 在场（Linux）。
    pub kvm_dev_exists: bool,
    /// `/dev/kvm` 当前进程可写（Linux；open(O_RDWR) 实证）。
    pub kvm_writable: bool,
    /// kvm 内核模块已载（kvm_intel / kvm_amd；Linux）。
    pub kvm_module_loaded: Option<&'static str>,
    /// CPU vendor（Linux；/proc/cpuinfo）。
    pub cpu_vendor: Option<CpuVendor>,
    /// WHPX DLL 可加载（Windows）。
    pub whpx_dll_present: bool,
    /// DISM 功能状态（Windows；查询不到 = None——非提权上下文常态）。
    pub dism_feature_state: Option<DismFeatureState>,
}

/// 加速器引导态分类（**纯函数**——事实面进，引导态出；单测表驱动钉死
/// 每态。采集与判定分离，让双平台状态机在任何宿主上可全测）。
pub fn classify_accel_bootstrap(f: &HostAccelFacts) -> AccelBootstrap {
    match f.os {
        "windows" => {
            if f.whpx_dll_present {
                return AccelBootstrap::Ok;
            }
            match f.dism_feature_state {
                // 功能已启用仍缺席：BIOS 虚拟化关闭或 hypervisor 冲突。
                Some(DismFeatureState::Enabled) => AccelBootstrap::NeedsBios,
                // 已挂起启用：差一次重启。
                Some(DismFeatureState::EnablePending) => AccelBootstrap::NeedsReboot,
                // 未启用 / 查询不到（非提权常态）：默认按未启用引导
                // （一键 DISM 本身无副作用，BIOS 缺席时它会失败，引导文案
                // 已覆盖两条路）。
                Some(DismFeatureState::Disabled) | None => AccelBootstrap::NeedsEnableWindows,
            }
        }
        "linux" => {
            if f.kvm_dev_exists {
                return if f.kvm_writable {
                    AccelBootstrap::Ok
                } else {
                    AccelBootstrap::NeedsGroup
                };
            }
            match f.kvm_module_loaded {
                // 模块载了设备却没有：矛盾态，诚实未知（不猜）。
                Some(m) => AccelBootstrap::Unknown {
                    reason: format!("内核模块 {m} 已载但 /dev/kvm 缺席——检查 udev / 内核版本"),
                },
                None => match f.cpu_vendor {
                    Some(v) => AccelBootstrap::NeedsKmod {
                        module: super::install::kvm_module_for_vendor(v),
                    },
                    None => AccelBootstrap::Unknown {
                        reason: "/dev/kvm 缺席且 CPU vendor 未知——无法给出 modprobe 引导"
                            .to_string(),
                    },
                },
            }
        }
        _ => AccelBootstrap::Unsupported,
    }
}

/// 事实采集（当前宿主；零进程化——文件存在性 / 权限位 / /proc 文本 /
/// LoadLibrary，最重一次 `sudo -n` 级别都没有）。
pub fn collect_accel_facts() -> HostAccelFacts {
    let mut f = HostAccelFacts {
        os: std::env::consts::OS,
        ..Default::default()
    };
    #[cfg(target_os = "linux")]
    {
        let dev = Path::new("/dev/kvm");
        f.kvm_dev_exists = dev.exists();
        if f.kvm_dev_exists {
            // 可写性用 open(O_RDWR) 实证（权限位 + uid/gid 推导是猜，
            // 打开是裁决；kvm fd 立即释放，无副作用）。
            f.kvm_writable = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(dev)
                .is_ok();
        }
        if let Ok(modules) = std::fs::read_to_string("/proc/modules") {
            f.kvm_module_loaded = if modules.contains("kvm_intel ") {
                Some("kvm_intel")
            } else if modules.contains("kvm_amd ") {
                Some("kvm_amd")
            } else {
                None
            };
        }
        if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
            f.cpu_vendor = super::install::parse_cpu_vendor(&cpuinfo);
        }
    }
    #[cfg(target_os = "windows")]
    {
        f.whpx_dll_present = whpx_dll_present();
    }
    f
}

#[cfg(test)]
mod tests;
