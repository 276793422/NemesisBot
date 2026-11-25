//! 纠正候选扫描 + 台账 + 游标 + 证据包行为测试。

use super::*;

// ── 夹具构建（chat_log jsonl 行形态锚点：chat_log.rs write_chat_entry）──

fn user_row(content: &str) -> String {
    serde_json::json!({
        "role": "user",
        "content": content,
        "timestamp": "2026-11-24T10:00:00+08:00",
    })
    .to_string()
}

/// 注入形态 user 行（system_notice 标记——不是用户说的话）。
fn system_notice_row(content: &str) -> String {
    serde_json::json!({
        "role": "user",
        "content": content,
        "timestamp": "2026-11-24T09:59:00+08:00",
        "system_notice": {"kind": "test"},
    })
    .to_string()
}

fn question_tool_record(args: &str, result: &str) -> serde_json::Value {
    serde_json::json!({
        "tool": "question",
        "call_id": "call_1",
        "ok": true,
        "duration_ms": 5000,
        "args_preview": args,
        "result_preview": result,
    })
}

fn other_tool_record(tool: &str) -> serde_json::Value {
    serde_json::json!({
        "tool": tool,
        "call_id": "call_x",
        "ok": true,
        "duration_ms": 100,
        "args_preview": "{}",
        "result_preview": "done",
    })
}

fn assistant_row(content: &str, tools: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "role": "assistant",
        "content": content,
        "timestamp": "2026-11-24T10:05:00+08:00",
        "work": {
            "started_at_ms": 1,
            "ended_at_ms": 2,
            "rounds": [{"text": "", "tools": tools}],
        },
    })
    .to_string()
}

fn plain_assistant_row(content: &str) -> String {
    serde_json::json!({
        "role": "assistant",
        "content": content,
        "timestamp": "2026-11-24T10:05:00+08:00",
    })
    .to_string()
}

fn log_bytes(rows: &[String]) -> Vec<u8> {
    rows.join("\n").into_bytes()
}

/// 标准「问→答→收敛」夹具 args/result 对。
const ASK_ARGS: &str = r#"{"question":"需要授权才能继续吗？","options":["需要","不需要"]}"#;
const ANSWERED: &str = "User selected: 不需要";

fn store_in(dir: &std::path::Path) -> CandidateStore {
    CandidateStore::from_forge_dir(dir)
}

// ── 扫描器 ────────────────────────────────────────────────────────────

#[test]
fn test_scan_full_shape_extracts_candidate() {
    let rows = [
        user_row("帮我跑测试并修复失败"),
        assistant_row(
            "已修复全部失败，测试通过。",
            vec![question_tool_record(ASK_ARGS, ANSWERED)],
        ),
    ];
    let drafts = scan_chat_log("web_12", None, &log_bytes(&rows));
    assert_eq!(drafts.len(), 1);
    let d = &drafts[0];
    assert_eq!(d.session_key, "web_12");
    assert_eq!(d.ts, "2026-11-24T10:05:00+08:00");
    assert_eq!(d.ask, "需要授权才能继续吗？", "args JSON 取 question 字段");
    assert_eq!(d.reply, ANSWERED);
    assert_eq!(d.goal, "帮我跑测试并修复失败");
    assert_eq!(d.result, "已修复全部失败，测试通过。");
    assert!(!d.fp.is_empty());
}

#[test]
fn test_scan_timeout_is_not_candidate() {
    for no_answer in [
        "No answer: user did not answer, proceed with your best judgment.",
        "No answer: emergency stop was triggered while waiting; do not retry this question, stop and report.",
    ] {
        let rows = [
            user_row("任务"),
            assistant_row("完成", vec![question_tool_record(ASK_ARGS, no_answer)]),
        ];
        assert!(
            scan_chat_log("web_12", None, &log_bytes(&rows)).is_empty(),
            "未作答（超时/estop）不是候选：{no_answer}"
        );
    }
}

#[test]
fn test_scan_ignores_non_question_and_plain_rows() {
    let rows = [
        user_row("任务"),
        assistant_row("带 exec", vec![other_tool_record("exec")]),
        plain_assistant_row("直答行（无 work）"),
    ];
    assert!(scan_chat_log("s", None, &log_bytes(&rows)).is_empty());
}

#[test]
fn test_scan_goal_skips_injected_user_rows() {
    let rows = [
        system_notice_row("（系统注入：时间同步）"),
        user_row("真正的任务目标"),
        assistant_row("done", vec![question_tool_record(ASK_ARGS, ANSWERED)]),
    ];
    let d = &scan_chat_log("s", None, &log_bytes(&rows))[0];
    assert_eq!(d.goal, "真正的任务目标", "goal 不吃注入行");
}

#[test]
fn test_scan_malformed_args_falls_back_to_raw_preview() {
    let rows = [
        user_row("任务"),
        assistant_row(
            "done",
            vec![question_tool_record("这是被截断的半截 JSON {", ANSWERED)],
        ),
    ];
    let d = &scan_chat_log("s", None, &log_bytes(&rows))[0];
    assert_eq!(d.ask, "这是被截断的半截 JSON {", "解析失败回退原始 preview");
}

#[test]
fn test_scan_summaries_capped() {
    let long_question = format!(r#"{{"question":"{}"}}"#, "长".repeat(500));
    let long_goal = "目".repeat(500);
    let long_result = "尾".repeat(500);
    let rows = [
        user_row(&long_goal),
        assistant_row(
            &long_result,
            vec![question_tool_record(&long_question, ANSWERED)],
        ),
    ];
    let d = &scan_chat_log("s", None, &log_bytes(&rows))[0];
    assert_eq!(d.ask.chars().count(), ASK_MAX_CHARS);
    assert_eq!(d.goal.chars().count(), GOAL_MAX_CHARS);
    assert_eq!(d.result.chars().count(), RESULT_MAX_CHARS);
    // reply 截断：超长答复取头
    let long_reply = format!("User selected: {}", "选".repeat(500));
    let rows2 = [
        user_row("t"),
        assistant_row("r", vec![question_tool_record(ASK_ARGS, &long_reply)]),
    ];
    let d2 = &scan_chat_log("s", None, &log_bytes(&rows2))[0];
    assert_eq!(d2.reply.chars().count(), REPLY_MAX_CHARS);
}

#[test]
fn test_scan_result_takes_tail_not_head() {
    let content = format!("{}结尾结论", "过".repeat(400));
    let rows = [
        user_row("t"),
        assistant_row(&content, vec![question_tool_record(ASK_ARGS, ANSWERED)]),
    ];
    let d = &scan_chat_log("s", None, &log_bytes(&rows))[0];
    assert!(
        d.result.ends_with("结尾结论"),
        "收敛结果取尾段：{}",
        d.result
    );
}

#[test]
fn test_scan_multiple_questions_each_a_candidate() {
    let args2 = r#"{"question":"第二个问题？"}"#;
    let rows = [
        user_row("t"),
        assistant_row(
            "done",
            vec![
                question_tool_record(ASK_ARGS, ANSWERED),
                question_tool_record(args2, "User selected: 选项甲"),
            ],
        ),
    ];
    let drafts = scan_chat_log("s", None, &log_bytes(&rows));
    assert_eq!(drafts.len(), 2, "同 turn 两次问询各成一个候选");
    assert_ne!(drafts[0].fp, drafts[1].fp);
}

#[test]
fn test_scan_workspace_logs_sorted_and_filtered() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    // 写两个会话（乱序创建时间——目录序不可依赖，扫描必须显式排序稳定）
    let rows_a = [
        user_row("A 会话目标"),
        assistant_row("A done", vec![question_tool_record(ASK_ARGS, ANSWERED)]),
    ];
    std::fs::write(logs.join("web_1.jsonl"), log_bytes(&rows_a)).unwrap();
    let rows_b = [
        user_row("B 会话目标"),
        assistant_row("B done", vec![question_tool_record(ASK_ARGS, ANSWERED)]),
    ];
    std::fs::write(logs.join("web_2.jsonl"), log_bytes(&rows_b)).unwrap();
    // 干扰文件：meta sidecar + boundary 无后缀 + 子目录
    std::fs::write(logs.join("web_1.meta.json"), "{}").unwrap();
    std::fs::write(logs.join("boundary_20261124"), "").unwrap();
    std::fs::create_dir_all(logs.join("nested_dir")).unwrap();
    let drafts = scan_workspace_logs(&logs);
    assert_eq!(drafts.len(), 2, "meta/boundary/子目录全过滤");
    // 显式排序：web_1 在前
    assert_eq!(drafts[0].session_key, "web_1");
    assert_eq!(drafts[0].goal, "A 会话目标");
    assert_eq!(drafts[1].session_key, "web_2");
    // 目录缺席 = 空（合法初态）
    assert!(scan_workspace_logs(&dir.path().join("missing")).is_empty());
}

#[test]
fn test_scan_stamps_scope_from_sidecar_project_id() {
    let dir = tempfile::tempdir().unwrap();
    let logs = dir.path().join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    let mk_rows = |goal: &str| -> Vec<u8> {
        log_bytes(&[
            user_row(goal),
            assistant_row("done", vec![question_tool_record(ASK_ARGS, ANSWERED)]),
        ])
    };
    // ① 有 sidecar 带 project_id → 项目戳
    std::fs::write(logs.join("proj_a.jsonl"), mk_rows("A 目标")).unwrap();
    std::fs::write(
        logs.join("proj_a.meta.json"),
        r#"{"project_id":"prj-42","project_path":"D:/work/a"}"#,
    )
    .unwrap();
    // ② 无 sidecar → 对话（None）
    std::fs::write(logs.join("chat_b.jsonl"), mk_rows("B 目标")).unwrap();
    // ③ sidecar 损坏 → None（诚实缺省不猜测）
    std::fs::write(logs.join("broken_c.jsonl"), mk_rows("C 目标")).unwrap();
    std::fs::write(logs.join("broken_c.meta.json"), "{not json").unwrap();
    // ④ sidecar project_id 空串 → None
    std::fs::write(logs.join("empty_d.jsonl"), mk_rows("D 目标")).unwrap();
    std::fs::write(logs.join("empty_d.meta.json"), r#"{"project_id":"  "}"#).unwrap();

    let drafts = scan_workspace_logs(&logs);
    assert_eq!(drafts.len(), 4);
    let scope_of = |k: &str| {
        drafts
            .iter()
            .find(|d| d.session_key == k)
            .unwrap()
            .scope
            .clone()
    };
    assert_eq!(scope_of("proj_a").as_deref(), Some("prj-42"));
    assert_eq!(scope_of("chat_b"), None);
    assert_eq!(scope_of("broken_c"), None);
    assert_eq!(scope_of("empty_d"), None);
}

// ── 台账 + 游标 ───────────────────────────────────────────────────────

fn sample_draft(session_key: &str, ask: &str) -> CandidateDraft {
    CandidateDraft {
        session_key: session_key.to_string(),
        ts: "2026-11-24T10:05:00+08:00".into(),
        ask: ask.into(),
        reply: ANSWERED.into(),
        goal: "目标".into(),
        result: "结果".into(),
        fp: candidate_fingerprint(session_key, ask, ANSWERED),
        scope: None,
    }
}

#[test]
fn test_ingest_dedups_by_fingerprint() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    let d = sample_draft("web_1", "问题甲");
    assert_eq!(store.ingest(vec![d.clone()]).unwrap(), 1, "首落 1 条");
    assert_eq!(store.ingest(vec![d]).unwrap(), 0, "同指纹重扫不重复");
    // 同会话不同答复（改选）= 新候选
    let mut d2 = sample_draft("web_1", "问题甲");
    d2.reply = "User selected: 需要".into();
    d2.fp = candidate_fingerprint("web_1", "问题甲", &d2.reply);
    assert_eq!(store.ingest(vec![d2]).unwrap(), 1);
    assert_eq!(store.read_all().len(), 2);
    // 空输入幂等
    assert_eq!(store.ingest(Vec::new()).unwrap(), 0);
}

#[test]
fn test_ingest_id_sequence_increments() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .ingest(vec![sample_draft("s1", "问一"), sample_draft("s2", "问二")])
        .unwrap();
    let all = store.read_all();
    assert_eq!(all.len(), 2);
    assert!(all[0].id.ends_with("-001"), "{}", all[0].id);
    assert!(all[1].id.ends_with("-002"), "{}", all[1].id);
    assert!(all[0].id.starts_with("cand-"));
    // 第二批继续递增（跨 ingest 不重号）
    store.ingest(vec![sample_draft("s3", "问三")]).unwrap();
    assert!(store.read_all()[2].id.ends_with("-003"));
}

#[test]
fn test_read_all_tolerates_torn_last_line() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store.ingest(vec![sample_draft("s1", "问一")]).unwrap();
    // 模拟 append 中断撕裂：追加半截行
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(store.candidates_path())
        .unwrap();
    write!(f, "{{\"id\":\"cand-x\", \"draft\": {{\"sess").unwrap();
    let all = store.read_all();
    assert_eq!(all.len(), 1, "撕裂行跳过不炸：{:?}", all.len());
}

#[test]
fn test_two_consumers_independent_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .ingest(vec![
            sample_draft("s1", "问一"),
            sample_draft("s2", "问二"),
            sample_draft("s3", "问三"),
        ])
        .unwrap();
    // dialogue 消费 1 条 → 剩 2
    let d1 = store.unconsumed(CONSUMER_DIALOGUE);
    assert_eq!(d1.len(), 3);
    store.mark_consumed(CONSUMER_DIALOGUE, 1).unwrap();
    assert_eq!(store.unconsumed(CONSUMER_DIALOGUE).len(), 2);
    // profile 独立游标：仍看全量（一次捕获两次提取）
    assert_eq!(store.unconsumed(CONSUMER_PROFILE).len(), 3);
    store.mark_consumed(CONSUMER_PROFILE, 3).unwrap();
    assert_eq!(store.unconsumed(CONSUMER_PROFILE).len(), 0);
    // dialogue 不受 profile 消费影响
    assert_eq!(store.unconsumed(CONSUMER_DIALOGUE).len(), 2);
    // 超额推进钳到总行数
    store.mark_consumed(CONSUMER_DIALOGUE, 99).unwrap();
    assert_eq!(store.unconsumed(CONSUMER_DIALOGUE).len(), 0);
    // 0 消费不写盘
    store.mark_consumed(CONSUMER_DIALOGUE, 0).unwrap();
}

#[test]
fn test_corrupt_consumed_ledger_resets_cursors() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(dir.path());
    store
        .ingest(vec![sample_draft("s1", "问一"), sample_draft("s2", "问二")])
        .unwrap();
    store.mark_consumed(CONSUMER_DIALOGUE, 1).unwrap();
    assert_eq!(store.unconsumed(CONSUMER_DIALOGUE).len(), 1);
    // 毁游标账 → 归零重投递
    std::fs::write(
        dir.path().join("evidence").join("consumed.json"),
        "{ corrupt",
    )
    .unwrap();
    assert_eq!(
        store.unconsumed(CONSUMER_DIALOGUE).len(),
        2,
        "游标损坏归零（重投递宁滥勿缺）"
    );
    // 归零后可正常推进（坏文件不挡写入）
    store.mark_consumed(CONSUMER_DIALOGUE, 2).unwrap();
    assert_eq!(store.unconsumed(CONSUMER_DIALOGUE).len(), 0);
}

// ── 证据包 ────────────────────────────────────────────────────────────

fn pack_candidate(id: &str) -> CorrectionCandidate {
    CorrectionCandidate {
        id: id.into(),
        draft: CandidateDraft {
            session_key: "web_1".into(),
            ts: "2026-11-24T10:05:00+08:00".into(),
            ask: "需要授权吗？".into(),
            reply: ANSWERED.into(),
            goal: "跑测试修失败".into(),
            result: "全部通过".into(),
            fp: "fp".into(),
            scope: None,
        },
    }
}

fn experience(success: bool, tool: &str, input: &str, output: &str) -> Experience {
    Experience {
        id: uuid::Uuid::new_v4().to_string(),
        tool_name: tool.into(),
        input_summary: input.into(),
        output_summary: output.into(),
        success,
        duration_ms: 10,
        timestamp: "2026-11-24T10:00:00+08:00".into(),
        session_key: "web:1".into(),
    }
}

fn insight_entry(statement: &str, status: crate::insights::InsightStatus) -> InsightEntry {
    InsightEntry {
        id: "ins-x-001".into(),
        fingerprint: "fp".into(),
        statement: statement.into(),
        category: "process_constraint".into(),
        evidence: String::new(),
        source: crate::insights::InsightSource::Dialogue,
        confidence: 0.9,
        occurrences: 3,
        first_seen: String::new(),
        last_seen: String::new(),
        status,
        retire_reason: None,
        activated_at: None,
        scope: None,
    }
}

#[test]
fn test_pack_full_assembly() {
    let cands = vec![pack_candidate("cand-20261124-001")];
    let insights = vec![
        insight_entry("先尝试再询问", crate::insights::InsightStatus::Active),
        insight_entry("退役条目不进包", crate::insights::InsightStatus::Retired),
    ];
    let exps = vec![
        experience(false, "exec", "cargo test", "1 failed"),
        experience(true, "read_file", "a.rs", "ok"),
    ];
    let pack = build_evidence_pack(
        Some("修登录 bug"),
        &cands,
        &[],
        Some(&insights),
        Some("- [efficiency_issue] web_search 3 次连跑"),
        &exps,
        Some("# 反思报告\n\n## 洞察\n- 高频 exec 失败"),
        Some("weather, github"),
    );
    assert_eq!(pack.candidate_count, 1);
    assert!(!pack.truncated);
    assert!(pack.text.contains("cand-20261124-001"));
    assert!(pack.text.contains("需要授权吗？"));
    assert!(pack.text.contains("修登录 bug"), "任务目标节");
    assert!(pack.text.contains("先尝试再询问"), "active 洞察节");
    assert!(!pack.text.contains("退役条目不进包"), "非 active 不进包");
    assert!(pack.text.contains("监控闭环模式高亮"), "模式高亮节");
    assert!(pack.text.contains("web_search 3 次连跑"));
    assert!(pack.text.contains("cargo test"), "失败经验节");
    assert!(!pack.text.contains("ok"), "成功经验不进 digest");
    assert!(pack.text.contains("最新反思报告摘录"), "报告摘录节");
    assert!(pack.text.contains("高频 exec 失败"));
    assert!(pack.text.contains("weather"), "技能库节");
}

#[test]
fn test_pack_pattern_and_report_none_omits_sections() {
    let pack = build_evidence_pack(None, &[], &[], None, None, &[], None, None);
    assert!(!pack.text.contains("监控闭环模式高亮"), "None 模式节省略");
    assert!(!pack.text.contains("最新反思报告摘录"), "None 报告节省略");
    // 空白串同样省略（filter 非 empty 语义）。
    let pack2 = build_evidence_pack(None, &[], &[], None, Some("  "), &[], Some("  "), None);
    assert!(!pack2.text.contains("监控闭环模式高亮"));
    assert!(!pack2.text.contains("最新反思报告摘录"));
}

#[test]
fn test_pack_report_excerpt_head_capped() {
    let big = "报".repeat(crate::session_evidence::REPORT_EXCERPT_MAX_CHARS + 500);
    let pack = build_evidence_pack(None, &[], &[], None, None, &[], Some(&big), None);
    let starts = pack.text.find("## 最新反思报告摘录").unwrap();
    let body_len = pack.text[starts..].chars().count();
    assert!(
        body_len
            <= crate::session_evidence::REPORT_EXCERPT_MAX_CHARS
                + "## 最新反思报告摘录\n\n".chars().count()
                + 10,
        "报告摘录头截断：{body_len}"
    );
}

#[test]
fn test_pack_empty_candidates_is_valid_pack() {
    let pack = build_evidence_pack(None, &[], &[], None, None, &[], None, None);
    assert_eq!(pack.candidate_count, 0);
    assert!(!pack.truncated);
    assert!(
        pack.text.contains("无候选"),
        "「没有值得学的」一等答案的原料：{}",
        pack.text
    );
}

#[test]
fn test_pack_experience_digest_tail_capped() {
    let mut exps = Vec::new();
    for i in 0..(EXPERIENCE_DIGEST_MAX_ITEMS + 5) {
        exps.push(experience(false, "exec", &format!("cmd{i}"), "fail"));
    }
    let pack = build_evidence_pack(None, &[], &[], None, None, &exps, None, None);
    assert!(
        pack.text
            .contains(&format!("cmd{}", EXPERIENCE_DIGEST_MAX_ITEMS + 4)),
        "取尾部"
    );
    assert!(!pack.text.contains("cmd0\n"), "头部溢出丢弃");
    assert!(
        pack.text.contains(&format!(
            "{} / {}",
            EXPERIENCE_DIGEST_MAX_ITEMS,
            EXPERIENCE_DIGEST_MAX_ITEMS + 5
        )),
        "计数诚实：截 N / 总 M"
    );
}

#[test]
fn test_pack_budget_cap_truncates_and_flags() {
    // 大量候选 + 超长技能摘要 → 技能节装箱失败
    let cands: Vec<CorrectionCandidate> = (0..40)
        .map(|i| pack_candidate(&format!("cand-20261124-{i:03}")))
        .collect();
    let big_skills = "技".repeat(EVIDENCE_PACK_MAX_CHARS);
    let pack = build_evidence_pack(None, &cands, &[], None, None, &[], None, Some(&big_skills));
    assert!(pack.truncated, "超预算诚实标记");
    assert!(
        pack.text.chars().count() <= EVIDENCE_PACK_MAX_CHARS,
        "预算内：{}",
        pack.text.chars().count()
    );
    assert!(
        pack.text.contains("cand-20261124-000"),
        "高优先节（候选）先进包"
    );
    assert_eq!(
        pack.candidate_count, CANDIDATE_PACK_MAX,
        "计数=实际进包条数（节帽修剪，超出不消费）——旧口径「输入全量」正是 BUG-1 游标整批推进的帮凶"
    );
    assert!(pack.text.contains("cand-20261124-011"), "节帽内末条进包");
    assert!(!pack.text.contains("cand-20261124-012"), "第 13 条不进包");
    assert!(
        pack.text.contains("另有 28 条候选超出本包容量"),
        "溢出注记诚实：{}",
        pack.text
    );
}

#[test]
fn test_pack_candidate_cap_prunes_without_truncation() {
    // 节帽修剪 ≠ 截断：15 条短候选 → 进 12 条，truncated 恒 false
    let cands: Vec<CorrectionCandidate> = (0..15)
        .map(|i| pack_candidate(&format!("cand-20261124-{i:03}")))
        .collect();
    let pack = build_evidence_pack(None, &cands, &[], None, None, &[], None, None);
    assert_eq!(pack.candidate_count, CANDIDATE_PACK_MAX);
    assert!(!pack.truncated, "节帽致停不置截断标记");
    assert!(pack.text.contains("（12 / 15 条）"), "计数诚实");
    assert!(pack.text.contains("另有 3 条候选超出本包容量"));
    assert!(
        pack.text.contains("留待下一轮，本轮不标记消费"),
        "游标不推进的承诺写进包文本"
    );
}

#[test]
fn test_pack_candidate_budget_stop_prunes_and_flags() {
    // 病态长字段：每条候选 ~2100 字符 → 总预算装 2 条后停（BUG-1 整节
    // drop 的残余口子：逐条装箱后 candidate_count 只数真进包的）
    let mut cands: Vec<CorrectionCandidate> = Vec::new();
    for i in 0..3 {
        let mut c = pack_candidate(&format!("cand-20261124-{i:03}"));
        c.draft.goal = "长".repeat(2000);
        cands.push(c);
    }
    let pack = build_evidence_pack(None, &cands, &[], None, None, &[], None, None);
    assert_eq!(pack.candidate_count, 2, "只数装得下的");
    assert!(pack.truncated, "预算致停诚实标记");
    assert!(
        pack.text.chars().count() <= EVIDENCE_PACK_MAX_CHARS,
        "预算内：{}",
        pack.text.chars().count()
    );
    assert!(pack.text.contains("（2 / 3 条）"));
    assert!(pack.text.contains("cand-20261124-000"));
    assert!(pack.text.contains("cand-20261124-001"));
    assert!(!pack.text.contains("cand-20261124-002"), "装不下的不进包");
    assert!(pack.text.contains("另有 1 条候选超出本包容量"));
    // 不变量对质：candidate_count ≡ 文本中真实出现的候选条数
    let in_text = ["000", "001", "002"]
        .iter()
        .filter(|s| pack.text.contains(&format!("cand-20261124-{s}")))
        .count();
    assert_eq!(pack.candidate_count, in_text, "计数≡进包数（消费游标安全）");
}

#[test]
fn test_pack_active_insights_cap() {
    // BUG-2：active 无上限会挤占后续节预算——封顶 8 条 + 溢出注记
    let insights: Vec<InsightEntry> = (0..10)
        .map(|i| {
            let mut e = insight_entry(
                &format!("洞察内容第{i:02}条"),
                crate::insights::InsightStatus::Active,
            );
            e.id = format!("ins-x-{i:03}");
            e
        })
        .collect();
    let pack = build_evidence_pack(None, &[], &[], Some(&insights), None, &[], None, None);
    assert!(
        pack.text.contains("（8 / 10 条）"),
        "active 节帽计数诚实：{}",
        pack.text
    );
    assert!(pack.text.contains("洞察内容第07条"), "节帽内末条进包");
    assert!(!pack.text.contains("洞察内容第08条"), "第 9 条不进包");
    assert!(pack.text.contains("另有 2 条 active 未列入本包"));
    assert!(!pack.truncated, "active 修剪不置截断");
    // 恰好在帽内：全部进包无注记
    let insights9: Vec<InsightEntry> = insights.into_iter().take(8).collect();
    let pack2 = build_evidence_pack(None, &[], &[], Some(&insights9), None, &[], None, None);
    assert!(pack2.text.contains("（8 / 8 条）"));
    assert!(!pack2.text.contains("另有"), "帽内不出现溢出注记");
}

// ── 教训节（双缝互通缝一，2026-11-25）────────────────────────────────

fn lesson(
    id: &str,
    category: &str,
    content: &str,
    confidence: f64,
    evidence: &[&str],
) -> LessonEntry {
    LessonEntry {
        id: id.into(),
        ts_ms: 0,
        kind: "lesson".into(),
        category: category.into(),
        content: content.into(),
        confidence,
        evidence: evidence.iter().map(|s| s.to_string()).collect(),
        session_count: 0,
        status: "pending".into(),
    }
}

/// lesson_ids ≡ 文本真实出现的教训条目（BUG-1 同款不变量对质）。
fn lessons_in_text(pack: &EvidencePack) -> Vec<String> {
    pack.lesson_ids
        .iter()
        .filter(|id| pack.text.contains(&format!("### {id} ")))
        .cloned()
        .collect()
}

#[test]
fn test_pack_lessons_section_packs_and_records_ids() {
    let lessons = vec![
        lesson("L1", "workflow", "先跑检查再汇报", 0.9, &["web_1"]),
        lesson("L2", "tool_preference", "偏好 ripgrep", 0.8, &[]),
        lesson("L3", "style", "回复用中文", 0.75, &["web_2", "web_3"]),
    ];
    let pack = build_evidence_pack(None, &[], &lessons, None, None, &[], None, None);
    assert!(
        pack.text.contains("## 用户会话教训（3 / 3 条）"),
        "节头计数诚实：{}",
        pack.text
    );
    for (id, cat, conf) in [
        ("L1", "workflow", "0.90"),
        ("L2", "tool_preference", "0.80"),
        ("L3", "style", "0.75"),
    ] {
        assert!(
            pack.text.contains(&format!("### {id} [{cat}] 置信 {conf}")),
            "条目格式（id/类别/置信）：{id}"
        );
    }
    assert_eq!(
        pack.lesson_ids,
        vec!["L1", "L2", "L3"],
        "lesson_ids ≡ 实际进包条目（ack 凭证）"
    );
    assert!(pack.text.contains("- 教训：先跑检查再汇报"));
    assert!(
        pack.text.contains("- 依据：web_1"),
        "非空 evidence 出依据行"
    );
    assert!(!pack.text.contains("另有"), "帽内无溢出注记");
    assert!(!pack.truncated);
    assert_eq!(lessons_in_text(&pack), pack.lesson_ids, "ids ≡ 文本");
}

#[test]
fn test_pack_lesson_without_evidence_omits_evidence_line() {
    let lessons = vec![lesson("L1", "style", "回复用中文", 0.8, &[])];
    let pack = build_evidence_pack(None, &[], &lessons, None, None, &[], None, None);
    assert!(!pack.text.contains("- 依据："), "空 evidence 不产依据行");
    assert!(pack.text.contains("- 教训：回复用中文"));
}

#[test]
fn test_pack_lesson_cap_excludes_beyond_max_unacked() {
    let lessons: Vec<LessonEntry> = (0..10)
        .map(|i| {
            lesson(
                &format!("L{i:02}"),
                "workflow",
                &format!("教训{i}"),
                0.8,
                &[],
            )
        })
        .collect();
    let pack = build_evidence_pack(None, &[], &lessons, None, None, &[], None, None);
    assert_eq!(pack.lesson_ids.len(), LESSON_PACK_MAX);
    assert_eq!(
        pack.lesson_ids,
        (0..8).map(|i| format!("L{i:02}")).collect::<Vec<_>>(),
        "取节帽内前 8 条"
    );
    assert!(pack.text.contains("（8 / 10 条）"));
    assert!(pack.text.contains("另有 2 条教训超出本包容量"));
    assert!(
        pack.text.contains("本轮不确认消费"),
        "不 ack 的承诺写进包文本"
    );
    assert!(!pack.text.contains("### L08"), "第 9 条不进包");
    assert!(!pack.truncated, "节帽致停不置截断");
    assert_eq!(lessons_in_text(&pack), pack.lesson_ids, "ids ≡ 文本");
}

#[test]
fn test_pack_lesson_budget_stop_marks_truncated_and_unpacked_absent() {
    // 教训条目字段都有截断上限（content 200 / evidence 120），单条不超
    // 预算；教训节预算致停的入口 = 候选节先吃掉预算 → 教训节装箱停。
    let cands: Vec<CorrectionCandidate> = (0..CANDIDATE_PACK_MAX)
        .map(|i| {
            let mut c = pack_candidate(&format!("cand-20261124-{i:03}"));
            c.draft.goal = "长".repeat(900);
            c
        })
        .collect();
    let lessons: Vec<LessonEntry> = (0..8)
        .map(|i| {
            lesson(
                &format!("L{i:02}"),
                "workflow",
                &format!("教训{}{}", i, "长".repeat(180)),
                0.8,
                &[],
            )
        })
        .collect();
    let pack = build_evidence_pack(None, &cands, &lessons, None, None, &[], None, None);
    assert!(pack.truncated, "预算致停诚实标记");
    assert!(
        pack.lesson_ids.len() < lessons.len(),
        "预算吃紧时教训部分装箱：{} / {}",
        pack.lesson_ids.len(),
        lessons.len()
    );
    assert!(
        pack.text.chars().count() <= EVIDENCE_PACK_MAX_CHARS,
        "预算内：{}",
        pack.text.chars().count()
    );
    assert_eq!(lessons_in_text(&pack), pack.lesson_ids, "ids ≡ 文本");
    assert!(
        pack.text.contains("另有") && pack.text.contains("本轮不确认消费"),
        "教训节溢出注记在场"
    );
}

#[test]
fn test_pack_lesson_content_capped_at_max_chars() {
    let long = "长".repeat(LESSON_CONTENT_MAX_CHARS + 100);
    let lessons = vec![lesson("L1", "style", &long, 0.8, &[])];
    let pack = build_evidence_pack(None, &[], &lessons, None, None, &[], None, None);
    let line = pack
        .text
        .split("- 教训：")
        .nth(1)
        .unwrap()
        .lines()
        .next()
        .unwrap();
    assert_eq!(
        line.chars().count(),
        LESSON_CONTENT_MAX_CHARS,
        "进包教训截断到上限"
    );
    assert!(pack.lesson_ids == vec!["L1"], "截断不碍装箱记账");
}

#[test]
fn test_pack_no_lessons_omits_section_entirely() {
    let pack = build_evidence_pack(None, &[], &[], None, None, &[], None, None);
    assert!(
        !pack.text.contains("用户会话教训"),
        "空队列 = 节整体省略（非一等信号）：{}",
        pack.text
    );
    assert!(pack.lesson_ids.is_empty());
}

#[test]
fn test_pack_joint_budget_keeps_both_packed_sections_intact() {
    // 联合预算窗回归锁：候选节与教训节都满装 + 尾部大节（报告/技能）→
    // 总装配预算超限时牺牲的是尾部节，已装箱的两节**不丢**——lesson_ids
    // 恒 ≡ 文本（若两节独立对全额预算预留，累计预留超支会让教训节在
    // 装配期被丢而 ids 已记账 = 未进包即 ack 的 BUG-1 变体）。
    let cands: Vec<CorrectionCandidate> = (0..12)
        .map(|i| pack_candidate(&format!("cand-20261124-{i:03}")))
        .collect();
    let lessons: Vec<LessonEntry> = (0..8)
        .map(|i| {
            lesson(
                &format!("L{i:02}"),
                "workflow",
                &format!("教训{i}"),
                0.8,
                &[],
            )
        })
        .collect();
    let big = "报".repeat(EVIDENCE_PACK_MAX_CHARS);
    let pack = build_evidence_pack(
        None,
        &cands,
        &lessons,
        None,
        None,
        &[],
        Some(&big),
        Some(&big),
    );
    assert_eq!(pack.candidate_count, CANDIDATE_PACK_MAX);
    assert_eq!(pack.lesson_ids.len(), LESSON_PACK_MAX);
    assert!(pack.truncated, "尾部大节装不下诚实标记");
    assert!(
        pack.text.contains("## 用户会话教训"),
        "教训节不被总装配丢弃"
    );
    assert!(pack.text.contains("## 纠正候选"));
    assert_eq!(
        lessons_in_text(&pack).len(),
        pack.lesson_ids.len(),
        "ids ≡ 文本（进包才可 ack）"
    );
    assert!(
        pack.text.chars().count() <= EVIDENCE_PACK_MAX_CHARS,
        "预算内：{}",
        pack.text.chars().count()
    );
}

// ── 漂移锁 ────────────────────────────────────────────────────────────

#[test]
fn test_answered_prefixes_match_interactive_contract() {
    // interactive.rs 返回文本形态漂移锁：前缀表与工具返回契约同步变更，
    // 否则扫描器静默漏抓。这里钉死已知形态全集。
    assert!(answered_reply("User selected: pnpm").is_some());
    assert!(answered_reply("User selected 2 option(s): a; b").is_some());
    assert!(answered_reply("User answered form: {\"env\":\"prod\"}").is_some());
    assert!(
        answered_reply("No answer: user did not answer, proceed with your best judgment.")
            .is_none()
    );
    assert!(answered_reply("").is_none());
    assert!(
        answered_reply("User task failed: {}").is_none(),
        "非答复形态不误抓"
    );
}
