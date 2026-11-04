//! 重规划器纯逻辑层单测（解析/校验/渲染/映射，零依赖零 IO）：硬校验
//! 逐条钉死（evidence 非空 / op 缺失 / modify 全空 / add 标题 / 职能
//! slug / depends_on 界内·自引用·环 / 条数上限），编排层测试在
//! nemesisbot `board_replan/tests.rs`。

use super::*;

fn act(json: serde_json::Value) -> Result<ReplanOutput, ReplanParseError> {
    parse_replan(&json.to_string())
}

#[test]
fn parse_accepts_valid_mixed_actions() {
    let out = act(serde_json::json!({
        "analysis": "两个子单已被新路径取代",
        "actions": [
            {"op": "cancel", "number": "NB-3", "reason": "被替代", "evidence": "NB-4 已完成同目标交付"},
            {"op": "modify", "number": "NB-5", "title": "改后的标题", "evidence": "NB-5 评审 FAIL：差距为缺少 X"},
            {"op": "add", "title": "补救子单", "acceptance_criteria": "- [DONE] 补上", "required_profession": "dev:cpp", "depends_on": [], "evidence": "unknown: 依赖服务版本未定"}
        ]
    }))
    .expect("合法混合动作应解析通过");
    assert_eq!(out.actions.len(), 3);
    assert_eq!(out.actions[0].op, Some(ReplanOp::Cancel));
    assert_eq!(out.actions[1].op, Some(ReplanOp::Modify));
    assert_eq!(out.actions[2].op, Some(ReplanOp::Add));
    assert_eq!(out.actions[2].required_profession, "dev:cpp");
}

#[test]
fn parse_empty_actions_is_legal() {
    let out = act(serde_json::json!({"analysis": "证据不足以支持任何 action"}))
        .expect("空 actions 合法（诚实无建议）");
    assert!(out.actions.is_empty());
    assert_eq!(out.analysis, "证据不足以支持任何 action");
}

#[test]
fn parse_tolerates_fences_and_prose() {
    let raw = format!(
        "好的，以下是我的建议：\n```json\n{}\n```\n以上。",
        serde_json::json!({"actions": [{"op": "add", "title": "t", "evidence": "e"}]})
    );
    let out = parse_replan(&raw).expect("围栏与杂散文字应被容忍");
    assert_eq!(out.actions.len(), 1);
}

#[test]
fn parse_fails_without_json() {
    let err = parse_replan("我觉得不需要重规划。").expect_err("无 JSON 应失败");
    assert!(err.message.contains("JSON"));
}

#[test]
fn parse_fails_on_missing_op() {
    let err = act(serde_json::json!({
        "actions": [{"number": "NB-3", "evidence": "e"}]
    }))
    .expect_err("缺 op 应失败");
    assert!(
        err.message.contains("op"),
        "回灌文案应指明 op：{}",
        err.message
    );
}

#[test]
fn parse_fails_on_empty_evidence() {
    let err = act(serde_json::json!({
        "actions": [{"op": "cancel", "number": "NB-3", "evidence": "  "}]
    }))
    .expect_err("evidence 空白应失败");
    assert!(
        err.message.contains("evidence"),
        "回灌文案应指明 evidence：{}",
        err.message
    );
}

#[test]
fn parse_fails_cancel_without_number() {
    let err = act(serde_json::json!({
        "actions": [{"op": "cancel", "evidence": "e"}]
    }))
    .expect_err("cancel 缺 number 应失败");
    assert!(err.message.contains("number"));
}

#[test]
fn parse_fails_modify_all_fields_empty() {
    let err = act(serde_json::json!({
        "actions": [{"op": "modify", "number": "NB-5", "evidence": "e"}]
    }))
    .expect_err("modify 全空应失败");
    assert!(err.message.contains("至少指明一个"));
}

#[test]
fn parse_fails_add_without_title() {
    let err = act(serde_json::json!({
        "actions": [{"op": "add", "evidence": "e"}]
    }))
    .expect_err("add 缺标题应失败");
    assert!(err.message.contains("标题"));
}

#[test]
fn parse_fails_invalid_profession_slug() {
    let err = act(serde_json::json!({
        "actions": [{"op": "add", "title": "t", "required_profession": "Dev", "evidence": "e"}]
    }))
    .expect_err("非法职能 slug 应失败");
    assert!(err.message.contains("required_profession"));
}

#[test]
fn parse_fails_depends_on_out_of_bounds() {
    let err = act(serde_json::json!({
        "actions": [
            {"op": "add", "title": "a", "depends_on": [1], "evidence": "e"},
            {"op": "cancel", "number": "NB-3", "evidence": "e"}
        ]
    }))
    .expect_err("depends_on 只计 add（1 越界）应失败");
    assert!(err.message.contains("越界"));
}

#[test]
fn parse_fails_add_self_reference() {
    let err = act(serde_json::json!({
        "actions": [{"op": "add", "title": "a", "depends_on": [0], "evidence": "e"}]
    }))
    .expect_err("add 自引用应失败");
    assert!(err.message.contains("自引用"));
}

#[test]
fn parse_fails_add_cycle() {
    let err = act(serde_json::json!({
        "actions": [
            {"op": "add", "title": "a", "depends_on": [1], "evidence": "e"},
            {"op": "add", "title": "b", "depends_on": [0], "evidence": "e"}
        ]
    }))
    .expect_err("add 批环依赖应失败");
    assert!(
        err.message.contains("depends_on"),
        "环错误应归入 depends_on 文案：{}",
        err.message
    );
}

#[test]
fn parse_fails_over_action_limit() {
    let actions: Vec<serde_json::Value> = (0..=MAX_REPLAN_ACTIONS)
        .map(|i| serde_json::json!({"op": "add", "title": format!("t{i}"), "evidence": "e"}))
        .collect();
    let err = act(serde_json::json!({"actions": actions})).expect_err("超上限应失败");
    assert!(err.message.contains("上限"));
}

#[test]
fn retry_prompt_carries_error_and_prev_output() {
    let err = parse_replan("不是 JSON").expect_err("应失败");
    let p = build_replan_retry_prompt("上一次输出原文", &err);
    assert!(p.contains("无法通过校验"));
    assert!(p.contains("上一次输出原文"));
    assert!(p.contains(&err.message));
}

#[test]
fn adds_as_planned_maps_fields_and_defaults_role() {
    let out = ReplanOutput {
        analysis: "a".into(),
        actions: vec![
            ReplanAction {
                op: Some(ReplanOp::Add),
                title: "  补救  ".into(),
                description: " 描述 ".into(),
                acceptance_criteria: "- [DONE] x".into(),
                required_profession: "dev".into(),
                depends_on: vec![],
                evidence: "e".into(),
                ..Default::default()
            },
            ReplanAction {
                op: Some(ReplanOp::Cancel),
                number: "NB-3".into(),
                evidence: "e".into(),
                ..Default::default()
            },
        ],
    };
    let adds = replan_adds_as_planned(&out);
    assert_eq!(adds.len(), 1, "只投影 add 动作");
    assert_eq!(adds[0].title, "补救", "title 应 trim");
    assert_eq!(adds[0].description, "描述");
    assert_eq!(adds[0].required_role, "worker", "缺省执行角色 = worker");
    assert_eq!(adds[0].required_profession, "dev");
}

#[test]
fn render_proposal_comment_shape() {
    let c = render_proposal_comment(
        2,
        "分析文",
        &[ReplanAction {
            op: Some(ReplanOp::Add),
            title: "新子单".into(),
            reason: "理由".into(),
            evidence: "证据文".into(),
            depends_on: vec![],
            ..Default::default()
        }],
        "unknown_block_timeout（触发单 NB-9）",
    );
    assert!(c.starts_with(PROPOSAL_COMMENT_PREFIX));
    assert!(c.contains("计划 v2"));
    assert!(c.contains("unknown_block_timeout（触发单 NB-9）"));
    assert!(c.contains("新增"));
    assert!(c.contains("新子单"));
    assert!(c.contains("证据文"));

    let empty = render_proposal_comment(1, "只有分析", &[], "t");
    assert!(
        empty.contains("无具体改动建议"),
        "空 actions 应渲染转人工指引"
    );
}

#[test]
fn system_prompt_is_nonempty() {
    assert!(!replan_system_prompt().is_empty());
}
