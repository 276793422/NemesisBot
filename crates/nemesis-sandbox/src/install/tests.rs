//! Tests for install-time config helpers.
//!
//! Kept in a separate file per the project's "tests in `<stem>/tests.rs" discipline.

use super::*;

/// `read_allow_network` reads the box-network switch from `<home>/config.json` so
/// `start`/`ensure_installed` honor it when rewriting Sandboxie.ini. A wrong read
/// would silently flip the box back to offline on every engine re-activation.
#[test]
fn read_allow_network_reads_executor_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    let paths = SandboxPaths::new(home);

    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": true } }"#,
    )
    .expect("seed config (true)");
    assert!(
        read_allow_network(&paths),
        "allow_network=true must read as true"
    );

    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": false } }"#,
    )
    .expect("seed config (false)");
    assert!(
        !read_allow_network(&paths),
        "allow_network=false must read as false"
    );
}

/// Missing field / missing section / missing file / unparseable file all default to
/// false (network blocked) — a fresh or partially-broken install must stay offline
/// until the user explicitly opts in, and must never panic.
#[test]
fn read_allow_network_defaults_false_when_unset_or_broken() {
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    let paths = SandboxPaths::new(home);

    // No config.json at all.
    assert!(!read_allow_network(&paths), "no config.json → false");

    // config.json without an executor section.
    std::fs::write(home.join("config.json"), r#"{ "other": 1 }"#).expect("write config");
    assert!(!read_allow_network(&paths), "no executor section → false");

    // executor without allow_network.
    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true } }"#,
    )
    .expect("write config");
    assert!(
        !read_allow_network(&paths),
        "executor without allow_network → false"
    );

    // Unparseable config.json — must not panic, must return false.
    std::fs::write(home.join("config.json"), "not json {").expect("write config");
    assert!(
        !read_allow_network(&paths),
        "unparseable config.json → false, not panic"
    );
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：
// wait_for_state / wait_for_installed 的立即命中与超时臂（只读 sc query）、
// start 的 verify_runtime 前置 bail、stop 的 tolerant 流程（tempdir 缺
// KmdUtil.exe → spawn 失败被吞，零系统副作用）+ purge、stop_service、
// start_service / ensure_installed 的前置 bail。
// 真装驱动/服务/写 HKLM 的深层臂 = 红线结构性不测（见批次报告）。
// ---------------------------------------------------------------------------

/// 一个肯定不存在的服务名：sc query 1060 → NotFound（跨机器确定性）。
const MISSING_SVC: &str = "NemesisS6DefinitelyMissing9527";

#[test]
fn wait_for_state_immediate_hit_and_timeout() {
    // 立即命中：目标态 == 当前态（垃圾名 → NotFound）→ 不睡直接返回
    let t0 = std::time::Instant::now();
    assert_eq!(
        wait_for_state(
            MISSING_SVC,
            ServiceState::NotFound,
            std::time::Duration::from_secs(5)
        ),
        ServiceState::NotFound
    );
    assert!(
        t0.elapsed() < std::time::Duration::from_secs(2),
        "命中态必须立即返回"
    );
    // 超时臂：目标态永不达成 → 睡满循环后返回当前态
    let t1 = std::time::Instant::now();
    assert_eq!(
        wait_for_state(
            MISSING_SVC,
            ServiceState::Running,
            std::time::Duration::from_millis(400)
        ),
        ServiceState::NotFound
    );
    assert!(
        t1.elapsed() >= std::time::Duration::from_millis(300),
        "超时臂必须真等"
    );
}

#[test]
fn wait_for_installed_timeout_when_service_missing() {
    let s = wait_for_installed(MISSING_SVC, std::time::Duration::from_millis(200));
    assert_eq!(s, ServiceState::NotFound, "不存在 + 超时 → NotFound");
}

#[cfg(windows)]
#[test]
fn wait_for_installed_returns_early_for_real_service() {
    // Themes 存在于所有桌面 Windows：立即返回非 NotFound（机器依赖，宽容）
    let s = wait_for_installed("Themes", std::time::Duration::from_secs(5));
    assert_ne!(s, ServiceState::NotFound);
}

#[test]
fn start_bails_on_missing_runtime_before_touching_system() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    // 空 tempdir：verify_runtime 先失败——绝不能走到 KmdUtil/注册表
    let err = start(&paths).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("runtime files missing"), "{msg}");
    assert!(msg.contains("sandbox install"), "{msg}");
}

#[test]
fn stop_is_tolerant_and_purge_removes_only_our_tempdirs() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    // 预置会被 purge 删的三处 + 一个 purge 不该碰的外层文件
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    std::fs::write(paths.runtime_dir.join("SbieDrv.sys"), b"x").unwrap();
    std::fs::create_dir_all(&paths.box_root).unwrap();
    std::fs::write(&paths.ini_path, b"[NemesisBox]").unwrap();
    let keep = tmp.path().join("keep.txt");
    std::fs::write(&keep, b"keep").unwrap();

    // tempdir 里没有 KmdUtil.exe → 四个 stop/delete 的 spawn 全部失败并被
    // tolerant 吞掉（零系统副作用）；sc query 只读
    stop(&paths, true).expect("tolerant stop 必须成功");

    assert!(!paths.runtime_dir.exists(), "purge 删 runtime");
    assert!(!paths.box_root.exists(), "purge 删 box root");
    assert!(!paths.ini_path.exists(), "purge 删 ini");
    assert!(keep.exists(), "purge 绝不能删 runtime/box/ini 之外的文件");
}

#[test]
fn stop_without_purge_keeps_files() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    stop(&paths, false).expect("tolerant stop ok");
    assert!(
        paths.runtime_dir.exists(),
        "无 purge 保留 runtime（可再 start）"
    );
}

#[test]
fn stop_service_missing_kmdutil_is_ok() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    stop_service(&paths).expect("spawn 失败被 tolerant 吞 → Ok");
}

#[cfg(windows)]
#[test]
fn start_service_consistent_with_current_sbievc_state() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    let running = matches!(service_state(USERMODE_SERVICE), ServiceState::Running);
    let r = start_service(&paths);
    if running {
        assert!(r.is_ok(), "SbieSvc 已在跑 → 复用直接 Ok（零副作用）");
    } else {
        // tempdir 缺 KmdUtil.exe → spawn Err → context 后 Err
        let err = r.unwrap_err();
        assert!(format!("{err:#}").contains("start SbieSvc"));
    }
}

#[test]
fn ensure_installed_bails_on_missing_runtime() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    // 空 tempdir → verify_runtime 先失败：绝不进归属门/注册表/KmdUtil
    let err = ensure_installed(&paths).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("runtime files missing"), "{msg}");
}

// ---------------------------------------------------------------------------
// R5 覆盖率批次（2026-08-27）：verify_runtime 通过后的 start /
// ensure_installed 深层臂——用「dummy 内容的假 runtime」让 verify 放行，
// 但 KmdUtil.exe 非 PE → spawn 必败（零系统副作用），钉住错误传播路径。
// ---------------------------------------------------------------------------

/// 造一套最小假 runtime：verify_runtime 要求的 5 个文件（内容 dummy，
/// 不可执行——正是要让 spawn 失败）。
fn plant_fake_runtime(paths: &SandboxPaths) {
    std::fs::create_dir_all(&paths.runtime_dir).unwrap();
    for f in [
        "SbieDrv.sys",
        "SbieSvc.exe",
        "SbieMsg.dll",
        "KmdUtil.exe",
        "Start.exe",
    ] {
        std::fs::write(paths.runtime_dir.join(f), b"dummy-not-a-pe").unwrap();
    }
}

#[test]
fn start_propagates_kmdutil_spawn_failure_after_runtime_ok() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    plant_fake_runtime(&paths);
    // dummy KmdUtil.exe 非 PE → kmdutil::run spawn 失败 → start 的
    // "install SbieDrv" context 传播。确定性、零系统副作用。
    let err = start(&paths).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("install SbieDrv"), "{msg}");
    assert!(
        msg.contains("spawn"),
        "失败必须发生在 spawn 层（非真 KmdUtil 执行）: {msg}"
    );
}

#[cfg(windows)]
#[test]
fn ensure_installed_bails_on_foreign_or_fails_at_service_start() {
    if crate::elevation::is_elevated() {
        return; // 提权 + 干净机器会真写 HKLM/真注册服务（dummy KmdUtil 也被
        // tolerant 吞掉不执行真操作，但 set_ini_path 会真写）——跳过
    }
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    plant_fake_runtime(&paths);
    let err = ensure_installed(&paths).unwrap_err();
    let msg = format!("{err:#}");
    // 两种机器形态都确定性 Err：
    // - 本机（SbieSvc/SbieDrv 注册在别处）：foreign 归属门 bail；
    // - 干净机器：tolerant 主体全吞 → start_service 的 KmdUtil spawn 失败。
    assert!(
        msg.contains("foreign Sandboxie") || msg.contains("start SbieSvc"),
        "{msg}"
    );
}

#[cfg(windows)]
#[test]
fn start_fails_at_set_ini_path_without_elevation() {
    if crate::elevation::is_elevated() {
        return; // 提权会真写 HKLM\SbieDrv\IniPath（重定向真沙盒 ini），跳过
    }
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let paths = SandboxPaths::new(tmp.path());
    plant_fake_runtime(&paths);
    // KmdUtil.exe 用 rundll32.exe 顶替：spawnable PE + 任意参数 exit 0
    // （实测），让 install_driver 这步"成功"，推进到 set_ini_path——非提权下
    // reg add HKLM 必 ACCESS DENIED → 确定性 Err，零注册表副作用。
    std::fs::copy(r"C:\Windows\System32\rundll32.exe", paths.kmdutil()).unwrap();
    let err = start(&paths).unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("set IniPath"), "{msg}");
    assert!(msg.contains("reg add"), "{msg}");
}
