//! Windows 安装/卸载运营链（计划 §5/§6）：进程检测 → 磁盘预检 → 安装引擎
//! → spawn `nemesisbot system register/unregister`（系统集成）→ 启动；
//! 卸载 = 先 spawn unregister（bot 还在盘上）→ 引擎卸载 + 自身延迟自删。
//!
//! 分工铁律（2026-11-10 拍板「安装器搬文件、bot 做集成」）：ARP/自启/快捷
//! 方式/证书等系统集成的**唯一实现住 bot**（nemesis-sysint）——安装器零第
//! 二实现，只负责 spawn 并把失败降为警告（注册失败留白诚实，绝不回滚已
//! 落位产物，也绝不回退到安装器内嵌实现）。
//!
//! 纪律：
//! - 全部子进程 spawn 挂 `CREATE_NO_WINDOW`（0x08000000）——绝无弹窗黑窗
//!   （与 CLAUDE.md「禁止 start /B 弹窗命令」纪律同源，powershell/tasklist
//!   /cmd/bot 全走隐藏窗口）；
//! - 卸载自删仅当自身文件名是 `uninstall.exe`（= 安装期骨架副本）才排程
//!   ——用户拿安装包本体跑 `--uninstall` 不得自毁安装包文件；
//! - 退出码契约：0 成功 / 1 通用失败（含降级被拒）/ 2 完整性 / 3 用户取消
//!   / 4 磁盘不足 / 5 签名风险未确认 / 6 检测到 bot 正在运行（静默闸与
//!   确认卸载后强停失败共用本码域）；
//! - 卸载双形态：静默 `--uninstall` 保持全局实例闸（脚本可消费契约）；
//!   GUI 确认卸载（[`run_uninstall_confirmed`]）= **路径级强制停止**
//!   （只杀镜像落在本安装目录子树内的实例，2026-11-10 用户拍板——他处
//!   部署/开发 checkout 的同名进程绝不误杀）。

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow};

use setup_core::install::{self, InstallPlan, SigPolicy, TypedFail};
use setup_core::pkgfile::{self, PackageInfo, Progress};
use setup_core::sig::SigGrade;

/// 子进程隐藏窗口旗标（Windows 专用）。
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 产品显示名（缺省安装目录名）。
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

// ---------- 路径级强停（确认卸载链，2026-11-10 用户拍板） ----------

/// 生产查询：Toolhelp 进程快照按镜像名筛 nemesisbot.exe +
/// QueryFullProcessImageNameW 取镜像路径（PROCESS_QUERY_LIMITED_INFORMATION
/// 零高权限；跨界/提权进程路径取不到 = 空串保留条目，由 core 过滤层
/// 按「不归属任何目录」的诚实边界处理）。
pub fn running_bot_processes() -> Result<Vec<setup_core::procs::BotProcess>> {
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };

    // 镜像路径读取（失败 = 空串诚实保留——不伪造路径也不丢弃进程）。
    unsafe fn image_path(handle: windows_sys::Win32::Foundation::HANDLE) -> String {
        // edition 2024：unsafe fn 体内不自动授予 unsafe 语境，逐调用显式块。
        unsafe {
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok =
                QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
            CloseHandle(handle);
            if ok == 0 {
                String::new()
            } else {
                String::from_utf16_lossy(&buf[..len as usize])
            }
        }
    }

    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return Err(anyhow!("进程快照失败（CreateToolhelp32Snapshot）"));
        }
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..std::mem::zeroed()
        };
        if Process32FirstW(snap, &mut entry) == 0 {
            CloseHandle(snap);
            return Err(anyhow!("进程快照枚举失败（Process32FirstW）"));
        }
        let mut bots = Vec::new();
        loop {
            let name_len = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..name_len]);
            if name.eq_ignore_ascii_case("nemesisbot.exe") {
                let pid = entry.th32ProcessID;
                let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
                let exe_path = if handle.is_null() {
                    String::new()
                } else {
                    image_path(handle)
                };
                bots.push(setup_core::procs::BotProcess { pid, exe_path });
            }
            if Process32NextW(snap, &mut entry) == 0 {
                break;
            }
        }
        CloseHandle(snap);
        Ok(bots)
    }
}

/// 生产终止：`taskkill /F /T /PID ×N`（树杀——bot 的子进程一并结束；单次
/// spawn 批量 PID）。退出码不裁决（「已杀与已退出混批」下 taskkill 恒
/// 非零）——权威判定在 core 停止编排的复核重查。
pub fn kill_bot_pids(pids: &[u32]) -> Result<()> {
    if pids.is_empty() {
        return Ok(());
    }
    let pid_strs: Vec<String> = pids.iter().map(|p| p.to_string()).collect();
    let mut args: Vec<&str> = vec!["/F", "/T"];
    for s in &pid_strs {
        args.push("/PID");
        args.push(s);
    }
    run_hidden("taskkill", &args)?;
    Ok(())
}

/// 强停复核重试参数（生产）：10 次 × 300ms ≈ 3s 窗口，覆盖 TerminateProcess
/// 返回后句柄释放的瞬时延迟。
const STOP_ATTEMPTS: u32 = 10;
const STOP_RETRY: std::time::Duration = std::time::Duration::from_millis(300);

/// 强制停止本安装目录下的运行实例（确认卸载链第一步）。返回被停止清单
/// （空 = 本目录没有实例）；他处实例（另一部署/开发 checkout）不涉杀。
pub fn stop_install_bots(install_dir: &Path) -> Result<Vec<setup_core::procs::BotProcess>> {
    setup_core::procs::stop_bots_in_dir(
        install_dir,
        &running_bot_processes,
        &kill_bot_pids,
        STOP_ATTEMPTS,
        STOP_RETRY,
    )
}

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

// ---------- 系统集成 spawn 契约（唯一实现住 bot，安装器零第二实现） ----------

/// 一条 `nemesisbot system …` 调用（exe + 参数；args[0..2] 恒 = 子命令）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemCall {
    pub exe: PathBuf,
    pub args: Vec<String>,
}

impl std::fmt::Display for SystemCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.exe.display(), self.args.join(" "))
    }
}

/// 系统集成执行注入点（生产 = [`run_system_call`] 真实 spawn；测试注入
/// 假执行器记录调用/返回非零 Output——离线验证契约，不依赖真 bot 装配）。
pub type SystemRunner<'a> = &'a dyn Fn(&SystemCall) -> Result<std::process::Output>;

/// 构造 `system register` 调用（恒 spawn：ARP 条目总想要——`--no-shortcuts`
/// 只关快捷方式这一项，是否注册由 bot 侧裁决）。
pub fn register_call(exe: &Path, dir: &Path, opts: &InstallOptions) -> SystemCall {
    let mut args = vec![
        "system".to_string(),
        "register".to_string(),
        "--dir".to_string(),
        dir.to_string_lossy().into_owned(),
    ];
    if opts.autostart {
        args.push("--autostart".into());
    }
    if opts.no_shortcuts {
        args.push("--no-shortcuts".into());
    }
    if opts.desktop_shortcut {
        args.push("--desktop".into());
    }
    SystemCall {
        exe: exe.to_path_buf(),
        args,
    }
}

/// 构造 `system unregister` 调用。
pub fn unregister_call(exe: &Path, dir: &Path) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec![
            "system".to_string(),
            "unregister".to_string(),
            "--dir".to_string(),
            dir.to_string_lossy().into_owned(),
        ],
    }
}

/// 构造 `taiwei emit install` 调用（遥测 M3：原子提交完成后 spawn；
/// install_id 安装器生成透传）。遥测失败绝不影响安装结果（调用方按警告降级）。
pub fn taiwei_install_call(exe: &Path, install_id: &str) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec![
            "taiwei".to_string(),
            "emit".to_string(),
            "install".to_string(),
            "--install-id".to_string(),
            install_id.to_string(),
        ],
    }
}

/// 构造 `taiwei emit uninstall` 调用（卸载链同窗口：bot 还在盘上）。
/// `reason` = GUI 问卷选择（D1：None = 静默卸载/用户跳过，不带旗标）。
pub fn taiwei_uninstall_call(exe: &Path, reason: Option<&str>) -> SystemCall {
    let mut args = vec![
        "taiwei".to_string(),
        "emit".to_string(),
        "uninstall".to_string(),
    ];
    if let Some(r) = reason {
        args.push("--reason".to_string());
        args.push(r.to_string());
    }
    SystemCall {
        exe: exe.to_path_buf(),
        args,
    }
}

/// 构造 `taiwei disable` 调用（遥测 M4 合规：用户取消勾选 = 安装完成前
/// 持久写 opt-out 权威位 `<home>/config/config.taiwei.json`；之后 emit
/// 读开关链诚实短路零网络——双重检查闸）。
pub fn taiwei_disable_call(exe: &Path) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec!["taiwei".to_string(), "disable".to_string()],
    }
}

/// 构造 `taiwei enable-precise` 调用（遥测 M6 opt-in 单独同意：用户勾选
/// 精确定位 = 持久写 opt-out 权威位 `precise_location` 键；默认不勾不 spawn）。
pub fn taiwei_precise_enable_call(exe: &Path) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec!["taiwei".to_string(), "enable-precise".to_string()],
    }
}

/// 构造 `taiwei machine-id ensure` 调用（设备标识播种：随机 128-bit
/// 持久 ID 的机器级存储幂等写入；install 流程在 emit install 之前 spawn
/// ——emit 读链随后命中机器存储。失败降为警告，身份走 workspace 兜底链）。
pub fn taiwei_machine_id_ensure_call(exe: &Path) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec![
            "taiwei".to_string(),
            "machine-id".to_string(),
            "ensure".to_string(),
        ],
    }
}

/// 构造 `taiwei machine-id remove` 调用（卸载链收尾：删机器级设备标识
/// ——用户拍板「卸载不留任何机器标识」；时序在 emit uninstall 之后——emit
/// 还要读 ID，spawn 语境 bot 还在盘上；幂等，不存在 = Ok。失败降为警告）。
pub fn taiwei_machine_id_remove_call(exe: &Path) -> SystemCall {
    SystemCall {
        exe: exe.to_path_buf(),
        args: vec![
            "taiwei".to_string(),
            "machine-id".to_string(),
            "remove".to_string(),
        ],
    }
}

/// 执行系统集成调用（隐藏窗口 spawn bot 子进程）。
pub fn run_system_call(call: &SystemCall) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    Command::new(&call.exe)
        .args(&call.args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .with_context(|| format!("spawn 系统集成失败: {call}"))
}

/// 执行并把非零退出降为警告串（Ok(Some(warn)) = 失败但降级；Ok(None) =
/// 成功零警告；Err = spawn 本身失败）。
pub fn run_system_cmd(call: &SystemCall, runner: SystemRunner<'_>) -> Result<Option<String>> {
    let out = runner(call)?;
    if out.status.success() {
        return Ok(None);
    }
    let mut stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if stderr.is_empty() {
        stderr = String::from_utf8_lossy(&out.stdout).trim().to_string();
    }
    let tail: String = stderr
        .chars()
        .rev()
        .take(300)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Ok(Some(format!(
        "{} 退出码 {:?}{}",
        call,
        out.status.code(),
        if tail.is_empty() {
            String::new()
        } else {
            format!(": {tail}")
        }
    )))
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
    // 确认卸载强停后仍有存活（与静默闸同为「bot 正在运行」码域）。
    if err
        .downcast_ref::<setup_core::procs::BotsSurvive>()
        .is_some()
    {
        return 6;
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
    /// 匿名使用统计 opt-in（遥测 M4 合规：默认 true；GUI 勾选 / 静默缺省
    /// 开，`--no-taiwei` 或取消勾选 = 安装完成前持久写 opt-out 权威位）。
    pub taiwei_opt_in: bool,
    /// 精确定位 opt-in（遥测 M6 合规：默认 **false**——单独同意，不随总闸
    /// 走；仅 taiwei_opt_in 也为 true 时安装完成前 spawn enable-precise）。
    pub precise_opt_in: bool,
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

/// 完整安装运营链：实例闸 → 检视 → 磁盘预检 → 引擎 → spawn system register → 启动。
pub fn run_install(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(Progress)>,
) -> Result<InstallOutcome> {
    run_install_with(opts, progress, &running_bots, &run_system_call)
}

/// 注入形态（测试专用）：进程检测与系统集成执行都可替换——测试喂假进程
/// 清单 + 假执行器，离线模拟全生命周期，不依赖机器实例状态、不 spawn 真
/// bot。生产入口 [`run_install`] 传真实检测与 [`run_system_call`]。
pub fn run_install_with(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(Progress)>,
    running: RunningCheck<'_>,
    system: SystemRunner<'_>,
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

    // ⑥ 系统集成（spawn bot；非致命：失败降为警告，不回滚——ARP/快捷
    // 方式缺席是诚实留白，已落位产物绝不撤销）。恒 spawn：ARP 条目总想要
    // （uninstall.exe 已在位），--no-shortcuts 只由 bot 侧裁决快捷方式项。
    let mut warnings = Vec::new();
    let call = register_call(
        &opts.target_dir.join("nemesisbot.exe"),
        &opts.target_dir,
        opts,
    );
    match run_system_cmd(&call, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("系统集成注册（system register）: {w}")),
        Err(e) => warnings.push(format!("系统集成注册（system register）: {e:#}")),
    }

    // ⑥b 产品使用遥测（M3/M4）：install 事件——原子提交（⑤）已完成，bot
    // 已落位。install_id 本地生成透传；用户取消勾选（taiwei_opt_in=false）
    // = 先 spawn `taiwei disable` 持久写 opt-out 权威位，再照常 spawn
    // emit（emit 读开关链诚实短路零网络——服务端零 install 行）。非致命：
    // 失败降为警告（遥测绝不影响安装结果）。
    let install_id = uuid::Uuid::new_v4().to_string();
    if !opts.taiwei_opt_in {
        let dcall = taiwei_disable_call(&opts.target_dir.join("nemesisbot.exe"));
        match run_system_cmd(&dcall, system) {
            Ok(None) => {}
            Ok(Some(w)) => warnings.push(format!("使用统计关闭（taiwei disable）: {w}")),
            Err(e) => warnings.push(format!("使用统计关闭（taiwei disable）: {e:#}")),
        }
    }
    if opts.taiwei_opt_in && opts.precise_opt_in {
        let pcall = taiwei_precise_enable_call(&opts.target_dir.join("nemesisbot.exe"));
        match run_system_cmd(&pcall, system) {
            Ok(None) => {}
            Ok(Some(w)) => warnings.push(format!("精确定位开启（taiwei enable-precise）: {w}")),
            Err(e) => warnings.push(format!("精确定位开启（taiwei enable-precise）: {e:#}")),
        }
    }
    // 设备标识播种（随机持久 ID）：机器级存储幂等写入，先于 emit install
    // （emit 读链随后命中）；失败降为警告（身份走 workspace 兜底链）。
    let ecall = taiwei_machine_id_ensure_call(&opts.target_dir.join("nemesisbot.exe"));
    match run_system_cmd(&ecall, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("设备标识播种（taiwei machine-id ensure）: {w}")),
        Err(e) => warnings.push(format!("设备标识播种（taiwei machine-id ensure）: {e:#}")),
    }
    let tcall = taiwei_install_call(&opts.target_dir.join("nemesisbot.exe"), &install_id);
    match run_system_cmd(&tcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("使用统计上报（taiwei emit install）: {w}")),
        Err(e) => warnings.push(format!("使用统计上报（taiwei emit install）: {e:#}")),
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
    /// 确认卸载链强制停止的运行实例数（静默闸路径恒 0）。
    pub bots_stopped: usize,
}

/// 完整卸载运营链（静默 `--uninstall`）：全局实例闸 → spawn system
/// unregister（bot 还在盘上）→ 引擎卸载 → 自身自删。
pub fn run_uninstall(install_dir: &Path) -> Result<UninstallOutcome> {
    run_uninstall_with(install_dir, &running_bots, &run_system_call)
}

/// 注入形态（测试专用）：同 [`run_install_with`]——假进程清单 + 假执行器。
pub fn run_uninstall_with(
    install_dir: &Path,
    running: RunningCheck<'_>,
    system: SystemRunner<'_>,
) -> Result<UninstallOutcome> {
    let running = running()?;
    if !running.is_empty() {
        return Err(anyhow::Error::new(OpsFail::Running(running)));
    }
    // 静默卸载无问卷窗（D1 时序契约：问卷只在交互向导 confirm 后问）→ None
    uninstall_body(install_dir, system, None)
}

/// 确认卸载链（GUI 向导确认后）：**路径级强制停止**本目录运行实例 →
/// unregister → 引擎卸载 → 自删。与静默闸的差异：无全局实例闸——路径级
/// 判定已含在强停步骤内（他处实例不锁本目录文件，卸载不受其影响）。
/// `reason` = 卸载原因问卷结果（D1；None = 静默/跳过）。
pub fn run_uninstall_confirmed(
    install_dir: &Path,
    reason: Option<&str>,
) -> Result<UninstallOutcome> {
    let dir = install_dir.to_path_buf();
    run_uninstall_confirmed_with(
        install_dir,
        &|| stop_install_bots(&dir),
        &run_system_call,
        reason,
    )
}

/// 确认卸载注入形态（测试专用）：假强停 + 假执行器。
pub type StopBots<'a> = &'a dyn Fn() -> Result<Vec<setup_core::procs::BotProcess>>;

pub fn run_uninstall_confirmed_with(
    install_dir: &Path,
    stop: StopBots<'_>,
    system: SystemRunner<'_>,
    reason: Option<&str>,
) -> Result<UninstallOutcome> {
    // 强停必须先于一切删除动作（运行中的 bot 锁着自身镜像与打开的文件）。
    let stopped = stop()?;
    let mut out = uninstall_body(install_dir, system, reason)?;
    out.bots_stopped = stopped.len();
    Ok(out)
}

/// 卸载公共体（静默闸与确认链共用）：unregister → 遥测 uninstall（同窗口：
/// bot 还在盘上）→ machine-id remove（设备标识删除）→ 引擎卸载 → 自删排程。
/// 顺序即契约：注销先于引擎删除（删完再调 = 给死二进制发指令），遥测随注销
/// 同窗，设备标识删除在 emit 之后（emit 还要读 ID）、引擎删除之前。问卷必须
/// 在卸载链**之前**问（D1 时序契约：emit 后 bot 很快被删），静默卸载传 None。
fn uninstall_body(
    install_dir: &Path,
    system: SystemRunner<'_>,
    reason: Option<&str>,
) -> Result<UninstallOutcome> {
    let mut warnings = Vec::new();
    let call = unregister_call(&install_dir.join("nemesisbot.exe"), install_dir);
    match run_system_cmd(&call, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("系统集成注销（system unregister）: {w}")),
        Err(e) => warnings.push(format!("系统集成注销（system unregister）: {e:#}")),
    }
    let tcall = taiwei_uninstall_call(&install_dir.join("nemesisbot.exe"), reason);
    match run_system_cmd(&tcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("使用统计上报（taiwei emit uninstall）: {w}")),
        Err(e) => warnings.push(format!("使用统计上报（taiwei emit uninstall）: {e:#}")),
    }
    // 设备标识删除（随机持久 ID 机器存储；用户拍板卸载不留机器标识）。
    // 时序契约：必须在 emit uninstall 之后（emit 还要读 ID）、引擎删除之前
    // （spawn 语境 bot 还在盘上）。失败降为警告，绝不阻断卸载。
    let rcall = taiwei_machine_id_remove_call(&install_dir.join("nemesisbot.exe"));
    match run_system_cmd(&rcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => warnings.push(format!("设备标识删除（taiwei machine-id remove）: {w}")),
        Err(e) => warnings.push(format!("设备标识删除（taiwei machine-id remove）: {e:#}")),
    }
    let (deleted, failures) = install::uninstall(install_dir)?;
    // 卸载副本摘除（与 Linux uninstall_body 对称——副本不在 record.files，
    // 引擎不删）：自身 = 副本时进程镜像锁定删除必败，跳过由延迟自删接手；
    // 从安装包本体跑 `--uninstall --dir D` 时副本无自删接手 → 显式摘除
    // （否则残留 uninstall.exe 与目录壳）。
    if !self_is_uninstall_copy()
        && let Err(e) = setup_core::fsutil::delete_file(&install_dir.join("uninstall.exe"))
    {
        warnings.push(format!("卸载副本删除失败: {e}"));
    }
    // 副本摘除后目录已空则剪壳（与 Linux 形态同纪律；自身 = 副本时非空，
    // 失败无害——自删脚本的 rd 接手）。
    let _ = std::fs::remove_dir(install_dir);
    let self_delete_scheduled = schedule_self_delete();
    Ok(UninstallOutcome {
        deleted,
        failures,
        warnings,
        self_delete_scheduled,
        bots_stopped: 0,
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

/// 自身是否为 `uninstall.exe` 副本形态（自删排程与副本摘除跳过的共用判据）。
fn self_is_uninstall_copy() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.file_name()
                .map(|n| n.eq_ignore_ascii_case("uninstall.exe"))
        })
        .unwrap_or(false)
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
    if !self_is_uninstall_copy() {
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
