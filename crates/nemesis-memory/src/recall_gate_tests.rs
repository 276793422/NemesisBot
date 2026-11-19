//! 召回质量闸（项2 P2）测试：mock judge 语义钉死（判定/fail-open/缓存/
//! 成本闸/截断/统计）+ 解析器 + prompt 形态 + MemoryManager 挂载面
//! （缺席直通 / 在场过滤 / 统计透出）。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::manager::MemoryManager;
use crate::recall_gate::{RecallGate, RecallJudgeLlm, judge_prompt, parse_judge_response};

/// 计数 mock：按预设脚本逐次返回 verdict；调用次数供缓存断言。
struct MockJudge {
    script: parking_lot::Mutex<Vec<Result<Vec<bool>, String>>>,
    calls: AtomicUsize,
}

impl MockJudge {
    fn new(script: Vec<Result<Vec<bool>, String>>) -> Arc<Self> {
        Arc::new(Self {
            script: parking_lot::Mutex::new(script),
            calls: AtomicUsize::new(0),
        })
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl RecallJudgeLlm for MockJudge {
    async fn judge(&self, _query: &str, candidates: &[String]) -> Result<Vec<bool>, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let mut script = self.script.lock();
        if script.is_empty() {
            return Err("mock script exhausted".to_string());
        }
        let v = script.remove(0);
        // 长度自适应：单元素 Ok 臂按候选数铺开（除非脚本显式给错长度）。
        match v {
            Ok(vs) if vs.len() == 1 && candidates.len() > 1 => Ok(vec![vs[0]; candidates.len()]),
            Ok(vs) if vs.len() == 1 => Ok(vs),
            other => other,
        }
    }
}

fn gate(judge: Arc<MockJudge>) -> Arc<RecallGate> {
    Arc::new(RecallGate::new(judge, 3, 0, 300))
}

fn cands(n: usize) -> Vec<String> {
    (1..=n).map(|i| format!("候选{i} 内容")).collect()
}

#[tokio::test]
async fn keeps_relevant_drops_irrelevant() {
    let judge = MockJudge::new(vec![Ok(vec![true, false, true, false])]);
    let g = gate(judge.clone());
    let out = g.filter("q", cands(4)).await;
    assert_eq!(out, vec!["候选1 内容", "候选3 内容"]);
    let s = g.stats();
    assert_eq!((s.recalled, s.kept, s.dropped, s.judge_calls), (4, 2, 2, 1));
    assert_eq!(s.errors, 0);
}

#[tokio::test]
async fn judge_error_fail_open_keeps_all() {
    let judge = MockJudge::new(vec![Err("provider down".into())]);
    let g = gate(judge.clone());
    let input = cands(4);
    let out = g.filter("q", input.clone()).await;
    assert_eq!(out, input, "fail-open 必须原样保留");
    assert_eq!(g.stats().errors, 1);
    assert_eq!(judge.calls(), 1);
}

#[tokio::test]
async fn length_mismatch_fail_open() {
    let judge = MockJudge::new(vec![Ok(vec![true, false])]); // 长度 2 ≠ 候选 4
    let g = gate(judge.clone());
    let input = cands(4);
    let out = g.filter("q", input.clone()).await;
    assert_eq!(out, input);
    assert_eq!(g.stats().errors, 1);
}

#[tokio::test]
async fn cache_reuses_verdict_without_recall() {
    let judge = MockJudge::new(vec![Ok(vec![true, false, true])]);
    let g = gate(judge.clone());
    let input = cands(3);
    let first = g.filter("same query", input.clone()).await;
    let second = g.filter("same query", input.clone()).await;
    assert_eq!(first, second);
    assert_eq!(first, vec!["候选1 内容", "候选3 内容"]);
    assert_eq!(judge.calls(), 1, "第二次必须走缓存不再调 LLM");
    let s = g.stats();
    assert_eq!((s.judge_calls, s.cache_hits), (1, 1));
}

#[tokio::test]
async fn different_query_or_candidates_miss_cache() {
    let judge = MockJudge::new(vec![Ok(vec![true, true, true]), Ok(vec![true, true, true])]);
    let g = gate(judge.clone());
    let input = cands(3);
    g.filter("query A", input.clone()).await;
    g.filter("query B", input.clone()).await; // query 不同
    assert_eq!(judge.calls(), 2);
    g.filter("query A", cands_rev()).await; // 候选集不同（顺序变）
    assert_eq!(judge.calls(), 3);
}

fn cands_rev() -> Vec<String> {
    let mut v = cands(3);
    v.reverse();
    v
}

#[tokio::test]
async fn cost_gate_bypasses_under_threshold() {
    let judge = MockJudge::new(vec![]); // 无脚本——被调即 Err
    let g = gate(judge.clone());
    let input = cands(2); // < min_candidates=3
    let out = g.filter("q", input.clone()).await;
    assert_eq!(out, input);
    assert_eq!(judge.calls(), 0, "成本闸直通不花 token");
    let s = g.stats();
    assert_eq!((s.skipped_cost_gate, s.recalled, s.judge_calls), (1, 0, 0));
}

#[tokio::test]
async fn max_items_truncates_kept_order_stable() {
    let judge = MockJudge::new(vec![Ok(vec![true, true, true, true])]);
    let g = RecallGate::new(judge, 3, 2, 300);
    let out = g.filter("q", cands(4)).await;
    assert_eq!(out, vec!["候选1 内容", "候选2 内容"]);
}

#[tokio::test]
async fn empty_candidates_short_circuit() {
    let judge = MockJudge::new(vec![]);
    let g = gate(judge.clone());
    let out = g.filter("q", Vec::new()).await;
    assert!(out.is_empty());
    assert_eq!(judge.calls(), 0);
}

#[tokio::test]
async fn all_irrelevant_is_legitimate_empty() {
    let judge = MockJudge::new(vec![Ok(vec![false, false, false])]);
    let g = gate(judge.clone());
    let out = g.filter("q", cands(3)).await;
    assert!(out.is_empty(), "全部判不相关 = 合法空集");
    let s = g.stats();
    assert_eq!((s.kept, s.dropped), (0, 3));
}

// -- 解析器 ---------------------------------------------------------------

#[test]
fn parse_plain_array() {
    assert_eq!(
        parse_judge_response("[true,false,true]").unwrap(),
        vec![true, false, true]
    );
}

#[test]
fn parse_with_surrounding_prose() {
    let raw = "好的，判定如下：\n[true, false]\n以上。";
    assert_eq!(parse_judge_response(raw).unwrap(), vec![true, false]);
}

#[test]
fn parse_failures_are_err() {
    assert!(parse_judge_response("没有数组").is_err());
    assert!(parse_judge_response("[true,").is_err());
    assert!(parse_judge_response("[1, 0]").is_err(), "非布尔元素 = Err");
}

// -- prompt ---------------------------------------------------------------

#[test]
fn prompt_lists_candidates_numbered() {
    let cands = vec!["昨天晴".to_string(), "明天雨".to_string()];
    let (system, user) = judge_prompt("用户问天气", &cands);
    assert!(system.contains("JSON"));
    assert!(user.contains("# 用户消息"));
    assert!(user.contains("1. 昨天晴"));
    assert!(user.contains("2. 明天雨"));
}

// -- MemoryManager 挂载面 ---------------------------------------------------

/// 装一个最小 manager（with_backends：存储后端与过滤路径无关）。
fn fresh_manager() -> MemoryManager {
    use crate::episodic::FileEpisodicStore;
    use crate::graph::InMemoryGraphStore;
    use crate::store::LocalStore;
    let dir = tempfile::tempdir().expect("tempdir");
    MemoryManager::with_backends(
        Arc::new(LocalStore::new()),
        Arc::new(FileEpisodicStore::new(dir.path().join("episodic"))),
        Arc::new(InMemoryGraphStore::new()),
    )
}

/// 闸缺席 = filter_recall_hits 直通现状（默认关，注入面字节不变）。
#[tokio::test]
async fn manager_passthrough_without_gate() {
    let mgr = fresh_manager();
    let input = cands(4);
    let out = mgr.filter_recall_hits("q", input.clone()).await;
    assert_eq!(out, input);
    assert!(mgr.recall_gate_stats().is_none(), "闸缺席无统计");
}

/// 闸在场 = 过滤生效 + 统计透出（recall_gate_stats 与 gate.stats 同源）。
#[tokio::test]
async fn manager_gate_filters_and_reports_stats() {
    let mgr = fresh_manager();
    let g = gate(MockJudge::new(vec![Ok(vec![true, false, true, false])]));
    mgr.set_recall_gate(g.clone());
    let out = mgr.filter_recall_hits("q", cands(4)).await;
    assert_eq!(out, vec!["候选1 内容", "候选3 内容"]);
    let s = mgr.recall_gate_stats().expect("stats after gate set");
    assert_eq!((s.recalled, s.kept, s.dropped), (4, 2, 2));
    assert_eq!(s.judge_calls, 1);
}

/// 二次调用走缓存（manager 持有的 gate 与外部 Arc 同实例——统计连续）。
#[tokio::test]
async fn manager_gate_cache_hit_second_call() {
    let mgr = fresh_manager();
    let judge = MockJudge::new(vec![Ok(vec![true, true, true])]);
    mgr.set_recall_gate(gate(judge.clone()));
    let input = cands(3);
    mgr.filter_recall_hits("same", input.clone()).await;
    mgr.filter_recall_hits("same", input.clone()).await;
    assert_eq!(judge.calls(), 1, "同 query+候选集第二次走缓存");
    let s = mgr.recall_gate_stats().expect("stats");
    assert_eq!((s.judge_calls, s.cache_hits), (1, 1));
}

// -- 全链：向量召回 → 闸过滤（对齐 loop prefetch_memory_context 语义）------

/// 全链测试：init_vector_store_with_embed（ngram fixture embed，不依赖
/// plugin_onnx.dll）→ search_auto_inject 召回 → filter_recall_hits 过滤。
/// 正交条目低于 0.35 阈值不进召回（注入候选天然相关）；闸再丢弃其中
/// 判定为不相关的部分。
#[tokio::test]
async fn full_chain_vector_recall_then_gate_filter() {
    use crate::manager::Config;
    use crate::types::{Entry, MemoryType};
    use crate::vector::{EmbeddingFunc, StoreConfig};

    let dir = tempfile::tempdir().expect("tempdir");
    let config = Config::new(dir.path());
    let mgr = MemoryManager::new(&config);
    let embed: EmbeddingFunc = Box::new(|text: &str| {
        if text.starts_with("match") {
            Ok(vec![1.0, 0.0])
        } else {
            Ok(vec![0.0, 1.0])
        }
    });
    let vs_config = StoreConfig {
        embedding_tier: "ngram".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.7,
        storage_path: dir.path().join("v.jsonl").to_string_lossy().to_string(),
    };
    mgr.init_vector_store_with_embed(embed, vs_config).unwrap();
    mgr.set_vector_enabled(true);

    // 4 条相关 + 1 条正交（低于 0.35 阈值不进召回候选）。
    for i in 1..=4 {
        mgr.store_entry(Entry::new(MemoryType::LongTerm, format!("match fact {i}")))
            .await
            .expect("store");
    }
    mgr.store_entry(Entry::new(
        MemoryType::LongTerm,
        "totally orthogonal noise".to_string(),
    ))
    .await
    .expect("store");

    let hits = mgr
        .search_auto_inject("match anything", 6)
        .await
        .expect("search");
    assert_eq!(hits.total, 4, "召回 4 条相关（正交条目低于阈值不进）");

    // 闸装配：保留第 1/3 条，丢弃第 2/4 条。
    mgr.set_recall_gate(gate(MockJudge::new(vec![Ok(vec![
        true, false, true, false,
    ])])));
    let texts: Vec<String> = hits
        .entries
        .iter()
        .map(|e| e.entry.content.clone())
        .collect();
    let kept = mgr.filter_recall_hits("match anything", texts).await;
    assert_eq!(kept.len(), 2, "闸后只剩判定相关的 2 条");
    let s = mgr.recall_gate_stats().expect("stats");
    assert_eq!((s.recalled, s.kept, s.dropped), (4, 2, 2));
    assert_eq!(s.judge_calls, 1);
}
