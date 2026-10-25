//! [`super`] 必检计划纯函数测试（derive / reconcile / render）。

use super::*;

// ---------------------------------------------------------------------------
// derive_check_plan：保守派生
// ---------------------------------------------------------------------------

#[test]
fn derive_extracts_backticked_multi_word_commands() {
    let plan = derive_check_plan(&[
        "修复登录超时。完成后运行 `cargo test -p nemesis-agent` 验证，再跑 `npm run build`。",
    ]);
    assert_eq!(plan, vec!["cargo test -p nemesis-agent", "npm run build"]);
}

#[test]
fn derive_skips_single_word_and_non_command_spans() {
    let plan = derive_check_plan(&[
        "参考 `TODO` 与 `src/main.rs`，注意 `e.g. something` 不是命令；`x` 也不是。",
    ]);
    assert!(
        plan.is_empty(),
        "单词片段 / 文件名 / 行文缩写不进计划: {plan:?}"
    );
}

#[test]
fn derive_ignores_odd_trailing_fragment_and_check_lines() {
    // 奇数反引号：尾部残片不是片段；[CHECK] 行整行剔除（锚点双检管辖）。
    let plan = derive_check_plan(&[
        "先跑 `cargo build` 然后 `尾部残片没有闭合",
        "- [CHECK] exec: `cargo test` 全过",
    ]);
    assert_eq!(plan, vec!["cargo build"], "[CHECK] 行与残片不计: {plan:?}");
}

#[test]
fn derive_dedupes_and_is_multiline() {
    let plan = derive_check_plan(&[
        "第一段提到 `go vet ./...`。\n第二段重复 `go vet ./...` 与 `make check`。",
    ]);
    assert_eq!(plan, vec!["go vet ./...", "make check"]);
}

// ---------------------------------------------------------------------------
// reconcile_check_plan：归一子串匹配 + 诚实 AND
// ---------------------------------------------------------------------------

fn check_entry(cmd: &str, ok: bool) -> serde_json::Value {
    serde_json::json!({"tool": "exec", "cmd": cmd, "ok": ok, "summary": "..."})
}

#[test]
fn reconcile_missing_check_is_unran() {
    let plan = vec!["cargo test".to_string()];
    let statuses = reconcile_check_plan(&plan, &[check_entry("npm run build", true)]);
    assert_eq!(statuses.len(), 1);
    assert!(!statuses[0].ran, "没跑就是没跑（checks 缺席同态）");
    assert_eq!(statuses[0].ok, None);
}

#[test]
fn reconcile_empty_checks_all_unran() {
    let plan = vec!["cargo test".to_string(), "npm run build".to_string()];
    let statuses = reconcile_check_plan(&plan, &[]);
    assert!(statuses.iter().all(|s| !s.ran), "空 checks = 全未跑");
}

#[test]
fn reconcile_substring_covers_narrower_plan() {
    let plan = vec!["cargo test".to_string()];
    let statuses = reconcile_check_plan(&plan, &[check_entry("cargo test --lib -p x", true)]);
    assert!(statuses[0].ran, "跑得更细 = 覆盖计划项");
    assert_eq!(statuses[0].ok, Some(true));
}

#[test]
fn reconcile_whitespace_normalized_matching() {
    let plan = vec!["cargo   test -p x".to_string()];
    let statuses = reconcile_check_plan(&plan, &[check_entry("cargo test -p x", true)]);
    assert!(statuses[0].ran, "空白差异归一后匹配");
}

#[test]
fn reconcile_multiple_runs_any_failure_is_failure() {
    let plan = vec!["cargo test".to_string()];
    let statuses = reconcile_check_plan(
        &plan,
        &[
            check_entry("cargo test", true),
            check_entry("cargo test", false),
        ],
    );
    assert!(statuses[0].ran);
    assert_eq!(statuses[0].ok, Some(false), "多次运行任一失败 = 该项失败");
}

#[test]
fn reconcile_malformed_entries_do_not_match() {
    let plan = vec!["cargo test".to_string()];
    let statuses = reconcile_check_plan(
        &plan,
        &[
            serde_json::json!({"tool": "exec"}), // 缺 cmd
            serde_json::json!({"cmd": 42}),      // cmd 非字符串
            check_entry("cargo test", true),     // 真匹配
        ],
    );
    assert_eq!(statuses[0].ok, Some(true), "形态非法条目跳过，不挡真证据");
}

// ---------------------------------------------------------------------------
// render_check_plan_reconciliation
// ---------------------------------------------------------------------------

#[test]
fn render_covers_three_states() {
    let statuses = vec![
        CheckPlanStatus {
            check: "cargo test".into(),
            ran: true,
            ok: Some(true),
        },
        CheckPlanStatus {
            check: "cargo clippy".into(),
            ran: true,
            ok: Some(false),
        },
        CheckPlanStatus {
            check: "npm test".into(),
            ran: false,
            ok: None,
        },
    ];
    let text = render_check_plan_reconciliation(&statuses);
    assert!(text.contains("✅ `cargo test`"));
    assert!(text.contains("❌ `cargo clippy`") && text.contains("失败"));
    assert!(text.contains("⏭ `npm test`") && text.contains("未执行"));
}
