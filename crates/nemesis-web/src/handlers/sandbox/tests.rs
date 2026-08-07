//! Tests for the sandbox handler.
//!
//! Kept in a separate file (not inlined in `sandbox.rs`) per the project's
//! "tests live in `<stem>/tests.rs`" discipline.

use super::*;

/// Field-merge fix: editing `enabled`/`sandbox` via [`update_executor`] must NOT
/// reset `allow_network`. The pre-fix code did `c.executor = Some({enabled, sandbox})`,
/// which clobbered `allow_network` on every `start`/`stop`. This test exercises the
/// CLI / no-store fallback path (read-merge-write of config.json) — the path that
/// carried the bug — and guards against regressing back to an overwrite.
#[test]
fn update_executor_preserves_allow_network_across_sibling_edits() {
    // The CLI fallback only runs when no process-global ConfigStore is installed.
    // A test process normally has none; if something installed one we can't isolate
    // the CLI path, so skip rather than flake (mirrors the test-isolation stance).
    if nemesis_config::global().is_some() {
        eprintln!(
            "skip update_executor_preserves_allow_network: \
             process-global ConfigStore installed, can't isolate CLI path"
        );
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let home = dir.path();
    std::fs::write(
        home.join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": true, "allow_network": true } }"#,
    )
    .expect("seed config.json");

    // Simulate `stop`: flip enabled+sandbox off — exactly what set_executor_config does.
    update_executor(home, |e| {
        e.enabled = false;
        e.sandbox = false;
    })
    .expect("update_executor");

    let executor = serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(home.join("config.json")).expect("read config.json"),
    )
    .expect("parse config.json")
    .get("executor")
    .expect("executor section")
    .clone();
    assert_eq!(executor["enabled"], false);
    assert_eq!(executor["sandbox"], false);
    assert_eq!(
        executor["allow_network"],
        true,
        "allow_network must survive enabled/sandbox edits (field-merge fix regressed)"
    );
}
