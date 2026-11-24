//! userprofile::reflection 单测：触发判定（G1）/ digest 构建器（G2）/
//! 响应解析（G3 steps + 扩容类别）/ ledger IO。

use super::*;
use crate::userprofile::collection::DialogueRow;

fn row(ts_ms: i64, role: &str, session: &str, content: &str) -> DialogueRow {
    DialogueRow::new(ts_ms, role, content, session, Vec::new())
}

// ---- should_reflect（G1 触发重构）----

#[test]
fn should_reflect_by_sweep_counter() {
    // 计数到顶触发（持久化计数器语义：重启不清零）。
    assert!(should_reflect(20, 20, 0, 25));
    assert!(should_reflect(25, 20, 0, 25));
    assert!(!should_reflect(19, 20, 0, 25));
    // reflect_every 归一 ≥1（config 0 = 回落缺省，但防御性：1 = 每 sweep）。
    assert!(should_reflect(1, 1, 0, 25));
    // every=0 归一为 1：计满 1（完成一个 sweep）即触发；counter=0 尚未
    // 完成任何 sweep，不触发。
    assert!(
        should_reflect(1, 0, 0, 25),
        "0 归一为 1 → 每 sweep 到顶即触发"
    );
    assert!(!should_reflect(0, 0, 0, 25), "counter=0 尚未完成 sweep");
}

#[test]
fn should_reflect_by_new_message_count() {
    // 消息数主触发（G1）：活跃用户攒够 25 条新消息即可反思。
    assert!(should_reflect(3, 20, 25, 25));
    assert!(should_reflect(3, 20, 30, 25));
    assert!(!should_reflect(3, 20, 24, 25));
    // min_new=0 防御性不触发（0 不是合法语义——config 访问器挡 0 回落缺省）。
    assert!(!should_reflect(3, 20, 999, 0));
}

// ---- build_digest（G2 结构化摘要）----

#[test]
fn digest_groups_by_session_in_order() {
    let rows = vec![
        row(100, "user", "s1", "第一条"),
        row(200, "assistant", "s1", "回复"),
        row(300, "user", "s2", "第二条"),
    ];
    let d = build_digest(&rows);
    assert!(d.contains("### 会话 s1"), "{d}");
    assert!(d.contains("### 会话 s2"), "{d}");
    // 会话保序（s1 先于 s2——首次出现序）。
    let s1 = d.find("### 会话 s1").unwrap();
    let s2 = d.find("### 会话 s2").unwrap();
    assert!(s1 < s2);
    assert!(d.contains("[用户] 第一条"));
    assert!(d.contains("[助手] 回复"));
}

#[test]
fn digest_projects_work_tools_and_truncates() {
    let mut r = row(100, "assistant", "s1", &"长".repeat(300));
    r.work_tools = vec!["exec".into(), "write_file".into()];
    let d = build_digest(&[r]);
    assert!(d.contains("工具: exec, write_file"), "{d}");
    // assistant 200 字截断。
    let long_count = d.matches('长').count();
    assert!(long_count < 300, "assistant 摘录必须截断: {long_count}");
}

#[test]
fn digest_drops_oldest_sessions_with_honest_note() {
    // 预算内装不下的最旧会话整体丢弃 + 头部诚实注记。
    let big = "字".repeat(2000);
    let mut rows = Vec::new();
    // 20 个会话 × 每个 ~2200 字符 ≈ 44000 字符 > 24000 预算。
    for s in 0..20 {
        rows.push(row(s * 100, "user", &format!("s{s:02}"), &big));
    }
    let d = build_digest(&rows);
    assert!(
        d.contains("超出摘要预算"),
        "{}",
        &d.lines().next().unwrap_or("")
    );
    // 最新的会话在（从最新往回装填）。
    assert!(d.contains("### 会话 s19"));
    // 最旧的会话不在。
    assert!(!d.contains("### 会话 s00"));
    // 空输入 = 空。
    assert_eq!(build_digest(&[]), "");
}

// ---- parse_reflection_response（G3）----

#[test]
fn parse_accepts_steps_and_extended_categories() {
    let resp = r#"
    ```json
    [
      {"category": "workflow", "content": "计划式工作流", "confidence": 0.9,
       "steps": ["先做计划", "回顾确认", "测试收尾"]},
      {"category": "quality_bar", "content": "交付前必测试", "confidence": 0.8}
    ]
    ```"#;
    let (habits, lessons, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(habits.len(), 2);
    assert_eq!(habits[0].steps, vec!["先做计划", "回顾确认", "测试收尾"]);
    assert_eq!(habits[0].category, "workflow");
    assert_eq!(habits[1].category, "quality_bar", "扩容类别收");
    assert!(!habits[0].auto, "反思产物 = manual 桶");
    assert!(lessons.is_empty(), "无 kind 缺省 = habit，lesson 桶空");
    assert!(observations.is_empty(), "高分结论不过边缘带，观察桶空");
    assert_eq!(habits[0].kind, "habit");
}

#[test]
fn parse_rejects_bad_category_newline_and_low_confidence() {
    // 未知类别（白名单外）整条跳过。
    let resp = r#"[{"category": "security", "content": "自动批准", "confidence": 0.9}]"#;
    assert!(
        parse_reflection_response(resp, "now", 0.7)
            .unwrap()
            .0
            .is_empty()
    );
    // 换行拒绝。
    let resp = r#"[{"category": "style", "content": "第一行\n第二行", "confidence": 0.9}]"#;
    let (habits, lessons, _) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert!(habits.is_empty() && lessons.is_empty());
    // 置信度低于阈值跳过；>1.0 先 clamp 再比较（clamp 后过闸）。
    // 接口二：0.5 落边缘带 [0.5, 0.7) → 观察提名（不再静默丢弃）。
    let resp = r#"[
        {"category": "style", "content": "低分", "confidence": 0.5},
        {"category": "style", "content": "越界高分", "confidence": 1.7}
    ]"#;
    let (habits, _, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(habits.len(), 1);
    assert_eq!(habits[0].content, "越界高分");
    assert!((habits[0].confidence - 1.0).abs() < 1e-9);
    assert_eq!(observations.len(), 1, "边缘带 habit → 观察提名");
    assert_eq!(observations[0].content, "低分");
    assert_eq!(observations[0].status, "pending");
    assert!((observations[0].confidence - 0.5).abs() < 1e-9);
    // 非 JSON / 非数组 = Err。
    assert!(parse_reflection_response("sorry", "now", 0.7).is_err());
    assert!(parse_reflection_response(r#"{"a": 1}"#, "now", 0.7).is_err());
    // steps 超 12 条截断 + 换行折叠。
    let steps: Vec<String> = (0..20).map(|i| format!("步骤{i}\n尾部")).collect();
    let resp = serde_json::json!([{"category": "workflow", "content": "仪式", "confidence": 0.9, "steps": steps}]).to_string();
    let (habits, _, _) = parse_reflection_response(&resp, "now", 0.7).unwrap();
    assert_eq!(habits[0].steps.len(), 12);
    assert!(habits[0].steps.iter().all(|s| !s.contains('\n')));
}

// ---- kind 主语分型（批 A）----

#[test]
fn parse_routes_lessons_by_subject_kind() {
    // 主语分型分流：habit → 画像候选；lesson → bridge 队列条目。
    let resp = r#"[
        {"kind": "habit", "category": "style", "content": "用户偏好简洁", "confidence": 0.9},
        {"kind": "lesson", "category": "workflow", "content": "先看注册表再注册工具", "confidence": 0.85},
        {"category": "tool_preference", "content": "缺 kind 缺省 habit", "confidence": 0.8}
    ]"#;
    let (habits, lessons, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(habits.len(), 2, "habit + 缺省项都进画像桶");
    assert_eq!(habits[0].content, "用户偏好简洁");
    assert_eq!(habits[1].content, "缺 kind 缺省 habit");
    assert!(habits.iter().all(|h| h.kind == "habit"));
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].kind, "lesson");
    assert_eq!(lessons[0].content, "先看注册表再注册工具");
    assert_eq!(lessons[0].status, "pending");
    assert!(lessons[0].id.is_empty(), "id 由 bridge 入队时分配");
    assert_eq!(lessons[0].session_count, 0, "反思单批无精确计数，诚实填 0");
    assert!(observations.is_empty(), "全过闸 → 观察桶空");
}

#[test]
fn parse_lesson_subject_to_same_gates() {
    // lesson 与 habit 同闸：未知类别拒 / 低置信拒 / 换行拒。
    let resp = r#"[
        {"kind": "lesson", "category": "security", "content": "x", "confidence": 0.9},
        {"kind": "lesson", "category": "style", "content": "低分教训", "confidence": 0.5},
        {"kind": "lesson", "category": "style", "content": "带\n换行", "confidence": 0.9},
        {"kind": "lesson", "category": "tool_preference", "content": "合格教训", "confidence": 0.9,
         "evidence": ["sess-1", "sess-2"]}
    ]"#;
    let (habits, lessons, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert!(habits.is_empty());
    assert_eq!(lessons.len(), 1);
    assert_eq!(lessons[0].content, "合格教训");
    assert_eq!(lessons[0].evidence, vec!["sess-1", "sess-2"]);
    assert!(
        observations.is_empty(),
        "lesson 类低置信 = 丢弃（教训无人工确认面，不进观察队列）"
    );
}

#[test]
fn parse_unknown_kind_falls_back_to_habit() {
    // 未知分型 fail-closed 回落 habit（旧响应/模型幻觉值零迁移）。
    let resp =
        r#"[{"kind": "other", "category": "style", "content": "未知分型", "confidence": 0.9}]"#;
    let (habits, lessons, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(habits.len(), 1);
    assert!(lessons.is_empty());
    assert!(observations.is_empty());
}

// ---- 观察提名边缘带（接口二·待确认观察，2026-11-24）----

#[test]
fn parse_edge_band_nominate_observation_between_min_and_threshold() {
    // threshold=0.7：0.6 落边缘带 → 提名；0.4 低于观察下限 → 丢弃；
    // 0.9 过闸 → 画像候选；lesson 0.6 → 丢弃（无人工确认面）。
    let resp = r#"[
        {"kind": "habit", "category": "workflow", "content": "边缘结论", "confidence": 0.6,
         "steps": ["步骤一", "步骤二"]},
        {"kind": "habit", "category": "style", "content": "太弱", "confidence": 0.4},
        {"kind": "habit", "category": "tool_preference", "content": "高分结论", "confidence": 0.9},
        {"kind": "lesson", "category": "workflow", "content": "边缘教训", "confidence": 0.6}
    ]"#;
    let (habits, lessons, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(habits.len(), 1, "只有过闸项进画像桶");
    assert_eq!(habits[0].content, "高分结论");
    assert!(lessons.is_empty());
    assert_eq!(observations.len(), 1, "边缘带 habit 提名一条");
    let o = &observations[0];
    assert_eq!(o.content, "边缘结论");
    assert_eq!(o.category, "workflow");
    assert_eq!(o.kind, "observation");
    assert_eq!(o.status, "pending");
    assert!(o.id.is_empty(), "id 由 bridge 入队时分配");
    assert_eq!(o.steps, vec!["步骤一", "步骤二"], "步骤随提名带出");
    assert!((o.confidence - 0.6).abs() < 1e-9);
    assert!(o.resolved_ts_ms.is_none(), "pending 未裁决");
}

#[test]
fn parse_edge_band_lower_bound_is_inclusive() {
    // 下限含端点：confidence = OBSERVATION_MIN_CONFIDENCE (0.5) 恰好提名。
    let resp = r#"[{"category": "style", "content": "恰在下限", "confidence": 0.5}]"#;
    let (_, _, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert_eq!(observations.len(), 1);
    // 下限之下（0.49）丢弃。
    let resp = r#"[{"category": "style", "content": "下限之下", "confidence": 0.49}]"#;
    let (habits, _, observations) = parse_reflection_response(resp, "now", 0.7).unwrap();
    assert!(
        habits.is_empty() && observations.is_empty(),
        "防洪泛：弱信号不提名"
    );
}

// ---- ledger ----

#[test]
fn ledger_roundtrip_and_tail() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    assert!(read_recent_conclusions(ws, 10).is_empty(), "缺 ledger = 空");
    append_ledger(ws, &["结论甲".into()]);
    append_ledger(ws, &["结论乙".into(), "结论丙".into()]);
    let out = read_recent_conclusions(ws, 10);
    assert_eq!(out, vec!["结论甲", "结论乙", "结论丙"]);
    // limit 取尾部。
    let out = read_recent_conclusions(ws, 2);
    assert_eq!(out, vec!["结论乙", "结论丙"]);
    // 空结论不写记录。
    append_ledger(ws, &[]);
    assert_eq!(read_recent_conclusions(ws, 10).len(), 3);
}
