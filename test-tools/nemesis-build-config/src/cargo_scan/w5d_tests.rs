//! W5d batch tests: real-file scanning, error paths and ordering contracts.

use super::*;
use std::fs;

#[test]
fn w5d_scan_file_reads_features_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("Cargo.toml");
    fs::write(
        &p,
        r#"
[package]
name = "nemesisbot"

[features]
default = ["channels-web"]
channels-web = ["nemesis-channels/web"]
sandbox = ["dep:nemesis-sandbox"]
"#,
    )
    .unwrap();
    let s = scan_file(&p).unwrap();
    assert_eq!(s.names(), vec!["channels-web".to_string(), "sandbox".to_string()]);
    assert!(s.is_default("channels-web"));
    assert!(!s.is_default("sandbox"));
}

#[test]
fn w5d_scan_file_missing_is_err_with_path() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("no-Cargo.toml");
    let err = scan_file(&p).unwrap_err().to_string();
    assert!(err.contains("read"), "err was: {err}");
    assert!(err.contains("no-Cargo.toml"), "err was: {err}");
}

#[test]
fn w5d_scan_file_malformed_is_err() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("Cargo.toml");
    fs::write(&p, "[features\nbroken").unwrap();
    let err = scan_file(&p).unwrap_err().to_string();
    assert!(err.contains("parse"), "err was: {err}");
}

#[test]
fn w5d_names_are_sorted() {
    // BTreeMap keys => deterministic sorted order (drives scaffold order)
    let s = scan_text(
        r#"
[features]
zeta = []
alpha = []
mid = []
"#,
    )
    .unwrap();
    assert_eq!(s.names(), vec!["alpha".to_string(), "mid".to_string(), "zeta".to_string()]);
}

#[test]
fn w5d_empty_default_list_enables_nothing() {
    let s = scan_text("[features]\ndefault = []\nfoo = []\n").unwrap();
    assert!(s.default_enabled().is_empty());
    assert!(!s.is_default("foo"));
    assert!(!s.is_default("default"));
}

#[test]
fn w5d_absent_default_key_enables_nothing() {
    // features table exists but has no `default` entry
    let s = scan_text("[features]\nfoo = []\n").unwrap();
    assert!(s.default_enabled().is_empty());
    assert!(!s.is_default("foo"));
}

#[test]
fn w5d_default_enabled_returns_exactly_the_list() {
    let s = scan_text(
        r#"
[features]
default = ["b", "a"]
a = []
b = []
c = []
"#,
    )
    .unwrap();
    // order inside the default list is preserved verbatim
    assert_eq!(s.default_enabled(), vec!["b".to_string(), "a".to_string()]);
    assert!(s.is_default("a") && s.is_default("b"));
    assert!(!s.is_default("c"));
}

#[test]
fn w5d_features_table_with_only_default() {
    let s = scan_text("[features]\ndefault = [\"x\"]\n").unwrap();
    assert!(s.names().is_empty());
    assert_eq!(s.default_enabled(), vec!["x".to_string()]);
    // "x" is default-enabled but not itself a declared feature name
    assert!(s.is_default("x"));
}
