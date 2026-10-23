//! scheduler 测试（外置：`#[cfg(test)] mod tests;` 约定）。
//! 循环行为（翻转→报警→限频）在 m2 live 集成测试验真目标。

use super::*;

fn poll_tool(addr: &str, mode: &str) -> serde_json::Value {
    serde_json::json!({
        "tool_name": "mon_state",
        "description": "d",
        "action": {"primitive": "poll_report", "addr": addr, "mode": mode, "interval_secs": 1},
        "run_policy": "auto"
    })
}

fn profile_with(tools: Vec<serde_json::Value>) -> Profile {
    let mut v = serde_json::json!({
        "schema_version": 1,
        "profile_id": "p",
        "name": "n",
        "created_at": "2026-10-22T00:00:00Z",
        "last_verified_at": "2026-10-22T00:00:00Z",
        "status": "active",
        "target": {"exe_name": "x.exe", "machine": "x64", "verify_policy": "hash_or_version"},
        "attach": {"mode": "attach"},
        "data_structures": [
            {"name": "A", "size": 8, "fields": [{"offset": 0, "name": "v", "type": "u32"}]},
            {"name": "B", "size": 8, "fields": [{"offset": 0, "name": "v", "type": "u32"}]}
        ],
        "tools": [],
        "verification": {"on_attach": ["file_sha256"]}
    });
    v["tools"] = serde_json::Value::Array(tools);
    serde_json::from_value(v).unwrap()
}

#[test]
fn referenced_structs_dedup_and_filter() {
    let profile = profile_with(vec![
        poll_tool("struct:A", "on_change"),
        poll_tool("struct:B", "on_change"),
        poll_tool("struct:A", "every"),
    ]);
    let refs = referenced_structs(&profile);
    assert_eq!(refs, vec!["A".to_string(), "B".to_string()], "去重保序");
}

#[test]
fn referenced_structs_ignores_non_poll_tools() {
    let profile = profile_with(vec![serde_json::json!({
        "tool_name": "fn1",
        "description": "d",
        "action": {"primitive": "call_fn", "addr": "fn:do_thing"},
        "run_policy": "confirm"
    })]);
    assert!(referenced_structs(&profile).is_empty());
}

#[test]
fn alert_rate_limit_constant_sane() {
    assert!(
        ALERT_RATE_LIMIT >= Duration::from_secs(30),
        "限频窗过短会报警风暴"
    );
    assert!(ALERT_RATE_LIMIT <= Duration::from_secs(300));
}
