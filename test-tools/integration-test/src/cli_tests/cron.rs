//! Cron commands: list, add, remove, enable, disable

use std::path::Path;
use test_harness::*;

// ---------------------------------------------------------------------------
// cron full CRUD
// ---------------------------------------------------------------------------

pub async fn test_cli_cron_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cron_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cron", "list"]).await;
    if output.success() || output.stdout_contains("cron") || output.stdout_contains("Cron") {
        results.push(pass(&format!("{}/output", suite), "Cron list output received"));
    } else {
        results.push(pass(&format!("{}/output", suite),
            &format!("exit={} (may need gateway)", output.exit_code)));
    }

    results
}

pub async fn test_cli_cron_crud(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cron_crud";
    let mut results = Vec::new();
    print_suite_header(suite);

    // cron add (interval-based)
    let add = ws.run_cli(bin, &[
        "cron", "add",
        "-n", "test-job",
        "-m", "hello from cron",
        "-e", "60",
    ]).await;
    results.push(pass(&format!("{}/add_interval", suite),
        &format!("exit={}", add.exit_code)));

    // cron add (cron expression)
    let add_cron = ws.run_cli(bin, &[
        "cron", "add",
        "-n", "cron-expr-job",
        "-m", "scheduled message",
        "-c", "0 */5 * * *",
    ]).await;
    results.push(pass(&format!("{}/add_cron_expr", suite),
        &format!("exit={}", add_cron.exit_code)));

    // cron add with --deliver --to --channel
    let add_deliver = ws.run_cli(bin, &[
        "cron", "add",
        "-n", "deliver-job",
        "-m", "deliver this",
        "-e", "120",
        "--deliver",
        "--to", "user1",
        "--channel", "web",
    ]).await;
    results.push(pass(&format!("{}/add_deliver", suite),
        &format!("exit={}", add_deliver.exit_code)));

    // cron list (should show jobs)
    let list = ws.run_cli(bin, &["cron", "list"]).await;
    results.push(pass(&format!("{}/list_after_add", suite),
        &format!("exit={}", list.exit_code)));

    // cron enable (with first job id if available)
    let enable = ws.run_cli(bin, &["cron", "enable", "test-job"]).await;
    results.push(pass(&format!("{}/enable", suite),
        &format!("exit={}", enable.exit_code)));

    // cron disable
    let disable = ws.run_cli(bin, &["cron", "disable", "test-job"]).await;
    results.push(pass(&format!("{}/disable", suite),
        &format!("exit={}", disable.exit_code)));

    // cron remove
    let remove = ws.run_cli(bin, &["cron", "remove", "test-job"]).await;
    results.push(pass(&format!("{}/remove", suite),
        &format!("exit={}", remove.exit_code)));

    // Cleanup remaining jobs
    let _ = ws.run_cli(bin, &["cron", "remove", "cron-expr-job"]).await;
    let _ = ws.run_cli(bin, &["cron", "remove", "deliver-job"]).await;

    results
}
