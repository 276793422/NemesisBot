use super::*;

// opt_u64 ----

#[test]
fn opt_u64_returns_default_when_missing() {
    let data = Some(serde_json::json!({"limit": 5}));
    assert_eq!(opt_u64(&data, "offset", 99), 99);
}

#[test]
fn opt_u64_returns_value_when_present() {
    let data = Some(serde_json::json!({"limit": 5}));
    assert_eq!(opt_u64(&data, "limit", 99), 5);
}

#[test]
fn opt_u64_returns_default_when_none_data() {
    assert_eq!(opt_u64(&None, "limit", 42), 42);
}

#[test]
fn opt_u64_returns_default_when_non_number() {
    let data = Some(serde_json::json!({"limit": "many"}));
    assert_eq!(opt_u64(&data, "limit", 7), 7);
}

// file_type_name ----

#[test]
fn file_type_name_strips_first_segment() {
    // "01.AI.Request.raw.json" → "AI.Request.raw.json"
    assert_eq!(
        file_type_name("01.AI.Request.raw.json"),
        "AI.Request.raw.json"
    );
}

#[test]
fn file_type_name_returns_whole_when_no_dot() {
    assert_eq!(file_type_name("README"), "README");
}

// sanitize_session_key ----

#[test]
fn sanitize_session_key_replaces_path_and_special_chars() {
    let cleaned = sanitize_session_key("web://user/chat|file?x");
    assert!(!cleaned.contains(':'));
    assert!(!cleaned.contains('/'));
    assert!(!cleaned.contains('?'));
    assert!(!cleaned.contains('|'));
    assert_eq!(cleaned, "web___user_chat_file_x");
}

#[test]
fn sanitize_session_key_passes_through_plain_text() {
    assert_eq!(sanitize_session_key("plain-key_123"), "plain-key_123");
}

// parse_duration_seconds ----

#[test]
fn parse_duration_seconds_whole_seconds() {
    assert_eq!(parse_duration_seconds("30s"), Some(30000));
}

#[test]
fn parse_duration_seconds_fractional() {
    assert_eq!(parse_duration_seconds("0.022s"), Some(22));
}

#[test]
fn parse_duration_seconds_zero() {
    assert_eq!(parse_duration_seconds("0s"), Some(0));
}

#[test]
fn parse_duration_seconds_rejects_non_numeric_unit() {
    // "5m" → not a valid f64 → None (this helper only handles plain seconds)
    assert_eq!(parse_duration_seconds("5m"), None);
}

#[test]
fn parse_duration_seconds_rejects_negative() {
    assert_eq!(parse_duration_seconds("-5s"), None);
}

#[test]
fn parse_duration_seconds_rejects_garbage() {
    assert_eq!(parse_duration_seconds("abc"), None);
}

// stringify_message_content ----

#[test]
fn stringify_message_content_none_is_empty() {
    assert_eq!(stringify_message_content(None), "");
}

#[test]
fn stringify_message_content_plain_string() {
    assert_eq!(
        stringify_message_content(Some(&serde_json::json!("hello world"))),
        "hello world"
    );
}

#[test]
fn stringify_message_content_array_of_text_parts() {
    let content = serde_json::json!([
        { "text": "line1" },
        { "text": "line2" },
    ]);
    assert_eq!(stringify_message_content(Some(&content)), "line1\nline2");
}

#[test]
fn stringify_message_content_array_of_content_parts() {
    let content = serde_json::json!([{ "content": "alt" }]);
    assert_eq!(stringify_message_content(Some(&content)), "alt");
}

#[test]
fn stringify_message_content_other_value_stringified() {
    let content = serde_json::json!(42);
    assert_eq!(stringify_message_content(Some(&content)), "42");
}

// compute_audit_hash ----

#[test]
fn compute_audit_hash_is_deterministic() {
    let ev = nemesis_security::integrity::AuditEvent {
        id: "e1".into(),
        timestamp: "2026-06-29T00:00:00Z".into(),
        operation: "file_write".into(),
        tool_name: "filesystem".into(),
        user: "alice".into(),
        source: "web".into(),
        target: "/tmp/f".into(),
        decision: "allow".into(),
        reason: "ok".into(),
        hash: String::new(),
        prev_hash: "genesis".into(),
        sign: None,
        hash_version: 0,
        instance_id: None,
        security_level: None,
    };
    let h1 = compute_audit_hash(&ev);
    let h2 = compute_audit_hash(&ev);
    assert_eq!(h1, h2);
    assert!(!h1.is_empty());
}

#[test]
fn compute_audit_hash_changes_with_field() {
    let base = || nemesis_security::integrity::AuditEvent {
        id: "e1".into(),
        timestamp: "t".into(),
        operation: "op".into(),
        tool_name: "tn".into(),
        user: "u".into(),
        source: "s".into(),
        target: "tgt".into(),
        decision: "allow".into(),
        reason: "r".into(),
        hash: String::new(),
        prev_hash: "p".into(),
        sign: None,
        hash_version: 0,
        instance_id: None,
        security_level: None,
    };
    let mut ev = base();
    let h1 = compute_audit_hash(&ev);
    ev.operation = "file_delete".into();
    let h2 = compute_audit_hash(&ev);
    assert_ne!(h1, h2);
}

#[test]
fn compute_audit_hash_v2_v3_delegated_not_empty() {
    // 漂移实录根修锁：#31 加 v2 时 wrapper cap 恒 >1，v2/v3 事件在此恒算
    // 空 hash → Dashboard 链诊断假 mismatch。现 cap 走共享常量，v2/v3
    // 委托真相源非空；仅超限版本（v4+）空串。
    let base = |hash_version: u8, security_level: Option<String>| {
        nemesis_security::integrity::AuditEvent {
            id: "e1".into(),
            timestamp: "t".into(),
            operation: "op".into(),
            tool_name: "tn".into(),
            user: "u".into(),
            source: "s".into(),
            target: "tgt".into(),
            decision: "allow".into(),
            reason: "r".into(),
            hash: String::new(),
            prev_hash: "p".into(),
            sign: None,
            hash_version,
            instance_id: Some("inst".into()),
            security_level,
        }
    };
    let v2 = base(2, None);
    assert!(!compute_audit_hash(&v2).is_empty(), "v2 事件必须可算 hash");
    let v3 = base(3, Some("strict".into()));
    assert!(!compute_audit_hash(&v3).is_empty(), "v3 事件必须可算 hash");
    assert_ne!(
        compute_audit_hash(&v2),
        compute_audit_hash(&v3),
        "v2/v3 公式不同（v3 多覆盖 security_level）"
    );
    let v4 = base(4, Some("strict".into()));
    assert!(
        compute_audit_hash(&v4).is_empty(),
        "超支持上限版本空串（fail-closed，与 verify 同语义）"
    );
    // v3 与真相源逐字节一致（单一真相源委托）。
    assert_eq!(
        compute_audit_hash(&v3),
        nemesis_security::integrity::AuditChain::compute_event_hash(&v3)
    );
}

// parse_local_tool_results ----

#[test]
fn parse_local_tool_results_empty() {
    assert!(parse_local_tool_results("").is_empty());
    assert!(parse_local_tool_results("no operations here").is_empty());
}

#[test]
fn parse_local_tool_results_success_with_args_and_duration() {
    let md = "\
## Operation 1: Tool Execution

**Name**: read_file
**Status**: success

### Arguments
{\"path\": \"/tmp/a\"}

### Result
hello contents

### Duration
0.022s

---
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    let op = &out[0];
    assert_eq!(op["name"], "read_file");
    assert_eq!(op["callId"], "read_file");
    assert_eq!(op["result"]["status"], "success");
    assert_eq!(op["result"]["output"], "hello contents\n\n");
    assert_eq!(op["args"]["path"], "/tmp/a");
    assert_eq!(op["duration_ms"], 22);
}

#[test]
fn parse_local_tool_results_error_branch() {
    let md = "\
## Operation 2: Tool Execution

**Name**: write_file
**Status**: error

### Error
permission denied

### Duration
0.005s
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    let op = &out[0];
    assert_eq!(op["name"], "write_file");
    assert_eq!(op["result"]["status"], "error");
    assert_eq!(op["result"]["error"], "permission denied\n\n");
    // No `output` key on the error branch.
    assert!(op["result"].get("output").is_none());
}

#[test]
fn parse_local_tool_results_args_not_json_falls_back_to_string() {
    let md = "\
## Operation 1: Tool Execution

**Name**: shell
**Status**: success

### Arguments
not a json blob

### Result
done
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 1);
    // Non-JSON args become a JSON string value.
    assert_eq!(out[0]["args"], "not a json blob");
}

#[test]
fn parse_local_tool_results_multiple_operations_flush() {
    let md = "\
## Operation 1: Tool Execution

**Name**: a
**Status**: success

## Operation 2: Tool Execution

**Name**: b
**Status**: success
";
    let out = parse_local_tool_results(md);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["name"], "a");
    assert_eq!(out[1]["name"], "b");
}

// session_recency_cmp（BUG-20261116-011）----
//
// logs.session_list 收尾重排此前用 lastTime 裸字典序——scan_session_logs 的
// BUG-20261102-325 纪律（解析成时间戳 + id 破平）没跟进到该位点。以下各测
// 钉住解析序的语义：desc（最近在前）+ 混 offset 正确 + 自适应小数位正确 +
// 不可解析沉底 + 同刻 id 破平。

#[test]
fn session_recency_cmp_mixed_offsets_orders_by_instant() {
    // 10:00+08:00 = 02:00Z，比 03:00Z 早——desc 序里 03:00Z 排前。比较器
    // 语义：cmp(a,b)=Greater ⟺ b 更新 ⟺ b 排前。裸字典序会把 '1' > '0'
    // 判成 a 更新（错序实锤形态）。
    let a = serde_json::json!({"id": "s1", "lastTime": "2026-11-16T10:00:00+08:00"});
    let b = serde_json::json!({"id": "s2", "lastTime": "2026-11-16T03:00:00Z"});
    assert_eq!(session_recency_cmp(&a, &b), std::cmp::Ordering::Greater);
    assert_eq!(session_recency_cmp(&b, &a), std::cmp::Ordering::Less);
}

#[test]
fn session_recency_cmp_adaptive_fraction_digits_z_suffix() {
    // UTC 形态下「无小数 + Z 后缀」vs「有小数」：22.5 > 22.0，b（22.5Z）
    // 更新、排前。裸字典序在公共前缀 "22" 后比较 'Z'(0x5A) vs '.'(0x2E)
    // 会把 22Z 判大（+08:00 形态因 '+' < '.' 恰好不出错——错序随 offset
    // 形态翻转）。
    let a = serde_json::json!({"id": "s1", "lastTime": "2026-11-16T10:00:22Z"});
    let b = serde_json::json!({"id": "s2", "lastTime": "2026-11-16T10:00:22.5Z"});
    assert_eq!(session_recency_cmp(&a, &b), std::cmp::Ordering::Greater);
}

#[test]
fn session_recency_cmp_unparseable_sinks_to_oldest() {
    let good = serde_json::json!({"id": "s1", "lastTime": "2026-11-16T10:00:00+08:00"});
    let bad = serde_json::json!({"id": "s2", "lastTime": ""});
    let missing = serde_json::json!({"id": "s3"});
    // 不可解析（空串/缺字段）= i64::MIN 沉底（仅向前语义）——good 更新、
    // 排前，cmp(good, X) = Less。
    assert_eq!(session_recency_cmp(&good, &bad), std::cmp::Ordering::Less);
    assert_eq!(
        session_recency_cmp(&good, &missing),
        std::cmp::Ordering::Less
    );
    // 两个都不可解析 → 回落 id 破平（升序）。
    assert_eq!(
        session_recency_cmp(&bad, &missing),
        std::cmp::Ordering::Less
    );
}

#[test]
fn session_recency_cmp_equal_last_time_breaks_tie_by_id() {
    let a = serde_json::json!({"id": "s2", "lastTime": "2026-11-16T10:00:00+08:00"});
    let b = serde_json::json!({"id": "s1", "lastTime": "2026-11-16T10:00:00+08:00"});
    // 同刻：id 升序破平（s1 在前 → cmp(a=s2, b=s1) = Greater）。
    assert_eq!(session_recency_cmp(&a, &b), std::cmp::Ordering::Greater);
    assert_eq!(session_recency_cmp(&b, &a), std::cmp::Ordering::Less);
}

#[test]
fn session_recency_cmp_sorts_slice_recent_first() {
    let mut sessions = [
        serde_json::json!({"id": "old", "lastTime": "2026-11-16T10:00:22Z"}),
        // 与 old 同刻但有小数——真序在 old 之前（22.5 > 22.0）。
        serde_json::json!({"id": "frac", "lastTime": "2026-11-16T10:00:22.5Z"}),
        serde_json::json!({"id": "new", "lastTime": "2026-11-16T10:01:00Z"}),
        serde_json::json!({"id": "bad", "lastTime": "garbage"}),
    ];
    sessions.sort_by(session_recency_cmp);
    let ids: Vec<&str> = sessions.iter().map(|s| s["id"].as_str().unwrap()).collect();
    assert_eq!(ids, vec!["new", "frac", "old", "bad"]);
}
