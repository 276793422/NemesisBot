//! Tests for Sandboxie.ini generation.
//!
//! Kept in a separate file per the project's "tests in `<stem>/tests.rs` discipline.

use super::*;

/// `allow_network` must map to the correct `AllowNetworkAccess=y|n` directive in the
/// generated ini. This is the config→ini half of the network switch — a wrong or
/// inverted mapping would flip the whole box's network state.
#[test]
fn write_sandboxie_ini_emits_allow_network_directive() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ini_path = dir.path().join("Sandboxie.ini");
    let box_root = dir.path().join("box").join("NemesisBox");

    // allow_network=true → AllowNetworkAccess=y (and NOT =n)
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, true).expect("write ini (true)");
    let content = std::fs::read_to_string(&ini_path).expect("read ini");
    assert!(
        content.contains("AllowNetworkAccess=y"),
        "allow_network=true must produce AllowNetworkAccess=y, got:\n{content}"
    );
    assert!(
        !content.contains("AllowNetworkAccess=n"),
        "allow_network=true must NOT also emit AllowNetworkAccess=n, got:\n{content}"
    );

    // allow_network=false → AllowNetworkAccess=n
    write_sandboxie_ini(&ini_path, "NemesisBox", &box_root, false).expect("write ini (false)");
    let content = std::fs::read_to_string(&ini_path).expect("read ini");
    assert!(
        content.contains("AllowNetworkAccess=n"),
        "allow_network=false must produce AllowNetworkAccess=n, got:\n{content}"
    );
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：subscriber 下让
// write_sandboxie_ini 的 info! 参数行真实求值；并钉 ini 内容的关键安全行。
// ---------------------------------------------------------------------------

#[test]
fn write_ini_logs_and_pins_security_directives() {
    let _log = crate::test_util::capture_logs();
    let tmp = tempfile::tempdir().unwrap();
    let ini = tmp.path().join("Sandboxie.ini");
    let box_root = tmp.path().join("box").join("NemesisBox");
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, false).unwrap();
    let text = std::fs::read_to_string(&ini).unwrap();
    // 安全命门行逐条钉死（改任何一行都会被这条测试抓住）
    assert!(text.contains("[NemesisBox]"));
    assert!(text.contains("Enabled=y"));
    assert!(text.contains("AllowNetworkAccess=n"), "默认断网");
    assert!(text.contains("DropAdminRights=y"), "盒内去管理员");
    assert!(text.contains(r"OpenPipePath=\Device\NamedPipe\NemesisBox_*"));
    assert!(
        text.contains("SbieCtrl_EnableAutoStart=n"),
        "headless 禁 GUI 自启"
    );
    assert!(text.contains(&format!(r"FileRootPath=\??\{}", box_root.display())));
    // 幂等重写（allow_network 翻转）
    write_sandboxie_ini(&ini, "NemesisBox", &box_root, true).unwrap();
    assert!(
        std::fs::read_to_string(&ini)
            .unwrap()
            .contains("AllowNetworkAccess=y")
    );
}
