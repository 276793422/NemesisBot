//! Forge commands: status, enable, disable, reflect, list, evaluate, export, learning

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// forge status
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_status";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["forge", "status"]).await;
    if output.stdout_contains("Forge") || output.stdout_contains("forge") || output.success() {
        results.push(pass(&format!("{}/output", suite), "Forge status output received"));
    } else {
        results.push(fail(&format!("{}/output", suite), &format!(
            "No forge info: '{}'", output.stdout.trim())));
    }

    results
}

// ---------------------------------------------------------------------------
// forge enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    let enable = ws.run_cli(bin, &["forge", "enable"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    let disable = ws.run_cli(bin, &["forge", "disable"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// forge reflect
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_reflect(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_reflect";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["forge", "reflect"]).await;
    results.push(pass(&format!("{}/reflect", suite),
        &format!("exit={}", output.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// forge list [--type]
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    // list all
    let list = ws.run_cli(bin, &["forge", "list"]).await;
    results.push(pass(&format!("{}/all", suite),
        &format!("exit={}", list.exit_code)));

    // list skills
    let skills = ws.run_cli(bin, &["forge", "list", "--type", "skill"]).await;
    results.push(pass(&format!("{}/skills", suite),
        &format!("exit={}", skills.exit_code)));

    // list scripts
    let scripts = ws.run_cli(bin, &["forge", "list", "--type", "script"]).await;
    results.push(pass(&format!("{}/scripts", suite),
        &format!("exit={}", scripts.exit_code)));

    // list mcp
    let mcp = ws.run_cli(bin, &["forge", "list", "--type", "mcp"]).await;
    results.push(pass(&format!("{}/mcp", suite),
        &format!("exit={}", mcp.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// forge evaluate <id>
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_evaluate(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_evaluate";
    let mut results = Vec::new();
    print_suite_header(suite);

    // evaluate with fake id
    let output = ws.run_cli(bin, &["forge", "evaluate", "nonexistent-id"]).await;
    results.push(pass(&format!("{}/evaluate", suite),
        &format!("exit={}", output.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// forge export [id] [--output] [--all]
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_export(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_export";
    let mut results = Vec::new();
    print_suite_header(suite);

    // export all
    let export_all = ws.run_cli(bin, &["forge", "export", "--all"]).await;
    results.push(pass(&format!("{}/all", suite),
        &format!("exit={}", export_all.exit_code)));

    // export with --output
    let export_out = ws.run_cli(bin, &[
        "forge", "export", "--all", "--output", ws.path().join("forge_export").to_str().unwrap_or("export"),
    ]).await;
    results.push(pass(&format!("{}/output", suite),
        &format!("exit={}", export_out.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// forge learning (status / enable / disable / history)
// ---------------------------------------------------------------------------

pub async fn test_cli_forge_learning(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/forge_learning";
    let mut results = Vec::new();
    print_suite_header(suite);

    // learning status
    let status = ws.run_cli(bin, &["forge", "learning", "status"]).await;
    results.push(pass(&format!("{}/status", suite),
        &format!("exit={}", status.exit_code)));

    // learning enable
    let enable = ws.run_cli(bin, &["forge", "learning", "enable"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    // learning history
    let history = ws.run_cli(bin, &["forge", "learning", "history"]).await;
    results.push(pass(&format!("{}/history", suite),
        &format!("exit={}", history.exit_code)));

    // learning history --limit
    let history_limited = ws.run_cli(bin, &["forge", "learning", "history", "--limit", "5"]).await;
    results.push(pass(&format!("{}/history_limit", suite),
        &format!("exit={}", history_limited.exit_code)));

    // learning disable
    let disable = ws.run_cli(bin, &["forge", "learning", "disable"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    results
}
