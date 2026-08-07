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
