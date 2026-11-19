//! Linux 安装/卸载运营链（计划 §5/§6）：实例闸 → 磁盘预检 → 安装引擎
//! → spawn `nemesisbot system register/unregister`（系统集成）→ 启动；
//! 卸载 = 先 spawn unregister（bot 还在盘上）→ 引擎卸载 + 副本摘除。
//!
//! 分工铁律（2026-11-10 拍板「安装器搬文件、bot 做集成」）：符号链接/
//! .desktop/PATH 哨兵/systemd unit 等系统集成的**唯一实现住 bot**
//! （nemesis-sysint）——安装器零第二实现，只负责 spawn 并把失败降为
//! 警告（注册失败留白诚实，绝不回滚已落位产物）。
//!
//! 编译形态：
//! - spawn 契约机制（SystemCall 系列）跨平台编译，测试任意主机可跑；
//! - IO/进程面（statvfs / pgrep / 真实 spawn 链）`#[cfg(target_os = "linux")]`，
//!   只在 Linux 编译与执行（Windows 上本 crate 走 stub main——诚实降级）。
//!
//! 纪律：
//! - 运行实例闸 `pgrep -x nemesisbot`；磁盘闸 statvfs（探测最深存在祖先，
//!   与 windows ops 同纪律）；
//! - 卸载双形态：静默 `--uninstall` 保持全局实例闸（脚本可消费契约）；
//!   向导确认卸载（[`run_uninstall_confirmed`]）= **路径级强制停止**
//!   （只杀镜像落在本安装目录子树内的实例，2026-11-10 用户拍板——他处
//!   部署/开发 checkout 的同名进程绝不误杀）；
//! - 卸载副本 `uninstall`（无扩展名，平台惯例）由核心引擎落位；
//! - 退出码契约与 windows 形态一致：0/1/2/3/4/5/6（ops::exit_code_of；
//!   静默闸与确认卸载后强停失败共用 6 域）。

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use setup_core::install::{InstallPlan, TypedFail};
// ---------- 系统集成 spawn 契约（跨平台编译；唯一实现住 bot） ----------

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
/// 假执行器记录调用/返回合成 Output——离线验证契约，不依赖真 bot 装配）。
pub type SystemRunner<'a> = &'a dyn Fn(&SystemCall) -> Result<std::process::Output>;

/// 构造 `system register` 调用（恒 spawn：与 Windows 形态同契约——是否
/// 注册各项由 bot 侧裁决，`--no-shortcuts` 只关快捷方式三件套）。
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
/// `reason` = 卸载原因问卷选择（D1：None = 静默卸载/用户跳过，不带旗标）。
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

/// 执行系统集成调用（普通 spawn；bot 自身已带控制台纪律）。
pub fn run_system_call(call: &SystemCall) -> Result<std::process::Output> {
    std::process::Command::new(&call.exe)
        .args(&call.args)
        .output()
        .with_context(|| format!("spawn 系统集成失败: {call}"))
}

/// 执行并把非零退出降为警告串（Ok(Some(warn)) = 失败但降级；Ok(None) =
/// 成功零警告；Err = spawn 本身失败）。与 windows 形态同语义。
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
    // 宏导入收在 cfg(linux) 函数体内——Windows 编译看不见本函数，
    // 顶部导入会在 Windows 形态报 unused（盲区纪律）。
    use anyhow::anyhow;
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
    /// 匿名使用统计 opt-in（遥测 M4 合规：默认 true；向导勾选 / 静默缺省开，
    /// `--no-taiwei` 或取消勾选 = 安装完成前持久写 opt-out 权威位）。
    pub taiwei_opt_in: bool,
    /// 精确定位 opt-in（遥测 M6 合规：默认 **false**——单独同意，不随总闸
    /// 走；仅 taiwei_opt_in 也为 true 时安装完成前 spawn enable-precise；
    /// Linux 定位后端缺席 = 运行期诚实降级 IP 城市，勾选无害）。
    pub precise_opt_in: bool,
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
    run_install_with(opts, progress, &running_bots, &run_system_call)
}

/// 注入形态（测试专用）：进程检测与系统集成执行都可替换——测试喂假
/// PID 清单 + 假执行器，离线模拟全生命周期，不依赖机器实例状态、不
/// spawn 真 bot。生产入口 [`run_install`] 传真实检测与 [`run_system_call`]。
#[cfg(target_os = "linux")]
pub fn run_install_with(
    opts: &InstallOptions,
    progress: Option<&dyn Fn(setup_core::pkgfile::Progress)>,
    running: RunningCheck<'_>,
    system: SystemRunner<'_>,
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

    // ⑥ 系统集成（spawn bot；非致命：失败降为 notes，不回滚——符号链接/
    // .desktop/systemd 缺席是诚实留白，已落位产物绝不撤销）。恒 spawn：
    // 与 Windows 形态同契约，--no-shortcuts 只由 bot 侧裁决快捷方式项。
    let mut notes = Vec::new();
    let call = register_call(&opts.target_dir.join("nemesisbot"), &opts.target_dir, opts);
    match run_system_cmd(&call, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("系统集成注册（system register）: {w}")),
        Err(e) => notes.push(format!("系统集成注册（system register）: {e:#}")),
    }

    // ⑥b 产品使用遥测（M3/M4）：install 事件——原子提交（⑤）已完成，bot
    // 已落位。install_id 本地生成透传；用户取消勾选（taiwei_opt_in=false）
    // = 先 spawn `taiwei disable` 持久写 opt-out 权威位，再照常 spawn
    // emit（emit 读开关链诚实短路零网络——服务端零 install 行）。非致命：
    // 失败降为 notes（遥测绝不影响安装结果）。
    let install_id = uuid::Uuid::new_v4().to_string();
    if !opts.taiwei_opt_in {
        let dcall = taiwei_disable_call(&opts.target_dir.join("nemesisbot"));
        match run_system_cmd(&dcall, system) {
            Ok(None) => {}
            Ok(Some(w)) => notes.push(format!("使用统计关闭（taiwei disable）: {w}")),
            Err(e) => notes.push(format!("使用统计关闭（taiwei disable）: {e:#}")),
        }
    }
    if opts.taiwei_opt_in && opts.precise_opt_in {
        let pcall = taiwei_precise_enable_call(&opts.target_dir.join("nemesisbot"));
        match run_system_cmd(&pcall, system) {
            Ok(None) => {}
            Ok(Some(w)) => notes.push(format!("精确定位开启（taiwei enable-precise）: {w}")),
            Err(e) => notes.push(format!("精确定位开启（taiwei enable-precise）: {e:#}")),
        }
    }
    // 设备标识播种（随机持久 ID）：机器级存储幂等写入，先于 emit install
    // （emit 读链随后命中）；失败降为警告（身份走 workspace 兜底链）。
    let ecall = taiwei_machine_id_ensure_call(&opts.target_dir.join("nemesisbot"));
    match run_system_cmd(&ecall, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("设备标识播种（taiwei machine-id ensure）: {w}")),
        Err(e) => notes.push(format!("设备标识播种（taiwei machine-id ensure）: {e:#}")),
    }
    let tcall = taiwei_install_call(&opts.target_dir.join("nemesisbot"), &install_id);
    match run_system_cmd(&tcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("使用统计上报（taiwei emit install）: {w}")),
        Err(e) => notes.push(format!("使用统计上报（taiwei emit install）: {e:#}")),
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
    /// 确认卸载链强制停止的运行实例数（静默闸路径恒 0）。
    pub bots_stopped: usize,
}

/// 完整卸载运营链（静默 `--uninstall`）：全局实例闸 → spawn system
/// unregister（bot 还在盘上）→ 引擎卸载 → 卸载副本摘除。
#[cfg(target_os = "linux")]
pub fn run_uninstall(install_dir: &Path) -> Result<UninstallOutcome> {
    run_uninstall_with(install_dir, &running_bots, &run_system_call)
}

/// 注入形态（测试专用）：同 [`run_install_with`]——假 PID 清单 + 假执行器。
#[cfg(target_os = "linux")]
pub fn run_uninstall_with(
    install_dir: &Path,
    running: RunningCheck<'_>,
    system: SystemRunner<'_>,
) -> Result<UninstallOutcome> {
    let running = running()?;
    if !running.is_empty() {
        return Err(anyhow::Error::new(OpsFail::Running(running)));
    }
    // 静默卸载无问卷（D1 时序契约：问卷只在交互向导确认后问）→ None。
    uninstall_body(install_dir, system, None)
}

// ---------- 路径级强停（确认卸载链，2026-11-10 用户拍板） ----------

/// 生产查询：/proc 扫描——readlink `/proc/<pid>/exe` 取镜像路径，basename
/// == nemesisbot 即命中（readlink 尾注「 (deleted)」剪除后判名——二进制
/// 被换名删除后仍在跑的形态）。无权限（他用户进程）readlink 失败 = 跳过
/// （不可见即不可杀；Linux 无文件锁，其删除面由韧性删除如实兜底）。
#[cfg(target_os = "linux")]
pub fn running_bot_processes() -> Result<Vec<setup_core::procs::BotProcess>> {
    let mut bots = Vec::new();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) else {
            continue;
        };
        let path = exe.to_string_lossy().into_owned();
        let base = path.strip_suffix(" (deleted)").unwrap_or(&path);
        let is_bot = std::path::Path::new(base)
            .file_name()
            .map(|n| n == std::ffi::OsStr::new("nemesisbot"))
            .unwrap_or(false);
        if is_bot {
            bots.push(setup_core::procs::BotProcess {
                pid,
                exe_path: base.to_owned(),
            });
        }
    }
    Ok(bots)
}

/// 生产终止：逐 PID `libc::kill(SIGKILL)`（不杀进程组——pgid 可能与用户
/// shell 作业共享，误伤面不可接受；bot 的子进程若也是 nemesisbot 形态，
/// 由查询面按镜像路径一并覆盖）。
#[cfg(target_os = "linux")]
pub fn kill_bot_pids(pids: &[u32]) -> Result<()> {
    for pid in pids {
        // ESRCH（已退出）忽略——权威判定在 core 停止编排的复核重查。
        unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) };
    }
    Ok(())
}

/// 强停复核重试参数（生产）：10 次 × 300ms ≈ 3s 窗口（与 windows 形态一致）。
const STOP_ATTEMPTS: u32 = 10;
const STOP_RETRY: std::time::Duration = std::time::Duration::from_millis(300);

/// 强制停止本安装目录下的运行实例（确认卸载链第一步）。返回被停止清单
/// （空 = 本目录没有实例）；他处实例（另一部署/开发 checkout）不涉杀。
#[cfg(target_os = "linux")]
pub fn stop_install_bots(install_dir: &Path) -> Result<Vec<setup_core::procs::BotProcess>> {
    setup_core::procs::stop_bots_in_dir(
        install_dir,
        &running_bot_processes,
        &kill_bot_pids,
        STOP_ATTEMPTS,
        STOP_RETRY,
    )
}

/// 确认卸载链（向导确认后）：**路径级强制停止**本目录运行实例 →
/// unregister → 引擎卸载 → 副本摘除。与静默闸的差异：无全局实例闸——
/// 路径级判定已含在强停步骤内（他处实例不锁本目录文件）。
/// `reason` = 卸载原因问卷选择（D1；None = 静默/跳过，不带 --reason）。
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
pub type StopBots<'a> = &'a dyn Fn() -> Result<Vec<setup_core::procs::BotProcess>>;

#[cfg(target_os = "linux")]
pub fn run_uninstall_confirmed_with(
    install_dir: &Path,
    stop: StopBots<'_>,
    system: SystemRunner<'_>,
    reason: Option<&str>,
) -> Result<UninstallOutcome> {
    // 强停必须先于一切删除动作（运行中的 bot 持有自身镜像）。
    let stopped = stop()?;
    let mut out = uninstall_body(install_dir, system, reason)?;
    out.bots_stopped = stopped.len();
    Ok(out)
}

/// 卸载公共体（静默闸与确认链共用）：unregister → 遥测 uninstall（同窗口：
/// bot 还在盘上）→ machine-id remove（设备标识删除）→ 引擎卸载 → 副本摘除 →
/// 剪壳。顺序即契约：注销先于引擎删除（删完再调 = 给死二进制发指令），遥测
/// 随注销同窗，设备标识删除在 emit 之后（emit 还要读 ID）、引擎删除之前。
/// `reason` = 问卷结果透传给 emit 调用（D1 时序契约：问卷必须在卸载链
/// **之前**问——emit 后 bot 很快被删；静默卸载传 None）。
#[cfg(target_os = "linux")]
fn uninstall_body(
    install_dir: &Path,
    system: SystemRunner<'_>,
    reason: Option<&str>,
) -> Result<UninstallOutcome> {
    use setup_core::install;

    // 系统集成注销必须先于引擎删除——bot 已落位才能执行注销；删完再调
    // 就是给死二进制发指令（顺序即契约，测试注入执行器时断言本序）。
    let mut notes = Vec::new();
    let call = unregister_call(&install_dir.join("nemesisbot"), install_dir);
    match run_system_cmd(&call, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("系统集成注销（system unregister）: {w}")),
        Err(e) => notes.push(format!("系统集成注销（system unregister）: {e:#}")),
    }
    let tcall = taiwei_uninstall_call(&install_dir.join("nemesisbot"), reason);
    match run_system_cmd(&tcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("使用统计上报（taiwei emit uninstall）: {w}")),
        Err(e) => notes.push(format!("使用统计上报（taiwei emit uninstall）: {e:#}")),
    }
    // 设备标识删除（随机持久 ID 机器存储；用户拍板卸载不留机器标识）。
    // 时序契约：必须在 emit uninstall 之后（emit 还要读 ID）、引擎删除之前
    // （spawn 语境 bot 还在盘上）。失败降为警告，绝不阻断卸载。
    let rcall = taiwei_machine_id_remove_call(&install_dir.join("nemesisbot"));
    match run_system_cmd(&rcall, system) {
        Ok(None) => {}
        Ok(Some(w)) => notes.push(format!("设备标识删除（taiwei machine-id remove）: {w}")),
        Err(e) => notes.push(format!("设备标识删除（taiwei machine-id remove）: {e:#}")),
    }
    let (deleted, failures) = install::uninstall(install_dir)?;
    // 卸载副本（引擎只删 record.files 清单——副本不在清单，显式摘）。
    let copy = install_dir.join("uninstall");
    if let Err(e) = std::fs::remove_file(&copy)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        notes.push(format!("卸载副本删除失败: {e}"));
    }
    // 副本摘除后目录已空则剪壳：引擎的空目录检查在副本摘除**前**跑过（副本
    // 在场 = 判非空保留），不补这一刀会留空目录壳（2026-11-10 Linux 全链
    // E2E 实录）。best-effort：目录非空（未知文件在场）删除失败静默保留，
    // 与 Windows 自删脚本的 rd 空目录语义对齐。
    let _ = std::fs::remove_dir(install_dir);
    Ok(UninstallOutcome {
        deleted,
        failures,
        notes,
        bots_stopped: 0,
    })
}

#[cfg(test)]
mod tests;
