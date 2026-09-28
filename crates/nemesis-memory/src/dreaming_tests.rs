//! P31 记忆 dreaming 单元测试（实现要求 1/2/3/5）：
//! - 信号打分表钉死（纯函数精确值）
//! - 幂等召回记账（同轮不重复计数 + 跨 TfIdf 重载持久化）
//! - LLM 决策到宿主产出的边界闸（mock LLM：越界产正文 / 未知 id / 非法动作 → 宿主拒绝）
//! - sweep 闭环一轮（mock LLM + 临时工作区，无真实 gateway）

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{Duration, Local};
use serde_json::json;
use tempfile::tempdir;

use crate::dreaming::{
    DUP_JACCARD, DreamAction, DreamDecision, DreamingLlm, DreamingWeights, META_DREAM_ARCHIVED,
    META_DREAM_MERGED_FROM, META_DREAM_PROCESSED_AT, META_DREAM_SOURCE_RELIABILITY, RelSignals,
    SweepReport, apply_decision, build_merged_content, build_signals, compute_relationship_signals,
    is_dream_settled, jaccard, parse_decisions, run_sweep, score_entry, select_candidates,
    signal_age, signal_conflict, signal_decay, signal_recall, signal_redundancy, signal_source,
    validate_decision,
};
use crate::manager::{Config, META_LAST_RECALL, META_RECALL_COUNT, MemoryManager};
use crate::types::{Entry, MemoryType};

/// 浮点近似断言（打分表钉死用，1e-9 容差）。
fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "打分值不匹配: expected {b}, got {a}");
}

/// 新建一条短期记忆条目（自动 uuid id）。
fn entry(content: &str) -> Entry {
    Entry::new(MemoryType::ShortTerm, content.to_string())
}

/// 内存态 MemoryManager（LocalStore，无 vector）。
fn mem_manager() -> MemoryManager {
    let dir = tempdir().expect("tempdir");
    MemoryManager::new(&Config::new(dir.path()))
}

// ---------------------------------------------------------------------------
// 6 信号打分表（P31 ②纯函数，值钉死）
// ---------------------------------------------------------------------------

#[test]
fn signal_recall_saturation_table() {
    approx(signal_recall(0), 0.0);
    approx(signal_recall(1), 0.5);
    approx(signal_recall(4), 0.8);
    approx(signal_recall(9), 0.9);
    approx(signal_recall(99), 0.99);
}

#[test]
fn signal_decay_conflict_redundancy_age_source_table() {
    // 衰减：90 天饱和
    approx(signal_decay(0.0), 0.0);
    approx(signal_decay(45.0), 0.5);
    approx(signal_decay(90.0), 1.0);
    approx(signal_decay(180.0), 1.0);
    // 冲突 / 冗余：3 个饱和
    approx(signal_conflict(0), 0.0);
    approx(signal_conflict(1), 1.0 / 3.0);
    approx(signal_conflict(3), 1.0);
    approx(signal_conflict(5), 1.0);
    approx(signal_redundancy(0), 0.0);
    approx(signal_redundancy(3), 1.0);
    // 年龄：180 天饱和
    approx(signal_age(0.0), 0.0);
    approx(signal_age(90.0), 0.5);
    approx(signal_age(180.0), 1.0);
    // 来源可靠性：1 - reliability
    approx(signal_source(0.0), 1.0);
    approx(signal_source(0.5), 0.5);
    approx(signal_source(1.0), 0.0);
}

#[test]
fn score_entry_default_weights_pinned() {
    let w = DreamingWeights::default();
    // 基线：全零信号 + 默认可靠性 0.5 → 只有来源信号贡献 0.10*0.5
    let base = crate::dreaming::DreamSignals {
        recall_count: 0,
        days_since_last_recall: 0.0,
        duplicate_count: 0,
        conflict_count: 0,
        age_days: 0.0,
        source_reliability: 0.5,
    };
    approx(score_entry(&base, &w), 0.05);

    // 召回 9 次：0.25*0.9 + 0.05
    let s = crate::dreaming::DreamSignals {
        recall_count: 9,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.275);

    // 召回 99 次：0.25*0.99 + 0.05
    let s = crate::dreaming::DreamSignals {
        recall_count: 99,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.2975);

    // 90 天未召回：0.20*1 + 0.05
    let s = crate::dreaming::DreamSignals {
        days_since_last_recall: 90.0,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.25);

    // 冲突 3 + 冗余 6：0.20 + 0.15 + 0.05
    let s = crate::dreaming::DreamSignals {
        conflict_count: 3,
        duplicate_count: 6,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.40);

    // 年龄 180 天：0.10 + 0.05
    let s = crate::dreaming::DreamSignals {
        age_days: 180.0,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.15);

    // 来源可靠性 0（模型自产）：来源信号独自贡献 0.10*1.0（0.05 基线本就
    // 来自 reliability=0.5，这里整体替换，不存在叠加）
    let s = crate::dreaming::DreamSignals {
        source_reliability: 0.0,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.10);

    // 来源可靠性 1（用户明示）：来源信号归零
    let s = crate::dreaming::DreamSignals {
        source_reliability: 1.0,
        ..base.clone()
    };
    approx(score_entry(&s, &w), 0.0);
}

#[test]
fn score_entry_zero_weights_returns_zero() {
    let w = DreamingWeights {
        recall: 0.0,
        decay: 0.0,
        conflict: 0.0,
        redundancy: 0.0,
        age: 0.0,
        source: 0.0,
    };
    let s = crate::dreaming::DreamSignals {
        recall_count: 50,
        days_since_last_recall: 500.0,
        conflict_count: 9,
        duplicate_count: 9,
        age_days: 900.0,
        source_reliability: 0.0,
    };
    approx(score_entry(&s, &w), 0.0);
}

#[test]
fn build_signals_reads_metadata_and_falls_back_to_age() {
    let now = Local::now();
    // 有记账：last_recall 3 天前、recall_count=7
    let mut e = entry("some fact");
    e.created_at = now - Duration::days(10);
    e.metadata
        .insert(META_RECALL_COUNT.to_string(), "7".to_string());
    e.metadata.insert(
        META_LAST_RECALL.to_string(),
        (now - Duration::days(3)).to_rfc3339(),
    );
    let sig = build_signals(&e, &RelSignals::default(), now);
    approx(sig.days_since_last_recall, 3.0);
    assert_eq!(sig.recall_count, 7);
    approx(sig.age_days, 10.0);
    approx(sig.source_reliability, 0.5); // 缺省可靠性

    // 从未召回：回落到条目年龄
    let mut e2 = entry("other fact");
    e2.created_at = now - Duration::days(30);
    let sig2 = build_signals(&e2, &RelSignals::default(), now);
    approx(sig2.days_since_last_recall, 30.0);
    assert_eq!(sig2.recall_count, 0);

    // 关系信号透传
    let sig3 = build_signals(
        &e2,
        &RelSignals {
            duplicate_count: 2,
            conflict_count: 1,
        },
        now,
    );
    assert_eq!(sig3.duplicate_count, 2);
    assert_eq!(sig3.conflict_count, 1);
}

// ---------------------------------------------------------------------------
// 词面关系启发式（冲突 / 冗余信号）
// ---------------------------------------------------------------------------

#[test]
fn jaccard_and_relationship_signals() {
    let empty: HashSet<String> = HashSet::new();
    approx(jaccard(&empty, &empty), 0.0);

    // 相同词集 → 1.0（>= DUP_JACCARD）
    let a: HashSet<String> = ["deploy", "bot"].iter().map(|s| s.to_string()).collect();
    let b: HashSet<String> = ["deploy", "bot", "x"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert!(jaccard(&a, &b) < DUP_JACCARD);

    let e_dup1 = entry("deploy bot on port 8080");
    let e_dup2 = entry("deploy bot on port 8080"); // 完全相同 → dup
    let e_c1 = entry("release v2 adds dark mode");
    let e_c2 = entry("release v2 removes light mode"); // 部分重叠 → 疑似冲突
    let e_far = entry("quarterly budget numbers");

    let entries = vec![
        e_dup1.clone(),
        e_dup2.clone(),
        e_c1.clone(),
        e_c2.clone(),
        e_far.clone(),
    ];
    let rel = compute_relationship_signals(&entries);

    // 完全相同的两条互记 duplicate_count=1（对称）
    assert_eq!(rel[&e_dup1.id].duplicate_count, 1);
    assert_eq!(rel[&e_dup2.id].duplicate_count, 1);
    assert_eq!(rel[&e_dup1.id].conflict_count, 0);
    // 部分重叠（0.4 <= j < 0.9）互记 conflict_count=1
    assert_eq!(rel[&e_c1.id].conflict_count, 1);
    assert_eq!(rel[&e_c2.id].conflict_count, 1);
    assert_eq!(rel[&e_c1.id].duplicate_count, 0);
    // 无关条目无任何关系信号
    assert_eq!(rel.get(&e_far.id), None);
}

// ---------------------------------------------------------------------------
// 候选选择
// ---------------------------------------------------------------------------

#[test]
fn select_candidates_excludes_settled_sorts_and_truncates() {
    let now = Local::now();
    let w = DreamingWeights::default();

    let mut processed = entry("old processed memory");
    processed.created_at = now - Duration::days(400);
    processed
        .metadata
        .insert(META_DREAM_PROCESSED_AT.to_string(), now.to_rfc3339());

    let mut archived = entry("archived memory");
    archived
        .metadata
        .insert(META_DREAM_ARCHIVED.to_string(), "true".to_string());

    let fresh = entry("brand new memory");
    let mut stale = entry("stale ancient memory");
    stale.created_at = now - Duration::days(200); // 从未召回 → decay/age 双饱和

    let candidates = select_candidates(
        vec![processed, archived, fresh.clone(), stale.clone()],
        &w,
        2,
        now,
    );

    // 已处理/已归档被排除；陈旧条目排最前；top_k=2 截断
    assert_eq!(candidates.len(), 2);
    assert_eq!(candidates[0].entry.id, stale.id);
    assert_eq!(candidates[1].entry.id, fresh.id);
    assert!(candidates[0].score > candidates[1].score);
}

#[test]
fn is_dream_settled_detects_both_markers() {
    let mut e = entry("x");
    assert!(!is_dream_settled(&e));
    e.metadata.insert(
        META_DREAM_PROCESSED_AT.to_string(),
        Local::now().to_rfc3339(),
    );
    assert!(is_dream_settled(&e));
    let mut e2 = entry("y");
    e2.metadata
        .insert(META_DREAM_ARCHIVED.to_string(), "true".to_string());
    assert!(is_dream_settled(&e2));
}

// ---------------------------------------------------------------------------
// 召回记账（P31 ①）幂等 + 持久化
// ---------------------------------------------------------------------------

#[tokio::test]
async fn record_recall_idempotent_same_turn() {
    let mgr = mem_manager();
    let id = mgr.store_entry(entry("alpha beta gamma")).await.unwrap();

    // 同一 (session, turn) 两次记账 → 只计一次
    let touched = mgr
        .record_recall(std::slice::from_ref(&id), "sess-a", "turn-1")
        .await;
    assert_eq!(touched, 1);
    let touched = mgr
        .record_recall(std::slice::from_ref(&id), "sess-a", "turn-1")
        .await;
    assert_eq!(touched, 0);

    let e = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_RECALL_COUNT).unwrap(), "1");
    assert!(e.metadata.contains_key(META_LAST_RECALL));
}

#[tokio::test]
async fn record_recall_counts_distinct_turns_and_sessions() {
    let mgr = mem_manager();
    let id = mgr.store_entry(entry("alpha beta gamma")).await.unwrap();

    mgr.record_recall(std::slice::from_ref(&id), "sess-a", "turn-1")
        .await;
    mgr.record_recall(std::slice::from_ref(&id), "sess-a", "turn-2")
        .await;
    mgr.record_recall(std::slice::from_ref(&id), "sess-b", "turn-1")
        .await;

    let e = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_RECALL_COUNT).unwrap(), "3");

    // 最近一次 touch 的同 (session, turn) 重放 → 幂等跳过（连续重放保护；
    // 诚实边界：last_recall_turn 是单槽标记，只记住「最近一次」——更早轮次
    // 的重放若夹在其他轮次 touch 之后会再计一次。真实接线（每次检索独立
    // turn token / loop 同轮共享 token）不产生这种交错重放。）
    mgr.record_recall(std::slice::from_ref(&id), "sess-b", "turn-1")
        .await;
    let e = mgr.get(&id).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_RECALL_COUNT).unwrap(), "3");
}

#[tokio::test]
async fn record_recall_persists_across_tfidf_reload() {
    let dir = tempdir().expect("tempdir");
    let cfg = Config::new(dir.path());

    let mgr = MemoryManager::new_with_jsonl(&cfg).await.unwrap();
    let id = mgr.store_entry(entry("persistent fact one")).await.unwrap();
    mgr.record_recall(std::slice::from_ref(&id), "s", "t1")
        .await;
    mgr.record_recall(std::slice::from_ref(&id), "s", "t2")
        .await;
    drop(mgr);

    // 重载后记账仍在（标记随条目持久化 → 崩溃重启后依旧幂等）
    let mgr2 = MemoryManager::new_with_jsonl(&cfg).await.unwrap();
    let e = mgr2.get(&id).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_RECALL_COUNT).unwrap(), "2");

    // 重载后同 turn 再记账不重复计数（幂等跨运行成立）
    let touched = mgr2
        .record_recall(std::slice::from_ref(&id), "s", "t2")
        .await;
    assert_eq!(touched, 0);
    let e = mgr2.get(&id).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_RECALL_COUNT).unwrap(), "2");
}

#[tokio::test]
async fn search_with_recall_touches_hits() {
    let mgr = mem_manager();
    mgr.store_entry(entry("deploy bot on port 8080"))
        .await
        .unwrap();
    mgr.store_entry(entry("cooking recipe for lasagna"))
        .await
        .unwrap();

    let result = mgr
        .search_with_recall("deploy", None, 10, "sess", "turn-9")
        .await
        .unwrap();
    assert!(!result.entries.is_empty());
    // 同 turn 重复检索 → 不重复计数
    mgr.search_with_recall("deploy", None, 10, "sess", "turn-9")
        .await
        .unwrap();

    let all = mgr.list_all_entries().await.unwrap();
    let touched: Vec<&Entry> = all
        .iter()
        .filter(|e| e.metadata.contains_key(META_RECALL_COUNT))
        .collect();
    assert_eq!(touched.len(), 1);
    assert_eq!(touched[0].metadata.get(META_RECALL_COUNT).unwrap(), "1");
}

// ---------------------------------------------------------------------------
// LLM 决策协议解析 + 校验边界闸（P31 ③边界）
// ---------------------------------------------------------------------------

/// 测试用 mock LLM：返回固定响应，记录调用次数。
struct MockDreamLlm {
    response: String,
    calls: Arc<AtomicUsize>,
}

impl MockDreamLlm {
    fn new(response: impl Into<String>) -> (Self, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Self {
                response: response.into(),
                calls: Arc::clone(&calls),
            },
            calls,
        )
    }
}

#[async_trait::async_trait]
impl DreamingLlm for MockDreamLlm {
    async fn decide(&self, _system_prompt: &str, _user_prompt: &str) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

/// 报错型 mock：LLM 通道故障。
struct FailingDreamLlm;

#[async_trait::async_trait]
impl DreamingLlm for FailingDreamLlm {
    async fn decide(&self, _system_prompt: &str, _user_prompt: &str) -> Result<String, String> {
        Err("llm channel down".to_string())
    }
}

#[test]
fn parse_decisions_strips_code_fence() {
    let raw = "```json\n{\"decisions\": [{\"action\": \"keep\", \"source_ids\": [\"a\"]}]}\n```";
    let parsed = parse_decisions(raw).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0]["action"], "keep");

    // 裸 JSON 也行
    let parsed = parse_decisions("{\"decisions\": []}").unwrap();
    assert!(parsed.is_empty());
}

#[test]
fn parse_decisions_rejects_missing_decisions_array() {
    assert!(parse_decisions("not json at all").is_err());
    assert!(parse_decisions("{\"foo\": 1}").is_err());
    assert!(parse_decisions("{\"decisions\": \"not-an-array\"}").is_err());
}

#[test]
fn validate_decision_accepts_four_actions() {
    let known: HashSet<String> = ["a".to_string(), "b".to_string()].into();

    let d = validate_decision(
        &json!({"action": "merge", "source_ids": ["a", "b"], "reason": "重复"}),
        &known,
    )
    .unwrap();
    assert_eq!(d.action, DreamAction::Merge);
    assert_eq!(d.source_ids, vec!["a", "b"]);
    assert_eq!(d.reason, "重复");

    for (action, expected) in [
        ("promote", DreamAction::Promote),
        ("expire", DreamAction::Expire),
        ("keep", DreamAction::Keep),
    ] {
        let d = validate_decision(&json!({"action": action, "source_ids": ["a"]}), &known).unwrap();
        assert_eq!(d.action, expected);
    }
}

#[test]
fn validate_decision_rejects_content_field() {
    // 边界闸核心：模型试图产正文 → 整条拒绝（模型只决策，宿主产出）
    let known: HashSet<String> = ["a".to_string()].into();
    let err = validate_decision(
        &json!({
            "action": "promote",
            "source_ids": ["a"],
            "content": "这是模型越界写的正文"
        }),
        &known,
    )
    .unwrap_err();
    assert!(err.contains("越界"), "实际错误: {err}");

    let err = validate_decision(
        &json!({
            "action": "merge",
            "source_ids": ["a", "b"],
            "new_content": "合并后的新正文"
        }),
        &HashSet::new(),
    )
    .unwrap_err();
    assert!(err.contains("越界"), "实际错误: {err}");
}

#[test]
fn validate_decision_rejects_unknown_field_action_and_id() {
    let known: HashSet<String> = ["a".to_string()].into();

    // 未知字段
    let err = validate_decision(
        &json!({"action": "keep", "source_ids": ["a"], "priority": 1}),
        &known,
    )
    .unwrap_err();
    assert!(err.contains("未知字段"), "实际错误: {err}");

    // 未知 action
    let err = validate_decision(
        &json!({"action": "delete_all", "source_ids": ["a"]}),
        &known,
    )
    .unwrap_err();
    assert!(err.contains("未知 action"), "实际错误: {err}");

    // 引用不存在的条目 id
    let err = validate_decision(
        &json!({"action": "expire", "source_ids": ["ghost-id"]}),
        &known,
    )
    .unwrap_err();
    assert!(err.contains("不存在的条目"), "实际错误: {err}");

    // 空 source_ids
    let err = validate_decision(&json!({"action": "keep", "source_ids": []}), &known).unwrap_err();
    assert!(err.contains("为空"), "实际错误: {err}");
}

#[test]
fn validate_decision_rejects_wrong_arity() {
    let known: HashSet<String> = ["a".to_string(), "b".to_string()].into();

    // merge 至少 2 条源
    let err =
        validate_decision(&json!({"action": "merge", "source_ids": ["a"]}), &known).unwrap_err();
    assert!(err.contains("至少 2 条源"), "实际错误: {err}");

    // 单源动作只接受 1 条源
    let err = validate_decision(
        &json!({"action": "promote", "source_ids": ["a", "b"]}),
        &known,
    )
    .unwrap_err();
    assert!(err.contains("只接受 1 条源"), "实际错误: {err}");
}

// ---------------------------------------------------------------------------
// 宿主产出（merge 拼接去重 / apply 幂等）
// ---------------------------------------------------------------------------

#[test]
fn build_merged_content_dedups_lines() {
    let mut a = entry("deploy bot on port 8080\nuses tls");
    a.id = "a".to_string();
    let mut b = entry("Deploy Bot On Port 8080\nuses grpc");
    b.id = "b".to_string();

    let merged = build_merged_content(&[a, b]);
    // 归一后相同的行去重；不同行保留；顺序 = 源出现序
    assert_eq!(merged, "deploy bot on port 8080\nuses tls\nuses grpc");
}

#[tokio::test]
async fn apply_decision_merge_creates_merged_entry_and_marks_sources() {
    let mgr = mem_manager();
    let mut a = entry("deploy bot on port 8080\nuses tls");
    a.tags = vec!["net".to_string()];
    let mut b = entry("Deploy Bot On Port 8080\nuses grpc");
    b.tags = vec!["ops".to_string(), "net".to_string()];
    let id_a = mgr.store_entry(a).await.unwrap();
    let id_b = mgr.store_entry(b).await.unwrap();

    let decision = DreamDecision {
        action: DreamAction::Merge,
        source_ids: vec![id_a.clone(), id_b.clone()],
        reason: "重复条目".to_string(),
    };
    let applied = apply_decision(&mgr, &decision, Local::now()).await.unwrap();
    assert_eq!(applied, "merge");

    // 新条目：LongTerm + dream_merged_from + tags 并集 + 拼接去重正文
    let all = mgr.list_all_entries().await.unwrap();
    assert_eq!(all.len(), 3);
    let merged = all
        .iter()
        .find(|e| e.metadata.contains_key(META_DREAM_MERGED_FROM))
        .expect("应存在 merge 产物");
    assert_eq!(merged.typ, MemoryType::LongTerm);
    assert_eq!(
        merged.metadata.get(META_DREAM_MERGED_FROM).unwrap(),
        &format!("{id_a},{id_b}")
    );
    assert_eq!(merged.tags, vec!["net", "ops"]);
    assert_eq!(
        merged.content,
        "deploy bot on port 8080\nuses tls\nuses grpc"
    );

    // 源条目：打 processed 标记（幂等防重复处理），本体保留
    for id in [&id_a, &id_b] {
        let e = mgr.get(id).await.unwrap().unwrap();
        assert!(e.metadata.contains_key(META_DREAM_PROCESSED_AT));
    }
}

#[tokio::test]
async fn apply_decision_promote_expire_keep() {
    let mgr = mem_manager();

    // promote：tier 提升 → long_term
    let id_p = mgr
        .store_entry(entry("short note worth keeping"))
        .await
        .unwrap();
    let applied = apply_decision(
        &mgr,
        &DreamDecision {
            action: DreamAction::Promote,
            source_ids: vec![id_p.clone()],
            reason: String::new(),
        },
        Local::now(),
    )
    .await
    .unwrap();
    assert_eq!(applied, "promote");
    let e = mgr.get(&id_p).await.unwrap().unwrap();
    assert_eq!(e.typ, MemoryType::LongTerm);
    assert!(e.metadata.contains_key(META_DREAM_PROCESSED_AT));

    // expire：归档标记 + 不物理删除（条目仍可查证）
    let id_e = mgr.store_entry(entry("obsolete fact")).await.unwrap();
    let applied = apply_decision(
        &mgr,
        &DreamDecision {
            action: DreamAction::Expire,
            source_ids: vec![id_e.clone()],
            reason: "过时".to_string(),
        },
        Local::now(),
    )
    .await
    .unwrap();
    assert_eq!(applied, "expire");
    let e = mgr.get(&id_e).await.unwrap().unwrap();
    assert_eq!(e.metadata.get(META_DREAM_ARCHIVED).unwrap(), "true");
    assert!(e.metadata.contains_key(META_DREAM_PROCESSED_AT));

    // keep：仅打 processed 标记
    let id_k = mgr.store_entry(entry("still valid")).await.unwrap();
    let applied = apply_decision(
        &mgr,
        &DreamDecision {
            action: DreamAction::Keep,
            source_ids: vec![id_k.clone()],
            reason: String::new(),
        },
        Local::now(),
    )
    .await
    .unwrap();
    assert_eq!(applied, "keep");
    let e = mgr.get(&id_k).await.unwrap().unwrap();
    assert!(e.metadata.contains_key(META_DREAM_PROCESSED_AT));
    assert!(!e.metadata.contains_key(META_DREAM_ARCHIVED));
}

#[tokio::test]
async fn apply_decision_rejects_missing_source_toctou() {
    let mgr = mem_manager();
    // 校验时 id 在 known 集合里，但应用时条目已不存在（TOCTOU 复核）
    let decision = DreamDecision {
        action: DreamAction::Promote,
        source_ids: vec!["ghost-id".to_string()],
        reason: String::new(),
    };
    let err = apply_decision(&mgr, &decision, Local::now())
        .await
        .unwrap_err();
    assert!(err.contains("已不存在"), "实际错误: {err}");
}

#[tokio::test]
async fn apply_decision_rejects_already_settled() {
    let mgr = mem_manager();
    let mut e = entry("already handled");
    e.metadata.insert(
        META_DREAM_PROCESSED_AT.to_string(),
        Local::now().to_rfc3339(),
    );
    let id = mgr.store_entry(e).await.unwrap();

    let decision = DreamDecision {
        action: DreamAction::Expire,
        source_ids: vec![id],
        reason: String::new(),
    };
    let err = apply_decision(&mgr, &decision, Local::now())
        .await
        .unwrap_err();
    assert!(err.contains("已被处理过"), "实际错误: {err}");
}

// ---------------------------------------------------------------------------
// sweep 闭环（P31 ②+③ 一轮，mock LLM + 临时工作区）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_sweep_closed_loop_merge_expire_and_reject() {
    let mgr = mem_manager();

    // 布景：2 条近似重复 + 1 条陈旧孤例
    let a = entry("deploy bot on port 8080\nuses tls");
    let b = entry("Deploy Bot On Port 8080\nuses grpc");
    let mut c = entry("quarterly budget numbers");
    c.created_at = Local::now() - Duration::days(200);
    let id_a = mgr.store_entry(a).await.unwrap();
    let id_b = mgr.store_entry(b).await.unwrap();
    let id_c = mgr.store_entry(c).await.unwrap();

    // LLM 决策：merge A+B + expire C + 一条越界决策（试图产正文）
    let response = format!(
        r#"{{"decisions": [
            {{"action": "merge", "source_ids": ["{id_a}", "{id_b}"], "reason": "重复"}},
            {{"action": "expire", "source_ids": ["{id_c}"], "reason": "过时"}},
            {{"action": "promote", "source_ids": ["{id_a}"], "content": "越界正文"}}
        ]}}"#
    );
    let (llm, calls) = MockDreamLlm::new(response);

    let report = run_sweep(&mgr, &llm, &DreamingWeights::default(), 8, Local::now())
        .await
        .unwrap();

    // 候选 = 3 条（无一被处理过）；3 决策返回，1 越界拒绝，2 应用
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.total_entries, 3);
    assert_eq!(report.candidates, 3);
    assert_eq!(report.decisions_returned, 3);
    assert_eq!(report.decisions_applied, 2);
    assert_eq!(report.decisions_rejected, 1);
    assert_eq!(report.merged, 1);
    assert_eq!(report.expired, 1);
    assert!(report.notes.iter().any(|n| n.contains("越界")));

    // merge 产物落库
    let all = mgr.list_all_entries().await.unwrap();
    assert_eq!(all.len(), 4);
    let merged = all
        .iter()
        .find(|e| e.metadata.contains_key(META_DREAM_MERGED_FROM))
        .unwrap();
    assert_eq!(merged.typ, MemoryType::LongTerm);

    // 三条源全部打 processed 标记
    for id in [&id_a, &id_b, &id_c] {
        let e = mgr.get(id).await.unwrap().unwrap();
        assert!(e.metadata.contains_key(META_DREAM_PROCESSED_AT));
    }
}

#[tokio::test]
async fn run_sweep_second_round_idempotent() {
    let mgr = mem_manager();
    let a = entry("deploy bot on port 8080\nuses tls");
    let b = entry("Deploy Bot On Port 8080\nuses grpc");
    let id_a = mgr.store_entry(a).await.unwrap();
    let id_b = mgr.store_entry(b).await.unwrap();

    let response = format!(
        r#"{{"decisions": [{{"action": "merge", "source_ids": ["{id_a}", "{id_b}"], "reason": "重复"}}]}}"#
    );
    let (llm, _) = MockDreamLlm::new(response);
    let report = run_sweep(&mgr, &llm, &DreamingWeights::default(), 8, Local::now())
        .await
        .unwrap();
    assert_eq!(report.merged, 1);

    // 第二轮：空决策。源条目已被处理不再进候选 → 候选只剩 merge 产物
    let (llm2, calls2) = MockDreamLlm::new(r#"{"decisions": []}"#);
    let report2 = run_sweep(&mgr, &llm2, &DreamingWeights::default(), 8, Local::now())
        .await
        .unwrap();
    assert_eq!(calls2.load(Ordering::SeqCst), 1);
    assert_eq!(report2.candidates, 1);
    assert_eq!(report2.decisions_returned, 0);
    assert_eq!(report2.decisions_applied, 0);

    // 产物本身没有 processed 标记（下一轮仍可被审视），
    // 且 merged_from 指向已处理的源
    let all = mgr.list_all_entries().await.unwrap();
    let merged = all
        .iter()
        .find(|e| e.metadata.contains_key(META_DREAM_MERGED_FROM))
        .unwrap();
    assert!(!is_dream_settled(merged));
    assert_eq!(
        merged.metadata.get(META_DREAM_MERGED_FROM).unwrap(),
        &format!("{id_a},{id_b}")
    );
}

#[tokio::test]
async fn run_sweep_llm_error_propagates() {
    let mgr = mem_manager();
    mgr.store_entry(entry("some memory")).await.unwrap();

    let result = run_sweep(
        &mgr,
        &FailingDreamLlm,
        &DreamingWeights::default(),
        8,
        Local::now(),
    )
    .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("llm channel down"));
}

#[tokio::test]
async fn run_sweep_no_candidates_skips_llm() {
    let mgr = mem_manager();
    // 空库：无候选 → LLM 不被调用，报告全零
    let (llm, calls) = MockDreamLlm::new(r#"{"decisions": []}"#);
    let report: SweepReport = run_sweep(&mgr, &llm, &DreamingWeights::default(), 8, Local::now())
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(report.total_entries, 0);
    assert_eq!(report.candidates, 0);

    // 全部已处理：同样跳过 LLM
    let mut e = entry("handled already");
    e.metadata.insert(
        META_DREAM_PROCESSED_AT.to_string(),
        Local::now().to_rfc3339(),
    );
    mgr.store_entry(e).await.unwrap();
    let (llm2, calls2) = MockDreamLlm::new(r#"{"decisions": []}"#);
    let report2 = run_sweep(&mgr, &llm2, &DreamingWeights::default(), 8, Local::now())
        .await
        .unwrap();
    assert_eq!(calls2.load(Ordering::SeqCst), 0);
    assert_eq!(report2.candidates, 0);
}

// ---------------------------------------------------------------------------
// 决策提示词（LLM 协议契约面）
// ---------------------------------------------------------------------------

#[test]
fn decision_prompt_contains_contract() {
    use crate::dreaming::{build_decision_user_prompt, decision_system_prompt};
    let system = decision_system_prompt();
    // 系统提示必须显式禁止产正文 + 给出 JSON 协议
    assert!(system.contains("不能输出任何记忆正文"));
    assert!(system.contains("decisions"));

    let now = Local::now();
    let mut e = entry("candidate content here");
    e.created_at = now - Duration::days(5);
    e.metadata
        .insert(META_RECALL_COUNT.to_string(), "3".to_string());
    e.metadata
        .insert(META_DREAM_SOURCE_RELIABILITY.to_string(), "0.5".to_string());
    let sig = build_signals(
        &e,
        &RelSignals {
            duplicate_count: 1,
            conflict_count: 0,
        },
        now,
    );
    let score = score_entry(&sig, &DreamingWeights::default());
    let user = build_decision_user_prompt(&[crate::dreaming::DreamCandidate {
        entry: e.clone(),
        score,
        signals: sig,
    }]);
    // 候选清单必须带 id、正文截断与证据信号
    assert!(user.contains(&e.id));
    assert!(user.contains("candidate content here"));
    assert!(user.contains("疑似重复条目数: 1"));
}
