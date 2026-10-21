//! `node_perf` 单测（滑窗中位数 / 回落链 / 稳定重排 / 零行为变化红线）。

use super::*;

fn sample(worker: &str, prof: &str, secs: i64) -> WorkerDurationSample {
    WorkerDurationSample {
        worker_id: worker.to_string(),
        profession: prof.to_string(),
        duration_secs: secs,
    }
}

#[test]
fn normalize_category_empty_becomes_wildcard() {
    assert_eq!(normalize_category(None), "_");
    assert_eq!(normalize_category(Some("  ")), "_");
    assert_eq!(normalize_category(Some("Dev:CPP")), "dev:cpp");
}

#[test]
fn no_samples_returns_original_order() {
    let ranked = vec!["a".to_string(), "b".to_string()];
    assert_eq!(
        reorder_by_predicted_duration(ranked.clone(), &[], "dev"),
        ranked
    );
}

#[test]
fn single_candidate_never_reordered() {
    let samples = vec![sample("a", "dev", 1)];
    let ranked = vec!["a".to_string()];
    assert_eq!(
        reorder_by_predicted_duration(ranked.clone(), &samples, "dev"),
        ranked
    );
}

#[test]
fn fast_worker_ranks_first_when_sufficiently_sampled() {
    // a = 快节点（3 样本中位数 10s），b = 慢节点（3 样本中位数 100s）。
    let samples = vec![
        sample("a", "dev", 9),
        sample("a", "dev", 10),
        sample("a", "dev", 11),
        sample("b", "dev", 90),
        sample("b", "dev", 100),
        sample("b", "dev", 110),
    ];
    let ranked = vec!["b".to_string(), "a".to_string()];
    let ordered = reorder_by_predicted_duration(ranked, &samples, "dev");
    assert_eq!(ordered, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn under_sampled_worker_falls_back_to_global_median() {
    // b 只有 1 个样本（< MIN_WORKER_SAMPLES）→ 回落全局中位数；
    // a 有 3 个快样本 → a 在前。
    let samples = vec![
        sample("a", "dev", 8),
        sample("a", "dev", 10),
        sample("a", "dev", 12),
        sample("b", "dev", 500),
    ];
    let ranked = vec!["b".to_string(), "a".to_string()];
    let ordered = reorder_by_predicted_duration(ranked, &samples, "dev");
    assert_eq!(ordered, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn categories_are_isolated() {
    // a 在 dev 类别快、b 在 test 类别快；本次派发是 dev → a 在前。
    let samples = vec![
        sample("a", "dev", 10),
        sample("a", "dev", 10),
        sample("a", "dev", 10),
        sample("b", "test", 10),
        sample("b", "test", 10),
        sample("b", "test", 10),
        sample("b", "dev", 999),
        sample("b", "dev", 999),
        sample("b", "dev", 999),
    ];
    let ranked = vec!["b".to_string(), "a".to_string()];
    let ordered = reorder_by_predicted_duration(ranked, &samples, "dev");
    assert_eq!(ordered, vec!["a".to_string(), "b".to_string()]);
}

#[test]
fn equal_predictions_stable_original_order() {
    // 两 worker 中位数相同 → 保原序（稳定排序，idx 决胜）。
    let samples = vec![
        sample("a", "dev", 10),
        sample("a", "dev", 10),
        sample("a", "dev", 10),
        sample("b", "dev", 10),
        sample("b", "dev", 10),
        sample("b", "dev", 10),
    ];
    let ranked = vec!["b".to_string(), "a".to_string()];
    let ordered = reorder_by_predicted_duration(ranked, &samples, "dev");
    assert_eq!(ordered, vec!["b".to_string(), "a".to_string()]);
}

#[test]
fn never_seen_worker_gets_global_median_not_bottom() {
    // c 从未出现在样本里 → 全局中位数档；a（快，5s）在前。全局中位数
    // = median([5,5,5,1000,1000,1000]) = 1000 → c 与 b（慢节点）同档，
    // 同档保原序（b 的匹配器名次靠前，不因平票裁决翻动）。
    let samples = vec![
        sample("a", "dev", 5),
        sample("a", "dev", 5),
        sample("a", "dev", 5),
        sample("b", "dev", 1000),
        sample("b", "dev", 1000),
        sample("b", "dev", 1000),
    ];
    let ranked = vec!["b".to_string(), "c".to_string(), "a".to_string()];
    let ordered = reorder_by_predicted_duration(ranked, &samples, "dev");
    assert_eq!(
        ordered,
        vec!["a".to_string(), "b".to_string(), "c".to_string()]
    );
}

#[test]
fn worker_window_keeps_the_newest_32() {
    // 方向可判别（旧断言 30×999+2×10 在两种滑窗方向下同为 999，测不出
    // 反向 bug）：样本按 store 契约最新优先排列 = 42,41,...,1。正确窗口
    // （保留迭代序前 32 = 值 42..11）中位数 = 升序第 16 位 = 27；反向
    // （误留最旧 32 = 值 32..1）中位数 = 17。钉死「最新窗口」语义。
    let samples: Vec<_> = (1..=42i64).rev().map(|d| sample("a", "dev", d)).collect();
    let pred = predicted_durations(&samples, "dev");
    assert_eq!(pred.get("a"), Some(&27));
}
