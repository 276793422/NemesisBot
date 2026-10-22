//! board_claim 工具单测：args 解析 / 信封形状 / browse 投影格式化。

use super::*;

#[test]
fn parse_args_browse_claim_and_errors() {
    // browse。
    let a = parse_claim_args(r#"{"action":"browse"}"#).unwrap();
    assert!(matches!(a, ClaimAction::Browse));

    // claim（编号形态）。
    let a = parse_claim_args(r#"{"action":"claim","issue":"NB-15"}"#).unwrap();
    assert!(matches!(a, ClaimAction::Claim { ref issue } if issue == "NB-15"));

    // claim（数字 id 形态——master 侧 resolve_issue_ref 同样收）。
    let a = parse_claim_args(r#"{"action":"claim","issue":"42"}"#).unwrap();
    assert!(matches!(a, ClaimAction::Claim { ref issue } if issue == "42"));

    // claim 缺 issue = 诚实报错（不是 browse 回退）。
    assert!(parse_claim_args(r#"{"action":"claim"}"#).is_err());
    // claim issue 空串 = 报错。
    assert!(parse_claim_args(r#"{"action":"claim","issue":"  "}"#).is_err());
    // 词表外 action。
    assert!(parse_claim_args(r#"{"action":"steal","issue":"NB-1"}"#).is_err());
    // 非法 JSON。
    assert!(parse_claim_args("not json").is_err());
    // action 缺省 = 词表外空串。
    assert!(parse_claim_args("{}").is_err());
}

#[test]
fn envelopes_match_master_side_consumed_shape() {
    let list = build_claimable_list_envelope(50);
    assert_eq!(list["ns"], "board");
    assert_eq!(list["op"], "claimable.list");
    assert_eq!(list["body"]["limit"], 50);
    // corr_id 非空（幂等/关联礼貌）。
    assert!(!list["corr_id"].as_str().unwrap_or("").is_empty());

    let claim = build_claim_envelope("NB-15");
    assert_eq!(claim["ns"], "board");
    assert_eq!(claim["op"], "claim");
    assert_eq!(claim["body"]["issue"], "NB-15");
}

#[test]
fn format_claimable_list_empty_and_entries() {
    // 空列表 = 诚实声明（不是错误）。
    let empty = serde_json::json!({"ok": true, "body": {"items": [], "count": 0}});
    let out = format_claimable_list(&empty);
    assert!(out.contains("No claimable issues"), "{out}");

    // 正常条目：eligible 与 blocked 双形态。
    let body = serde_json::json!({"ok": true, "body": {"items": [
        {"number": "NB-1", "title": "plain", "priority": 2, "status": "todo",
         "required_role": null, "required_profession": null, "eligible": true},
        {"number": "NB-2", "title": "ui work", "priority": 1, "status": "backlog",
         "required_role": null, "required_profession": "ui-design",
         "eligible": false, "blocked_reason": "需职能 ui-design，本节点 [dev:cpp]"}
    ], "count": 2}});
    let out = format_claimable_list(&body);
    assert!(out.contains("NB-1"), "{out}");
    assert!(out.contains("ELIGIBLE"), "{out}");
    assert!(out.contains("BLOCKED"), "{out}");
    assert!(out.contains("需职能 ui-design"), "{out}");
    // items 在 /body 下（顶层读恒空——run4 实录口径）。
    let top_level = serde_json::json!({"ok": true, "items": [
        {"number": "X", "title": "t", "priority": 0, "status": "todo", "eligible": true}
    ]});
    assert!(
        format_claimable_list(&top_level).contains("No claimable issues"),
        "顶层 items 必须读不到（防形状回归）"
    );
}
