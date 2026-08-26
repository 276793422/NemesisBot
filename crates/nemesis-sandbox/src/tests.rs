//! `crate::SandboxPaths` 路径布局与 `verify_runtime`（S6 覆盖率批次）。

use super::*;

#[test]
fn sandbox_paths_layout_under_home() {
    let home = std::path::Path::new(r"C:\BotHome");
    let p = SandboxPaths::new(home);
    let s = |pb: &std::path::Path| pb.to_string_lossy().to_string();
    assert_eq!(s(&p.home), r"C:\BotHome");
    assert_eq!(
        s(&p.runtime_dir),
        r"C:\BotHome\workspace\tools\sandboxie\runtime"
    );
    assert_eq!(
        s(&p.ini_path),
        r"C:\BotHome\workspace\tools\sandboxie\Sandboxie.ini"
    );
    assert_eq!(
        s(&p.box_root),
        r"C:\BotHome\workspace\tools\sandboxie\box\NemesisBox"
    );
    // 访问器：五个运行时文件的固定文件名
    assert!(s(&p.kmdutil()).ends_with(r"runtime\KmdUtil.exe"));
    assert!(s(&p.start_exe()).ends_with(r"runtime\Start.exe"));
    assert!(s(&p.sbiedrv_sys()).ends_with(r"runtime\SbieDrv.sys"));
    assert!(s(&p.sbiesvc_exe()).ends_with(r"runtime\SbieSvc.exe"));
    assert!(s(&p.sbiemsg_dll()).ends_with(r"runtime\SbieMsg.dll"));
}

#[test]
fn verify_runtime_ok_when_all_five_files_exist() {
    let tmp = tempfile::tempdir().unwrap();
    let p = SandboxPaths::new(tmp.path());
    for name in [
        "SbieDrv.sys",
        "SbieSvc.exe",
        "SbieMsg.dll",
        "KmdUtil.exe",
        "Start.exe",
    ] {
        std::fs::create_dir_all(&p.runtime_dir).unwrap();
        std::fs::write(p.runtime_dir.join(name), b"x").unwrap();
    }
    p.verify_runtime().expect("五个文件齐 → Ok");
}

#[test]
fn verify_runtime_bails_on_empty_dir_naming_first_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let p = SandboxPaths::new(tmp.path());
    let err = p.verify_runtime().unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("SbieDrv.sys"), "{msg}");
    assert!(msg.contains("missing after extract"), "{msg}");
    assert!(msg.contains("runtime"), "{msg}");
}

#[test]
fn verify_runtime_reports_each_missing_file() {
    // 补齐前四个、缺 Start.exe → 错误必须点名 Start.exe（而非首个）
    let tmp = tempfile::tempdir().unwrap();
    let p = SandboxPaths::new(tmp.path());
    for name in ["SbieDrv.sys", "SbieSvc.exe", "SbieMsg.dll", "KmdUtil.exe"] {
        std::fs::create_dir_all(&p.runtime_dir).unwrap();
        std::fs::write(p.runtime_dir.join(name), b"x").unwrap();
    }
    let err = p.verify_runtime().unwrap_err();
    assert!(format!("{err:#}").contains("Start.exe"));
}
