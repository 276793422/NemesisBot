//! 能力扩展 P34：指纹纯逻辑单测（任务类型分桶 / 三档判定边界 / 稳定加权
//! 排序）+ BoardStore 记账幂等与读取单测。决策表接入的端到端行为（成功率
//! 影响换节点人选、决策评论带档位）在 nemesisbot `board_review::tests` 的
//! P34 用例覆盖；双节点真流验收挂账 cluster-uat（主会话跑）。

use super::{
    FINGERPRINT_MIN_SAMPLES, FingerprintTier, TASK_TYPE_BUCKETS, apply_fingerprint_weights,
    classify_tier, task_type_of, tier_note,
};
use crate::BoardStore;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};

// ---------- task_type_of：标签优先 / 无标签标题分桶 ----------

#[test]
fn task_type_prefers_normalized_tags() {
    // 标签归一：小写 + 去空白 + 字典序 + 去重——同义不同序的标签集同型。
    assert_eq!(
        task_type_of("任意标题", &["Rust".to_string(), "auth".to_string()]),
        "tags:auth,rust"
    );
    assert_eq!(
        task_type_of(
            "另一标题",
            &[" rust ".to_string(), "Auth".to_string(), "rust".to_string()]
        ),
        "tags:auth,rust",
        "标签集合相同（顺序/大小写/重复不同）必须同型"
    );
    // 全空白标签视同无标签 → 回落标题分桶。
    assert_eq!(
        task_type_of("标题", &["  ".to_string()]),
        task_type_of("标题", &[]),
    );
}

#[test]
fn task_type_buckets_titles_deterministically() {
    // 同标题（大小写/空白差异归一）→ 同桶；确定可重放。
    let a = task_type_of("修复登录 Bug", &[]);
    assert_eq!(a, task_type_of("修复登录bug", &[]));
    assert_eq!(a, task_type_of("  修复登录   Bug ", &[]));
    // 桶号有限域：bucket:<0..16>。
    assert!(a.starts_with("bucket:"));
    let bucket: u32 = a.trim_start_matches("bucket:").parse().unwrap();
    assert!(bucket < TASK_TYPE_BUCKETS);
    // 不同标题几乎必然散进不同桶（找一对反例即失败；16 桶下撞桶概率
    // 理论存在，这里用固定样本对钉死 FNV 实现——若改桶数/哈希需同步改）。
    let diff = task_type_of("alpha", &[]) != task_type_of("beta", &[]);
    assert!(diff, "不同标题不应全落同桶");
}

// ---------- classify_tier：三档判定边界矩阵 ----------

#[test]
fn tier_boundaries_follow_thresholds() {
    // 样本 <3：一律 neutral（哪怕全成/全败）——小样本不一票定生死。
    assert_eq!(classify_tier(0, 0), FingerprintTier::Neutral);
    assert_eq!(classify_tier(2, 2), FingerprintTier::Neutral);
    assert_eq!(classify_tier(0, 2), FingerprintTier::Neutral);
    assert_eq!(
        classify_tier(FINGERPRINT_MIN_SAMPLES - 1, FINGERPRINT_MIN_SAMPLES - 1),
        FingerprintTier::Neutral
    );
    // 样本 =3 起判：≥0.7 → prefer。
    assert_eq!(classify_tier(3, 3), FingerprintTier::Prefer);
    assert_eq!(classify_tier(5, 6), FingerprintTier::Prefer); // 0.833
    assert_eq!(classify_tier(7, 10), FingerprintTier::Prefer); // 0.7 压线
    // ≤0.3 → avoid。
    assert_eq!(classify_tier(0, 3), FingerprintTier::Avoid);
    assert_eq!(classify_tier(2, 7), FingerprintTier::Avoid); // 0.286
    assert_eq!(classify_tier(3, 10), FingerprintTier::Avoid); // 0.3 压线
    // 中间 → neutral。
    assert_eq!(classify_tier(1, 3), FingerprintTier::Neutral); // 0.333
    assert_eq!(classify_tier(2, 3), FingerprintTier::Neutral); // 0.667
    assert_eq!(classify_tier(5, 8), FingerprintTier::Neutral); // 0.625
}

#[test]
fn tier_note_renders_audit_text() {
    assert_eq!(tier_note(5, 6), "档位 prefer，成功率 5/6");
    assert_eq!(tier_note(0, 3), "档位 avoid，成功率 0/3");
    assert_eq!(tier_note(2, 3), "档位 neutral，成功率 2/3");
    // 无样本：诚实注明，不虚构 0/0 成功率。
    assert_eq!(tier_note(0, 0), "档位 neutral（无历史样本）");
}

// ---------- apply_fingerprint_weights：稳定三段分区 ----------

fn fps(pairs: &[(&str, u64, u64)]) -> HashMap<String, (u64, u64)> {
    pairs
        .iter()
        .map(|(w, s, t)| (w.to_string(), (*s, *t)))
        .collect()
}

#[test]
fn weights_put_prefer_first_and_avoid_last_without_exclusion() {
    // 匹配器原始序 b→d→e；b=avoid(0/3)、d=prefer(5/6)、e=neutral(无记录)。
    let ranked = vec![
        "node-b".to_string(),
        "node-d".to_string(),
        "node-e".to_string(),
    ];
    let ordered = apply_fingerprint_weights(ranked, &fps(&[("node-b", 0, 3), ("node-d", 5, 6)]));
    assert_eq!(
        ordered,
        vec!["node-d", "node-e", "node-b"],
        "prefer→neutral→avoid"
    );
    // avoid 不除名：全员 avoid 仍全量返回（有人干活好过没人干活）。
    let all_avoid = apply_fingerprint_weights(
        vec!["x".to_string(), "y".to_string()],
        &fps(&[("x", 0, 3), ("y", 1, 4)]),
    );
    assert_eq!(all_avoid, vec!["x", "y"], "同档内保持原始序，不排除");
}

#[test]
fn weights_stay_stable_within_tiers_and_starved_samples_stay_neutral() {
    // 段内稳定：同 prefer 档的 b/d 保持匹配器原始序。
    let ranked = vec!["node-d".to_string(), "node-b".to_string()];
    let ordered = apply_fingerprint_weights(ranked, &fps(&[("node-b", 9, 9), ("node-d", 5, 6)]));
    assert_eq!(ordered, vec!["node-d", "node-b"], "同档不重排");
    // 样本不足（1/1、2/2 高成功率）= neutral：不插队到无记录候选之前。
    let ranked = vec!["node-x".to_string(), "node-y".to_string()];
    let ordered = apply_fingerprint_weights(ranked, &fps(&[("node-y", 2, 2)]));
    assert_eq!(ordered, vec!["node-x", "node-y"], "样本<3 不得影响排序");
    // 空指纹表 / 单候选：原样返回。
    assert_eq!(
        apply_fingerprint_weights(vec!["a".to_string()], &fps(&[("a", 3, 3)])),
        vec!["a"]
    );
    assert_eq!(
        apply_fingerprint_weights(vec!["a".to_string(), "b".to_string()], &HashMap::new()),
        vec!["a", "b"]
    );
}

// ---------- BoardStore：记账幂等 + 指纹读取 ----------

fn temp_store(name: &str) -> BoardStore {
    static SEQ: AtomicU32 = AtomicU32::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-fptest-{}-{name}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    BoardStore::open(&dir.join("board.db"), "NB").expect("open store")
}

#[test]
fn store_records_and_counts_by_worker_and_task_type() {
    let store = temp_store("count");
    store
        .record_fingerprint_outcome("node-b", "tags:rust", "t-1", true)
        .unwrap();
    store
        .record_fingerprint_outcome("node-b", "tags:rust", "t-2", true)
        .unwrap();
    store
        .record_fingerprint_outcome("node-b", "tags:rust", "t-3", false)
        .unwrap();
    // 不同 task_type 各自独立记账。
    store
        .record_fingerprint_outcome("node-b", "tags:web", "t-4", false)
        .unwrap();
    assert_eq!(
        store.worker_fingerprint("node-b", "tags:rust").unwrap(),
        Some((2, 3)),
        "2 成 1 败 = (2,3)"
    );
    assert_eq!(
        store.worker_fingerprint("node-b", "tags:web").unwrap(),
        Some((0, 1))
    );
    assert_eq!(
        store.worker_fingerprint("node-c", "tags:rust").unwrap(),
        None,
        "没记过账的 worker 无行"
    );
    let all = store.worker_fingerprints("tags:rust").unwrap();
    assert_eq!(all.get("node-b"), Some(&(2, 3)));
    assert!(!all.contains_key("node-b-web-guard"));
}

#[test]
fn store_accounting_is_idempotent_per_task_id() {
    let store = temp_store("idem");
    assert!(
        store
            .record_fingerprint_outcome("node-b", "bucket:3", "task-same", true)
            .unwrap(),
        "首次记账 = true"
    );
    // 同一 task_id 重放（评审重跑/estop 复评）→ 幂等跳过，不重复计数。
    assert!(
        !store
            .record_fingerprint_outcome("node-b", "bucket:3", "task-same", true)
            .unwrap()
    );
    // 就算重放带了不同 outcome（异常时序）也不改账：台账键是 task_id。
    assert!(
        !store
            .record_fingerprint_outcome("node-b", "bucket:3", "task-same", false)
            .unwrap()
    );
    assert_eq!(
        store.worker_fingerprint("node-b", "bucket:3").unwrap(),
        Some((1, 1)),
        "重放不重复计数"
    );
    // 不同 task_id 照常累加。
    assert!(
        store
            .record_fingerprint_outcome("node-b", "bucket:3", "task-next", false)
            .unwrap()
    );
    assert_eq!(
        store.worker_fingerprint("node-b", "bucket:3").unwrap(),
        Some((1, 2))
    );
}
