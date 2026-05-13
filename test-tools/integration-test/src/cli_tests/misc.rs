//! Miscellaneous CLI commands: daemon, migrate, gateway (flags)

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// daemon cluster (--help only, actually running would start background process)
// ---------------------------------------------------------------------------

pub async fn test_cli_daemon(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/daemon";
    let mut results = Vec::new();
    print_suite_header(suite);

    // daemon --help
    let help = ws.run_cli(bin, &["daemon", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Daemon help works"));
    } else {
        results.push(fail(&format!("{}/help", suite),
            &format!("exit={}", help.exit_code)));
    }

    // daemon cluster --help
    let cluster_help = ws.run_cli(bin, &["daemon", "cluster", "--help"]).await;
    results.push(pass(&format!("{}/cluster_help", suite),
        &format!("exit={}", cluster_help.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// migrate (--help and flags)
// ---------------------------------------------------------------------------

pub async fn test_cli_migrate(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/migrate";
    let mut results = Vec::new();
    print_suite_header(suite);

    // migrate --help
    let help = ws.run_cli(bin, &["migrate", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Migrate help works"));
    } else {
        results.push(fail(&format!("{}/help", suite),
            &format!("exit={}", help.exit_code)));
    }

    // Verify all flags present in help
    let flags = ["--dry-run", "--config-only", "--workspace-only", "--force", "--openclaw-home", "--refresh"];
    for flag in &flags {
        if help.stdout_contains(flag) {
            results.push(pass(&format!("{}/flag_{}", suite, flag.trim_start_matches("--")),
                &format!("{} present", flag)));
        } else {
            results.push(fail(&format!("{}/flag_{}", suite, flag.trim_start_matches("--")),
                &format!("{} missing from help", flag)));
        }
    }

    // migrate --dry-run (won't actually do anything without openclaw-home)
    let dry_run = ws.run_cli(bin, &["migrate", "--dry-run"]).await;
    results.push(pass(&format!("{}/dry_run", suite),
        &format!("exit={}", dry_run.exit_code)));

    results
}

// ---------------------------------------------------------------------------
// gateway flags (--help only, actually running would start server)
// ---------------------------------------------------------------------------

pub async fn test_cli_gateway_flags(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/gateway_flags";
    let mut results = Vec::new();
    print_suite_header(suite);

    let help = ws.run_cli(bin, &["gateway", "--help"]).await;
    if help.success() {
        results.push(pass(&format!("{}/help", suite), "Gateway help works"));
    } else {
        results.push(fail(&format!("{}/help", suite),
            &format!("exit={}", help.exit_code)));
    }

    // Check all gateway flags
    let flags = ["--debug", "-d", "--quiet", "-q", "--no-console"];
    for flag in &flags {
        if help.stdout_contains(flag) {
            results.push(pass(&format!("{}/flag_{}", suite, flag.trim_start_matches("-")),
                &format!("{} present", flag)));
        } else {
            results.push(fail(&format!("{}/flag_{}", suite, flag.trim_start_matches("-")),
                &format!("{} missing from help", flag)));
        }
    }

    results
}
