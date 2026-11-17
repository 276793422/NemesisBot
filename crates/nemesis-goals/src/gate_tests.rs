//! gate 纯逻辑测试（状态机/预算/解析/注入消息/文件保护）。

use crate::gate::{
    budget_block_reason, build_eval_prompt, build_round_message, fence_denial, parse_verdict,
    should_eval_on_outbound, transition_allowed, ToolCallMaterial,
};
use crate::state::{EvalVerdict, GoalCreatedBy, GoalEntry, GoalStatus};

fn entry(status: GoalStatus, max_rounds: u64, rounds_used: u64) -> GoalEntry {
    let mut e = GoalEntry::new(
        "G1".to_string(),
        "agent:main:session:s1".to_string(),
        GoalCreatedBy::Human,
        Some(max_rounds),
        None,
        1_000,
    );
    e.status = status;
    e.rounds_used = rounds_used;
    e
}

#[test]
fn eval_trigger_filter() {
    // 带会话键的终局回复 = 触发
    assert!(should_eval_on_outbound(
        Some("agent:main:session:s1"),
        false,
        None
    ));
    // 回执卡 / 瞬态帧 / 无会话键 / 空会话键 = 不触发
    assert!(!should_eval_on_outbound(
        Some("agent:main:session:s1"),
        true,
        None
    ));
    assert!(!should_eval_on_outbound(
        Some("agent:main:session:s1"),
        false,
        Some(false)
    ));
    assert!(!should_eval_on_outbound(None, false, None));
    assert!(!should_eval_on_outbound(Some(""), false, None));
}

#[test]
fn transition_matrix() {
    use GoalStatus::*;
    // 合法：人面 enable/disable + 引擎停车 + 评估闸判定
    assert!(transition_allowed(Active, Paused));
    assert!(transition_allowed(Active, Blocked));
    assert!(transition_allowed(Active, Done));
    assert!(transition_allowed(Paused, Active));
    assert!(transition_allowed(Blocked, Active));
    // 非法：done 终态不转出；paused/blocked 不直达 done；同态；乱转
    assert!(!transition_allowed(Done, Active));
    assert!(!transition_allowed(Done, Paused));
    assert!(!transition_allowed(Paused, Done));
    assert!(!transition_allowed(Blocked, Done));
    assert!(!transition_allowed(Paused, Blocked));
    assert!(!transition_allowed(Active, Active));
    assert!(!transition_allowed(Done, Done));
}

#[test]
fn budget_exhaustion_and_unlimited() {
    // 未耗尽
    assert!(budget_block_reason(&entry(GoalStatus::Active, 256, 100)).is_none());
    // 耗尽（rounds_used >= max_rounds）
    let (code, msg) = budget_block_reason(&entry(GoalStatus::Active, 3, 3)).unwrap();
    assert_eq!(code, "round-limit");
    assert!(msg.contains("3/3"));
    // 超限（防御性）
    assert!(budget_block_reason(&entry(GoalStatus::Active, 3, 5)).is_some());
    // 0 = 不限
    assert!(budget_block_reason(&entry(GoalStatus::Active, 0, 9_999)).is_none());
}

#[test]
fn verdict_parse_matrix() {
    // 纯 JSON / 带围栏 / 前后杂讯 / 大小写
    assert_eq!(
        parse_verdict("{\"verdict\":\"DONE\"}").unwrap().0,
        EvalVerdict::Done
    );
    let r = parse_verdict("```json\n{\"verdict\":\"CONTINUE\",\"gap\":\"还差测试\"}\n```").unwrap();
    assert_eq!(r.0, EvalVerdict::Continue);
    assert_eq!(r.1.as_deref(), Some("还差测试"));
    let r = parse_verdict("评估结果如下：{\"verdict\": \"Done\"} 以上。").unwrap();
    assert_eq!(r.0, EvalVerdict::Done);
    assert_eq!(r.1, None);
    // gap 空串归一为 None
    let r = parse_verdict("{\"verdict\":\"CONTINUE\",\"gap\":\"  \"}").unwrap();
    assert_eq!(r.1, None);
    // 失败面
    assert!(parse_verdict("我觉得完成了").is_err());
    assert!(parse_verdict("{\"verdict\":\"MAYBE\"}").is_err());
    assert!(parse_verdict("{\"status\":\"done\"}").is_err());
    assert!(parse_verdict("{broken").is_err());
}

#[test]
fn round_message_shape() {
    let msg = build_round_message("G7", 3, 256, Some("缺回归测试"));
    assert!(msg.contains("<goal_round id=\"G7\" round=\"3\" max=\"256\">"));
    assert!(msg.contains("缺回归测试"));
    assert!(msg.contains("goals/G7.goal.md"));
    assert!(msg.contains("</goal_round>"));
    // gap 缺省兜底文案
    let msg = build_round_message("G1", 1, 256, None);
    assert!(msg.contains("未给出缺口注记"));
}

#[test]
fn eval_prompt_carries_materials() {
    let calls = vec![
        ToolCallMaterial {
            tool: "exec".to_string(),
            args_preview: r#"{"cmd":"cargo test"}"#.to_string(),
            ok: Some(true),
            result_preview: "test result: ok. 13 passed".to_string(),
        },
        ToolCallMaterial {
            tool: "edit_file".to_string(),
            args_preview: String::new(),
            ok: None,
            result_preview: String::new(),
        },
    ];
    let p = build_eval_prompt(
        "修复登录 bug\n验收:测试全绿",
        "请修复",
        "已修复并跑通测试",
        &calls,
        2,
        256,
    );
    assert!(p.contains("修复登录 bug"));
    assert!(p.contains("请修复"));
    assert!(p.contains("已修复并跑通测试"));
    // 工具调用记录：工具名 + 参数 + 实际返回逐条带出
    assert!(p.contains("本轮工具调用记录"));
    assert!(p.contains(
        "1. exec — 参数: {\"cmd\":\"cargo test\"} → 成功，结果: test result: ok. 13 passed"
    ));
    // 未观测到返回（ToolFinished 缺席）诚实标注，不伪装成成功
    assert!(p.contains("2. edit_file — 参数: - → 未观测到返回，结果: （空）"));
    assert!(p.contains("第 2 轮 / 上限 256"));
    // 无工具轮
    let p = build_eval_prompt("g", "u", "r", &[], 1, 0);
    assert!(p.contains("本轮未调用工具"));
    assert!(p.contains("0=不限"));
    // 失败调用标注
    let calls = vec![ToolCallMaterial {
        tool: "read_file".to_string(),
        args_preview: r#"{"path":"a.txt"}"#.to_string(),
        ok: Some(false),
        result_preview: "Tool error: not found".to_string(),
    }];
    let p = build_eval_prompt("g", "u", "r", &calls, 1, 256);
    assert!(p.contains("→ 失败，结果: Tool error: not found"));
}

#[test]
fn fence_protection_matrix() {
    let human = |_id: &str| Some(GoalCreatedBy::Human);
    let agent = |_id: &str| Some(GoalCreatedBy::Agent);
    let none = |_id: &str| None;

    // 人建 goal.md：写/删拒；agent 自建放行
    assert!(fence_denial("goals/G1.goal.md", &human).is_some());
    assert!(fence_denial("goals\\G1.goal.md", &human).is_some()); // 反斜杠归一
    assert!(fence_denial("goals/G1.goal.md", &agent).is_none());
    // 评估台账恒拒（不论创建者）
    assert!(fence_denial("goals/G1.evals.jsonl", &agent).is_some());
    assert!(fence_denial("goals/G1.evals.jsonl", &human).is_some());
    // 注册表恒拒
    assert!(fence_denial("config/goals.json", &agent).is_some());
    assert!(fence_denial("config\\goals.json", &human).is_some());
    // 未注册 id / 杂散文件 / 子目录 / 目录本身
    assert!(fence_denial("goals/G9.goal.md", &none).is_some());
    assert!(fence_denial("goals/notes.txt", &human).is_some());
    assert!(fence_denial("goals/sub/x.md", &agent).is_some());
    assert!(fence_denial("goals", &agent).is_some());
    // goals 之外的路径放行
    assert!(fence_denial("src/main.rs", &human).is_none());
    assert!(fence_denial("config/config.json", &agent).is_none());
}
