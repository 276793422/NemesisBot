//! Guardian 二审误拦统计批次（2026-11-20）。guardian_snapshot 统计面
//! 的确定性臂：
//! - `GuardianAuditStats::observe` 四类 guardian 行计数 + 非 guardian /
//!   链形态行（顶层 source 无嵌套 request）不计数
//! - `to_json`：误拦率 = approved ÷ escalated；escalated=0 → null
//!   （无分母不造 0%）；default 全零 + available=false
//! - `audit()` handler 集成：混合行 + 粘行垃圾行跳过 + 空/缺目录诚实
//!   降级（guardian_stats 字段恒在场）

use super::*;

fn review_row(decision: &str, tool: &str) -> serde_json::Value {
    serde_json::json!({
        "event_id": "e1",
        "request": {
            "op_type": "process_exec",
            "danger_level": "critical",
            "source": "guardian_review",
            "target": tool,
        },
        "decision": decision,
        "reason": "recommendation=deny",
        "timestamp": "2026-11-20T10:00:00+08:00",
        "policy_rule": "guardian_mode=critical",
    })
}

fn approval_row(decision: &str, tool: &str) -> serde_json::Value {
    serde_json::json!({
        "event_id": "e2",
        "request": {
            "op_type": "process_exec",
            "danger_level": "critical",
            "source": "guardian_verdict_approval",
            "target": tool,
        },
        "decision": decision,
        "reason": "guardian verdict; user approved",
        "timestamp": "2026-11-20T10:00:01+08:00",
        "policy_rule": "guardian_mode=critical",
    })
}

#[test]
fn observe_counts_all_four_guardian_row_kinds() {
    let mut gs = GuardianAuditStats::default();
    gs.observe(&review_row("allow", "exec"));
    gs.observe(&review_row("allow", "write_file"));
    gs.observe(&review_row("escalate", "exec"));
    gs.observe(&approval_row("approved", "exec"));
    gs.observe(&approval_row("denied", "exec"));
    assert_eq!(gs.reviewed_total, 3);
    assert_eq!(gs.allowed_by_judge, 2);
    assert_eq!(gs.escalated, 1);
    assert_eq!(gs.approved_after_review, 1);
    assert_eq!(gs.denied_after_review, 1);
    let out = gs.to_json();
    assert_eq!(out["available"], true);
    // 误拦率 = approved ÷ escalated = 1/1
    assert_eq!(out["misjudge_rate"], serde_json::json!(1.0));
}

#[test]
fn observe_ignores_non_guardian_and_chain_form_rows() {
    let mut gs = GuardianAuditStats::default();
    // 普通管线审计行（source 非 guardian_）
    gs.observe(&serde_json::json!({
        "event_id": "e3",
        "request": { "op_type": "process_exec", "source": "pipeline", "target": "ls" },
        "decision": "denied",
        "timestamp": "2026-11-20T10:00:00+08:00",
        "policy_rule": "abac",
    }));
    // 审计链形态行（顶层 source/decision，无嵌套 request）——IntegrityChain
    // 的 AuditEvent 序列化形态，不得混入统计。
    gs.observe(&serde_json::json!({
        "source": "guardian_review",
        "decision": "escalate",
        "tool_name": "exec",
        "timestamp": "2026-11-20T10:00:00+08:00",
    }));
    // decision 值域外的 guardian 行（未知形态诚实忽略）
    gs.observe(&review_row("blocked", "exec"));
    assert_eq!(gs.reviewed_total, 0);
    assert_eq!(gs.approved_after_review, 0);
    assert_eq!(gs.denied_after_review, 0);
    let out = gs.to_json();
    assert_eq!(out["available"], false);
    assert_eq!(out["misjudge_rate"], serde_json::Value::Null);
}

#[test]
fn to_json_without_escalation_has_null_rate_not_zero() {
    let mut gs = GuardianAuditStats::default();
    gs.observe(&review_row("allow", "exec"));
    let out = gs.to_json();
    assert_eq!(out["available"], true);
    assert_eq!(out["reviewed_total"], 1);
    assert_eq!(out["escalated"], 0);
    assert_eq!(out["misjudge_rate"], serde_json::Value::Null);
}

#[test]
fn audit_handler_counts_guardian_stats_and_skips_garbage_lines() {
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs/security_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    let mut body = String::new();
    body.push_str(&format!("{}\n", review_row("escalate", "exec")));
    body.push_str(&format!("{}\n", approval_row("approved", "exec")));
    body.push_str(&format!("{}\n", review_row("allow", "write_file")));
    // 粘行垃圾（半行 JSON）——统计与条目同口径跳过，不炸不计数
    body.push_str("{\"event_id\": \"trunc\", \"request\": {\"source\": \"guar\n");
    body.push_str(&format!("{}\n", approval_row("denied", "exec")));
    std::fs::write(log_dir.join("audit.jsonl"), body).unwrap();

    let h = SecurityHandler::default();
    let resp = h
        .audit(dir.path().to_str().unwrap(), 100, 0)
        .unwrap()
        .unwrap();
    let stats = resp.get("guardian_stats").expect("guardian_stats 字段在场");
    assert_eq!(stats["available"], true);
    assert_eq!(stats["reviewed_total"], 2);
    assert_eq!(stats["allowed_by_judge"], 1);
    assert_eq!(stats["escalated"], 1);
    assert_eq!(stats["approved_after_review"], 1);
    assert_eq!(stats["denied_after_review"], 1);
    assert_eq!(stats["misjudge_rate"], serde_json::json!(1.0));
    // 4 条合法行照常进条目列表
    assert_eq!(resp["total"], 4);
}

#[test]
fn audit_handler_honest_zero_when_no_guardian_rows_or_no_dir() {
    let h = SecurityHandler::default();

    // 目录存在但无 guardian 行（guardian 未开启的常态）
    let dir = tempfile::tempdir().unwrap();
    let log_dir = dir.path().join("logs/security_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("audit.jsonl"),
        format!("{}\n", review_row("allow", "exec")).replace("guardian_review", "pipeline"),
    )
    .unwrap();
    let resp = h
        .audit(dir.path().to_str().unwrap(), 100, 0)
        .unwrap()
        .unwrap();
    assert_eq!(resp["guardian_stats"]["available"], false);
    assert_eq!(
        resp["guardian_stats"]["misjudge_rate"],
        serde_json::Value::Null
    );

    // 目录不存在（审计未接线/从未落盘）——字段恒在场，前端诚实降级
    let empty = tempfile::tempdir().unwrap();
    let resp2 = h
        .audit(empty.path().to_str().unwrap(), 100, 0)
        .unwrap()
        .unwrap();
    assert_eq!(resp2["guardian_stats"]["available"], false);
    assert_eq!(resp2["entries"].as_array().unwrap().len(), 0);
}
