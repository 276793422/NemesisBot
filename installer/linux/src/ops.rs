//! Linux 安装/卸载运营链（计划 §5/§6）：
//! - 纯内容构造器（.desktop / systemd unit / PATH 片段）跨平台编译，
//!   测试在任意主机可跑；
//! - IO/进程面（statvfs / pgrep / symlink / systemctl）`#[cfg(target_os = "linux")]`，
//!   只在 Linux 编译与执行（Windows 上本 crate 走 stub main——诚实降级）。
//!
//! 纪律：
//! - 快捷方式三件套 = `~/.local/bin` 符号链接 + XDG .desktop + PATH 注记
//!   （`~/.profile` 带哨兵注释块，幂等可识别）；
//! - systemd user unit 是 --autostart 可选项（best-effort：systemctl 缺席
//!   → WARN 跳过，绝不失败整个安装）；
//! - 运行实例闸 `pgrep -x nemesisbot`；磁盘闸 statvfs（探测最深存在祖先，
//!   与 windows ops 同纪律）；
//! - 卸载副本 `uninstall`（无扩展名，平台惯例）由核心引擎落位；
//! - 退出码契约与 windows 形态一致：0/1/2/3/4/5/6（ops::exit_code_of）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use setup_core::install::{InstallPlan, TypedFail};

// ---------- 纯内容构造器（跨平台可测） ----------

/// `~/.profile` PATH 注记哨兵（幂等识别标记）。
pub const PATH_MARKER_BEGIN: &str = "# >>> NemesisBot installer PATH >>>";
pub const PATH_MARKER_END: &str = "# <<< NemesisBot installer PATH <<<";

/// PATH 注记块（~/.local/bin 前置）。
pub fn path_snippet() -> String {
    format!("{PATH_MARKER_BEGIN}\nexport PATH=\"$HOME/.local/bin:$PATH\"\n{PATH_MARKER_END}\n")
}

/// 是否需要追加 PATH 注记（已含哨兵 = 已写过；PATH 环境已精确含
/// `local_bin` = 不需要；形近路径不算）。
pub fn needs_path_append(path_env: &str, profile_content: &str, local_bin: &str) -> bool {
    if profile_content.contains(PATH_MARKER_BEGIN) {
        return false;
    }
    !path_env.split(':').any(|p| p == local_bin)
}

/// XDG 桌面入口内容。
pub fn desktop_entry_text(exec_path: &Path, icon_path: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=NemesisBot\n\
         Comment=NemesisBot Dashboard\n\
         Exec={} dashboard\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=Development;\n",
        shell_quote(&exec_path.to_string_lossy()),
        icon_path.display()
    )
}

/// systemd user unit 内容（Restart=on-failure 韧性重启）。
pub fn systemd_unit_text(exec_path: &Path) -> String {
    format!(
        "[Unit]\n\
         Description=NemesisBot Gateway\n\
         After=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         ExecStart={} gateway\n\
         Restart=on-failure\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        shell_quote(&exec_path.to_string_lossy())
    )
}

/// shell 单引号字面转义（' → '\''）。
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 安装计划人读文本（与 windows ops::plan_text 同内容——两平台文案一致性）。
pub fn plan_text(plan: &InstallPlan) -> String {
    match plan {
        InstallPlan::Fresh => "全新安装".into(),
        InstallPlan::Repair(v) => format!("同版本修复重装（当前 {v}）"),
        InstallPlan::Upgrade(v) => format!("覆盖升级（当前 {v}）"),
        InstallPlan::Downgrade { current, incoming } => {
            format!("降级（当前 {current} → 待装 {incoming}，需 --force）")
        }
    }
}

/// XDG 数据目录下的缺省安装根（XDG_DATA_HOME 尊重 → $HOME/.local/share）。
pub fn default_install_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Ok(PathBuf::from(xdg).join("NemesisBot"));
    }
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home).join(".local/share").join("NemesisBot"))
}

/// `~/.local/bin` 符号链接目标路径。
pub fn local_bin_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home).join(".local/bin/nemesisbot"))
}

/// `~/.profile` 路径。
pub fn profile_path() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home).join(".profile"))
}

/// 自身所在目录（uninstall 副本 = 安装目录）。
pub fn own_install_dir() -> Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("无法定位自身目录")
}

/// 自身目录是否带 install.json（uninstall 副本直接运行启发式 → 卸载模式）。
pub fn own_dir_has_install_record() -> bool {
    own_install_dir()
        .map(|d| d.join("install.json").exists())
        .unwrap_or(false)
}

/// XDG 应用入口路径。
pub fn desktop_entry_path() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME")
        && !xdg.trim().is_empty()
    {
        return Ok(PathBuf::from(xdg).join("applications/nemesisbot.desktop"));
    }
    let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
    Ok(PathBuf::from(home)
        .join(".local/share/applications")
        .join("nemesisbot.desktop"))
}

/// systemd user unit 路径（XDG_CONFIG_HOME 尊重 → $HOME/.config）。
pub fn systemd_unit_path() -> Result<PathBuf> {
    let base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let base = match base {
        Some(b) => PathBuf::from(b),
        None => {
            let home = std::env::var("HOME").context("HOME 环境变量缺席")?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("systemd/user/nemesisbot.service"))
}

// ---------- 运营层类型化失败（与 windows 同构） ----------

/// 类型化失败（退出码 3/4/6 域；2/5/1 域归引擎 TypedFail）。
#[derive(Debug)]
pub enum OpsFail {
    /// bot 正在运行。
    Running(Vec<String>),
    /// 磁盘空间不足。
    DiskSpace { needed: u64, free: u64 },
    /// 用户在向导/确认处取消。
    Cancelled,
}

impl std::fmt::Display for OpsFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpsFail::Running(list) => write!(
                f,
                "检测到 NemesisBot 正在运行（PID: {}），请先全部退出",
                list.join(", ")
            ),
            OpsFail::DiskSpace { needed, free } => write!(
                f,
                "磁盘空间不足：需要约 {:.1} MB（含 20% 余量），目标卷仅剩 {:.1} MB",
                *needed as f64 / 1_048_576.0,
                *free as f64 / 1_048_576.0
            ),
            OpsFail::Cancelled => write!(f, "操作已取消"),
        }
    }
}
impl std::error::Error for OpsFail {}

/// 退出码映射（与 windows 形态同契约：0/1/2/3/4/5/6）。
pub fn exit_code_of(err: &anyhow::Error) -> i32 {
    if let Some(tf) = err.downcast_ref::<TypedFail>() {
        return match tf {
            TypedFail::Integrity(_) => 2,
            TypedFail::SigRisk(_) => 5,
            TypedFail::DowngradeRejected { .. } => 1,
        };
    }
    if let Some(of) = err.downcast_ref::<OpsFail>() {
        return match of {
            OpsFail::Running(_) => 6,
            OpsFail::DiskSpace { .. } => 4,
            OpsFail::Cancelled => 3,
        };
    }
    1
}

// ---------- Linux IO/进程面 ----------

/// pgrep 输出解析（纯函数，`running_bots` 的解析层——测试喂假 pgrep 文本
/// 即可模拟任意 PID 清单，不 spawn 真进程；跨平台可测）。
pub fn parse_pgrep_output(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// 运行中的 nemesisbot PID 清单（`pgrep -x nemesisbot`；无匹配 = 空）。
#[cfg(target_os = "linux")]
pub fn running_bots() -> Result<Vec<String>> {
    let out = std::process::Command::new("pgrep")
        .args(["-x", "nemesisbot"])
        .output()
        .context("pgrep 执行失败")?;
    Ok(parse_pgrep_output(&String::from_utf8_lossy(&out.stdout)))
}

/// 运行实例检测注入点（生产 = [`running_bots`]；测试注入假 PID 清单——
/// 离线模拟全生命周期，不依赖机器上有无真实实例在跑）。
pub type RunningCheck<'a> = &'a dyn Fn() -> Result<Vec<String>>;

/// 目标路径所在卷剩余字节数（statvfs；目标不存在时探最深存在祖先）。
#[cfg(target_os = "linux")]
pub fn free_disk_bytes(path: &Path) -> Result<u64> {
    let mut probe = path.to_path_buf();
    loop {
        if probe.exists() {
            break;
        }
        match probe.parent() {
            Some(p) if !p.as_os_str().is_empty() => probe = p.to_path_buf(),
            _ => {
                return Err(anyhow!(
                    "无法定位既有祖先目录用于磁盘查询: {}",
                    path.display()
                ));
            }
        }
    }
    let c =
        std::ffi::CString::new(probe.to_string_lossy().as_bytes()).context("路径含 NUL 字节")?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
    if rc != 0 {
        return Err(anyhow!("statvfs 失败: {}", probe.display()));
    }
    Ok(st.f_bavail as u64 * st.f_frsize as u64)
}

/// 安装所需磁盘字节数（× 1.2 余量，与 windows 形态一致）。
pub fn required_disk_bytes(manifest_total: u64) -> u64 {
    manifest_total + manifest_total / 5
}

/// 幂等重建符号链接（已指向同目标 = 不动；否则摘旧重建）。
#[cfg(target_os = "linux")]
pub fn install_symlink(link: &Path, target: &Path) -> Result<()> {
    use std::os::unix::fs::symlink;
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("建目录失败: {}", parent.display()))?;
    }
    // 幂等：已是正确符号链接则跳过。
    if let Ok(meta) = std::fs::symlink_metadata(link)
        && meta.file_type().is_symlink()
        && std::fs::read_link(link).is_ok_and(|p| p == target)
    {
        return Ok(());
    }
    let _ = std::fs::remove_file(link);
    symlink(target, link)
        .with_context(|| format!("建符号链接失败: {} → {}", link.display(), target.display()))
}

/// 写 XDG .desktop 入口。
#[cfg(target_os = "linux")]
pub fn install_desktop_entry(install_dir: &Path) -> Result<PathBuf> {
    let exec = install_dir.join("nemesisbot");
    let icon = install_dir.join("share/icon.png");
    let path = desktop_entry_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, desktop_entry_text(&exec, &icon))
        .with_context(|| format!("写桌面入口失败: {}", path.display()))?;
    Ok(path)
}

/// 写 systemd user unit + daemon-reload + enable（best-effort：systemctl
/// 缺席 = WARN 跳过）。返回警告（空 = 完全成功）。
#[cfg(target_os = "linux")]
pub fn install_autostart(install_dir: &Path) -> Result<Vec<String>> {
    let mut warnings = Vec::new();
    let exec = install_dir.join("nemesisbot");
    let path = systemd_unit_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, systemd_unit_text(&exec))
        .with_context(|| format!("写 systemd unit 失败: {}", path.display()))?;
    let reload = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .output();
    match reload {
        Ok(o) if o.status.success() => {
            let enable = std::process::Command::new("systemctl")
                .args(["--user", "enable", "nemesisbot.service"])
                .output()
                .context("systemctl enable 执行失败")?;
            if !enable.status.success() {
                warnings.push(format!(
                    "systemctl enable 失败: {}",
                    String::from_utf8_lossy(&enable.stderr).trim()
                ));
            }
        }
        Ok(o) => warnings.push(format!(
            "systemctl --user daemon-reload 失败（自启未激活）: {}",
            String::from_utf8_lossy(&o.stderr).trim()
        )),
        Err(_) => warnings
            .push("systemctl 缺席（无 systemd 或 PATH 缺失），自启 unit 已写入但未激活".into()),
    }
    Ok(warnings)
}

/// 追加 PATH 注记到 ~/.profile（best-effort，返回警告或 None）。
#[cfg(target_os = "linux")]
pub fn install_path_snippet() -> Result<Option<String>> {
    let path = profile_path()?;
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let local_bin = local_bin_path()?;
    if !needs_path_append(
        &std::env::var("PATH").unwrap_or_default(),
        &existing,
        &local_bin.to_string_lossy(),
    ) {
        return Ok(None);
    }
    {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(&path)?;
        writeln!(f, "{}", path_snippet())?;
    }
    Ok(Some(
        "~/.local/bin 已写入 ~/.profile PATH 注记（重登录生效）".into(),
    ))
}

#[cfg(target_os = "linux")]
pub fn remove_shortcut_artifacts() -> Vec<String> {
    let mut notes = Vec::new();
    if let Ok(p) = local_bin_path() {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(p) = desktop_entry_path() {
        let _ = std::fs::remove_file(&p);
    }
    if let Ok(p) = systemd_unit_path() {
        let _ = std::process::Command::new("systemctl")
            .args(["--user", "disable", "--now", "nemesisbot.service"])
            .output();
        if std::fs::remove_file(&p).is_ok() {
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .output();
        }
    }
    // PATH 注记保留（~/.profile 可能含用户手工编辑的邻近内容，自动摘除
    // 有误伤面——哨兵块留在原地无害，README 诚实注记）。
    notes
}

// ---------- 安装/卸载运营链 ----------

/// 安装参数（静默/向导共用）。
pub struct InstallOptions {
    pub package: PathBuf,
    pub target_dir: PathBuf,
    pub accept_risk: bool,
    pub force_downgrade: bool,
    /// 完全不建快捷方式三件套（符号链接/.desktop/PATH）。
    pub no_shortcuts: bool,
    pub autostart: bool,
    pub launch: bool,
}

/// 安装结果。
#[derive(Debug)]
pub struct InstallOutcome {
    pub install_dir: PathBuf,
    pub bot_version: String,
    pub plan_text: String,
    pub files_installed: usize,
    pub workspace_skipped: usize,
    pub launched: bool,
    pub notes: Vec<String>,
}

/// 检视安装包（完整性硬闸在这里先跑）。
pub fn inspect_package(path: &Path) -> Result<setup_core::pkgfile::PackageInfo> {
    setup_core::pkgfile::inspect(path)
}

/// 完整安装运营链（计划 §5 Linux 臂）。
#[cfg(target_os = "linux")]
pub fn run_install(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(setup_core::pkgfile::Progress)>,
) -> Result<InstallOutcome> {
    run_install_with(opts, progress, &running_bots)
}

/// 注入形态（测试专用）：进程检测可替换——测试喂假 PID 清单离线模拟
/// 全生命周期。生产入口 [`run_install`] 传真实检测。
#[cfg(target_os = "linux")]
pub fn run_install_with(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(setup_core::pkgfile::Progress)>,
    running: RunningCheck<'_>,
) -> Result<InstallOutcome> {
    use setup_core::install::{self, SigPolicy};

    // ① 外壳自检（计划 §4a：防安装器程序本体被改——启动链第一步，排在运行
    // 实例闸之前，本体不可信先于一切业务预检上报；红级与 manifest 分级同
    // 消费 accept_risk——静默旗标 / 向导知情确认）。
    let shell = setup_core::sig::shell_self_check();
    match shell.grade {
        setup_core::sig::SigGrade::Green => {}
        setup_core::sig::SigGrade::Yellow => eprintln!("[WARN] 安装器本体: {}", shell.text),
        setup_core::sig::SigGrade::Red if !opts.accept_risk => {
            // 提示后缀由 TypedFail::SigRisk 的 Display 统一追加（payload 不带，
            // 否则 {e:#} 打印双份）。
            return Err(anyhow::Error::new(TypedFail::SigRisk(format!(
                "安装器本体签名异常: {}",
                shell.text
            ))));
        }
        setup_core::sig::SigGrade::Red => {
            eprintln!(
                "[WARN] 已按知情确认继续（安装器本体签名异常：{}）",
                shell.text
            );
        }
    }

    // ② 运行实例闸（注入点：测试喂假清单模拟有/无实例两态）。
    let running = running()?;
    if !running.is_empty() {
        return Err(anyhow::Error::new(OpsFail::Running(running)));
    }

    // ③ 检视 + 计划预览。
    let info = inspect_package(&opts.package)?;
    let plan = install::plan(&opts.target_dir, Some(&info.manifest.bot_version))?;
    let plan_str = plan_text(&plan);

    // ④ 磁盘预检。
    let need = required_disk_bytes(info.manifest.total_size);
    let free = free_disk_bytes(&opts.target_dir)?;
    if free < need {
        return Err(anyhow::Error::new(OpsFail::DiskSpace {
            needed: need,
            free,
        }));
    }

    // ⑤ 引擎（含 uninstall 副本）。
    let core_opts = install::InstallOptions {
        target_dir: opts.target_dir.clone(),
        force_downgrade: opts.force_downgrade,
        sig_policy: SigPolicy::Enforce {
            accept_risk: opts.accept_risk,
        },
        keep_uninstall_copy: true,
    };
    let res = install::perform(&opts.package, &core_opts, progress)?;

    // ⑥ 快捷方式三件套 + 自启（非致命收集）。
    let mut notes = Vec::new();
    if !opts.no_shortcuts {
        let link = local_bin_path()?;
        if let Err(e) = install_symlink(&link, &opts.target_dir.join("nemesisbot")) {
            notes.push(format!("符号链接: {e:#}"));
        }
        if let Err(e) = install_desktop_entry(&opts.target_dir) {
            notes.push(format!("桌面入口: {e:#}"));
        }
        match install_path_snippet() {
            Ok(Some(note)) => notes.push(note),
            Ok(None) => {}
            Err(e) => notes.push(format!("PATH 注记: {e:#}")),
        }
    }
    if opts.autostart
        && let Err(e) = install_autostart(&opts.target_dir)
    {
        for w in e {
            notes.push(format!("自启: {w}"));
        }
    }

    // ⑦ 启动。
    let launched = if opts.launch {
        launch_dashboard(&opts.target_dir).unwrap_or(false)
    } else {
        false
    };

    Ok(InstallOutcome {
        install_dir: res.install_dir,
        bot_version: res.bot_version,
        plan_text: plan_str,
        files_installed: res.files_installed,
        workspace_skipped: res.workspace_skipped,
        launched,
        notes,
    })
}

/// 启动 Dashboard（`nemesisbot dashboard`，nohup 语义 = detach spawn）。
#[cfg(target_os = "linux")]
pub fn launch_dashboard(target: &Path) -> Result<bool> {
    let exe = target.join("nemesisbot");
    if !exe.exists() {
        return Ok(false);
    }
    std::process::Command::new(&exe)
        .arg("dashboard")
        .spawn()
        .with_context(|| format!("启动失败: {}", exe.display()))?;
    Ok(true)
}

/// 卸载结果。
#[derive(Debug)]
pub struct UninstallOutcome {
    pub deleted: usize,
    pub failures: Vec<(String, String)>,
    pub notes: Vec<String>,
}

/// 完整卸载运营链：实例闸 → 快捷方式三件套清理 → 引擎卸载 → 卸载副本摘除。
#[cfg(target_os = "linux")]
pub fn run_uninstall(install_dir: &Path) -> Result<UninstallOutcome> {
    run_uninstall_with(install_dir, &running_bots)
}

/// 注入形态（测试专用）：同 [`run_install_with`]——假 PID 清单。
#[cfg(target_os = "linux")]
pub fn run_uninstall_with(
    install_dir: &Path,
    running: RunningCheck<'_>,
) -> Result<UninstallOutcome> {
    use setup_core::install;

    let running = running()?;
    if !running.is_empty() {
        return Err(anyhow::Error::new(OpsFail::Running(running)));
    }
    let mut notes = remove_shortcut_artifacts();
    let (deleted, failures) = install::uninstall(install_dir)?;
    // 卸载副本（引擎只删 record.files 清单——副本不在清单，显式摘）。
    let copy = install_dir.join("uninstall");
    if let Err(e) = std::fs::remove_file(&copy)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        notes.push(format!("卸载副本删除失败: {e}"));
    }
    notes.push("~/.profile PATH 注记保留（手工摘除或忽略均可，哨兵块无副作用）".into());
    Ok(UninstallOutcome {
        deleted,
        failures,
        notes,
    })
}

#[cfg(test)]
mod tests;
