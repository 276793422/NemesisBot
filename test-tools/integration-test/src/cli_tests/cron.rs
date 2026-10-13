//! Cron commands: list, add, remove, enable, disable
//!
//! 断言纪律（2026-10-13 存储格式统一修复时收紧）：旧版所有臂都无条件
//! `pass(exit=N)`——CLI 崩了也记过，从未真测到行为；enable/disable/remove
//! 还把 job **name** 当 id 传（永远 not found 空转）。现在：exit 0 + 关键
//! stdout 佐证双断言，id 从 add 输出解析（remove/enable 只认 id）。

use std::path::Path;
use test_harness::*;

/// 从 `Added job '<name>' (<id>)` 输出解析 job id。
fn parse_added_job_id(stdout: &str) -> Option<String> {
    let rest = stdout.rsplit('(').next()?;
    let id = rest.split(')').next()?.trim();
    if id.is_empty() || id.contains(' ') || id.contains('\'') {
        return None;
    }
    Some(id.to_string())
}

// ---------------------------------------------------------------------------
// cron full CRUD
// ---------------------------------------------------------------------------

pub async fn test_cli_cron_list(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cron_list";
    let mut results = Vec::new();
    print_suite_header(suite);

    let output = ws.run_cli(bin, &["cron", "list"]).await;
    if output.success() {
        results.push(pass(
            &format!("{}/output", suite),
            "Cron list output received",
        ));
    } else {
        results.push(fail(
            &format!("{}/output", suite),
            format!(
                "exit={} stdout={} stderr={}",
                output.exit_code, output.stdout, output.stderr
            ),
        ));
    }

    results
}

pub async fn test_cli_cron_crud(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/cron_crud";
    let mut results = Vec::new();
    print_suite_header(suite);

    // cron add (interval-based)
    let add = ws
        .run_cli(
            bin,
            &[
                "cron",
                "add",
                "-n",
                "test-job",
                "-m",
                "hello from cron",
                "-e",
                "60",
            ],
        )
        .await;
    let job_id = match add
        .success()
        .then(|| parse_added_job_id(&add.stdout))
        .flatten()
    {
        Some(id) => {
            results.push(pass(
                &format!("{}/add_interval", suite),
                format!("exit=0 id={id}"),
            ));
            id
        }
        None => {
            results.push(fail(
                &format!("{}/add_interval", suite),
                format!(
                    "exit={} stdout={} stderr={}",
                    add.exit_code, add.stdout, add.stderr
                ),
            ));
            return results;
        }
    };

    // cron add (cron expression)
    let add_cron = ws
        .run_cli(
            bin,
            &[
                "cron",
                "add",
                "-n",
                "cron-expr-job",
                "-m",
                "scheduled message",
                "-c",
                "0 */5 * * *",
            ],
        )
        .await;
    if add_cron.success() && parse_added_job_id(&add_cron.stdout).is_some() {
        results.push(pass(&format!("{}/add_cron_expr", suite), "exit=0"));
    } else {
        results.push(fail(
            &format!("{}/add_cron_expr", suite),
            format!(
                "exit={} stdout={} stderr={}",
                add_cron.exit_code, add_cron.stdout, add_cron.stderr
            ),
        ));
    }

    // cron add with --deliver --to --channel
    let add_deliver = ws
        .run_cli(
            bin,
            &[
                "cron",
                "add",
                "-n",
                "deliver-job",
                "-m",
                "deliver this",
                "-e",
                "120",
                "--deliver",
                "--to",
                "user1",
                "--channel",
                "web",
            ],
        )
        .await;
    if add_deliver.success() && parse_added_job_id(&add_deliver.stdout).is_some() {
        results.push(pass(&format!("{}/add_deliver", suite), "exit=0"));
    } else {
        results.push(fail(
            &format!("{}/add_deliver", suite),
            format!(
                "exit={} stdout={} stderr={}",
                add_deliver.exit_code, add_deliver.stdout, add_deliver.stderr
            ),
        ));
    }

    // cron list (should show jobs)
    let list = ws.run_cli(bin, &["cron", "list"]).await;
    if list.success() && list.stdout_contains("test-job") {
        results.push(pass(&format!("{}/list_after_add", suite), "jobs visible"));
    } else {
        results.push(fail(
            &format!("{}/list_after_add", suite),
            format!("exit={} stdout={}", list.exit_code, list.stdout),
        ));
    }

    // cron enable/disable/remove —— 用真实 id（name 不是 id，旧版一直空转）。
    let enable = ws.run_cli(bin, &["cron", "enable", &job_id]).await;
    if enable.success() && enable.stdout_contains("enabled") {
        results.push(pass(&format!("{}/enable", suite), "enabled"));
    } else {
        results.push(fail(
            &format!("{}/enable", suite),
            format!(
                "exit={} stdout={} stderr={}",
                enable.exit_code, enable.stdout, enable.stderr
            ),
        ));
    }

    let disable = ws.run_cli(bin, &["cron", "disable", &job_id]).await;
    if disable.success() && disable.stdout_contains("disabled") {
        results.push(pass(&format!("{}/disable", suite), "disabled"));
    } else {
        results.push(fail(
            &format!("{}/disable", suite),
            format!(
                "exit={} stdout={} stderr={}",
                disable.exit_code, disable.stdout, disable.stderr
            ),
        ));
    }

    let remove = ws.run_cli(bin, &["cron", "remove", &job_id]).await;
    if remove.success() && remove.stdout_contains("Removed job") {
        results.push(pass(&format!("{}/remove", suite), "removed"));
    } else {
        results.push(fail(
            &format!("{}/remove", suite),
            format!(
                "exit={} stdout={} stderr={}",
                remove.exit_code, remove.stdout, remove.stderr
            ),
        ));
    }

    // Cleanup remaining jobs
    let _ = ws.run_cli(bin, &["cron", "remove", "cron-expr-job"]).await;
    let _ = ws.run_cli(bin, &["cron", "remove", "deliver-job"]).await;

    results
}
