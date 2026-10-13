//! Model prices CLI：自定义条目 + L3 阶梯价（--price-tier）端到端。
//!
//! 纯 CLI 操作（直开 DataStore，不依赖运行中 gateway 的会话语义），验证：
//! add 落盘 → list 回显（含阶梯徽记）→ 幂等 upsert → remove；坏阶梯
//! 参数响亮拒绝（退码非 0 + 错误原文带条目序号）。

use std::path::Path;
use test_harness::*;

pub async fn test_cli_model_prices(ws: &TestWorkspace, bin: &Path) -> Vec<TestResult> {
    let suite = "cli/model_prices";
    let mut results = Vec::new();
    print_suite_header(suite);

    // ① add：基础价 + 两条阶梯价（L3 --price-tier 重复参数）。
    let output = ws
        .run_cli(
            bin,
            &[
                "model",
                "prices",
                "add",
                "prices-tier-model",
                "--input",
                "1.0",
                "--output",
                "2.0",
                "--price-tier",
                "200000:3.0:9.0",
                "--price-tier",
                "500000:4.0:16.0",
            ],
        )
        .await;
    if output.success() && output.stdout_contains("prices-tier-model") {
        results.push(pass(
            &format!("{suite}/add"),
            "prices add with tiers succeeded",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/add"),
            format!(
                "exit={}, stdout='{}', stderr='{}'",
                output.exit_code,
                output.stdout.trim(),
                output.stderr.trim()
            ),
        ));
    }

    // ② list：条目回显 + 阶梯明细（≥200000 / ≥500000 徽记文本）。
    let output = ws.run_cli(bin, &["model", "prices", "list"]).await;
    let list_ok = output.success()
        && output.stdout_contains("prices-tier-model")
        && output.stdout_contains("200000")
        && output.stdout_contains("500000");
    if list_ok {
        results.push(pass(
            &format!("{suite}/list"),
            "prices list shows entry with tiers",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/list"),
            format!(
                "exit={}, stdout='{}'",
                output.exit_code,
                output.stdout.trim()
            ),
        ));
    }

    // ③ 幂等 upsert：同名重写（换阶梯）不新增条目——重新 add 后 list 仍
    //    只有一次条目名（500000 档价换成 5.0 可见）。
    let output = ws
        .run_cli(
            bin,
            &[
                "model",
                "prices",
                "add",
                "prices-tier-model",
                "--input",
                "1.5",
                "--output",
                "2.5",
                "--price-tier",
                "500000:5.0:20.0",
            ],
        )
        .await;
    let output_list = ws.run_cli(bin, &["model", "prices", "list"]).await;
    let occurrences = output_list.stdout.matches("prices-tier-model").count();
    if output.success() && occurrences >= 1 && output_list.stdout.contains("500000:in=5") {
        results.push(pass(
            &format!("{suite}/upsert_idempotent"),
            "re-add rewrote tiers in place",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/upsert_idempotent"),
            format!(
                "exit={}, occurrences={occurrences}, stdout='{}'",
                output.exit_code,
                output_list.stdout.trim()
            ),
        ));
    }

    // ④ 坏参数响亮拒绝：段数不足（2 段）→ 退码非 0 + 错误带序号。
    let output = ws
        .run_cli(
            bin,
            &[
                "model",
                "prices",
                "add",
                "bad-tier-model",
                "--input",
                "1.0",
                "--output",
                "2.0",
                "--price-tier",
                "200000:3.0",
            ],
        )
        .await;
    if !output.success() && output.stderr.contains("第 1 条") {
        results.push(pass(
            &format!("{suite}/bad_shape_rejected"),
            "malformed tier (2 segments) rejected loudly",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/bad_shape_rejected"),
            format!(
                "exit={}, stderr='{}'",
                output.exit_code,
                output.stderr.trim()
            ),
        ));
    }

    // ⑤ 坏参数响亮拒绝：阈值非整数。
    let output = ws
        .run_cli(
            bin,
            &[
                "model",
                "prices",
                "add",
                "bad-tier-model",
                "--input",
                "1.0",
                "--output",
                "2.0",
                "--price-tier",
                "abc:3.0:9.0",
            ],
        )
        .await;
    if !output.success() && output.stderr.contains("阈值非法") {
        results.push(pass(
            &format!("{suite}/bad_threshold_rejected"),
            "non-integer threshold rejected loudly",
        ));
    } else {
        results.push(fail(
            &format!("{suite}/bad_threshold_rejected"),
            format!(
                "exit={}, stderr='{}'",
                output.exit_code,
                output.stderr.trim()
            ),
        ));
    }

    // ⑥ remove 清理（避免污染后续套件的 list 断言）。
    let output = ws
        .run_cli(bin, &["model", "prices", "remove", "prices-tier-model"])
        .await;
    if output.success() && output.stdout_contains("已删除") {
        results.push(pass(&format!("{suite}/remove"), "prices remove succeeded"));
    } else {
        results.push(fail(
            &format!("{suite}/remove"),
            format!(
                "exit={}, stdout='{}', stderr='{}'",
                output.exit_code,
                output.stdout.trim(),
                output.stderr.trim()
            ),
        ));
    }

    results
}
