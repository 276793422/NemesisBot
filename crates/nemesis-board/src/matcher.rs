//! Swarm M1：智能选节点匹配器（纯函数，无集群依赖）。
//!
//! 打分语义（impl-plan §3.2）：
//! - `required_role` 是**硬条件**：给了但无人精确命中 → 空结果（诚实留
//!   backlog，不硬塞给错误角色）；
//! - `required_tags` 是**硬条件**：给了则必须有交集；
//! - capabilities 对描述的词面覆盖是**加分项**（+5/命中，封顶 15）；
//! - 排序：(分数↓, 负载↑, name 字典序) —— 同分时选最闲的节点。
//!
//! 数据源由调用方组装（在线 peers 经 cluster `peers_fn`，负载经
//! DispatchRecord 未终态计数），本模块只做纯打分——单测零依赖。

use std::collections::HashMap;

/// 参与选节的 peer 快照（调用方从 NodeInfo/ExtendedNodeInfo 投影）。
#[derive(Debug, Clone, PartialEq)]
pub struct PeerCandidate {
    /// 节点 id（派发目标；name 可能改，id 才是路由键）。
    pub id: String,
    pub name: String,
    /// 节点自报角色（worker/manager/coordinator/...）。
    pub role: String,
    pub tags: Vec<String>,
    /// 能力词（capabilities 广播；小写短语如 "rust" / "docker"）。
    pub capabilities: Vec<String>,
}

/// 匹配输入（子单的派发需求侧）。
#[derive(Debug, Clone)]
pub struct MatchInput<'a> {
    /// 需求角色；None/空 = 不限角色。
    pub required_role: Option<&'a str>,
    /// 需求标签；空 = 不限标签。
    pub required_tags: &'a [String],
    /// 子单描述（capabilities 词面覆盖加分用；小写比较）。
    pub description: &'a str,
}

/// 单节点加分上限（capabilities 覆盖分；防长描述刷分）。
const CAPABILITY_BONUS_CAP: i64 = 15;
const CAPABILITY_BONUS_PER_HIT: i64 = 5;

/// 对候选节点打分排序，返回 `(node_id, score)` 列表（已按 分数↓ 负载↑
/// 排序）。空结果 = 无满足硬条件的节点（调用方诚实留 backlog + 通知）。
pub fn rank_peers(
    input: &MatchInput<'_>,
    peers: &[PeerCandidate],
    load: &HashMap<String, usize>,
) -> Vec<(String, i64)> {
    let desc_lower = input.description.to_lowercase();
    let role_need = input
        .required_role
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase);

    let mut scored: Vec<(String, i64, usize)> = peers
        .iter()
        .filter_map(|peer| {
            // 硬条件 1：角色精确命中（节点 role 自报，小写比较）。
            if let Some(need) = &role_need
                && peer.role.to_lowercase() != *need
            {
                return None;
            }
            // 硬条件 2：标签交集非空（给了 required_tags 就必须有命中）。
            let tag_hits = input
                .required_tags
                .iter()
                .filter(|t| peer.tags.iter().any(|pt| pt.eq_ignore_ascii_case(t)))
                .count();
            if !input.required_tags.is_empty() && tag_hits == 0 {
                return None;
            }
            // 加分：capabilities 词面覆盖描述（大小写不敏感包含）。
            let mut cap_bonus = 0i64;
            for cap in &peer.capabilities {
                let cap = cap.trim();
                if cap.len() >= 2 && desc_lower.contains(&cap.to_lowercase()) {
                    cap_bonus += CAPABILITY_BONUS_PER_HIT;
                    if cap_bonus >= CAPABILITY_BONUS_CAP {
                        break;
                    }
                }
            }
            let score = 100 * tag_hits as i64 + cap_bonus;
            let node_load = load.get(&peer.id).copied().unwrap_or(0);
            Some((peer.id.clone(), score, node_load))
        })
        .collect();

    // 分数↓ 负载↑ name 字典序（同分同载的确定性兜底）。
    scored.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)).then(a.0.cmp(&b.0)));
    scored
        .into_iter()
        .map(|(id, score, _)| (id, score))
        .collect()
}

/// 取第一候选的便捷封装（无满足条件的节点 → None）。
pub fn pick_peer(
    input: &MatchInput<'_>,
    peers: &[PeerCandidate],
    load: &HashMap<String, usize>,
) -> Option<String> {
    rank_peers(input, peers, load)
        .into_iter()
        .next()
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests;
