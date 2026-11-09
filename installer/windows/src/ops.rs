//! Windows 安装/卸载运营链（计划 §5/§6）：进程检测 → 磁盘预检 → 安装引擎
//! → 快捷方式（PowerShell WScript.Shell）→ HKCU ARP/Run 注册表 → 启动；
//! 卸载 = 注册表/快捷方式清理 + 引擎卸载 + 自身延迟自删。
//!
//! 纪律：
//! - 全部子进程 spawn 挂 `CREATE_NO_WINDOW`（0x08000000）——绝无弹窗黑窗
//!   （与 CLAUDE.md「禁止 start /B 弹窗命令」纪律同源，powershell/tasklist
//!   /cmd 全走隐藏窗口）；
//! - 注册表只写 HKCU（用户级安装，零 UAC）；
//! - 快捷方式参数固定 `dashboard`（一键进 Dashboard，产品既有一键入口）；
//! - 卸载自删仅当自身文件名是 `uninstall.exe`（= 安装期骨架副本）才排程
//!   ——用户拿安装包本体跑 `--uninstall` 不得自毁安装包文件；
//! - 退出码契约：0 成功 / 1 通用失败（含降级被拒）/ 2 完整性 / 3 用户取消
//!   / 4 磁盘不足 / 5 签名风险未确认 / 6 检测到 bot 正在运行。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use setup_core::install::{self, InstallPlan, SigPolicy, TypedFail};
use setup_core::pkgfile::{self, PackageInfo, Progress};
use setup_core::sig::SigGrade;

/// 子进程隐藏窗口旗标（Windows 专用）。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// HKCU ARP 卸载面板键路径。
pub const ARP_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\NemesisBot";
/// HKCU Run 自启键路径。
pub const RUN_PATH: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
/// Run 键值名 / 显示名。
pub const APP_NAME: &str = "NemesisBot";

/// 隐藏窗口执行外部命令。
pub fn run_hidden(program: &str, args: &[&str]) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("spawn {program} 失败"))
}

/// tasklist 输出解析（纯函数，`running_bots` 的解析层——测试喂假 tasklist
/// 文本即可模拟任意进程清单，不 spawn 真进程）。
pub fn parse_tasklist_output(text: &str) -> Vec<String> {
    text.lines()
        .filter(|l| l.to_ascii_lowercase().contains("nemesisbot.exe"))
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect()
}

/// 运行中的 nemesisbot 进程描述行（空 = 无运行实例）。
pub fn running_bots() -> Result<Vec<String>> {
    let out = run_hidden("tasklist", &["/FI", "IMAGENAME eq nemesisbot.exe", "/NH"])?;
    Ok(parse_tasklist_output(&String::from_utf8_lossy(&out.stdout)))
}

/// 运行实例检测注入点（生产 = [`running_bots`]；测试注入假进程清单——
/// 离线模拟全生命周期，不依赖机器上有无真实实例在跑）。
pub type RunningCheck<'a> = &'a dyn Fn() -> Result<Vec<String>>;

/// 目标路径所在卷的剩余字节数（GetDiskFreeSpaceExW）。
/// 目标目录可能尚未创建——向上探到最深的存在祖先再查（同一卷）。
pub fn free_disk_bytes(path: &Path) -> Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
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
    let wide: Vec<u16> = probe
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut free: u64 = 0;
    let mut total: u64 = 0;
    let mut caller_free: u64 = 0;
    let ok = unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut free, &mut total, &mut caller_free) };
    if ok == 0 {
        return Err(anyhow!(
            "磁盘剩余空间查询失败: {}（目录不存在或卷不可用）",
            probe.display()
        ));
    }
    Ok(free)
}

/// 安装所需磁盘字节数（载荷原大 × 1.2 余量）。
pub fn required_disk_bytes(manifest_total: u64) -> u64 {
    manifest_total + manifest_total / 5
}

/// 缺省安装目录：`%LOCALAPPDATA%\NemesisBot`（用户级，零 UAC）。
pub fn default_install_dir() -> Result<PathBuf> {
    let base = std::env::var("LOCALAPPDATA")
        .or_else(|_| -> Result<String> {
            let home = std::env::var("USERPROFILE")?;
            Ok(format!("{home}\\AppData\\Local"))
        })
        .context("LOCALAPPDATA/USERPROFILE 环境变量均缺席")?;
    Ok(PathBuf::from(base).join(APP_NAME))
}

/// 自身所在目录（uninstall.exe 副本 = 安装目录）。
pub fn own_install_dir() -> Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .context("无法定位自身目录")
}

/// 自身目录是否带 install.json（uninstall 副本双击启发式 → 卸载模式）。
pub fn own_dir_has_install_record() -> bool {
    own_install_dir()
        .map(|d| d.join("install.json").exists())
        .unwrap_or(false)
}

/// 开始菜单快捷方式路径。
pub fn start_menu_shortcut() -> Result<PathBuf> {
    let base = std::env::var("APPDATA").context("APPDATA 环境变量缺席")?;
    Ok(PathBuf::from(base)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(APP_NAME)
        .join(format!("{APP_NAME}.lnk")))
}

/// 桌面快捷方式路径。
pub fn desktop_shortcut_path() -> Result<PathBuf> {
    let home = std::env::var("USERPROFILE").context("USERPROFILE 环境变量缺席")?;
    Ok(PathBuf::from(home)
        .join("Desktop")
        .join(format!("{APP_NAME}.lnk")))
}

/// PowerShell 单引号字面转义（' → ''）。
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// 经 WScript.Shell 创建 .lnk（目标 = nemesisbot.exe dashboard）。
pub fn make_lnk(lnk: &Path, target_exe: &Path, arguments: &str, workdir: &Path) -> Result<()> {
    if let Some(p) = lnk.parent() {
        std::fs::create_dir_all(p)?;
    }
    let script = format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({lnk}); \
         $s.TargetPath={exe}; $s.Arguments={args}; $s.WorkingDirectory={wd}; $s.Save()",
        lnk = ps_quote(&lnk.to_string_lossy()),
        exe = ps_quote(&target_exe.to_string_lossy()),
        args = ps_quote(arguments),
        wd = ps_quote(&workdir.to_string_lossy()),
    );
    let out = run_hidden(
        "powershell",
        &[
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &script,
        ],
    )?;
    if !out.status.success() {
        return Err(anyhow!(
            "创建快捷方式失败（{}）: {}",
            lnk.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    if !lnk.exists() {
        return Err(anyhow!("快捷方式未落盘: {}", lnk.display()));
    }
    Ok(())
}

/// 创建快捷方式（开始菜单恒建；桌面可选）。返回已创建路径。
pub fn create_shortcuts(target: &Path, desktop: bool) -> Result<Vec<PathBuf>> {
    let exe = target.join("nemesisbot.exe");
    let mut made = Vec::new();
    let sm = start_menu_shortcut()?;
    make_lnk(&sm, &exe, "dashboard", target)?;
    made.push(sm);
    if desktop {
        let d = desktop_shortcut_path()?;
        make_lnk(&d, &exe, "dashboard", target)?;
        made.push(d);
    }
    Ok(made)
}

/// 删除快捷方式（best-effort，返回失败清单）；开始菜单 NemesisBot 空目录
/// 壳顺手剪除（删 .lnk 后目录残留是资源管理器可见垃圾，2026-11-10 实测确认）。
pub fn remove_shortcuts() -> Vec<(String, String)> {
    let mut failures = Vec::new();
    for p in [start_menu_shortcut(), desktop_shortcut_path()] {
        if let Ok(path) = p
            && path.exists()
            && let Err(e) = std::fs::remove_file(&path)
        {
            failures.push((path.to_string_lossy().into(), e.to_string()));
        }
    }
    if let Ok(sm) = start_menu_shortcut()
        && let Some(dir) = sm.parent()
        && std::fs::read_dir(dir).is_ok_and(|mut it| it.next().is_none())
    {
        let _ = std::fs::remove_dir(dir);
    }
    failures
}

/// 写 HKCU ARP 卸载面板键（控制面板「应用」可见）。
///
/// 键路径由调用方给定：生产链 [`run_install`] 传 [`ARP_PATH`]，测试经
/// [`run_install_with`] 传测试键（绝不写真实注册表）。
fn write_arp_at(path: &str, target: &Path, version: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(path)
        .with_context(|| format!("建注册表键失败: HKCU\\{path}"))?;
    let uninstaller = target.join("uninstall.exe");
    key.set_value("DisplayName", &APP_NAME)?;
    key.set_value("DisplayVersion", &version)?;
    key.set_value("Publisher", &"NemesisBot Project")?;
    key.set_value(
        "DisplayIcon",
        &target.join("nemesisbot.exe").to_string_lossy().to_string(),
    )?;
    key.set_value("InstallLocation", &target.to_string_lossy().to_string())?;
    key.set_value(
        "UninstallString",
        &format!("\"{}\" --uninstall", uninstaller.display()),
    )?;
    key.set_value("NoModify", &1u32)?;
    key.set_value("NoRepair", &1u32)?;
    Ok(())
}

/// 摘 ARP 键（键路径注入同 [`write_arp_at`]；NotFound = 幂等删除）。
fn remove_arp_at(path: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    // NotFound = 幂等删除（键本来就不在——卸载 best-effort 不该为它报警；
    // 与 core fsutil::delete_file 的幂等纪律同源）。
    match RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(anyhow::Error::new(e).context(format!("删注册表键失败: HKCU\\{path}"))),
    }
}

/// 写 HKCU Run 自启值。
pub fn set_autostart(target: &Path) -> Result<()> {
    set_autostart_at(RUN_PATH, target)
}

fn set_autostart_at(path: &str, target: &Path) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::HKEY_CURRENT_USER;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (key, _) = hkcu
        .create_subkey(path)
        .with_context(|| format!("建注册表键失败: HKCU\\{path}"))?;
    key.set_value(
        APP_NAME,
        &format!("\"{}\" dashboard", target.join("nemesisbot.exe").display()),
    )?;
    Ok(())
}

/// 摘 Run 自启值（值缺席不报错）。
pub fn remove_autostart() -> Result<()> {
    remove_autostart_at(RUN_PATH)
}

fn remove_autostart_at(path: &str) -> Result<()> {
    use winreg::RegKey;
    use winreg::enums::{HKEY_CURRENT_USER, KEY_WRITE};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let key = hkcu
        .open_subkey_with_flags(path, KEY_WRITE)
        .context("打开 Run 键失败")?;
    let _ = key.delete_value(APP_NAME);
    Ok(())
}

/// 启动 Dashboard（`nemesisbot.exe dashboard` 一键入口；隐藏控制台窗口）。
/// 返回是否实际 spawn。
pub fn launch_dashboard(target: &Path) -> Result<bool> {
    let exe = target.join("nemesisbot.exe");
    if !exe.exists() {
        return Ok(false);
    }
    use std::os::windows::process::CommandExt;
    Command::new(&exe)
        .arg("dashboard")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .with_context(|| format!("启动失败: {}", exe.display()))?;
    Ok(true)
}

/// 运营层类型化失败（退出码 3/4/6 域；2/5/1 域归引擎 TypedFail）。
#[derive(Debug)]
pub enum OpsFail {
    /// bot 正在运行（安装/卸载都拒绝）。
    Running(Vec<String>),
    /// 磁盘空间不足。
    DiskSpace { needed: u64, free: u64 },
    /// 用户在确认对话框取消（GUI 取消路径直接写退出码 3，不经本变体
    /// 构造——变体是退出码契约的映射锚，exit_code_of 的测试消费它）。
    #[allow(dead_code)]
    Cancelled,
}

impl std::fmt::Display for OpsFail {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpsFail::Running(list) => write!(
                f,
                "检测到 NemesisBot 正在运行（{} 个进程），请先全部退出:\n{}",
                list.len(),
                list.join("\n")
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

/// 退出码映射（main 侧唯一出口）。
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

/// 安装计划人读文本。
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

/// 安装参数（静默/GUI 共用）。
pub struct InstallOptions {
    pub package: PathBuf,
    pub target_dir: PathBuf,
    /// 红色级知情确认（静默 --accept-signature-risk / GUI 勾选）。
    pub accept_risk: bool,
    pub force_downgrade: bool,
    pub desktop_shortcut: bool,
    /// 完全不建快捷方式。
    pub no_shortcuts: bool,
    /// 开机自启（HKCU Run）。
    pub autostart: bool,
    /// 装完启动 Dashboard。
    pub launch: bool,
}

/// 安装结果（含非致命警告——快捷方式/注册表失败不回滚已落位产物）。
#[derive(Clone, Debug)]
pub struct InstallOutcome {
    pub install_dir: PathBuf,
    pub bot_version: String,
    pub plan_text: String,
    pub files_installed: usize,
    pub workspace_skipped: usize,
    pub launched: bool,
    pub warnings: Vec<String>,
}

/// 检视安装包（GUI 向导首屏信息源；完整性硬闸在这里先跑）。
pub fn inspect_package(path: &Path) -> Result<PackageInfo> {
    pkgfile::inspect(path)
}

/// 完整安装运营链：实例闸 → 检视 → 磁盘预检 → 引擎 → 快捷方式/注册表 → 启动。
pub fn run_install(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(Progress)>,
) -> Result<InstallOutcome> {
    run_install_with(opts, progress, &running_bots, ARP_PATH)
}

/// 注入形态（测试专用）：进程检测与 ARP 键路径都可替换——测试喂假进程
/// 清单 + 测试注册表键，离线模拟全生命周期，不依赖机器实例状态、
/// 绝不写真实 ARP 键。生产入口 [`run_install`] 传真实检测与 [`ARP_PATH`]。
pub fn run_install_with(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(Progress)>,
    running: RunningCheck<'_>,
    arp_key: &str,
) -> Result<InstallOutcome> {
    // ① 外壳自检（计划 §4a：防安装器程序本体被改——启动链第一步，排在运行
    // 实例闸之前，本体不可信先于一切业务预检上报；红级与 manifest 分级同
    // 消费 `accept_risk` 知情确认旗标——GUI 勾选 / 静默 --accept-signature-risk）。
    let shell = setup_core::sig::shell_self_check();
    match shell.grade {
        SigGrade::Green => {}
        SigGrade::Yellow => eprintln!("[WARN] 安装器本体: {}", shell.text),
        SigGrade::Red if !opts.accept_risk => {
            // 提示后缀由 TypedFail::SigRisk 的 Display 统一追加（payload 不带，
            // 否则 {e:#} 打印双份）。
            return Err(anyhow::Error::new(TypedFail::SigRisk(format!(
                "安装器本体签名异常: {}",
                shell.text
            ))));
        }
        SigGrade::Red => {
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

    // ③ 检视（完整性/信任分级在 inspect 内）。
    let info = pkgfile::inspect(&opts.package)?;
    let plan = install::plan(&opts.target_dir, Some(&info.manifest.bot_version))?;
    let plan_str = plan_text(&plan);

    // ④ 磁盘预检（载荷原大 × 1.2）。
    let need = required_disk_bytes(info.manifest.total_size);
    let free = free_disk_bytes(&opts.target_dir)?;
    if free < need {
        return Err(anyhow::Error::new(OpsFail::DiskSpace {
            needed: need,
            free,
        }));
    }

    // ⑤ 引擎（staging + 原子提交 + 卸载副本）。
    let core_opts = install::InstallOptions {
        target_dir: opts.target_dir.clone(),
        force_downgrade: opts.force_downgrade,
        sig_policy: SigPolicy::Enforce {
            accept_risk: opts.accept_risk,
        },
        keep_uninstall_copy: true,
    };
    let res = install::perform(&opts.package, &core_opts, progress)?;

    // ⑥ 快捷方式 + ARP + 自启（非致命：失败收集为警告，不回滚）。
    let mut warnings = Vec::new();
    if opts.no_shortcuts {
        // 显式要求不建。
    } else if let Err(e) = create_shortcuts(&opts.target_dir, opts.desktop_shortcut) {
        warnings.push(format!("快捷方式: {e:#}"));
    }
    if let Err(e) = write_arp_at(arp_key, &opts.target_dir, &res.bot_version) {
        warnings.push(format!("卸载注册表项: {e:#}"));
    }
    if opts.autostart
        && let Err(e) = set_autostart(&opts.target_dir)
    {
        warnings.push(format!("开机自启: {e:#}"));
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
        warnings,
    })
}

/// 卸载结果。
#[derive(Debug)]
pub struct UninstallOutcome {
    pub deleted: usize,
    pub failures: Vec<(String, String)>,
    pub warnings: Vec<String>,
    /// 自身延迟自删是否排程（仅 uninstall.exe 副本形态）。
    pub self_delete_scheduled: bool,
}

/// 完整卸载运营链：实例闸 → 注册表/快捷方式清理 → 引擎卸载 → 自身自删。
pub fn run_uninstall(install_dir: &Path) -> Result<UninstallOutcome> {
    run_uninstall_with(install_dir, &running_bots, ARP_PATH)
}

/// 注入形态（测试专用）：同 [`run_install_with`]——假进程清单 + 测试 ARP 键。
pub fn run_uninstall_with(
    install_dir: &Path,
    running: RunningCheck<'_>,
    arp_key: &str,
) -> Result<UninstallOutcome> {
    let running = running()?;
    if !running.is_empty() {
        return Err(anyhow::Error::new(OpsFail::Running(running)));
    }
    let mut warnings = Vec::new();
    if let Err(e) = remove_arp_at(arp_key) {
        warnings.push(format!("ARP 键: {e:#}"));
    }
    if let Err(e) = remove_autostart() {
        warnings.push(format!("开机自启值: {e:#}"));
    }
    for (path, err) in remove_shortcuts() {
        warnings.push(format!("快捷方式 {path}: {err}"));
    }
    let (deleted, failures) = install::uninstall(install_dir)?;
    let self_delete_scheduled = schedule_self_delete();
    Ok(UninstallOutcome {
        deleted,
        failures,
        warnings,
        self_delete_scheduled,
    })
}

/// 自删脚本串（纯函数，可测）：等 2s → 删自身 → 试删空目录（`rd` 不带 /s，
/// 只删空目录——非空残留诚实保留）。
fn self_delete_script(self_path: &Path) -> String {
    let mut script = format!(
        "ping -n 3 127.0.0.1 >nul & del /f /q \"{}\"",
        self_path.display()
    );
    if let Some(dir) = self_path.parent() {
        script.push_str(&format!(" & rd \"{}\"", dir.display()));
    }
    script
}

/// 自身延迟自删（仅 uninstall.exe 副本形态）。
///
/// 引号纪律：脚本串必须走 `raw_arg` 原样上命令行——`args(["/C", script])` 会
/// 按 MSVCRT 规则把内层引号转义成 `\"`，cmd 收到字面 `\"C:\...\"` 路径 =
/// del/rd 双双「文件名、目录名或卷标语法不正确」，且 CREATE_NO_WINDOW 无声
/// 吞掉一切现场（2026-11-10 实装测试抓到的真 bug：卸载副本永残）。
fn schedule_self_delete() -> bool {
    use std::os::windows::process::CommandExt;
    let Ok(self_path) = std::env::current_exe() else {
        return false;
    };
    let is_copy = self_path
        .file_name()
        .map(|n| n.eq_ignore_ascii_case("uninstall.exe"))
        .unwrap_or(false);
    if !is_copy {
        return false;
    }
    Command::new("cmd")
        .raw_arg("/C")
        .raw_arg(self_delete_script(&self_path))
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

#[cfg(test)]
mod tests;
