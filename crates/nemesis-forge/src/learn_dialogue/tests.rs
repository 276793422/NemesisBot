//! 学习对话引擎行为测试：脚本 caller 四阶段推进 / 预算透传 / 非法 JSON
//! 诚实丢弃 / 留痕 / estop / 互斥 / 「没有值得学的」/ supersede 退役。

use super::*;
use crate::insights::InsightStatus;
use crate::learn_dialogue::TEST_LOCK;
use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;

/// 脚本 caller：按调用序弹出预设回复；记录每次 (system, user, max_tokens)。
struct ScriptedCaller {
    steps: StdMutex<VecDeque<Result<String, String>>>,
    seen: StdMutex<Vec<(String, String, Option<i64>)>>,
    /// 第 N 次调用结束后置位（estop 中断注入）。
    stop_after: Option<(usize, Arc<AtomicBool>)>,
    calls: std::sync::atomic::AtomicUsize,
}

impl ScriptedCaller {
    fn new(steps: Vec<Result<String, String>>) -> Self {
        Self {
            steps: StdMutex::new(steps.into()),
            seen: StdMutex::new(Vec::new()),
            stop_after: None,
            calls: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    fn with_stop_after(mut self, n: usize, flag: Arc<AtomicBool>) -> Self {
        self.stop_after = Some((n, flag));
        self
    }

    fn seen(&self) -> Vec<(String, String, Option<i64>)> {
        self.seen.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl LLMCaller for ScriptedCaller {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> Result<String, String> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        self.seen.lock().unwrap().push((
            system_prompt.to_string(),
            user_prompt.to_string(),
            max_tokens,
        ));
        let reply = {
            let mut q = self.steps.lock().unwrap();
            q.pop_front()
                .unwrap_or_else(|| Ok("{\"proposals\":[],\"empty_reason\":\"脚本耗尽\"}".into()))
        };
        if let Some((after, flag)) = &self.stop_after
            && n == *after
        {
            flag.store(true, Ordering::SeqCst);
        }
        reply
    }
}

// ── 脚本 JSON 常量 ────────────────────────────────────────────────────

const P1_TWO: &str = r#"{"proposals":[
  {"id":"p1","title":"别乱停","observation":"agent 半途停下问授权","lesson":"先自己尝试可行路径"},
  {"id":"p2","title":"构建配方","observation":"多次成功序列","lesson":"完整操作步骤"}]}"#;

const P2_INSIGHT_SKILL: &str = r#"{"items":[{"id":"p1","form":"insight","reason":"一句话常驻"},{"id":"p2","form":"skill","reason":"完整配方"}]}"#;

const P3_KEEP_BOTH: &str = r#"{"items":[{"id":"p1","verdict":"keep"},{"id":"p2","verdict":"keep","target":"","note":""}]}"#;

const P4_FINAL: &str = r#"{"insights":[
  {"id":"p1","statement":"停下询问前先自己尝试可行路径","category":"behavior","evidence":"候选 cand-1：问授权被纠正","confidence":0.9}],
 "skills":[
  {"id":"p2","slug":"build-recipe","skill_md":"---\nname: build-recipe\ndescription: 构建配方\nversion: \"1.0\"\ntests:\n  - name: t1\n    turns:\n      - user: \"构建\"\n    expect:\n      contains: [\"done\"]\n---\n# 构建配方\n\n1. cargo build"}]}"#;

fn two_phase_script() -> ScriptedCaller {
    ScriptedCaller::new(vec![
        Ok(P1_TWO.into()),
        Ok(P2_INSIGHT_SKILL.into()),
        Ok(P3_KEEP_BOTH.into()),
        Ok(P4_FINAL.into()),
    ])
}

fn store_in(dir: &std::path::Path) -> InsightStore {
    InsightStore::new(dir)
}

fn pack_of(text: &str) -> EvidencePack {
    EvidencePack {
        text: text.to_string(),
        truncated: false,
        candidate_count: 1,
        lesson_ids: Vec::new(),
    }
}

// ── 全流程 ────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_full_four_phase_dialogue_lands() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("learn").join("t_manual");
    let caller = two_phase_script();
    let store = store_in(&dir.path().join("ws"));
    let out = run(
        &caller,
        &pack_of("# 学习证据包\n\n候选 1 条"),
        &store,
        "weather, github",
        &|| false,
        &trace,
        &dir.path().join("learn.lock"),
        777,
        None,
        None,
    )
    .await
    .unwrap();

    // 四阶段全走完；洞察落库为 candidate；skill 草稿带 tests 预探测通过。
    assert_eq!(out.phases_completed, 4);
    assert_eq!(out.proposals_total, 2);
    assert_eq!(out.insights.len(), 1);
    assert!(out.insights[0].id.starts_with("ins-"));
    assert_eq!(out.insights[0].statement, "停下询问前先自己尝试可行路径");
    assert_eq!(out.skills.len(), 1);
    assert_eq!(out.skills[0].slug, "build-recipe");
    assert!(out.skills[0].has_tests, "tests 段合法 → has_tests");
    assert!(out.dropped.is_empty());
    assert!(out.empty_reason.is_none());
    assert!(!out.truncated);

    // 落库实况：candidate 态 + source=dialogue。
    let cands = store.list(Some(InsightStatus::Candidate)).unwrap();
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].id, out.insights[0].id);
    assert_eq!(cands[0].category, "behavior");

    // 预算透传：每次调用 max_tokens = Some(777)；system = 协议 system。
    let seen = caller.seen();
    assert_eq!(seen.len(), 4, "四个阶段各一次调用");
    for (i, (sys, user, mt)) in seen.iter().enumerate() {
        assert_eq!(*mt, Some(777), "阶段{} 预算透传", i + 1);
        assert_eq!(sys, &LEARN_DIALOGUE_SYSTEM_PROMPT);
        assert!(
            user.starts_with(&format!("# 学习对话阶段 {}/4：", i + 1)),
            "阶段标记契约：{user}"
        );
    }
    // 阶段 2/3/4 的 user prompt 串了前几轮 transcript（多轮推进）。
    assert!(seen[1].1.contains(&seen[0].1), "阶段2 携带阶段1 往返");
    assert!(seen[2].1.contains(&seen[1].1), "阶段3 携带阶段2 往返");
    assert!(seen[2].1.contains("weather"), "阶段3 携带技能库清单");
    assert!(seen[3].1.contains(&seen[2].1), "阶段4 携带阶段3 往返");

    // 留痕：transcript.md 含四阶段标记；outcome.json 可解析回读。
    let tp = std::fs::read_to_string(trace.join("transcript.md")).unwrap();
    for phase in 1..=4 {
        assert!(
            tp.contains(&format!("## 阶段 {phase}")),
            "transcript 阶段{phase}"
        );
    }
    assert!(
        tp.contains("停下询问前先自己尝试可行路径"),
        "模型原始回复在场"
    );
    let oj: LearnDialogueOutcome =
        serde_json::from_str(&std::fs::read_to_string(trace.join("outcome.json")).unwrap())
            .unwrap();
    assert_eq!(oj.phases_completed, 4);
}

#[tokio::test]
async fn test_truncation_flag_passthrough() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = ScriptedCaller::new(vec![Ok(
        r#"{"proposals":[],"empty_reason":"证据被截断，不足以判断"}"#.into(),
    )]);
    let mut pack = pack_of("x");
    pack.truncated = true;
    let out = run(
        &caller,
        &pack,
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert!(out.truncated, "组装器截断标记诚实透传");
    assert_eq!(out.empty_reason.as_deref(), Some("证据被截断，不足以判断"));
}

// ── 场景透传：带 scope 的对话 → 落库条目继承场景 ──────────────────────

#[tokio::test]
async fn test_scope_passthrough_to_created_entries() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = two_phase_script();
    let store = store_in(&dir.path().join("ws"));
    let out = run(
        &caller,
        &pack_of("# 学习证据包\n\n项目会话候选"),
        &store,
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        Some("prj-9"),
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.insights.len(), 1);
    // 落库实况：条目带项目场景戳。
    let cands = store.list(Some(InsightStatus::Candidate)).unwrap();
    assert_eq!(cands.len(), 1);
    assert_eq!(cands[0].scope.as_deref(), Some("prj-9"), "场景透传到条目");
    // None（对话批）→ 条目场景为空（不同 statement 避开指纹去重）。
    let caller2 = ScriptedCaller::new(vec![
        Ok(r#"{"proposals":[{"id":"p1","title":"t","observation":"o","lesson":"l"}]}"#.into()),
        Ok(r#"{"items":[{"id":"p1","form":"insight"}]}"#.into()),
        Ok(r#"{"items":[{"id":"p1","verdict":"keep"}]}"#.into()),
        Ok(r#"{"insights":[{"id":"p1","statement":"对话批独有的教训","category":"behavior","evidence":"e","confidence":0.9}]}"#.into()),
    ]);
    let out2 = run(
        &caller2,
        &pack_of("对话会话候选"),
        &store,
        "",
        &|| false,
        &dir.path().join("t2"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out2.insights.len(), 1);
    let cands2 = store.list(Some(InsightStatus::Candidate)).unwrap();
    assert_eq!(cands2.len(), 2);
    let dialogue_entry = cands2.iter().find(|e| e.id == out2.insights[0].id).unwrap();
    assert_eq!(dialogue_entry.scope, None, "对话批条目场景为空");
}

// ── 「没有值得学的」一等答案 ──────────────────────────────────────────

#[tokio::test]
async fn test_nothing_worth_learning_is_first_class() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = ScriptedCaller::new(vec![Ok(
        r#"{"proposals":[],"empty_reason":"候选全是正常咨询，失败经验是环境噪声"}"#.into(),
    )]);
    let out = run(
        &caller,
        &pack_of("无候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 1);
    assert_eq!(out.proposals_total, 0);
    assert_eq!(
        out.empty_reason.as_deref(),
        Some("候选全是正常咨询，失败经验是环境噪声")
    );
    assert_eq!(caller.seen().len(), 1, "空提议后对话即时收场");
    assert!(out.insights.is_empty() && out.skills.is_empty());
}

// ── 非法 JSON：单次重试 + 二次失败诚实丢弃 ────────────────────────────

#[tokio::test]
async fn test_invalid_json_phase_drops_items_honestly() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("t");
    // 阶段1 合法；阶段2 连续两次垃圾 → 全部条目放弃，阶段3/4 不再调用。
    let caller = ScriptedCaller::new(vec![
        Ok(P1_TWO.into()),
        Ok("这不是 JSON，我觉得 p1 是洞察".into()),
        Ok("抱歉，p1 insight p2 skill（仍是散文）".into()),
    ]);
    let out = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &trace,
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 2);
    assert!(out.empty_reason.is_some());
    assert!(
        out.dropped
            .iter()
            .any(|d| d.contains("两次输出均非合法 JSON")),
        "{:?}",
        out.dropped
    );
    assert_eq!(caller.seen().len(), 3, "阶段2 一次原始 + 一次重试");
    // 重试 prompt 带纠正注记。
    assert!(caller.seen()[2].1.contains("不是合法 JSON 对象"));
    // 留痕带现场（垃圾原文在场）。
    let tp = std::fs::read_to_string(trace.join("transcript.md")).unwrap();
    assert!(tp.contains("两次非法 JSON"));
    assert!(tp.contains("我觉得 p1 是洞察"));
}

#[tokio::test]
async fn test_transport_failure_aborts_with_trace() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("t");
    let caller = ScriptedCaller::new(vec![Ok(P1_TWO.into()), Err("connect error: dead".into())]);
    let err = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &trace,
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("阶段2"), "{err}");
    // transcript-so-far 已留痕（阶段1 在场）。
    let tp = std::fs::read_to_string(trace.join("transcript.md")).unwrap();
    assert!(tp.contains("## 阶段 1"));
}

// ── 定形/对账的逐条诚实裁决 ───────────────────────────────────────────

#[tokio::test]
async fn test_none_and_unknown_verdicts_drop_individually() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = ScriptedCaller::new(vec![
        Ok(P1_TWO.into()),
        // p1 none；p2 未知形态值 → 两条都丢，formed 空 → 阶段2 收场。
        Ok(r#"{"items":[{"id":"p1","form":"none"},{"id":"p2","form":"super-power"}]}"#.into()),
    ]);
    let out = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 2);
    assert!(
        out.dropped.iter().any(|d| d.contains("p1：定形为 none"))
            && out.dropped.iter().any(|d| d.contains("p2：定形值非法")),
        "{:?}",
        out.dropped
    );
}

#[tokio::test]
async fn test_duplicate_verdict_excludes_from_final() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir.path().join("ws"));
    // 预置 active 条目 → 渲染进台账 → 对账判 duplicate。
    store
        .add_candidate(
            "先尝试再询问",
            "behavior",
            "e",
            InsightSource::Manual,
            0.9,
            None,
        )
        .unwrap();
    let active = store.list(Some(InsightStatus::Active)).unwrap();
    let target = active[0].id.clone();

    let caller = ScriptedCaller::new(vec![
        Ok(r#"{"proposals":[{"id":"p1","title":"t","observation":"o","lesson":"l"}]}"#.into()),
        Ok(r#"{"items":[{"id":"p1","form":"insight"}]}"#.into()),
        Ok(format!(
            r#"{{"items":[{{"id":"p1","verdict":"duplicate","target":"{target}"}}]}}"#
        )),
        // 不应被调用
        Ok(P4_FINAL.into()),
    ]);
    let out = run(
        &caller,
        &pack_of("候选"),
        &store,
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 3);
    assert_eq!(caller.seen().len(), 3, "对账全重复 → 阶段4 不调用");
    assert!(
        out.dropped.iter().any(|d| d.contains("duplicate")),
        "{:?}",
        out.dropped
    );
    // 阶段3 prompt 带台账渲染（active 条目在场）。
    assert!(caller.seen()[2].1.contains("先尝试再询问"));
}

// ── supersede：新条目落库 + 旧 active 条目退役 ────────────────────────

#[tokio::test]
async fn test_supersede_retires_active_target() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir.path().join("ws"));
    store
        .add_candidate(
            "旧规则：总是先询问",
            "behavior",
            "e",
            InsightSource::Manual,
            0.9,
            None,
        )
        .unwrap();
    let old_id = store.list(Some(InsightStatus::Active)).unwrap()[0]
        .id
        .clone();

    let caller = ScriptedCaller::new(vec![
        Ok(r#"{"proposals":[{"id":"p1","title":"新规则","observation":"o","lesson":"l"}]}"#.into()),
        Ok(r#"{"items":[{"id":"p1","form":"insight"}]}"#.into()),
        Ok(format!(
            r#"{{"items":[{{"id":"p1","verdict":"supersede","target":"{old_id}"}}]}}"#
        )),
        Ok(r#"{"insights":[{"id":"p1","statement":"新规则：可行路径先自己试再询问","category":"behavior","evidence":"更可信的新证据","confidence":0.95}]}"#.into()),
    ]);
    let out = run(
        &caller,
        &pack_of("候选"),
        &store,
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 4);
    assert_eq!(out.insights.len(), 1, "新条目落库");
    assert_eq!(out.retired, vec![old_id.clone()], "旧条目退役记录");
    let all = store.list(None).unwrap();
    let old = all.iter().find(|e| e.id == old_id).unwrap();
    assert_eq!(old.status, InsightStatus::Retired);
    assert_eq!(
        old.retire_reason,
        Some(crate::insights::RetireReason::Superseded)
    );
}

// ── 定稿逐条闸：保留清单外 / 形态不符 / 落库被拒 ──────────────────────

#[tokio::test]
async fn test_phase4_per_item_gates() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = ScriptedCaller::new(vec![
        Ok(P1_TWO.into()),
        // p1 定形 skill、p2 定形 both——p4 载荷的形态错配（p1 收 insight、
        // p2 收超长 insight + 空 slug skill）才能逐条命中各闸。
        Ok(r#"{"items":[{"id":"p1","form":"skill"},{"id":"p2","form":"both"}]}"#.into()),
        Ok(P3_KEEP_BOTH.into()),
        Ok(
            // p9 不在保留清单；p1 带 skill 形态不符；p2 超 120 字 statement
            // 落库被拒；skill 空 slug 丢弃。
            r#"{"insights":[
              {"id":"p9","statement":"孤儿条目"},
              {"id":"p1","statement":"形态不符的洞察","category":"behavior"},
              {"id":"p2","statement":"长","category":"behavior"}],
             "skills":[{"id":"p2","slug":"  ","skill_md":"---\nname: x\n---\n内容"}]}"#
                .into(),
        ),
    ]);
    let long_statement = format!("长{}", "字".repeat(120)); // 121 字 > 120
    let raw = caller.steps.lock().unwrap()[3].as_ref().unwrap().replace(
        "\"statement\":\"长\"",
        &format!("\"statement\":\"{long_statement}\""),
    );
    caller.steps.lock().unwrap()[3] = Ok(raw);

    let out = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 4);
    assert!(out.insights.is_empty() && out.skills.is_empty());
    assert!(
        out.dropped
            .iter()
            .any(|d| d.contains("p9：不在对账保留清单"))
            && out
                .dropped
                .iter()
                .any(|d| d.contains("p1：定形未选 insight 形态"))
            && out.dropped.iter().any(|d| d.contains("p2：落库被拒"))
            && out.dropped.iter().any(|d| d.contains("slug/skill_md 为空")),
        "{:?}",
        out.dropped
    );
}

// ── estop ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_estop_at_entry_rejects() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let caller = two_phase_script();
    let err = run(
        &caller,
        &pack_of("x"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| true,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("急停"), "{err}");
    assert_eq!(caller.seen().len(), 0, "入口即拒，零 LLM 调用");
}

#[tokio::test]
async fn test_estop_mid_dialogue_persists_trace() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("t");
    let flag = Arc::new(AtomicBool::new(false));
    // 第 1 次调用（阶段1）后置位 estop。
    let caller = two_phase_script().with_stop_after(1, flag.clone());
    let err = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| flag.load(Ordering::SeqCst),
        &trace,
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("急停中断"), "{err}");
    let tp = std::fs::read_to_string(trace.join("transcript.md")).unwrap();
    assert!(tp.contains("## 阶段 1"), "中断现场留痕");
}

// ── 互斥 ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_busy_rejects_and_releases_on_completion() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    // 手动占用互斥位 → 第二次触发诚实拒绝。
    DIALOGUE_BUSY.store(true, Ordering::SeqCst);
    let caller = two_phase_script();
    let err = run(
        &caller,
        &pack_of("x"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("已在进行"), "{err}");
    assert_eq!(caller.seen().len(), 0);
    DIALOGUE_BUSY.store(false, Ordering::SeqCst);

    // 正常跑完后互斥位释放（可再次进入）。
    let out = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t2"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 4);
    assert!(
        !DIALOGUE_BUSY.load(Ordering::SeqCst),
        "Drop guard 释放互斥位"
    );
}

#[tokio::test]
async fn test_mutex_releases_on_error_path() {
    let _serial = TEST_LOCK.lock().await;
    // 错误出口同样释放（守卫 Drop 覆盖 ? 早退）。
    let dir = tempfile::tempdir().unwrap();
    let caller = ScriptedCaller::new(vec![Err("boom".into())]);
    let _ = run(
        &caller,
        &pack_of("x"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await;
    assert!(!DIALOGUE_BUSY.load(Ordering::SeqCst));
}

// ── 跨进程锁文件（learn.lock）────────────────────────────────────────

#[tokio::test]
async fn test_lockfile_blocks_when_held_elsewhere() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("learn.lock");
    // 模拟另一进程持锁：手工落锁文件（create_new 已被占 ⇒ AlreadyExists 分支）。
    std::fs::write(&lock, "pid=999999 started=人为占位").unwrap();
    let caller = two_phase_script();
    let err = run(
        &caller,
        &pack_of("x"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &lock,
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("learn.lock 在场"), "{err}");
    assert_eq!(caller.seen().len(), 0, "锁文件占用，零 LLM 调用");
    assert!(lock.exists(), "报忙路径不得移除他人锁文件");
}

#[tokio::test]
async fn test_lockfile_stale_takeover() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let lock = dir.path().join("learn.lock");
    // 陈旧锁：mtime 回拨 3h（> LOCK_STALE_SECS 2h）= 持锁进程已死亡。
    std::fs::write(&lock, "pid=1 started=stale").unwrap();
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 3600);
    let f = std::fs::File::options().write(true).open(&lock).unwrap();
    f.set_modified(stale).unwrap();
    drop(f);

    let caller = two_phase_script();
    let out = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &lock,
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 4, "陈旧锁诚实接管，对话正常推进");
    assert!(!lock.exists(), "本进程 Drop 释放自己接管的锁");
}

// ── estop 阶段内中止（在飞 LLM 调用打断）────────────────────────────

/// 慢 caller：调用后 100ms 置位 estop，再挂 30s 模拟在飞 LLM——
/// select! 打断时此 future 被 drop，不会真等满 30s。
struct SlowThenStopCaller {
    flag: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl LLMCaller for SlowThenStopCaller {
    async fn chat(
        &self,
        _system_prompt: &str,
        _user_prompt: &str,
        _max_tokens: Option<i64>,
    ) -> Result<String, String> {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.flag.store(true, Ordering::SeqCst);
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Ok(P1_TWO.into())
    }
}

#[tokio::test]
async fn test_estop_mid_call_interrupts_inflight_llm() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let trace = dir.path().join("t");
    let flag = Arc::new(AtomicBool::new(false));
    let caller = SlowThenStopCaller { flag: flag.clone() };
    let started = std::time::Instant::now();
    let err = run(
        &caller,
        &pack_of("候选"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| flag.load(Ordering::SeqCst),
        &trace,
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap_err();
    // 在飞调用被打断：100ms 置位 + ≤300ms 轮询粒度，远小于挂起的 30s。
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "estop 应打断在飞调用，实际耗时 {:?}",
        started.elapsed()
    );
    assert!(err.contains("阶段1"), "{err}");
    assert!(err.contains("急停"), "{err}");
    let tp = std::fs::read_to_string(trace.join("transcript.md")).unwrap();
    assert!(tp.contains("## 阶段 1"), "中断现场留痕");
}

// ── 自动激活可观测（activated）───────────────────────────────────────

#[tokio::test]
async fn test_recurrence_reach_reports_activated() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let store = InsightStore::with_recurrence(&ws, 2);
    // 预置 candidate（occurrences=1，dialogue 源）；对话复现同一陈述
    // → occurrences=2 ≥ 2 → 达标自动激活。
    store
        .add_candidate(
            "停下询问前先自己尝试可行路径",
            "behavior",
            "e",
            InsightSource::Dialogue,
            0.9,
            None,
        )
        .unwrap();
    let trace = dir.path().join("t");
    let caller = two_phase_script(); // P4_FINAL 陈述与预置同指纹
    let out = run(
        &caller,
        &pack_of("候选"),
        &store,
        "",
        &|| false,
        &trace,
        &dir.path().join("learn.lock"),
        8192,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(out.insights.len(), 1, "同指纹复现不建新条目");
    assert_eq!(out.activated.len(), 1, "激活记录进 outcome");
    assert_eq!(out.activated[0], out.insights[0].id);
    let all = store.list(None).unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].status, InsightStatus::Active);
    assert_eq!(all[0].occurrences, 2);
    // outcome.json 落盘带 activated（留痕面可回查）。
    let oj: LearnDialogueOutcome =
        serde_json::from_str(&std::fs::read_to_string(trace.join("outcome.json")).unwrap())
            .unwrap();
    assert_eq!(oj.activated, out.activated);
}

// ── 缝二：主语分流（user 结论 → 画像回抛路由，2026-11-25）─────────────

use crate::userprofile::bridge::{ObservationEntry, observations_path};
use crate::userprofile::store::{load_profile, profile_path};

/// 单 proposal 脚本（主语分流用）。
const P1_ONE: &str = r#"{"proposals":[{"id":"p1","title":"用户偏好","observation":"提交前跑测试","lesson":"用户偏好先跑测试"}]}"#;
const P2_INSIGHT_ONE: &str = r#"{"items":[{"id":"p1","form":"insight","reason":"一句话"}]}"#;
const P3_KEEP_ONE: &str = r#"{"items":[{"id":"p1","verdict":"keep","target":"","note":""}]}"#;

/// 过闸（subject=user + 画像词表 + 置信 ≥ 0.7）→ 直入画像。
const P4_USER_FINDING: &str = r#"{"insights":[
  {"id":"p1","subject":"user","statement":"用户偏好先跑测试再提交","category":"tool_preference","evidence":"多次提交前跑测试","confidence":0.9}],
 "skills":[]}"#;

/// 低置信 → 观察提名队列（人工裁决带）。
const P4_USER_LOW_CONF: &str = r#"{"insights":[
  {"id":"p1","subject":"user","statement":"用户可能偏好浅色主题","category":"style","evidence":"一次提及","confidence":0.4}],
 "skills":[]}"#;

/// 非法词表（behavior 是 assistant 词表，画像白名单外）→ 观察提名。
const P4_USER_BAD_VOCAB: &str = r#"{"insights":[
  {"id":"p1","subject":"user","statement":"用户讨厌长回复","category":"behavior","evidence":"e","confidence":0.95}],
 "skills":[]}"#;

/// user 主语混入 skills 数组 → 防御性丢弃（用户习惯不写成 agent 技能）。
const P4_USER_WITH_SKILL: &str = r#"{"insights":[
  {"id":"p1","subject":"user","statement":"用户偏好先跑测试再提交","category":"tool_preference","evidence":"e","confidence":0.9}],
 "skills":[{"id":"p2","subject":"user","slug":"user-skill","skill_md":"---\nname: user-skill\n---\n内容"}]}"#;

/// 主语缺省（旧输出零迁移）→ 正常落洞察台账。
const P4_SUBJECT_OMITTED: &str = r#"{"insights":[
  {"id":"p1","statement":"停下询问前先自己尝试可行路径","category":"behavior","evidence":"候选","confidence":0.9}],
 "skills":[]}"#;

fn user_finding_script(p4: &str) -> ScriptedCaller {
    ScriptedCaller::new(vec![
        Ok(P1_ONE.into()),
        Ok(P2_INSIGHT_ONE.into()),
        Ok(P3_KEEP_ONE.into()),
        Ok(p4.into()),
    ])
}

fn observations_in(ws: &std::path::Path) -> Vec<ObservationEntry> {
    std::fs::read_to_string(observations_path(ws))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[tokio::test]
async fn test_user_finding_high_confidence_ingests_profile() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let out = run(
        &user_finding_script(P4_USER_FINDING),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    assert_eq!(out.phases_completed, 4);
    assert!(
        out.insights.is_empty(),
        "user 主语不进洞察台账（不是 agent 资产）"
    );
    assert_eq!(out.user_findings.len(), 1);
    let uf = &out.user_findings[0];
    assert_eq!(uf.disposition, "ingested");
    assert_eq!(uf.id, "p1");
    assert_eq!(uf.statement, "用户偏好先跑测试再提交");
    assert_eq!(uf.category, "tool_preference");
    // 画像落盘实况：source=learn_dialogue / auto=false / kind 强制 habit。
    let profile = load_profile(&ws);
    assert_eq!(profile.habits.len(), 1);
    let h = &profile.habits[0];
    assert_eq!(h.content, "用户偏好先跑测试再提交");
    assert_eq!(h.category, "tool_preference");
    assert_eq!(h.source, "learn_dialogue");
    assert!(!h.auto);
    assert_eq!(h.kind, "habit");
    assert_eq!(h.status, "active");
    assert_eq!(h.evidence, vec!["多次提交前跑测试".to_string()]);
    // 观察队列未动；洞察库确认为空。
    assert!(observations_in(&ws).is_empty());
    assert!(
        store_in(&ws)
            .list(Some(InsightStatus::Candidate))
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn test_user_finding_low_confidence_parks_observation() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let out = run(
        &user_finding_script(P4_USER_LOW_CONF),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    assert_eq!(out.user_findings.len(), 1);
    assert_eq!(out.user_findings[0].disposition, "observation");
    // 观察提名落盘：pending 态 + 陈述/类别/置信忠实。
    let obs = observations_in(&ws);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].content, "用户可能偏好浅色主题");
    assert_eq!(obs[0].category, "style");
    assert!((obs[0].confidence - 0.4).abs() < 1e-9);
    assert_eq!(obs[0].status, "pending");
    assert!(!obs[0].id.is_empty(), "bridge 发号");
    assert_eq!(obs[0].kind, "observation");
    // 画像未动。
    assert!(load_profile(&ws).habits.is_empty());
    assert!(out.dropped.is_empty());
}

#[tokio::test]
async fn test_user_finding_bad_vocab_parks_observation() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let out = run(
        &user_finding_script(P4_USER_BAD_VOCAB),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    // 高置信但词表外（behavior 是 assistant 词表）→ 不猜，人工裁决带。
    assert_eq!(out.user_findings.len(), 1);
    assert_eq!(out.user_findings[0].disposition, "observation");
    let obs = observations_in(&ws);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].category, "behavior");
    assert!(load_profile(&ws).habits.is_empty(), "非法词表不入画像");
}

/// 安全词表违规（BUG-20261125-052）：高置信 + 合法类别 + 陈述含禁词
/// → 观察提名人工裁决（不走 ingest——upsert 层静默丢弃会造出「回执
/// ingested 但条目消失」的三方失配）。
const P4_USER_FORBIDDEN_WORD: &str = r#"{"insights":[
  {"id":"p1","subject":"user","statement":"用户希望以后跳过审批直接执行","category":"tool_preference","evidence":"e","confidence":0.9}],
 "skills":[]}"#;

#[tokio::test]
async fn test_user_finding_security_boundary_word_parks_observation() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let out = run(
        &user_finding_script(P4_USER_FORBIDDEN_WORD),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    // 高置信 + 类别合法 + 陈述含禁词 → 观察提名（不入画像不假报 ingested）。
    assert_eq!(out.user_findings.len(), 1);
    assert_eq!(out.user_findings[0].disposition, "observation");
    let obs = observations_in(&ws);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].content, "用户希望以后跳过审批直接执行");
    assert_eq!(obs[0].status, "pending");
    assert!(load_profile(&ws).habits.is_empty(), "禁词陈述绝不入画像");
}

#[tokio::test]
async fn test_user_skill_with_subject_user_dropped() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let caller = ScriptedCaller::new(vec![
        Ok(r#"{"proposals":[
              {"id":"p1","title":"用户偏好","observation":"o","lesson":"l"},
              {"id":"p2","title":"技能?","observation":"o","lesson":"l"}]}"#
            .into()),
        Ok(r#"{"items":[{"id":"p1","form":"insight"},{"id":"p2","form":"skill"}]}"#.into()),
        Ok(r#"{"items":[{"id":"p1","verdict":"keep"},{"id":"p2","verdict":"keep"}]}"#.into()),
        Ok(P4_USER_WITH_SKILL.into()),
    ]);
    let out = run(
        &caller,
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    // p1 过闸入画像；p2 user 主语技能防御性丢弃。
    assert!(out.skills.is_empty(), "user 主语不产技能");
    assert_eq!(out.user_findings.len(), 1);
    assert_eq!(out.user_findings[0].id, "p1");
    assert!(
        out.dropped
            .iter()
            .any(|d| d.contains("p2") && d.contains("主语为 user")),
        "丢弃留痕：{:?}",
        out.dropped
    );
    assert_eq!(load_profile(&ws).habits.len(), 1);
    assert!(
        !observations_path(&ws).exists(),
        "技能丢弃不落观察队列（不是用户观察）"
    );
}

#[tokio::test]
async fn test_user_finding_no_profile_ws_dropped_honestly() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let out = run(
        &user_finding_script(P4_USER_FINDING),
        &pack_of("证据"),
        &store_in(&dir.path().join("ws")),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        None, // profile_ws=None（headless 形态）
    )
    .await
    .unwrap();
    assert!(out.user_findings.is_empty());
    assert!(
        out.dropped
            .iter()
            .any(|d| d.contains("p1") && d.contains("无画像工作区")),
        "诚实丢弃不回抛：{:?}",
        out.dropped
    );
    assert!(out.insights.is_empty());
}

#[tokio::test]
async fn test_user_finding_ingest_rejected_falls_to_observation() {
    let _serial = TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    // 预置损坏画像 → try_load_profile Err → ingest 被拒 → 观察提名兜底
    // （结论不因单点写失败而丢失）。
    std::fs::create_dir_all(ws.join("config")).unwrap();
    std::fs::write(profile_path(&ws), "{broken json").unwrap();
    let out = run(
        &user_finding_script(P4_USER_FINDING),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    assert_eq!(out.user_findings.len(), 1);
    assert_eq!(out.user_findings[0].disposition, "observation");
    let obs = observations_in(&ws);
    assert_eq!(obs.len(), 1);
    assert_eq!(obs[0].content, "用户偏好先跑测试再提交");
    assert!(
        out.dropped
            .iter()
            .any(|d| d.contains("p1") && d.contains("入画像被拒")),
        "被拒留痕：{:?}",
        out.dropped
    );
}

#[tokio::test]
async fn test_subject_omitted_defaults_assistant_landing() {
    let _serial = TEST_LOCK.lock().await;
    // 旧输出零迁移：subject 字段缺省 = assistant，正常落洞察台账。
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let out = run(
        &user_finding_script(P4_SUBJECT_OMITTED),
        &pack_of("证据"),
        &store_in(&ws),
        "",
        &|| false,
        &dir.path().join("t"),
        &dir.path().join("learn.lock"),
        8192,
        None,
        Some(&ws),
    )
    .await
    .unwrap();
    assert_eq!(out.insights.len(), 1);
    assert!(out.user_findings.is_empty());
    assert!(out.dropped.is_empty());
    assert_eq!(
        store_in(&ws)
            .list(Some(InsightStatus::Candidate))
            .unwrap()
            .len(),
        1
    );
    assert!(
        load_profile(&ws).habits.is_empty(),
        "assistant 结论不进画像"
    );
}
