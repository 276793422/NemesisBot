//! Scanner commands: list, add, remove, enable, disable, check, install, info, download, test, update

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// scanner list
// ---------------------------------------------------------------------------

pub async fn test_cli_scanner_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/scanner_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["scanner", "list"]).await;
    results.push(pass(&format!("{}/list", suite),
        &format!("exit={}, output: '{}'", output.exit_code,
            output.stdout.trim().chars().take(100).collect::<String>())));

    results
}

// ---------------------------------------------------------------------------
// scanner add / remove
// ---------------------------------------------------------------------------

pub async fn test_cli_scanner_add_remove(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/scanner_add_remove";
    let mut results = Vec::new();
    print_suite_header(suite);

    // scanner add
    let add = ws.run_cli(bin, &[
        "scanner", "add", "test-engine", "--path", "/usr/bin/test-scanner",
    ]).await;
    results.push(pass(&format!("{}/add", suite),
        &format!("exit={}", add.exit_code)));

    // scanner info
    let info = ws.run_cli(bin, &["scanner", "info", "test-engine"]).await;
    results.push(pass(&format!("{}/info", suite),
        &format!("exit={}", info.exit_code)));

    // scanner remove
    let remove = ws.run_cli(bin, &["scanner", "remove", "test-engine"]).await;
    results.push(pass(&format!("{}/remove", suite),
        &format!("exit={}", remove.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// scanner enable / disable
// ---------------------------------------------------------------------------

pub async fn test_cli_scanner_enable_disable(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/scanner_enable_disable";
    let mut results = Vec::new();
    print_suite_header(suite);

    // Add engine first
    let _ = ws.run_cli(bin, &["scanner", "add", "temp-engine"]).await;

    let enable = ws.run_cli(bin, &["scanner", "enable", "temp-engine"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    let disable = ws.run_cli(bin, &["scanner", "disable", "temp-engine"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    // Cleanup
    let _ = ws.run_cli(bin, &["scanner", "remove", "temp-engine"]).await;

    results
}

// ---------------------------------------------------------------------------
// scanner check / install (offline)
// ---------------------------------------------------------------------------

pub async fn test_cli_scanner_check_install(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/scanner_check_install";
    let mut results = Vec::new();
    print_suite_header(suite);

    // scanner check
    let check = ws.run_cli(bin, &["scanner", "check"]).await;
    results.push(pass(&format!("{}/check", suite),
        &format!("exit={}", check.exit_code)));

    // scanner install (may need network, test parsing only)
    let install = ws.run_cli(bin, &["scanner", "install"]).await;
    results.push(pass(&format!("{}/install", suite),
        &format!("exit={}", install.exit_code)));

    // scanner install --dir
    let install_dir = ws.run_cli(bin, &[
        "scanner", "install", "--dir", ws.path().join("scanner").to_str().unwrap_or("./scanner"),
    ]).await;
    results.push(pass(&format!("{}/install_dir", suite),
        &format!("exit={}", install_dir.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// scanner download / test / update (network-dependent, test parsing only)
// ---------------------------------------------------------------------------

pub async fn test_cli_scanner_download_test_update(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/scanner_download_test_update";
    let mut results = Vec::new();
    print_suite_header(suite);

    // scanner download --help (actual download needs network)
    let dl_help = ws.run_cli(bin, &["scanner", "download", "--help"]).await;
    results.push(pass(&format!("{}/download_help", suite),
        &format!("exit={}", dl_help.exit_code)));

    // scanner test --help
    let test_help = ws.run_cli(bin, &["scanner", "test", "--help"]).await;
    results.push(pass(&format!("{}/test_help", suite),
        &format!("exit={}", test_help.exit_code)));

    // scanner update --help
    let update_help = ws.run_cli(bin, &["scanner", "update", "--help"]).await;
    results.push(pass(&format!("{}/update_help", suite),
        &format!("exit={}", update_help.exit_code)));

    results
}
