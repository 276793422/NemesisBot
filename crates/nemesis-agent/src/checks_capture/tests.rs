//! [`super`] 检查证据捕获单元测试（②typed checks 块，2026-10-19）。

use super::*;

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
    assert_eq!(classify_result("build ok, 3 tests passed"), Some(true));
    assert_eq!(
        classify_result("Tool error: command not found"),
        Some(false)
    );
    // 拒绝家族（未执行，不入账）：
    assert_eq!(classify_result("⛔ ESTOP: 已急停 — 工具调用已被拒绝"), None);
    assert_eq!(
        classify_result("⛔ RATE LIMIT — NO APPROVAL CHANNEL [layer:limits]"),
        None
    );
    assert_eq!(
        classify_result("Plan mode: file modifications are denied."),
        None
    );
    assert_eq!(
        classify_result(
            "Error: Tool 'exec' is hidden by configuration (agents.hidden_tools) and cannot be executed."
        ),
        None
    );
    assert_eq!(
        classify_result(
            "Error: Tool 'exec' is not permitted by this agent's tool_policy allow-list and cannot be executed."
        ),
        None
    );
    assert_eq!(
        classify_result(
            "Error: Tool 'exec' requires model tier 'big' or above; the active model's tier is lower."
        ),
        None
    );
    assert_eq!(
        classify_result("⛔ SECURITY BLOCKED [layer:ssrf] target denied"),
        None
    );
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
