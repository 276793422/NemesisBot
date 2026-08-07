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
    assert!(read_allow_network(&paths), "allow_network=true must read as true");

    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": false } }"#,
    )
    .expect("seed config (false)");
    assert!(!read_allow_network(&paths), "allow_network=false must read as false");
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
    assert!(!read_allow_network(&paths), "executor without allow_network → false");

    // Unparseable config.json — must not panic, must return false.
    std::fs::write(home.join("config.json"), "not json {").expect("write config");
    assert!(!read_allow_network(&paths), "unparseable config.json → false, not panic");
}
