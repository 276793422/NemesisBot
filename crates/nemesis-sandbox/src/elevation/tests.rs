//! `crate::elevation` 测试（S6 覆盖率批次）。
//!
//! `relaunch_elevated` 的**成功臂**结构性不可测：真实调用会弹 UAC 对话框
//! （红线，见 SandboxPaths 红线清单），且 fire-and-forget 无法在测试里观测
//! 副作用。**失败臂**可用「不存在的 exe」确定性触发（SE_ERR_FNF，不弹
//! UAC），R5 批次（2026-08-27）已测（见文件末尾）。`is_elevated` 走
//! `GetTokenInformation(TokenElevation)` 只读直查（不再依赖 net.exe PATH 与
//! LanmanServer 服务状态），测试钉「net session 成功 ⇒ 必须判定提权」的单向
//! 蕴含——反向不蕴含（服务停转/net.exe 缺失时提权进程也会失败，正是弃用
//! net session 探测的原因）。

use super::*;

#[cfg(windows)]
#[test]
fn net_session_success_implies_elevated() {
    let status = std::process::Command::new("net")
        .arg("session")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    // 单向蕴含：net session 成功 = 提权的充分证据；失败方向不做断言
    // （LanmanServer 停转 / net.exe 不可用的窗口里，TokenElevation 直查
    // 比旧 net session 探测更诚实——宁可多弹一次 UAC，不误判非管理员）。
    if let Ok(s) = status
        && s.success()
    {
        assert!(
            is_elevated(),
            "net session 成功（提权充分证据）⇒ TokenElevation 必须为 true"
        );
    }
}

#[cfg(not(windows))]
#[test]
fn is_elevated_always_false_off_windows() {
    assert!(!is_elevated());
    assert!(relaunch_elevated(std::path::Path::new("/x"), &[]).is_err());
}

// ---------------------------------------------------------------------------
// R5 覆盖率批次（2026-08-27）：relaunch_elevated 的**失败臂**可以确定性
// 触发——ShellExecuteW("runas", <不存在的 exe>) 在解析 UAC 前就报
// SE_ERR_FNF（实测 PowerShell Start-Process -Verb RunAs 同层行为：立即
// "系统找不到指定的文件"，不弹对话框）。成功臂仍为红线（真 UAC）。
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn relaunch_elevated_missing_exe_bails_without_uac_prompt() {
    let err = relaunch_elevated(
        std::path::Path::new(r"C:\nonexistent_dir_r5\missing.exe"),
        &["--internal".to_string(), "--flag with space".to_string()],
    )
    .unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("ShellExecuteW"), "{msg}");
    assert!(msg.contains("1223"), "提示串里保留 UAC 拒绝码语义: {msg}");
}
