// kmdutil.rs 覆盖率补充（wave 5）：run 的成功 / 容错失败 / 严格失败 /
// spawn 失败四臂 + format_command 文本。
//
// 纪律：不跑任何真 KmdUtil（机器上没有）——用系统自带的只读外部程序
// （Windows: where.exe；Unix: sh）扮演「能跑/能退非零的外部命令」，
// 全程无副作用、不碰 SCM/注册表。

use super::*;

/// 必然成功的外部只读命令（where.exe 查一个必然存在的文件）。
fn ok_cmd() -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("C:\\Windows\\System32\\where.exe");
        c.arg("cmd.exe");
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg("true");
        c
    }
}

/// 必然非零退出的外部只读命令（查一个必然不存在的文件名）。
fn failing_cmd() -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = Command::new("C:\\Windows\\System32\\where.exe");
        c.arg("nb_cov_definitely_not_on_disk_xyz.exe");
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = Command::new("sh");
        c.arg("-c").arg("exit 7");
        c
    }
}

/// 成功臂：exit 0 → debug 日志 + Ok（含 format_command 求值）。
#[test]
fn run_ok_on_zero_exit() {
    let _logs = crate::test_util::capture_logs();
    run(ok_cmd(), false).expect("zero exit is Ok");
}

/// 容错臂：非零 + tolerant=true → warn 但 Ok。
#[test]
fn run_tolerant_swallows_nonzero_exit() {
    let _logs = crate::test_util::capture_logs();
    run(failing_cmd(), true).expect("tolerant run maps non-zero to Ok");
}

/// 严格臂：非零 + tolerant=false → bail 带 cmd/stdout 上下文。
#[test]
fn run_strict_bails_on_nonzero_exit() {
    let _logs = crate::test_util::capture_logs();
    let err = run(failing_cmd(), false).expect_err("strict run must fail");
    let msg = err.to_string();
    assert!(msg.contains("kmdutil failed"), "{msg}");
    assert!(msg.contains("cmd="), "cmd echo present: {msg}");
}

/// spawn 失败臂：程序不存在 → 带 format_command 文案的 Err。
#[test]
fn run_missing_program_reports_spawn_context() {
    let _logs = crate::test_util::capture_logs();
    let mut c = Command::new("nb_cov_no_such_binary_xyz.exe");
    c.arg("whatever");
    let err = run(c, true).expect_err("missing program must fail");
    assert!(err.to_string().contains("spawn"), "{err}");
}

/// 命令构建器：install_driver / install_service / start / stop / delete 的
/// 参数形状（纯构建函数，顺手全跑一遍锁形状）。
#[test]
fn command_builders_shape_args() {
    let kmd = std::path::PathBuf::from(r"C:\rt\KmdUtil.exe");
    let sys = std::path::PathBuf::from(r"C:\rt\SbieDrv.sys");
    let exe = std::path::PathBuf::from(r"C:\rt\SbieSvc.exe");
    let dll = std::path::PathBuf::from(r"C:\rt\SbieMsg.dll");

    let s = format_command(&install_driver(&kmd, &sys, &dll));
    assert!(s.contains("install") && s.contains("SbieDrv"), "{s}");
    assert!(
        s.contains("type=kernel") && s.contains("start=demand"),
        "{s}"
    );
    assert!(s.contains("altitude="), "{s}");

    let s = format_command(&install_service(&kmd, &exe, &dll));
    assert!(s.contains("SbieSvc") && s.contains("type=own"), "{s}");

    let s = format_command(&start(&kmd, crate::USERMODE_SERVICE));
    assert!(s.contains("start") && s.contains("SbieSvc"), "{s}");
    let s = format_command(&stop(&kmd, crate::DRIVER_SERVICE));
    assert!(s.contains("stop") && s.contains("SbieDrv"), "{s}");
    let s = format_command(&delete(&kmd, crate::DRIVER_SERVICE));
    assert!(s.contains("delete"), "{s}");
}
