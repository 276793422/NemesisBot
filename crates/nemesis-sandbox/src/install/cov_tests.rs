// install.rs 覆盖率补充（wave 5）：wait_for_state / wait_for_installed 轮询
// 助手 + stop(purge) 容错链——全部只读 SCM 查询 + 临时目录文件操作，
// 不装驱动、不起服务（KmdUtil.exe 不存在时 kmdutil::run 在 spawn 处失败，
// stop 的 `let _` 容错语义正好把每一步都走到）。

use super::*;
use std::time::Duration;

/// wait_for_state：目标就是当前态 → 首探即返回（32-41）。
#[test]
fn wait_for_state_returns_first_probe_when_already_at_target() {
    let _logs = crate::test_util::capture_logs();
    let s = wait_for_state(
        "nb_cov_missing_svc_xyz",
        ServiceState::NotFound,
        Duration::ZERO,
    );
    assert_eq!(s, ServiceState::NotFound);
}

/// wait_for_installed：服务不存在 → 轮询到超时仍 NotFound（46-55）。
#[test]
fn wait_for_installed_times_out_on_missing_service() {
    let _logs = crate::test_util::capture_logs();
    let s = wait_for_installed("nb_cov_missing_svc_xyz", Duration::from_millis(350));
    assert_eq!(s, ServiceState::NotFound);
}

/// stop(purge=true)：无引擎环境全链容错（kmdutil spawn 失败被 `let _` 吞掉、
/// SCM 查询 NotFound、purge 删临时文件）（130-151）。
#[test]
fn stop_with_purge_is_tolerant_without_engine_and_removes_files() {
    let _logs = crate::test_util::capture_logs();
    let home = tempfile::tempdir().unwrap();
    let paths = crate::SandboxPaths::new(home.path());
    // 造些可被 purge 的文件。
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    std::fs::create_dir_all(&paths.box_root).unwrap();
    std::fs::write(&paths.ini_path, b"[GlobalSettings]\n").unwrap();

    stop(&paths, true).expect("stop must be Ok even with no engine installed");
    assert!(!paths.runtime_dir.exists(), "purge removes runtime");
    assert!(!paths.box_root.exists(), "purge removes box root");
    assert!(!paths.ini_path.exists(), "purge removes ini");
}

/// stop(purge=false)：文件保留（140 分支 false 臂）。
#[test]
fn stop_without_purge_keeps_files() {
    let _logs = crate::test_util::capture_logs();
    let home = tempfile::tempdir().unwrap();
    let paths = crate::SandboxPaths::new(home.path());
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    std::fs::write(&paths.ini_path, b"[GlobalSettings]\n").unwrap();

    stop(&paths, false).expect("stop must be Ok");
    assert!(paths.runtime_dir.exists(), "no purge keeps runtime");
    assert!(paths.ini_path.exists(), "no purge keeps ini");
}
