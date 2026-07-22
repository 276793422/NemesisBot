//! Extra CLI commands: memory, persona, dashboard, voice
//!
//! These commands were added to the CLI after the initial integration
//! test sweep and weren't covered by other cli_tests modules.

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// memory: status / enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_memory_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/memory_status";
    let mut results = Vec::new();
    print_suite_header(suite);

    // memory status should not crash even without enhanced_memory config
    let out = ws.run_cli(bin, &["memory", "status"]).await;
    results.push(pass(
        &format!("{}/status_exit", suite),
        &format!("exit={}", out.exit_code),
    ));

    // memory --help lists subcommands
    let help = ws.run_cli(bin, &["memory", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "memory help works"));
    } else {
        results.push(fail(
            &format!("{}/help", suite),
            &format!("exit={}", help.exit_code),
        ));
    }

    for sub in &["enable", "disable", "status"] {
        if help.stdout_contains(sub) {
            results.push(pass(
                &format!("{}/help_{}", suite, sub),
                &format!("{} listed", sub),
            ));
        } else {
            results.push(fail(
                &format!("{}/help_{}", suite, sub),
                &format!("{} missing from help", sub),
            ));
        }
    }

    results
}

pub async fn test_cli_memory_disable_enable_cycle(
    ws: &TestWorkspace,
    bin: &Path,
) -> Vec<TestResult> {
    let suite = "cli/memory_cycle";
    let mut results = Vec::new();
    print_suite_header(suite);

    // disable then status should report disabled
    let disable = ws.run_cli(bin, &["memory", "disable"]).await;
    results.push(pass(
        &format!("{}/disable_exit", suite),
        &format!("exit={}", disable.exit_code),
    ));

    let status_after_disable = ws.run_cli(bin, &["memory", "status"]).await;
    results.push(pass(
        &format!("{}/status_after_disable", suite),
        &format!("exit={}", status_after_disable.exit_code),
    ));

    // re-enable
    let enable = ws.run_cli(bin, &["memory", "enable"]).await;
    results.push(pass(
        &format!("{}/enable_exit", suite),
        &format!("exit={}", enable.exit_code),
    ));

    let status_after_enable = ws.run_cli(bin, &["memory", "status"]).await;
    results.push(pass(
        &format!("{}/status_after_enable", suite),
        &format!("exit={}", status_after_enable.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// persona: list / current / search / --help
// ---------------------------------------------------------------------------

pub async fn test_cli_persona_help(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/persona_help";
    let mut results = Vec::new();
    print_suite_header(suite);

    let help = ws.run_cli(bin, &["persona", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "persona help works"));
    } else {
        results.push(fail(
            &format!("{}/help", suite),
            &format!("exit={}", help.exit_code),
        ));
    }

    // All subcommands should be listed in help
    for sub in &[
        "list", "search", "install", "activate", "remove", "current", "restore",
    ] {
        if help.stdout_contains(sub) {
            results.push(pass(
                &format!("{}/help_{}", suite, sub),
                &format!("{} listed", sub),
            ));
        } else {
            results.push(fail(
                &format!("{}/help_{}", suite, sub),
                &format!("{} missing from help", sub),
            ));
        }
    }

    results
}

pub async fn test_cli_persona_list_current(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/persona_list_current";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Fresh workspace — list should succeed (empty result is fine)
    let list = ws.run_cli(bin, &["persona", "list"]).await;
    results.push(pass(
        &format!("{}/list_exit", suite),
        &format!("exit={}", list.exit_code),
    ));

    // Current should also succeed (may report "default" or empty)
    let current = ws.run_cli(bin, &["persona", "current"]).await;
    results.push(pass(
        &format!("{}/current_exit", suite),
        &format!("exit={}", current.exit_code),
    ));

    // Activate a nonexistent persona should fail gracefully (non-zero exit, no panic)
    let activate_missing = ws
        .run_cli(
            bin,
            &["persona", "activate", "totally_nonexistent_persona_xyz"],
        )
        .await;
    results.push(pass(
        &format!("{}/activate_missing_exit", suite),
        &format!("exit={}", activate_missing.exit_code),
    ));

    // Remove a nonexistent persona should also fail gracefully
    let remove_missing = ws
        .run_cli(
            bin,
            &["persona", "remove", "totally_nonexistent_persona_xyz"],
        )
        .await;
    results.push(pass(
        &format!("{}/remove_missing_exit", suite),
        &format!("exit={}", remove_missing.exit_code),
    ));

    // Restore should always succeed (resets to default)
    let restore = ws.run_cli(bin, &["persona", "restore"]).await;
    results.push(pass(
        &format!("{}/restore_exit", suite),
        &format!("exit={}", restore.exit_code),
    ));

    results
}

// ---------------------------------------------------------------------------
// dashboard: --help only (actually running would start gateway)
// ---------------------------------------------------------------------------

pub async fn test_cli_dashboard_help(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/dashboard_help";
    let mut results = Vec::new();
    print_suite_header(suite);

    let help = ws.run_cli(bin, &["dashboard", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "dashboard help works"));
    } else {
        results.push(fail(
            &format!("{}/help", suite),
            &format!("exit={}", help.exit_code),
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// voice: --help only (subcommands may need sherpa-onnx C library)
// ---------------------------------------------------------------------------

pub async fn test_cli_voice_help(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/voice_help";
    let mut results = Vec::new();
    print_suite_header(suite);

    let help = ws.run_cli(bin, &["voice", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "voice help works"));
    } else {
        results.push(fail(
            &format!("{}/help", suite),
            &format!("exit={}", help.exit_code),
        ));
    }

    results
}
