//! `crate::elevation` 测试（S6 覆盖率批次）。
//!
//! `relaunch_elevated` 的**成功臂**结构性不可测：真实调用会弹 UAC 对话框
//! （红线，见 SandboxPaths 红线清单），且 fire-and-forget 无法在测试里观测
//! 副作用。**失败臂**可用「不存在的 exe」确定性触发（SE_ERR_FNF，不弹
//! UAC），R5 批次（2026-08-27）已测（见文件末尾）。`is_elevated` 走只读
//! `net session`，可测——断言它与独立执行的 `net session` 退出码一致
//! （同一底层信号，钉住映射不漂移）。

use super::*;

#[cfg(windows)]
#[test]
fn is_elevated_matches_net_session_exit_code() {
    let status = std::process::Command::new("net")
        .arg("session")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(s) => assert_eq!(is_elevated(), s.success(), "is_elevated == net session 成功位"),
        // net.exe 不可用时 is_elevated 内部同样 Err → false；两边一致性仍成立
        Err(_) => assert!(!is_elevated()),
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
