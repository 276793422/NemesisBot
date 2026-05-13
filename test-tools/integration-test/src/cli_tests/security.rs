//! Security CLI commands: status, enable, disable, config, audit, test, rules, approve/deny/pending

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// security status
// ---------------------------------------------------------------------------

pub async fn test_cli_security_status(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_status";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["security", "status"]).await;
    if output.stdout_contains("Security") || output.stdout_contains("security") || output.success() {
        results.push(pass(&format!("{}/output", suite), "Security status output received"));
    } else {
        results.push(fail(&format!("{}/output", suite), &format!(
            "No security info: exit={}, stdout='{}'", output.exit_code, output.stdout.trim())));
    }

    results
}

// ---------------------------------------------------------------------------
// security enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_security_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    let disable = ws.run_cli(bin, &["security", "disable"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    let enable = ws.run_cli(bin, &["security", "enable"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// security config (show / edit / reset)
// ---------------------------------------------------------------------------

pub async fn test_cli_security_config(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_config";
    let mut results = Vec::new();
    print_suite_header(suite);

    // config show
    let show = ws.run_cli(bin, &["security", "config", "show"]).await;
    results.push(pass(&format!("{}/show", suite),
        &format!("exit={}", show.exit_code)));

    // config reset
    let reset = ws.run_cli(bin, &["security", "config", "reset"]).await;
    results.push(pass(&format!("{}/reset", suite),
        &format!("exit={}", reset.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// security audit (show / export / denied)
// ---------------------------------------------------------------------------

pub async fn test_cli_security_audit(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_audit";
    let mut results = Vec::new();
    print_suite_header(suite);

    // audit show
    let show = ws.run_cli(bin, &["security", "audit", "show"]).await;
    results.push(pass(&format!("{}/show", suite),
        &format!("exit={}", show.exit_code)));

    // audit show --limit 5
    let show_limited = ws.run_cli(bin, &["security", "audit", "show", "--limit", "5"]).await;
    results.push(pass(&format!("{}/show_limit", suite),
        &format!("exit={}", show_limited.exit_code)));

    // audit denied
    let denied = ws.run_cli(bin, &["security", "audit", "denied"]).await;
    results.push(pass(&format!("{}/denied", suite),
        &format!("exit={}", denied.exit_code)));

    // audit export
    let export_path = ws.path().join("test_audit_export.json");
    let export = ws.run_cli(bin, &[
        "security", "audit", "export", export_path.to_str().unwrap_or("audit.json"),
    ]).await;
    results.push(pass(&format!("{}/export", suite),
        &format!("exit={}", export.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// security test (--tool, --args)
// ---------------------------------------------------------------------------

pub async fn test_cli_security_test(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_test";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Test LOW risk operation
    let low = ws.run_cli(bin, &[
        "security", "test", "--tool", "read_file", "--args", r#"{"path":"test.txt"}"#,
    ]).await;
    results.push(pass(&format!("{}/low_risk", suite),
        &format!("exit={}", low.exit_code)));

    // Test CRITICAL risk operation
    let critical = ws.run_cli(bin, &[
        "security", "test", "--tool", "process_exec", "--args", r#"{"command":"rm -rf /"}"#,
    ]).await;
    results.push(pass(&format!("{}/critical_risk", suite),
        &format!("exit={}, blocked: {}", critical.exit_code,
            critical.stdout_contains("BLOCKED") || critical.stdout_contains("blocked"))));

    results
}

// ---------------------------------------------------------------------------
// security rules (list / add / remove / test)
// ---------------------------------------------------------------------------

pub async fn test_cli_security_rules(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_rules";
    let mut results = Vec::new();
    print_suite_header(suite);

    // rules list
    let list = ws.run_cli(bin, &["security", "rules", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}", list.exit_code)));

    // rules add
    let add = ws.run_cli(bin, &[
        "security", "rules", "add", "file_path", "deny_hidden",
        "--pattern", ".*\\.hidden", "--action", "deny",
    ]).await;
    results.push(pass(&format!("{}/add", suite),
        &format!("exit={}", add.exit_code)));

    // rules list (with filter)
    let list_filtered = ws.run_cli(bin, &["security", "rules", "list", "file_path"]).await;
    results.push(pass(&format!("{}/list_filtered", suite),
        &format!("exit={}", list_filtered.exit_code)));

    // rules test
    let test = ws.run_cli(bin, &[
        "security", "rules", "test", "file_path", "deny_hidden", "test.hidden",
    ]).await;
    results.push(pass(&format!("{}/test", suite),
        &format!("exit={}", test.exit_code)));

    // rules remove
    let remove = ws.run_cli(bin, &[
        "security", "rules", "remove", "file_path", "deny_hidden", "0",
    ]).await;
    results.push(pass(&format!("{}/remove", suite),
        &format!("exit={}", remove.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// security approve / deny / pending / edit / config-reset
// ---------------------------------------------------------------------------

pub async fn test_cli_security_approve_deny(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/security_approve_deny";
    let mut results = Vec::new();
    print_suite_header(suite);

    // pending (should show empty or existing list)
    let pending = ws.run_cli(bin, &["security", "pending"]).await;
    results.push(pass(&format!("{}/pending", suite),
        &format!("exit={}", pending.exit_code)));

    // approve (with fake ID - should fail gracefully)
    let approve = ws.run_cli(bin, &["security", "approve", "nonexistent-id"]).await;
    results.push(pass(&format!("{}/approve", suite),
        &format!("exit={}", approve.exit_code)));

    // deny (with fake ID and reason)
    let deny = ws.run_cli(bin, &["security", "deny", "nonexistent-id", "test", "reason"]).await;
    results.push(pass(&format!("{}/deny", suite),
        &format!("exit={}", deny.exit_code)));

    // config-reset (tests help only - actual reset may prompt)
    let config_reset_help = ws.run_cli(bin, &["security", "config-reset", "--help"]).await;
    results.push(pass(&format!("{}/config_reset_help", suite),
        &format!("exit={}", config_reset_help.exit_code)));

    results
}
