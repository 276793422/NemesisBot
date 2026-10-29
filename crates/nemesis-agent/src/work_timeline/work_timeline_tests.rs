//! Work 时间线累加器单元测试（work_timeline.rs，2026-10-30）。
//!
//! 覆盖：分组规则（叙述轮承接工具批次 / 纯工具轮 / 不误挂上一轮）、
//! call_id 幂等、会话不匹配忽略、finish 取走语义、快照不扰动、
//! MAX_WORK_ROUNDS 截断诚实注记、begin 重置。

use super::*;

const SK: &str = "agent:main:session:s1";

#[test]
fn test_grouping_text_round_carries_its_batch() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "我先看下配置");
    rec.tool_started(SK, "c1", "exec", "ls");
    rec.tool_finished(SK, "c1", true, 42, "ok");
    rec.push_round_text(SK, "再改文件");
    rec.tool_started(SK, "c2", "edit_file", "{}");
    rec.tool_finished(SK, "c2", false, 7, "err");

    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), 2);
    assert_eq!(tl.rounds[0].text, "我先看下配置");
    assert_eq!(tl.rounds[0].tools.len(), 1);
    assert!(tl.rounds[0].tools[0].ok);
    assert_eq!(tl.rounds[1].text, "再改文件");
    assert_eq!(tl.rounds[1].tools.len(), 1);
    assert!(!tl.rounds[1].tools[0].ok);
}

#[test]
fn test_grouping_empty_text_round_becomes_tools_only_round() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "第一轮有叙述");
    rec.tool_started(SK, "c1", "exec", "");
    rec.tool_finished(SK, "c1", true, 1, "");
    // 第二轮空正文（不发 RoundText）→ 批次不得误挂上一轮。
    rec.tool_started(SK, "c2", "grep", "pat");
    rec.tool_finished(SK, "c2", true, 2, "hits");

    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), 2);
    assert_eq!(tl.rounds[1].text, "");
    assert_eq!(tl.rounds[1].tools.len(), 1);
    assert_eq!(tl.rounds[1].tools[0].tool, "grep");
}

#[test]
fn test_first_event_is_tool_starts_tools_only_round() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.tool_started(SK, "c1", "web_search", "q");
    rec.tool_finished(SK, "c1", true, 9, "r");
    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
    assert!(tl.rounds[0].text.is_empty());
    assert_eq!(tl.rounds[0].tools.len(), 1);
}

#[test]
fn test_duplicate_call_id_is_idempotent() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "t");
    rec.tool_started(SK, "c1", "exec", "a=1");
    rec.tool_started(SK, "c1", "exec", "a=2");
    rec.tool_finished(SK, "c1", true, 5, "r");
    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds[0].tools.len(), 1);
    assert_eq!(tl.rounds[0].tools[0].args_preview, "a=2");
}

#[test]
fn test_tool_finished_unknown_call_id_ignored() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "t");
    rec.tool_finished(SK, "ghost", true, 5, "r");
    let tl = rec.finish().expect("timeline");
    assert!(tl.rounds[0].tools.is_empty());
}

#[test]
fn test_session_mismatch_feeds_ignored() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text("agent:main:session:other", "别串台");
    rec.tool_started("agent:main:session:other", "c1", "exec", "");
    let tl = rec.finish();
    assert!(tl.is_none(), "不匹配喂入不得产生轮");
}

#[test]
fn test_finish_takes_slot_and_direct_answer_is_none() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    // 直答：无任何中间事件。
    assert!(rec.finish().is_none(), "直答无中间轮 = 不带 work");
    assert!(rec.finish().is_none(), "槽已清空，二次 finish 仍 None");

    rec.begin(SK);
    rec.push_round_text(SK, "t");
    assert!(rec.finish().is_some());
    assert!(rec.finish().is_none(), "finish 取走后槽空");
}

#[test]
fn test_snapshot_does_not_disturb_and_session_gated() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    assert!(rec.snapshot_json(SK).is_none(), "尚无任何轮 = None");
    rec.push_round_text(SK, "进行中");
    let v1 = rec.snapshot_json(SK).expect("snapshot");
    let v2 = rec.snapshot_json(SK).expect("snapshot 可重复");
    // 快照不取槽：rounds 与 started_at_ms 稳定（ended_at_ms 每次取当前
    // 时钟——live 用时随时间增长是语义，不比较全量）。
    assert_eq!(v1["rounds"], v2["rounds"]);
    assert_eq!(v1["started_at_ms"], v2["started_at_ms"]);
    assert!(rec.snapshot_json("agent:main:session:zz").is_none());
    // 快照后 finish 仍能取走全量。
    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
}

#[test]
fn test_snapshot_shape_roundtrips_expected_keys() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "叙述");
    rec.tool_started(SK, "c1", "exec", "ls -la");
    let v = rec.snapshot_json(SK).expect("snapshot");
    let rounds = v["rounds"].as_array().expect("rounds array");
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0]["text"], "叙述");
    let tools = rounds[0]["tools"].as_array().expect("tools array");
    assert_eq!(tools[0]["running"], true, "执行中工具带 running 旗标");
    assert!(v.get("truncated").is_none(), "未截断不落 truncated 键");
    assert!(v["started_at_ms"].as_u64().is_some());
    rec.tool_finished(SK, "c1", true, 3, "out");
    let v = rec.snapshot_json(SK).expect("snapshot");
    let tools = v["rounds"][0]["tools"].as_array().unwrap();
    assert!(tools[0].get("running").is_none(), "完成工具省略 running");
    assert_eq!(tools[0]["ok"], true);
    assert_eq!(tools[0]["duration_ms"], 3);
}

#[test]
fn test_max_rounds_truncation_honest_note() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    for i in 0..(MAX_WORK_ROUNDS + 10) {
        rec.push_round_text(SK, &format!("r{i}"));
    }
    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), MAX_WORK_ROUNDS);
    assert!(tl.truncated, "截断必须带诚实注记");
    // 截断形态序列化带 truncated 键。
    let v = serde_json::to_value(&tl).unwrap();
    assert_eq!(v["truncated"], true);
}

#[test]
fn test_begin_resets_previous_turn_residual() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_text(SK, "上一轮残余");
    rec.begin(SK); // 新 turn：重置
    let tl = rec.finish();
    assert!(tl.is_none(), "begin 后旧残余不得残留");
}

#[test]
fn test_truncated_slot_refuses_tool_writes() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    for i in 0..MAX_WORK_ROUNDS {
        rec.push_round_text(SK, &format!("r{i}"));
    }
    // 恰好满载：末轮（r{MAX-1}）尚无批次 → 工具合法挂末轮（分组规则），
    // 不开新轮、不截断。
    rec.tool_started(SK, "fill", "exec", "");
    rec.tool_finished(SK, "fill", true, 1, "");
    // 末轮已带批次后再来工具 → 纯工具轮路径 → 超限拒绝 + truncated。
    rec.tool_started(SK, "overflow", "exec", "");
    let tl = rec.finish().expect("timeline");
    assert_eq!(tl.rounds.len(), MAX_WORK_ROUNDS);
    assert!(tl.truncated);
    assert_eq!(
        tl.rounds[MAX_WORK_ROUNDS - 1].tools[0].call_id,
        "fill",
        "满载末轮承接自己的批次"
    );
    assert!(
        tl.rounds
            .iter()
            .all(|r| r.tools.iter().all(|t| t.call_id != "overflow"))
    );
}
