//! Work 时间线累加器单元测试（work_timeline.rs，2026-10-30；批次二
//! 2026-10-31 扩思考链；BUG-20261106-039 扩并发分桶）。
//!
//! 覆盖：分组规则（叙述/思考轮承接工具批次 / 纯工具轮 / 不误挂上一轮）、
//! 输出开轮语义（皆空跳过 / thinking-only 开轮）、call_id 幂等、无桶喂入
//! 忽略、finish 按键取走语义、并发桶隔离（spawn/评审 detached 与父 turn
//! 互不串台）、快照不扰动、MAX_WORK_ROUNDS 截断诚实注记、begin 重置、
//! reasoning 字段序列化（空省略 / 有值落键）。

use super::*;

const SK: &str = "agent:main:session:s1";

#[test]
fn test_grouping_text_round_carries_its_batch() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "我先看下配置", "");
    rec.tool_started(SK, "c1", "exec", "ls");
    rec.tool_finished(SK, "c1", true, 42, "ok");
    rec.push_round_output(SK, "再改文件", "");
    rec.tool_started(SK, "c2", "edit_file", "{}");
    rec.tool_finished(SK, "c2", false, 7, "err");

    let tl = rec.finish(SK).expect("timeline");
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
    rec.push_round_output(SK, "第一轮有叙述", "");
    rec.tool_started(SK, "c1", "exec", "");
    rec.tool_finished(SK, "c1", true, 1, "");
    // 第二轮空输出（皆空不建轮）→ 批次不得误挂上一轮。
    rec.tool_started(SK, "c2", "grep", "pat");
    rec.tool_finished(SK, "c2", true, 2, "hits");

    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 2);
    assert_eq!(tl.rounds[1].text, "");
    assert_eq!(tl.rounds[1].tools.len(), 1);
    assert_eq!(tl.rounds[1].tools[0].tool, "grep");
}

#[test]
fn test_thinking_only_output_opens_round() {
    // thinking-only 轮（叙述空、思考有值）：照发照落——UI 折叠展示面。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "", "先分析一下目录结构");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
    assert_eq!(tl.rounds[0].text, "");
    assert_eq!(tl.rounds[0].reasoning, "先分析一下目录结构");
    assert!(tl.rounds[0].tools.is_empty());
}

#[test]
fn test_both_empty_output_is_skipped() {
    // 皆空不建轮（直答无思考 = 不带 work 键的既有语义保持）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "", "");
    rec.push_round_output(SK, "   ", "");
    rec.push_round_output(SK, "", "   ");
    let tl = rec.finish(SK);
    assert!(tl.is_none(), "皆空输出不得产生轮");
}

#[test]
fn test_thinking_round_carries_tool_batch() {
    // ensure_tool_round 扩展条件：末轮有思考且尚无工具 → 批次挂思考轮
    // （否则思考与工具劈成两轮）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "", "先想清楚改哪里");
    rec.tool_started(SK, "c1", "edit_file", "{}");
    rec.tool_finished(SK, "c1", true, 5, "ok");

    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 1, "思考轮必须承接自己的工具批次");
    assert_eq!(tl.rounds[0].reasoning, "先想清楚改哪里");
    assert_eq!(tl.rounds[0].tools.len(), 1);
}

#[test]
fn test_thinking_then_tools_then_thinking_two_rounds() {
    // 思考轮带完批次后，下一条思考开新轮（末轮已有工具不再承接）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "", "第一段思考");
    rec.tool_started(SK, "c1", "exec", "");
    rec.tool_finished(SK, "c1", true, 1, "");
    rec.push_round_output(SK, "", "第二段思考");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 2);
    assert_eq!(tl.rounds[0].tools.len(), 1);
    assert!(tl.rounds[1].tools.is_empty());
    assert_eq!(tl.rounds[1].reasoning, "第二段思考");
}

#[test]
fn test_first_event_is_tool_starts_tools_only_round() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.tool_started(SK, "c1", "web_search", "q");
    rec.tool_finished(SK, "c1", true, 9, "r");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
    assert!(tl.rounds[0].text.is_empty());
    assert_eq!(tl.rounds[0].tools.len(), 1);
}

#[test]
fn test_duplicate_call_id_is_idempotent() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "t", "");
    rec.tool_started(SK, "c1", "exec", "a=1");
    rec.tool_started(SK, "c1", "exec", "a=2");
    rec.tool_finished(SK, "c1", true, 5, "r");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds[0].tools.len(), 1);
    assert_eq!(tl.rounds[0].tools[0].args_preview, "a=2");
}

#[test]
fn test_tool_finished_unknown_call_id_ignored() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "t", "");
    rec.tool_finished(SK, "ghost", true, 5, "r");
    let tl = rec.finish(SK).expect("timeline");
    assert!(tl.rounds[0].tools.is_empty());
}

#[test]
fn test_session_mismatch_feeds_ignored() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output("agent:main:session:other", "别串台", "");
    rec.tool_started("agent:main:session:other", "c1", "exec", "");
    let tl = rec.finish(SK);
    assert!(tl.is_none(), "不匹配喂入不得产生轮");
}

#[test]
fn test_finish_takes_slot_and_direct_answer_is_none() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    // 直答：无任何中间事件。
    assert!(rec.finish(SK).is_none(), "直答无中间轮 = 不带 work");
    assert!(rec.finish(SK).is_none(), "槽已清空，二次 finish 仍 None");

    rec.begin(SK);
    rec.push_round_output(SK, "t", "");
    assert!(rec.finish(SK).is_some());
    assert!(rec.finish(SK).is_none(), "finish 取走后槽空");
}

#[test]
fn test_snapshot_does_not_disturb_and_session_gated() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    assert!(rec.snapshot_json(SK).is_none(), "尚无任何轮 = None");
    rec.push_round_output(SK, "进行中", "");
    let v1 = rec.snapshot_json(SK).expect("snapshot");
    let v2 = rec.snapshot_json(SK).expect("snapshot 可重复");
    // 快照不取槽：rounds 与 started_at_ms 稳定（ended_at_ms 每次取当前
    // 时钟——live 用时随时间增长是语义，不比较全量）。
    assert_eq!(v1["rounds"], v2["rounds"]);
    assert_eq!(v1["started_at_ms"], v2["started_at_ms"]);
    assert!(rec.snapshot_json("agent:main:session:zz").is_none());
    // 快照后 finish 仍能取走全量。
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
}

#[test]
fn test_snapshot_shape_roundtrips_expected_keys() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "叙述", "");
    rec.tool_started(SK, "c1", "exec", "ls -la");
    let v = rec.snapshot_json(SK).expect("snapshot");
    let rounds = v["rounds"].as_array().expect("rounds array");
    assert_eq!(rounds.len(), 1);
    assert_eq!(rounds[0]["text"], "叙述");
    assert!(
        rounds[0].get("reasoning").is_none(),
        "空思考省略 reasoning 键（skip_serializing_if）"
    );
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
fn test_snapshot_reasoning_key_present_when_nonempty() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "", "有思考");
    let v = rec.snapshot_json(SK).expect("snapshot");
    assert_eq!(v["rounds"][0]["reasoning"], "有思考");
    assert_eq!(v["rounds"][0]["text"], "");
}

#[test]
fn test_max_rounds_truncation_honest_note() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    for i in 0..(MAX_WORK_ROUNDS + 10) {
        rec.push_round_output(SK, &format!("r{i}"), "");
    }
    let tl = rec.finish(SK).expect("timeline");
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
    rec.push_round_output(SK, "上一轮残余", "");
    rec.begin(SK); // 新 turn：重置
    let tl = rec.finish(SK);
    assert!(tl.is_none(), "begin 后旧残余不得残留");
}

#[test]
fn test_truncated_slot_refuses_tool_writes() {
    let rec = WorkRecorder::default();
    rec.begin(SK);
    for i in 0..MAX_WORK_ROUNDS {
        rec.push_round_output(SK, &format!("r{i}"), "");
    }
    // 恰好满载：末轮（r{MAX-1}）尚无批次 → 工具合法挂末轮（分组规则），
    // 不开新轮、不截断。
    rec.tool_started(SK, "fill", "exec", "");
    rec.tool_finished(SK, "fill", true, 1, "");
    // 末轮已带批次后再来工具 → 纯工具轮路径 → 超限拒绝 + truncated。
    rec.tool_started(SK, "overflow", "exec", "");
    let tl = rec.finish(SK).expect("timeline");
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

#[test]
fn test_parallel_recordings_isolated() {
    // BUG-20261106-039 核心回归锁：父 turn 与两个 detached 子代理在同一
    // recorder 上交错 begin/喂入（spawn 前台阻塞 / 后台 G4 的真实时序）
    // ——桶间互不串台，各键 finish 拿到自己的全量。单槽形态下：begin(S1)
    // 互踩父累积、finish(P) 无键 take 错取别人的桶（红态：finish(P) 拿到
    // S2 的时间线或 None）。
    const P: &str = "agent:main:session:parent";
    const S1: &str = "subagent:uuid-1";
    const S2: &str = "subagent:uuid-2";
    let rec = WorkRecorder::default();

    rec.begin(P);
    rec.push_round_output(P, "父叙述", "");
    rec.tool_started(P, "p1", "exec", "");
    rec.tool_finished(P, "p1", true, 1, "");

    // 前台 spawn：子代理 begin 互踩父（单槽形态）——分桶后父桶原样保留。
    rec.begin(S1);
    rec.push_round_output(S1, "子一叙述", "");
    rec.tool_started(S1, "s1a", "read_file", "");
    rec.tool_finished(S1, "s1a", true, 2, "");

    // 后台 spawn：第二个子代理并发。
    rec.begin(S2);
    rec.push_round_output(S2, "子二叙述", "");

    // 父先收尾（后台子仍在跑）——拿到的必须是父自己的全量，不是子代理的。
    let tp = rec.finish(P).expect("parent timeline");
    assert_eq!(tp.rounds.len(), 1);
    assert_eq!(tp.rounds[0].text, "父叙述");
    assert_eq!(tp.rounds[0].tools[0].call_id, "p1");

    let t1 = rec.finish(S1).expect("sub1 timeline");
    assert_eq!(t1.rounds.len(), 1);
    assert_eq!(t1.rounds[0].text, "子一叙述");
    assert_eq!(t1.rounds[0].tools[0].call_id, "s1a");

    let t2 = rec.finish(S2).expect("sub2 timeline");
    assert_eq!(t2.rounds.len(), 1);
    assert_eq!(t2.rounds[0].text, "子二叙述");
    assert!(t2.rounds[0].tools.is_empty());

    // 三桶全部取走，无残余。
    assert!(rec.finish(P).is_none());
    assert!(rec.finish(S1).is_none());
    assert!(rec.finish(S2).is_none());
}

#[test]
fn test_finish_takes_only_own_key() {
    // BUG-20261106-039 次级回归锁：finish 按键路由——别人的键取不走本键
    // 的桶（单槽无键 take 的错误归属形态）。detached 不再 finish 时父桶
    // 也不会被误取。
    const P: &str = "agent:main:session:parent";
    const S: &str = "subagent:uuid-x";
    let rec = WorkRecorder::default();
    rec.begin(P);
    rec.push_round_output(P, "父内容", "");

    assert!(rec.finish(S).is_none(), "无桶键 finish = None");
    // 关键：错键 finish 之后本键桶必须原样在。
    let tl = rec.finish(P).expect("parent timeline intact");
    assert_eq!(tl.rounds.len(), 1);
    assert_eq!(tl.rounds[0].text, "父内容");
}

// --- 插话注入注记轮（2026-11-21 用户反馈改版：注记进时间线过程流）---

#[test]
fn test_steer_note_round_not_reused_by_tools_or_output() {
    // 注记轮（三者皆空）不得被后续工具批次复用（末轮分组规则要求叙述或
    // 思考非空），也不得被后续输出并入（push_round_output 恒开新轮）——
    // 注记独立成轮，时序锚 = 注入时刻。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "第一轮叙述", "");
    rec.tool_started(SK, "c1", "exec", "");
    rec.tool_finished(SK, "c1", true, 1, "");
    rec.record_steer_note(SK, "⚡ 插话已注入本轮思考：补充边界");
    // 注记后的工具批次：开纯工具轮（不误挂注记轮）。
    rec.tool_started(SK, "c2", "grep", "pat");
    rec.tool_finished(SK, "c2", true, 2, "hits");
    // 注记后的输出：开新轮。
    rec.push_round_output(SK, "继续处理", "");

    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 4, "叙述轮/注记轮/纯工具轮/输出轮");
    assert_eq!(
        tl.rounds[1].steer_note.as_deref(),
        Some("⚡ 插话已注入本轮思考：补充边界")
    );
    assert!(tl.rounds[1].text.is_empty());
    assert!(tl.rounds[1].reasoning.is_empty());
    assert!(tl.rounds[1].tools.is_empty(), "注记轮无工具");
    assert_eq!(tl.rounds[2].tools.len(), 1, "注记后批次开纯工具轮");
    assert_eq!(tl.rounds[2].steer_note, None);
    assert_eq!(tl.rounds[3].text, "继续处理");
    assert!(tl.rounds[0].steer_note.is_none());
}

#[test]
fn test_steer_note_only_round_persists_via_finish_and_snapshot() {
    // claim 后直答（无中间轮）：仅注记轮 → finish 返回单轮时间线（行带
    // work 键，前端折叠块可见插话落点）；快照同样可见（执行中切回恢复面）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.record_steer_note(SK, "⚡ 插话已注入本轮思考：换思路");
    let v = rec.snapshot_json(SK).expect("snapshot");
    assert_eq!(
        v["rounds"][0]["steer_note"],
        "⚡ 插话已注入本轮思考：换思路"
    );
    let tl = rec.finish(SK).expect("note-only turn carries work key");
    assert_eq!(tl.rounds.len(), 1);
    assert_eq!(
        tl.rounds[0].steer_note.as_deref(),
        Some("⚡ 插话已注入本轮思考：换思路")
    );
    assert!(tl.rounds[0].text.is_empty());
    assert!(tl.rounds[0].tools.is_empty());
    // 落盘序列化：正常轮省略 steer_note 键（skip_serializing_if）。
    let v = serde_json::to_value(&tl).unwrap();
    assert!(v["rounds"][0].get("steer_note").is_some());
    let plain = WorkRound {
        text: "t".to_string(),
        reasoning: String::new(),
        tools: Vec::new(),
        steer_note: None,
        steer_response: None,
    };
    let v = serde_json::to_value(&plain).unwrap();
    assert!(v.get("steer_note").is_none(), "无注记轮省略键");
}

#[test]
fn test_steer_note_session_gated_and_truncation_respected() {
    // 无桶（未 begin）忽略；桶满置 truncated 诚实注记。
    let rec = WorkRecorder::default();
    rec.record_steer_note(SK, "无桶忽略");
    assert!(rec.finish(SK).is_none());

    rec.begin(SK);
    for i in 0..MAX_WORK_ROUNDS {
        rec.push_round_output(SK, &format!("r{i}"), "");
    }
    rec.record_steer_note(SK, "超限拒绝");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), MAX_WORK_ROUNDS);
    assert!(tl.truncated);
    assert!(tl.rounds.iter().all(|r| r.steer_note.is_none()));
}

// --- 插话专轮回应（2026-11-21 插话专轮批：回应记在注记轮上）---

#[test]
fn test_steer_response_lands_on_note_round_reverse_find() {
    // 收口回应反查最近的未回注记轮：注记轮后有中间输出/批次也不偏移
    // （反查按 steer_note 在场 + 未回，不按位置）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.push_round_output(SK, "工具前叙述", "");
    rec.record_steer_note(SK, "⚡ 插话已注入本轮思考：换思路");
    // 专轮内的工具批次与叙述照常累加（反查必须跳过它们）。
    rec.tool_started(SK, "c1", "exec", "");
    rec.tool_finished(SK, "c1", true, 3, "ok");
    rec.push_round_output(SK, "专轮内叙述", "");
    rec.record_steer_response(SK, "插话回应：好的，换思路");

    let tl = rec.finish(SK).expect("timeline");
    let note_idx = tl
        .rounds
        .iter()
        .position(|r| r.steer_note.is_some())
        .expect("note round");
    assert_eq!(
        tl.rounds[note_idx].steer_response.as_deref(),
        Some("插话回应：好的，换思路"),
        "回应记在注记轮上（反查跳过中间轮）"
    );
    assert!(
        tl.rounds
            .iter()
            .enumerate()
            .all(|(i, r)| i == note_idx || r.steer_response.is_none()),
        "其余轮不带回应段"
    );
}

#[test]
fn test_steer_response_second_call_targets_next_note_round() {
    // 两段专轮：第二次 record_steer_response 反查到第二个注记轮（第一个
    // 已有回应不再命中）——排空场景下两段回应各归各轮。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.record_steer_note(SK, "插话一");
    rec.record_steer_response(SK, "回应一");
    rec.record_steer_note(SK, "插话二");
    rec.record_steer_response(SK, "回应二");

    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 2);
    assert_eq!(tl.rounds[0].steer_note.as_deref(), Some("插话一"));
    assert_eq!(tl.rounds[0].steer_response.as_deref(), Some("回应一"));
    assert_eq!(tl.rounds[1].steer_note.as_deref(), Some("插话二"));
    assert_eq!(tl.rounds[1].steer_response.as_deref(), Some("回应二"));
}

#[test]
fn test_steer_response_fallback_appends_when_no_note_round() {
    // 无注记轮可回（注记轮被 MAX_WORK_ROUNDS 截断的极端场景）→ 追加独立
    // 回应轮兜底；超限则诚实拒绝（truncated，不追加）。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    rec.record_steer_response(SK, "无注记轮兜底");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), 1);
    assert!(tl.rounds[0].steer_note.is_none());
    assert_eq!(tl.rounds[0].steer_response.as_deref(), Some("无注记轮兜底"));

    // 超限兜底：满桶 + 无注记轮 → truncated、不追加。
    let rec = WorkRecorder::default();
    rec.begin(SK);
    for i in 0..MAX_WORK_ROUNDS {
        rec.push_round_output(SK, &format!("r{i}"), "");
    }
    rec.record_steer_response(SK, "超限拒绝");
    let tl = rec.finish(SK).expect("timeline");
    assert_eq!(tl.rounds.len(), MAX_WORK_ROUNDS);
    assert!(tl.truncated);
    assert!(tl.rounds.iter().all(|r| r.steer_response.is_none()));
}
