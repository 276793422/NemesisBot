//! CORS commands: list, add, remove, show, validate, dev-mode

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// cors full CRUD
// ---------------------------------------------------------------------------

pub async fn test_cli_cors_full(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cors";
    let mut results = Vec::new();
    print_suite_header(suite);

    // cors show
    let show = ws.run_cli(bin, &["cors", "show"]).await;
    results.push(pass(&format!("{}/show", suite),
        &format!("exit={}", show.exit_code)));

    // cors list
    let list = ws.run_cli(bin, &["cors", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}", list.exit_code)));

    // cors add
    let add = ws.run_cli(bin, &["cors", "add", "http://localhost:3000"]).await;
    results.push(pass(&format!("{}/add", suite),
        &format!("exit={}", add.exit_code)));

    // cors add --cdn
    let add_cdn = ws.run_cli(bin, &["cors", "add", "https://cdn.example.com", "--cdn"]).await;
    results.push(pass(&format!("{}/add_cdn", suite),
        &format!("exit={}", add_cdn.exit_code)));

    // cors list (should show added origins)
    let list2 = ws.run_cli(bin, &["cors", "list"]).await;
    results.push(pass(&format!("{}/list_after_add", suite),
        &format!("exit={}, output len={}", list2.exit_code, list2.stdout.len())));

    // cors validate
    let validate_ok = ws.run_cli(bin, &["cors", "validate", "http://localhost:3000"]).await;
    results.push(pass(&format!("{}/validate_allowed", suite),
        &format!("exit={}", validate_ok.exit_code)));

    let validate_fail = ws.run_cli(bin, &["cors", "validate", "http://evil.com"]).await;
    results.push(pass(&format!("{}/validate_blocked", suite),
        &format!("exit={}", validate_fail.exit_code)));

    // cors dev-mode status
    let dev_status = ws.run_cli(bin, &["cors", "dev-mode", "status"]).await;
    results.push(pass(&format!("{}/dev_mode_status", suite),
        &format!("exit={}", dev_status.exit_code)));

    // cors dev-mode enable
    let dev_enable = ws.run_cli(bin, &["cors", "dev-mode", "enable"]).await;
    results.push(pass(&format!("{}/dev_mode_enable", suite),
        &format!("exit={}", dev_enable.exit_code)));

    // cors dev-mode disable
    let dev_disable = ws.run_cli(bin, &["cors", "dev-mode", "disable"]).await;
    results.push(pass(&format!("{}/dev_mode_disable", suite),
        &format!("exit={}", dev_disable.exit_code)));

    // cors remove
    let remove = ws.run_cli(bin, &["cors", "remove", "http://localhost:3000"]).await;
    results.push(pass(&format!("{}/remove", suite),
        &format!("exit={}", remove.exit_code)));

    // cors remove --cdn
    let remove_cdn = ws.run_cli(bin, &["cors", "remove", "https://cdn.example.com", "--cdn"]).await;
    results.push(pass(&format!("{}/remove_cdn", suite),
        &format!("exit={}", remove_cdn.exit_code)));

    results
}
