//! 能力扩展 P34：重派决策量化——worker × 任务类型指纹（nanobot 指纹晋升
//! 思想移植）。
//!
//! 职责边界（镜像 [`crate::team_memory`] 的分层）：本模块**不含任何 IO 与
//! LLM 调用**——任务类型指纹怎么算、三档怎么判、加权怎么排，全是纯函数
//! （单测零依赖）。成败记账的落库时机在 board_review 评审定案处（master
//! 侧验收结论是唯一成败真相源：PASS=成功 / FAIL=失败；UNSURE 与评审自身
//! 故障不计——没定案的轮次不配给任何 worker 记成败），落库走
//! [`crate::BoardStore::record_fingerprint_outcome`]（task_id 幂等）。
//! 加权消费在 nemesisbot `pick_redispatch_target`，挂在
//! `board.fingerprint_weighting` 开关后面（默认 false = 决策表现状行为
//! 字节等价；记账与开关解耦，灰度期照常攒数据）。
//!
//! 三档语义（阈值灰度期写死带注释，不进 config 防配置面膨胀）：
//! - 样本 < [`FINGERPRINT_MIN_SAMPLES`]：一律 neutral（不许小样本一票定
//!   生死——3 连成不一定真强，2 连败也不一定真废）；
//! - 样本 ≥ 3 且成功率 ≥ [`PREFER_RATE`]：prefer（排序加权靠前）；
//! - 样本 ≥ 3 且成功率 ≤ [`AVOID_RATE`]：avoid（排序加权靠后，**非排除**
//!   ——全员 avoid 时仍按序选，有人干活好过没人干活）；
//! - 其余：neutral（不干预匹配器原始序）。

use std::collections::HashMap;

/// 晋档/降档的最小样本数（P34 任务给定；灰度期写死，调参改常量再评估）。
pub const FINGERPRINT_MIN_SAMPLES: u64 = 3;

/// prefer 阈值：成功率 ≥ 此值晋升（3/3、7/10 都算）。
const PREFER_RATE: f64 = 0.7;

/// avoid 阈值：成功率 ≤ 此值降档（0/3、2/7 都算）。
const AVOID_RATE: f64 = 0.3;

/// 无标签任务按标题 hash 分桶的桶数（有限桶让 (worker, 类型) 二元组有限
/// 可积累样本；16 桶对常见任务粒度够分）。
pub const TASK_TYPE_BUCKETS: u32 = 16;

/// 指纹三档。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintTier {
    /// 成功率显著为正 → 排序加权靠前。
    Prefer,
    /// 成功率显著为负 → 排序加权靠后（非排除）。
    Avoid,
    /// 样本不足 / 中间地带 → 不干预（匹配器原始序）。
    Neutral,
}

impl FingerprintTier {
    /// 审计文案用稳定英文名（评论「档位 prefer」形态；跨版本不翻译，
    /// 便于日志/评论检索）。
    pub fn as_str(self) -> &'static str {
        match self {
            FingerprintTier::Prefer => "prefer",
            FingerprintTier::Avoid => "avoid",
            FingerprintTier::Neutral => "neutral",
        }
    }
}

/// 任务类型指纹：标签优先（语义稳定——planner 拆解的 required_tags 归一
/// 去重后拼接），无标签回落标题 hash 分桶（去空白 + 小写归一 → FNV-1a
/// 32bit → [`TASK_TYPE_BUCKETS`] 桶）。同一单的多次重派天然同型（title/
/// tags 不随轮次变），跨单同标签任务共享同一指纹（「node-b 擅长 rust 类
/// 活」就是按这个粒度沉淀的）。
pub fn task_type_of(title: &str, tags: &[String]) -> String {
    let mut norm_tags: Vec<String> = tags
        .iter()
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    norm_tags.sort();
    norm_tags.dedup();
    if !norm_tags.is_empty() {
        return format!("tags:{}", norm_tags.join(","));
    }
    // 归一：去全部空白 + 小写（Unicode 感知），只留字符内容做指纹。
    let normalized = title.split_whitespace().collect::<Vec<_>>().join("");
    let normalized = normalized.to_lowercase();
    // FNV-1a 32bit：实现简单、分布均匀，够分桶用途（非密码学场景）。
    let mut h: u32 = 0x811c_9dc5;
    for b in normalized.as_bytes() {
        h ^= u32::from(*b);
        h = h.wrapping_mul(0x0100_0193);
    }
    format!("bucket:{}", h % TASK_TYPE_BUCKETS)
}

/// 三档判定（单测钉死边界矩阵）：样本 <[`FINGERPRINT_MIN_SAMPLES`] 一律
/// neutral；否则成功率 ≥[`PREFER_RATE`] 晋 prefer、≤[`AVOID_RATE`] 降
/// avoid、中间 neutral。
pub fn classify_tier(success: u64, total: u64) -> FingerprintTier {
    if total < FINGERPRINT_MIN_SAMPLES {
        return FingerprintTier::Neutral;
    }
    let rate = success as f64 / total as f64;
    if rate >= PREFER_RATE {
        FingerprintTier::Prefer
    } else if rate <= AVOID_RATE {
        FingerprintTier::Avoid
    } else {
        FingerprintTier::Neutral
    }
}

/// 匹配器排序之上的稳定加权（[`crate::matcher::rank_peers`] 的**次级重排**，
/// 不改资格面）：按 (worker, task_type) 指纹把候选分成 prefer → neutral →
/// avoid 三段，段内保持匹配器原始序（稳定分区）。指纹缺失的 worker 视作
/// neutral（不惩罚新节点——样本 <3 本来就 neutral）。avoid 非绝对排除：
/// 排后不除名。候选 <2 或指纹表空 = 原样返回（无可加权面）。
///
/// `fingerprints`：worker → (成功数, 总数)（同一任务类型维度下）。
pub fn apply_fingerprint_weights(
    ranked: Vec<String>,
    fingerprints: &HashMap<String, (u64, u64)>,
) -> Vec<String> {
    if fingerprints.is_empty() || ranked.len() < 2 {
        return ranked;
    }
    let tier_rank = |w: &str| match fingerprints.get(w) {
        Some(&(s, t)) => match classify_tier(s, t) {
            FingerprintTier::Prefer => 0u8,
            FingerprintTier::Neutral => 1u8,
            FingerprintTier::Avoid => 2u8,
        },
        // 无指纹记录 = neutral（与 classify_tier(0,0) 同档，显式写出语义）。
        None => 1u8,
    };
    let mut ordered: Vec<(u8, usize, String)> = ranked
        .into_iter()
        .enumerate()
        .map(|(i, w)| (tier_rank(&w), i, w))
        .collect();
    // (档位, 原始下标) 双键排序 = 稳定分区（段内保匹配器序，确定性）。
    ordered.sort_by_key(|(tier, i, _)| (*tier, *i));
    ordered.into_iter().map(|(_, _, w)| w).collect()
}

/// 重派换节点评论的档位注记（可审计：「档位 prefer，成功率 5/6」形态；
/// 调用方负责包括号）。有样本给档位+成功率，无样本诚实注明（不虚构
/// 0/0 这种没人记过账的假成功率）。
pub fn tier_note(success: u64, total: u64) -> String {
    if total == 0 {
        return format!(
            "档位 {}（无历史样本）",
            classify_tier(success, total).as_str()
        );
    }
    format!(
        "档位 {}，成功率 {success}/{total}",
        classify_tier(success, total).as_str()
    )
}

#[cfg(test)]
mod tests;
