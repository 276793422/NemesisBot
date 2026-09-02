//! Board CLI commands: issue, autopilot（W2 P3/P4 集成断言）。
//!
//! 直接操作 `--local` 工作空间的共享 SQLite store（`workspace/board/board.db`，
//! BoardStore::open 自动建目录），不依赖运行中的 gateway。
//! 断言对齐 CLI 输出文案（issue.rs / autopilot.rs 的 println）。

use std::path::Path;
use test_harness::*;

/// stderr 首行截断（harness 只有 stdout_first_line）。
fn stderr_snip(out: &CliOutput) -> String {
    out.stderr
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(120)
        .collect()
}

// ---------------------------------------------------------------------------
// issue: create → list → get → status → comment → stats
// ---------------------------------------------------------------------------

pub async fn test_cli_issue_crud(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/issue_crud";
    let mut results = Vec::new();
    print_suite_header(suite);

    // create → 打印编号
    let out = ws
        .run_cli(bin, &["issue", "create", "集成测试任务", "--priority", "2"])
        .await;
    if out.success() && out.stdout_contains("已创建 NB-") {
        results.push(pass(
            &format!("{}/create", suite),
            format!("exit={}", out.exit_code),
        ));
    } else {
        results.push(fail(
            &format!("{}/create", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // list → 标题可见
    let out = ws.run_cli(bin, &["issue", "list"]).await;
    if out.success() && out.stdout_contains("集成测试任务") {
        results.push(pass(&format!("{}/list", suite), "created issue listed"));
    } else {
        results.push(fail(
            &format!("{}/list", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // get NB-1 → 详情
    let out = ws.run_cli(bin, &["issue", "get", "NB-1"]).await;
    if out.success() && out.stdout_contains("集成测试任务") {
        results.push(pass(&format!("{}/get_by_number", suite), "NB-1 resolved"));
    } else {
        results.push(fail(
            &format!("{}/get_by_number", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // status 转移 → in_progress
    let out = ws
        .run_cli(bin, &["issue", "status", "NB-1", "in_progress"])
        .await;
    if out.success() && out.stdout_contains("状态已转移") && out.stdout_contains("in_progress")
    {
        results.push(pass(&format!("{}/status", suite), "transitioned"));
    } else {
        results.push(fail(
            &format!("{}/status", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // 非法转移被状态机拒绝：in_progress → cancelled 是合法转移，但
    // backlog 直达等非法路径必须报错退出（此处用不存在的状态名验证报错路径）。
    let out = ws
        .run_cli(bin, &["issue", "status", "NB-1", "no_such_status"])
        .await;
    if !out.success() {
        results.push(pass(
            &format!("{}/status_bad_rejected", suite),
            "invalid status rejected",
        ));
    } else {
        results.push(fail(
            &format!("{}/status_bad_rejected", suite),
            "invalid status accepted",
        ));
    }

    // comment → 确认
    let out = ws
        .run_cli(bin, &["issue", "comment", "NB-1", "集成评论"])
        .await;
    if out.success() && out.stdout_contains("评论已添加") {
        results.push(pass(&format!("{}/comment", suite), "commented"));
    } else {
        results.push(fail(
            &format!("{}/comment", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }
    let out = ws.run_cli(bin, &["issue", "get", "NB-1"]).await;
    if out.stdout_contains("集成评论") {
        results.push(pass(
            &format!("{}/comment_visible", suite),
            "comment in get",
        ));
    } else {
        results.push(fail(
            &format!("{}/comment_visible", suite),
            "comment missing",
        ));
    }

    // stats → 有输出且含状态键
    let out = ws.run_cli(bin, &["issue", "stats"]).await;
    if out.success() && out.stdout_contains("in_progress: 1") {
        results.push(pass(&format!("{}/stats", suite), "counts visible"));
    } else {
        results.push(fail(
            &format!("{}/stats", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    results
}

// ---------------------------------------------------------------------------
// autopilot: create → list → update → enable/disable → run → runs → remove
// ---------------------------------------------------------------------------

pub async fn test_cli_autopilot_crud(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/autopilot_crud";
    let mut results = Vec::new();
    print_suite_header(suite);

    // create（仅建单，无派发目标）
    let out = ws
        .run_cli(
            bin,
            &[
                "autopilot",
                "create",
                "每日站会",
                "--cron",
                "0 9 * * *",
                "--title",
                "每日站会 {date}",
                "--description",
                "集成测试规则",
            ],
        )
        .await;
    if out.success() && out.stdout_contains("已创建 autopilot #1") {
        results.push(pass(&format!("{}/create", suite), "rule #1 created"));
    } else {
        results.push(fail(
            &format!("{}/create", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // 非法 cron 被拒绝
    let out = ws
        .run_cli(
            bin,
            &[
                "autopilot",
                "create",
                "坏规则",
                "--cron",
                "not-a-cron",
                "--title",
                "x",
            ],
        )
        .await;
    if !out.success() && out.stderr_contains("cron 表达式无效") {
        results.push(pass(
            &format!("{}/invalid_cron_rejected", suite),
            "validated",
        ));
    } else {
        results.push(fail(
            &format!("{}/invalid_cron_rejected", suite),
            format!("exit={} stderr={}", out.exit_code, stderr_snip(&out)),
        ));
    }

    // list → 规则可见（未挂载 = 等 gateway 启动同步）
    let out = ws.run_cli(bin, &["autopilot", "list"]).await;
    if out.success() && out.stdout_contains("每日站会") && out.stdout_contains("0 9 * * *") {
        results.push(pass(&format!("{}/list", suite), "rule listed"));
    } else {
        results.push(fail(
            &format!("{}/list", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // update 标题
    let out = ws
        .run_cli(
            bin,
            &["autopilot", "update", "1", "--title", "每日站会 v2 {date}"],
        )
        .await;
    if out.success() && out.stdout_contains("已更新 autopilot #1") {
        results.push(pass(&format!("{}/update", suite), "title patched"));
    } else {
        results.push(fail(
            &format!("{}/update", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // disable → enable
    let out = ws.run_cli(bin, &["autopilot", "disable", "1"]).await;
    if out.success() && out.stdout_contains("已停用") {
        results.push(pass(&format!("{}/disable", suite), "disabled"));
    } else {
        results.push(fail(
            &format!("{}/disable", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }
    let out = ws.run_cli(bin, &["autopilot", "enable", "1"]).await;
    if out.success() && out.stdout_contains("已启用") {
        results.push(pass(&format!("{}/enable", suite), "enabled"));
    } else {
        results.push(fail(
            &format!("{}/enable", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // run（仅建单规则，CLI 无集群也能触发）→ 建单 + runs 历史可查
    let out = ws.run_cli(bin, &["autopilot", "run", "1"]).await;
    if out.success() && out.stdout_contains("已触发：建单 NB-") {
        results.push(pass(&format!("{}/run", suite), "issue created"));
    } else {
        results.push(fail(
            &format!("{}/run", suite),
            format!(
                "exit={} stdout={} stderr={}",
                out.exit_code,
                out.stdout_first_line(),
                stderr_snip(&out)
            ),
        ));
    }
    let out = ws.run_cli(bin, &["autopilot", "runs", "1"]).await;
    if out.success() && out.stdout_contains("每日站会 v2") {
        results.push(pass(&format!("{}/runs", suite), "history visible"));
    } else {
        results.push(fail(
            &format!("{}/runs", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }

    // 派发目标规则在 CLI run 被诚实拒绝（无集群连接）
    let out = ws
        .run_cli(
            bin,
            &[
                "autopilot",
                "create",
                "派发规则",
                "--cron",
                "0 10 * * *",
                "--title",
                "派发 {date}",
                "--target",
                "node-b",
            ],
        )
        .await;
    if !out.success() {
        results.push(fail(
            &format!("{}/create_targeted", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
        return results;
    }
    let targeted_id = "2";
    let out = ws.run_cli(bin, &["autopilot", "run", targeted_id]).await;
    if !out.success() && out.stderr_contains("集群未运行") {
        results.push(pass(
            &format!("{}/run_targeted_rejected", suite),
            "honest rejection",
        ));
    } else {
        results.push(fail(
            &format!("{}/run_targeted_rejected", suite),
            format!("exit={} stderr={}", out.exit_code, stderr_snip(&out)),
        ));
    }

    // remove → 再 list 不再出现
    let out = ws.run_cli(bin, &["autopilot", "remove", "2"]).await;
    if out.success() && out.stdout_contains("已删除 autopilot #2") {
        results.push(pass(&format!("{}/remove", suite), "rule #2 removed"));
    } else {
        results.push(fail(
            &format!("{}/remove", suite),
            format!("exit={} stdout={}", out.exit_code, out.stdout_first_line()),
        ));
    }
    let out = ws.run_cli(bin, &["autopilot", "list"]).await;
    if out.success() && !out.stdout_contains("派发规则") {
        results.push(pass(
            &format!("{}/removed_gone", suite),
            "#2 absent from list",
        ));
    } else {
        results.push(fail(&format!("{}/removed_gone", suite), "#2 still listed"));
    }

    results
}

// ---------------------------------------------------------------------------
// help 覆盖（子命令完整列出）
// ---------------------------------------------------------------------------

pub async fn test_cli_board_help(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/board_help";
    let mut results = Vec::new();
    print_suite_header(suite);

    let issue_help = ws.run_cli(bin, &["issue", "--help"]).await;
    for sub in &["create", "list", "get", "assign", "status", "comment"] {
        if issue_help.stdout_contains(sub) {
            results.push(pass(&format!("{}/issue_{sub}", suite), "listed"));
        } else {
            results.push(fail(&format!("{}/issue_{sub}", suite), "missing"));
        }
    }

    let ap_help = ws.run_cli(bin, &["autopilot", "--help"]).await;
    for sub in &[
        "list", "create", "update", "enable", "disable", "remove", "run", "runs",
    ] {
        if ap_help.stdout_contains(sub) {
            results.push(pass(&format!("{}/autopilot_{sub}", suite), "listed"));
        } else {
            results.push(fail(&format!("{}/autopilot_{sub}", suite), "missing"));
        }
    }

    results
}
