//! [`super`] 检查证据捕获单元测试（②typed checks 块，2026-10-19）。

use super::*;

// R4 回归锁（原 review 探针反转）：真实命令失败必须分类为 ok=false。
#[tokio::test]
async fn nonzero_exec_is_classified_failed() {
    use crate::r#loop::Tool;
    let tmp = tempfile::tempdir().unwrap();
    let tool = crate::loop_tools::ExecTool::new(&tmp.path().to_string_lossy(), true);
    let ctx = crate::context::RequestContext::new("web", "review", "tester", "review");
    let result = tool.execute(r#"{"command":"exit 7"}"#, &ctx).await.unwrap();
    let classification = classify_result("exec", &result);
    assert!(result.contains("Exit code: 7"));
    assert_eq!(
        classification,
        Some(false),
        "non-zero exit must not be recorded as success"
    );
}

// R4 回归锁：成功输出**中途**打印 `Tool error:` 字样不误判（前缀而非 contains）。
#[test]
fn successful_output_mentioning_tool_error_is_not_misjudged() {
    let output = "all checks passed\nnote: fixture echoes the literal string Tool error: end";
    assert_eq!(classify_result("exec", output), Some(true));
}

#[test]
fn is_checks_class_matches_exact_names() {
    assert!(is_checks_class("exec"));
    assert!(is_checks_class("run_checks"));
    // exec_async 不在名单（异步结果未到账即捕获 = 谎称已验，见模块头）。
    assert!(!is_checks_class("exec_async"));
    assert!(!is_checks_class("shell"));
    assert!(!is_checks_class("exec2"));
}

#[test]
fn extract_cmd_exec_full_command() {
    let cmd = extract_cmd(
        "exec",
        r#"{"command":"cargo test -p nemesis-agent","timeout":30}"#,
    )
    .expect("command field present");
    assert_eq!(cmd, "cargo test -p nemesis-agent");
}

#[test]
fn extract_cmd_exec_missing_or_bad_args_is_none() {
    // 缺 command 字段 / args 非 JSON / 空命令 = 调用没执行成功，不造值。
    assert_eq!(extract_cmd("exec", r#"{"timeout":30}"#), None);
    assert_eq!(extract_cmd("exec", "not-json"), None);
    assert_eq!(extract_cmd("exec", r#"{"command":"  "#), None);
}

#[test]
fn extract_cmd_run_checks_renders_scope_and_filter() {
    assert_eq!(
        extract_cmd("run_checks", r#"{}"#).unwrap(),
        "run_checks scope=all"
    );
    assert_eq!(
        extract_cmd("run_checks", r#"{"scope":"test","filter":"journal"}"#).unwrap(),
        "run_checks scope=test filter=journal"
    );
}

#[test]
fn classify_result_executed_vs_refused() {
    // 真实执行：成功 / 工具失败（失败也是证据）。
    assert_eq!(
        classify_result("exec", "build ok, 3 tests passed"),
        Some(true)
    );
    assert_eq!(
        classify_result("exec", "Tool error: command not found"),
        Some(false)
    );
    // 拒绝家族（未执行，不入账）：
    assert_eq!(
        classify_result("exec", "⛔ ESTOP: 已急停 — 工具调用已被拒绝"),
        None
    );
    assert_eq!(
        classify_result("exec", "⛔ RATE LIMIT — NO APPROVAL CHANNEL [layer:limits]"),
        None
    );
    assert_eq!(
        classify_result("exec", "Plan mode: file modifications are denied."),
        None
    );
    assert_eq!(
        classify_result(
            "exec",
            "Error: Tool 'exec' is hidden by configuration (agents.hidden_tools) and cannot be executed."
        ),
        None
    );
    assert_eq!(
        classify_result(
            "exec",
            "Error: Tool 'exec' is not permitted by this agent's tool_policy allow-list and cannot be executed."
        ),
        None
    );
    assert_eq!(
        classify_result(
            "exec",
            "Error: Tool 'exec' requires model tier 'big' or above; the active model's tier is lower."
        ),
        None
    );
    assert_eq!(
        classify_result("exec", "⛔ SECURITY BLOCKED [layer:ssrf] target denied"),
        None
    );
}

// R4：run_checks 阶段状态行判据（行首 ✗/⏱ = 失败；缩进回显不误判）。
#[test]
fn classify_result_run_checks_stage_lines() {
    // 全部通过：既有 ▶ 行、无 ✗/⏱ 行首。
    let pass = "[run_checks] scope=all · rust\n▶ build: exit 0 (1.2s)\n全部通过，无失败签名行。\n";
    assert_eq!(classify_result("run_checks", pass), Some(true));
    // exit 非零阶段：✗ 行首。
    let fail = "[run_checks] scope=all · rust\n▶ build: exit 0 (1.2s)\n✗ test: exit 101 (3.4s) — passed 9 / failed 1\n失败签名行（去重后 1 行）：\n";
    assert_eq!(classify_result("run_checks", fail), Some(false));
    // 超时阶段：⏱ 行首。
    let timeout = "[run_checks] scope=all · rust\n⏱ test: TIMED OUT after 600s (600.1s elapsed)\n";
    assert_eq!(classify_result("run_checks", timeout), Some(false));
    // 聚焦摘录区缩进回显 stdout 含 ✗ 字符（非行首）不误判。
    let echo = "[run_checks] scope=test · rust\n▶ test: exit 0 (2.0s)\n失败签名行（去重后 1 行）：\n  ✗ 从 stdout 原文缩进回显的行\n";
    assert_eq!(classify_result("run_checks", echo), Some(true));
    // 拒绝族对 run_checks 同样成立。
    assert_eq!(
        classify_result("run_checks", "Plan mode: file modifications are denied."),
        None
    );
}

// R4：record_check 端到端——run_checks 失败阶段（✗ 行首）记 ok=false。
#[test]
fn record_check_run_checks_failed_stage_recorded_as_evidence() {
    let map: ChecksCaptureMap = RwLock::new(HashMap::new());
    map.write()
        .insert("sess".into(), Arc::new(Mutex::new(Vec::new())));

    let pass_reply =
        "[run_checks] scope=all · rust\n▶ build: exit 0 (1.2s)\n全部通过，无失败签名行。\n";
    record_check(&map, "sess", "run_checks", r#"{"scope":"all"}"#, pass_reply);
    let fail_reply =
        "[run_checks] scope=all · rust\n✗ test: exit 101 (3.4s) — passed 9 / failed 1\n";
    record_check(&map, "sess", "run_checks", r#"{"scope":"all"}"#, fail_reply);

    let sink = map.read().get("sess").cloned().unwrap();
    let records = sink.lock();
    assert_eq!(records.len(), 2);
    assert!(records[0].ok, "all-pass run_checks is ok");
    assert!(!records[1].ok, "failed stage must be recorded as evidence");
}

#[test]
fn truncate_chars_is_char_safe() {
    assert_eq!(truncate_chars("hello", 10), "hello");
    let s = truncate_chars("中文测试文本", 3);
    assert_eq!(s, "中文测…");
}

#[test]
fn record_check_happy_path_and_session_isolation() {
    let map: ChecksCaptureMap = RwLock::new(HashMap::new());
    map.write()
        .insert("sess-a".into(), Arc::new(Mutex::new(Vec::new())));

    record_check(&map, "sess-a", "exec", r#"{"command":"cargo build"}"#, "ok");
    // 其他会话 / 非检查类工具 / 未 begin 的会话：全部 no-op。
    record_check(&map, "sess-b", "exec", r#"{"command":"evil"}"#, "ok");
    record_check(&map, "sess-a", "write_file", r#"{"path":"x"}"#, "ok");

    let sink = map.read().get("sess-a").cloned().unwrap();
    let records = sink.lock();
    assert_eq!(records.len(), 1, "only sess-a exec recorded");
    assert_eq!(records[0].tool, "exec");
    assert_eq!(records[0].cmd, "cargo build");
    assert!(records[0].ok);
    assert_eq!(records[0].summary, "ok");
}

#[test]
fn record_check_skips_refusals_and_unextractable_cmds() {
    let map: ChecksCaptureMap = RwLock::new(HashMap::new());
    map.write()
        .insert("sess".into(), Arc::new(Mutex::new(Vec::new())));

    // 被闸拒绝未执行：不入账（记进去 = 谎称检查跑了且失败）。
    record_check(
        &map,
        "sess",
        "exec",
        r#"{"command":"cargo test"}"#,
        "Error: Tool 'exec' is hidden by configuration and cannot be executed.",
    );
    // args 解析失败：调用没执行成功，不造值。
    record_check(&map, "sess", "exec", "not-json", "Tool error: Invalid JSON");
    // 工具失败但真实执行过：入账 ok=false（失败也是证据）。
    record_check(
        &map,
        "sess",
        "exec",
        r#"{"command":"cargo test"}"#,
        "Tool error: compile failed",
    );

    let sink = map.read().get("sess").cloned().unwrap();
    let records = sink.lock();
    assert_eq!(records.len(), 1);
    assert!(
        !records[0].ok,
        "executed-but-failed is recorded as evidence"
    );
}

#[test]
fn record_check_summary_truncated_to_cap() {
    let map: ChecksCaptureMap = RwLock::new(HashMap::new());
    map.write()
        .insert("sess".into(), Arc::new(Mutex::new(Vec::new())));
    let big = "x".repeat(SUMMARY_CAP + 100);
    record_check(&map, "sess", "exec", r#"{"command":"dump"}"#, &big);

    let sink = map.read().get("sess").cloned().unwrap();
    let records = sink.lock();
    assert_eq!(records[0].summary.chars().count(), SUMMARY_CAP + 1);
    assert!(records[0].summary.ends_with('…'));
}
