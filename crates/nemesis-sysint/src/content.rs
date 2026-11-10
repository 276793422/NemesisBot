//! 系统集成「写入内容」的单一真相源（纯构造器，零 IO、跨平台编译可测）。
//!
//! 纪律：平台 IO 层（windows.rs / linux.rs）只负责把这里构造的内容落盘/
//! 落注册表——内容改这里一处，两平台测试（本仓库 CI + 远程 Linux 盒）
//! 共用同一批断言。移植自 installer 两侧 ops.rs（2026-11-10 系统集成面
//! 收编 bot，安装器瘦身只留文件落位）。

use std::path::Path;

/// 产品显示名（ARP DisplayName / Run 值名 / 快捷方式名）。
pub const APP_NAME: &str = "NemesisBot";

/// ARP（添加/删除程序面板）条目内容。
///
/// 值类型分两族：字符串族（REG_SZ）与 dword 族（REG_DWORD，NoModify/
/// NoRepair 面板按钮语义）——落库层（winreg）按类型分发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegValue {
    Str(String),
    Dword(u32),
}

/// HKCU ARP 卸载面板键的全部值（落库点 `...\Uninstall\NemesisBot`）。
pub fn arp_values(target_dir: &Path, version: &str) -> Vec<(&'static str, RegValue)> {
    let uninstaller = target_dir.join("uninstall.exe");
    vec![
        ("DisplayName", RegValue::Str(APP_NAME.to_string())),
        ("DisplayVersion", RegValue::Str(version.to_string())),
        ("Publisher", RegValue::Str("NemesisBot Project".into())),
        (
            "DisplayIcon",
            RegValue::Str(
                target_dir
                    .join("nemesisbot.exe")
                    .to_string_lossy()
                    .to_string(),
            ),
        ),
        (
            "InstallLocation",
            RegValue::Str(target_dir.to_string_lossy().to_string()),
        ),
        (
            "UninstallString",
            RegValue::Str(format!("\"{}\" --uninstall", uninstaller.display())),
        ),
        ("NoModify", RegValue::Dword(1)),
        ("NoRepair", RegValue::Dword(1)),
    ]
}

/// HKCU Run 自启值内容（引号包裹 exe + dashboard 一键入口参数）。
pub fn autostart_run_value(target_dir: &Path) -> String {
    format!(
        "\"{}\" dashboard",
        target_dir.join("nemesisbot.exe").display()
    )
}

/// PowerShell 单引号字面转义（' → ''）——WScript.Shell 内联脚本参数安全。
pub fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// shell 单引号字面转义（' → '\''）——.desktop Exec / unit ExecStart 路径安全。
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// ---------- Linux 快捷方式三件套内容 ----------

/// `~/.profile` PATH 注记哨兵（幂等识别标记；卸载按设计保留——自动摘除
/// 有误伤用户手工编辑面的风险，哨兵块原地无害）。
pub const PATH_MARKER_BEGIN: &str = "# >>> NemesisBot integration PATH >>>";
pub const PATH_MARKER_END: &str = "# <<< NemesisBot integration PATH <<<";

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

/// XDG 桌面入口内容（Exec 路径 shell 引号包裹，dashboard 参数在引号外）。
pub fn desktop_entry_text(exec_path: &Path, icon_path: &Path) -> String {
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={APP_NAME}\n\
         Comment=NemesisBot Dashboard\n\
         Exec={} dashboard\n\
         Icon={}\n\
         Terminal=false\n\
         Categories=Development;\n",
        shell_quote(&exec_path.to_string_lossy()),
        icon_path.display()
    )
}

/// systemd user unit 内容（Restart=on-failure 韧性重启；WantedBy=
/// default.target——user unit 语义）。
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

// ---------- Windows 快捷方式内容 ----------

/// WScript.Shell 建快捷方式的 PowerShell 内联脚本（目标 = nemesisbot.exe
/// dashboard 一键入口；四参数全部 ps_quote 转义）。
pub fn lnk_script(lnk: &Path, target_exe: &Path, arguments: &str, workdir: &Path) -> String {
    format!(
        "$s=(New-Object -ComObject WScript.Shell).CreateShortcut({lnk}); \
         $s.TargetPath={exe}; $s.Arguments={args}; $s.WorkingDirectory={wd}; $s.Save()",
        lnk = ps_quote(&lnk.to_string_lossy()),
        exe = ps_quote(&target_exe.to_string_lossy()),
        args = ps_quote(arguments),
        wd = ps_quote(&workdir.to_string_lossy()),
    )
}

/// 快捷方式固定参数（产品既有一键入口：dashboard）。
pub const LNK_ARGUMENTS: &str = "dashboard";
