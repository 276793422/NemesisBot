//! Tests for `kmdutil` command builders + `run` 的两态退出（Phase 3 覆盖率，
//! 2026-08-25）。reg.exe 写 HKLM 的三个函数需要管理员 → 结构性不测。

use super::*;

#[test]
fn install_driver_builds_official_nsis_sequence() {
    let c = install_driver(
        Path::new(r"C:\k\KmdUtil.exe"),
        Path::new(r"C:\k\SbieDrv.sys"),
        Path::new(r"C:\k\SbieMsg.dll"),
    );
    assert_eq!(c.get_program().to_string_lossy(), r"C:\k\KmdUtil.exe");
    let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert_eq!(
        args,
        vec![
            "install", "SbieDrv", r"C:\k\SbieDrv.sys", "type=kernel", "start=demand",
            "msgfile=C:\\k\\SbieMsg.dll", "altitude=86900",
        ]
    );
}

#[test]
fn install_service_builds_own_auto_ugroup_sequence() {
    let c = install_service(
        Path::new(r"C:\k\KmdUtil.exe"),
        Path::new(r"C:\k\SbieSvc.exe"),
        Path::new(r"C:\k\SbieMsg.dll"),
    );
    let args: Vec<String> = c.get_args().map(|a| a.to_string_lossy().to_string()).collect();
    assert_eq!(
        args,
        vec![
            "install", "SbieSvc", r"C:\k\SbieSvc.exe", "type=own", "start=auto",
            "display=Sandboxie Service", "group=UIGroup", "msgfile=C:\\k\\SbieMsg.dll",
        ]
    );
}

#[test]
fn start_stop_delete_build_simple_verbs() {
    let args = |c: &Command| -> Vec<String> {
        c.get_args().map(|a| a.to_string_lossy().to_string()).collect()
    };
    assert_eq!(args(&start(Path::new("k"), "SbieSvc")), vec!["start", "SbieSvc"]);
    assert_eq!(args(&stop(Path::new("k"), "SbieDrv")), vec!["stop", "SbieDrv"]);
    assert_eq!(args(&delete(Path::new("k"), "SbieSvc")), vec!["delete", "SbieSvc"]);
}

/// 造一个假 KmdUtil：exit `code`，stderr 打一行。
fn fake_kmdutil(dir: &Path, name: &str, code: u32) -> std::path::PathBuf {
    if cfg!(windows) {
        let p = dir.join(format!("{name}.bat"));
        std::fs::write(
            &p,
            format!("@echo off\r\necho kmdutil-failure-noise 1>&2\r\nexit /b {code}\r\n"),
        )
        .unwrap();
        p
    } else {
        let p = dir.join(format!("{name}.sh"));
        std::fs::write(&p, format!("#!/bin/sh\necho kmdutil-failure-noise 1>&2\nexit {code}\n")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        p
    }
}

#[test]
fn run_success_exit_code_zero_is_ok() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_kmdutil(dir.path(), "ok0", 0);
    let cmd = Command::new(&fake);
    assert!(run(cmd, false).is_ok());
}

#[test]
fn run_nonzero_strict_propagates_with_stderr_in_message() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_kmdutil(dir.path(), "bad1", 1);
    let cmd = Command::new(&fake);
    let err = run(cmd, false).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("kmdutil failed"), "{msg}");
    assert!(msg.contains("kmdutil-failure-noise"), "stderr 必须带回：{msg}");
}

#[test]
fn run_nonzero_tolerant_swallows_but_still_ok() {
    let dir = tempfile::tempdir().unwrap();
    let fake = fake_kmdutil(dir.path(), "tol", 1);
    let cmd = Command::new(&fake);
    assert!(run(cmd, true).is_ok(), "tolerant=true：非零退出只记日志不传播");
}

#[test]
fn run_missing_binary_is_err_with_spawn_context() {
    let cmd = Command::new(r"Z:\definitely\missing\KmdUtil.exe");
    let err = run(cmd, false).unwrap_err();
    assert!(format!("{err:#}").contains("spawn"), "{err:#}");
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次：subscriber 下让 run 的 debug!/warn! 参数行真实求值。
// ---------------------------------------------------------------------------

#[test]
fn run_logs_success_and_tolerant_failure_under_subscriber() {
    let _log = crate::test_util::capture_logs();
    let dir = tempfile::tempdir().unwrap();
    // 成功 → tracing::debug! 参数行
    let ok = Command::new(&fake_kmdutil(dir.path(), "s6ok", 0));
    assert!(run(ok, false).is_ok());
    // tolerant 失败 → tracing::warn! 参数行
    let bad = Command::new(&fake_kmdutil(dir.path(), "s6bad", 3));
    assert!(run(bad, true).is_ok());
}
