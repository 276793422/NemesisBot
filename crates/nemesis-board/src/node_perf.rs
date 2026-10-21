//! A1（2026-10-21 DeepInfer 思路引入）：节点耗时估计器（纯函数，无 IO）。
//!
//! 借鉴 DeepInfer `mtp_varlen.hpp` 的运行时估计器形态——按桶维护滑窗
//! 中位数、样本不足回落全局中位数、无数据零干预——但数据源是本仓
//! **现成的** `issue_dispatch` 派发表（`completed_at - dispatched_at`，
//! master 侧单钟），不需要任何新计时埋点。
//!
//! 语义：
//! - 类别键 = `issue.required_profession`（归一小写；空 → `"_")——与
//!   matcher 的职能硬条件同源，防异构任务互相污染时长统计；
//! - per-worker 滑窗中位数（每 worker 取最近 [`WORKER_SAMPLE_CAP`] 个
//!   样本）；样本 < [`MIN_WORKER_SAMPLES`] 回落类别全局中位数（回落链
//!   与 DeepInfer 一致：个体 → 全局；全局也没有 = 无预测面）；
//! - 排序 = 稳定排序（预测耗时 ↑）——同预测值保持匹配器原序（匹配器
//!   已按分数↓负载↑排好，本模块**只**在数据充分时细调先后，永不把
//!   硬条件合格的节点排除出候选集）。
//!
//! 与 P34 指纹稳定加权的关系：指纹管「成功率」（prefer/neutral/avoid
//! 三段），本模块管「速度」（耗时中位数排序）——正交信号，消费方按
//! 指纹 → 耗时的顺序叠加（成功率先行，速度在成功率中性段内细分）。

use std::collections::HashMap;

/// per-worker 参与中位数的最大样本数（滑窗；只保留最近 N 个）。
pub const WORKER_SAMPLE_CAP: usize = 32;
/// per-worker 中位数生效的最小样本数（不足回落类别全局中位数——
/// 1-2 个样本的方差太大，不足以支撑排序决策）。
pub const MIN_WORKER_SAMPLES: usize = 3;

/// 单条完成的派发耗时样本（store 查询投影；`profession` 已归一，
/// 空职能 = `"_"`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerDurationSample {
    pub worker_id: String,
    pub profession: String,
    /// `completed_at - dispatched_at`（秒；master 侧单钟测量）。
    pub duration_secs: i64,
}

/// 类别键归一单点：trim + 小写；空 → `"_"`（无职能派发的公共桶，
/// 与 store 查询的 COALESCE 口径一致）。
pub fn normalize_category(raw: Option<&str>) -> String {
    match raw.map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => s.to_ascii_lowercase(),
        None => "_".to_string(),
    }
}

fn median(values: &mut [i64]) -> Option<i64> {
    if values.is_empty() {
        return None;
    }
    values.sort_unstable();
    Some(values[values.len() / 2])
}

/// 按 (worker, category) 样本预测各 worker 的耗时中位数，返回
/// worker → 预测秒数映射。预测面：
/// - worker 样本 ≥ [`MIN_WORKER_SAMPLES`] → 该 worker 的滑窗中位数；
/// - 否则 → 类别全局中位数（所有欠采样 worker 同值 = 不参与区分）；
/// - 类别无任何样本 → 空映射（调用方原序返回，零行为变化）。
///
/// **输入顺序契约**：`samples` 必须最新优先（store 查询 rowid DESC 的
/// 原序）——滑窗保留的是迭代序前 [`WORKER_SAMPLE_CAP`] 个 = 时间上
/// 最近的样本。旧版 `remove(0)` 在最新优先输入下会反向保留最旧窗口
/// （回查修正 2026-10-21）。
pub fn predicted_durations(
    samples: &[WorkerDurationSample],
    category: &str,
) -> HashMap<String, i64> {
    let cat = normalize_category(Some(category));
    let mut by_worker: HashMap<String, Vec<i64>> = HashMap::new();
    let mut all: Vec<i64> = Vec::new();
    for s in samples {
        if normalize_category(Some(&s.profession)) != cat {
            continue;
        }
        all.push(s.duration_secs);
        let v = by_worker.entry(s.worker_id.clone()).or_default();
        v.push(s.duration_secs);
        if v.len() > WORKER_SAMPLE_CAP {
            // 样本按 store 的最新优先序到达（rowid DESC，front=最新）；
            // 溢出时从尾部丢（刚推入的最旧者）→ 窗口恒为最近 32 个。
            v.pop();
        }
    }
    if all.is_empty() {
        return HashMap::new();
    }
    let global = median(&mut all).unwrap_or(0);
    by_worker
        .into_iter()
        .map(|(worker, mut durs)| {
            let predicted = if durs.len() >= MIN_WORKER_SAMPLES {
                median(&mut durs).unwrap_or(global)
            } else {
                global
            };
            (worker, predicted)
        })
        .collect()
}

/// 稳定重排候选序：预测耗时升序（同值保持原序）。样本为空或预测面
/// 为空 → 原序返回（零行为变化红线）。`ranked` 里不在预测面的 worker
/// 视作全局中位数档（与欠采样 worker 同档，不歧视无历史的新节点）。
pub fn reorder_by_predicted_duration(
    ranked: Vec<String>,
    samples: &[WorkerDurationSample],
    category: &str,
) -> Vec<String> {
    let predictions = predicted_durations(samples, category);
    if predictions.is_empty() || ranked.len() < 2 {
        return ranked;
    }
    // 无预测条目的 worker 用全局中位数兜底：欠采样者已在预测面里拿
    // 全局中位数，这里对「样本里从未出现过的节点」同样取全局中位数
    // ——新节点不因无历史而沉底（诚实中性），也不因此置顶。
    let global_fallback = {
        let mut all: Vec<i64> = samples
            .iter()
            .filter(|s| {
                normalize_category(Some(&s.profession)) == normalize_category(Some(category))
            })
            .map(|s| s.duration_secs)
            .collect();
        median(&mut all).unwrap_or(0)
    };
    let mut keyed: Vec<(i64, usize, String)> = ranked
        .into_iter()
        .enumerate()
        .map(|(idx, worker)| {
            let predicted = predictions.get(&worker).copied().unwrap_or(global_fallback);
            (predicted, idx, worker)
        })
        .collect();
    keyed.sort_by_key(|(predicted, idx, _)| (*predicted, *idx));
    keyed.into_iter().map(|(_, _, worker)| worker).collect()
}

#[cfg(test)]
mod tests;
