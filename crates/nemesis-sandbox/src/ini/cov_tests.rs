// ini.rs 覆盖率补充（wave 5）：Sandboxie.ini 落盘（含父目录自动创建 +
// AllowNetworkAccess 双臂 + FileRootPath/管道路径模板）。

use super::*;

/// 父目录不存在时自动创建（27-31）+ 默认断网卡 n（net 分支）。
#[test]
fn write_ini_creates_missing_parent_dirs_and_blocks_network_by_default() {
    let _logs = crate::test_util::capture_logs();
    let home = tempfile::tempdir().unwrap();
    let ini_path = home
        .path()
        .join("config")
        .join("nested")
        .join("Sandboxie.ini");
    let box_root = home.path().join("box").join("NemesisBox");

    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, false)
        .expect("ini write must create parent dirs");

    let text = std::fs::read_to_string(&ini_path).unwrap();
    assert!(text.starts_with("[GlobalSettings]"), "{text}");
    assert!(text.contains("SbieCtrl_EnableAutoStart=n"), "{text}");
    assert!(text.contains("[NemesisBox]"), "{text}");
    assert!(text.contains("AllowNetworkAccess=n"), "{text}");
    assert!(text.contains("DropAdminRights=y"), "{text}");
    assert!(
        text.contains(r"OpenPipePath=\Device\NamedPipe\NemesisBox_*"),
        "{text}"
    );
    assert!(
        text.contains(&format!(r"FileRootPath=\??\{}", box_root.display())),
        "{text}"
    );
}

/// allow_network=true → y（box 级 WFP 开关 true 臂）。
#[test]
fn write_ini_reflects_allow_network_switch() {
    let _logs = crate::test_util::capture_logs();
    let home = tempfile::tempdir().unwrap();
    let ini_path = home.path().join("Sandboxie.ini");
    let box_root = home.path().join("box");

    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, true).expect("flat ini write");
    let text = std::fs::read_to_string(&ini_path).unwrap();
    assert!(text.contains("AllowNetworkAccess=y"), "{text}");
}
