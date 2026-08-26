//! `crate::elevation` 测试（S6 覆盖率批次）。
//!
//! `relaunch_elevated` 是结构性不可测：真实调用会弹 UAC 对话框（红线，
//! 见 SandboxPaths 红线清单），且 fire-and-forget 无法在测试里观测副作用。
//! `is_elevated` 走只读 `net session`，可测——断言它与独立执行的
//! `net session` 退出码一致（同一底层信号，钉住映射不漂移）。

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
