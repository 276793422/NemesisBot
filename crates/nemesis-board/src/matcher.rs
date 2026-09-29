//! Swarm M1：智能选节点匹配器（纯函数，无集群依赖）。
//!
//! 打分语义（impl-plan §3.2 + 集群专业职能框架 M2）：
//! - `required_role` 是**硬条件**：给了但无人精确命中 → 空结果（诚实留
//!   backlog，不硬塞给错误角色）；
//! - `required_tags` 是**硬条件**：给了则必须有交集；
//! - `required_profession` 是**硬条件 3**：给了则节点必须宣告同 slug
//!   （精确匹配，无继承；`dev` 不命中 `dev:cpp`，反向亦然）；
//! - **硬条件 4（tier 门槛，D3）**：职能有 min_tier 且节点 tier 已知但
//!   低于门槛 → 排除；tier 未知（None，旧节点）fail-open 放行。tier
//!   门槛**不参与松弛**（D13）——有职能的松弛级照常带门槛；
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
    /// 节点宣告的职能 slug 清单（`family[:spec]`；M2 匹配数据源）。
    pub professions: Vec<String>,
    /// 节点自报档位（mini/normal/big；None = 未宣告，tier 闸 fail-open）。
    pub tier: Option<String>,
}

/// 匹配输入（子单的派发需求侧）。
#[derive(Debug, Clone)]
pub struct MatchInput<'a> {
    /// 需求角色；None/空 = 不限角色。
    pub required_role: Option<&'a str>,
    /// 需求标签；空 = 不限标签。
    pub required_tags: &'a [String],
    /// 需求职能 slug；None/空 = 不限职能（硬条件 3/4 跳过）。
    pub required_profession: Option<&'a str>,
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
    // 职能归一（小写）；空串视作未给。min_tier 只查内置目录（M1：用户
    // 自定义职能无门槛元数据 → 门槛跳过）。
    let prof_need = input
        .required_profession
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(nemesis_prompts::professions::meta::normalize_slug);
    let prof_min_tier = prof_need
        .as_deref()
        .and_then(nemesis_prompts::professions::meta::find_builtin)
        .map(|m| m.min_tier);

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
            // 硬条件 3：职能精确命中（大小写不敏感；无继承——宣告 `dev`
            // 的节点接不了 `dev:cpp` 的子单，反向亦然）。
            if let Some(need) = &prof_need
                && !peer
                    .professions
                    .iter()
                    .any(|p| nemesis_prompts::professions::meta::normalize_slug(p) == *need)
            {
                return None;
            }
            // 硬条件 4：tier 门槛（D3，不参与松弛）。节点 tier 已知且低于
            // 职能 min_tier → 排除；未知（None）fail-open 放行。
            if let Some(min_tier) = prof_min_tier
                && !nemesis_prompts::professions::meta::tier_allows(peer.tier.as_deref(), min_tier)
            {
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

/// D13 松弛阶梯的纯函数内核（fallback 序，调用方先自己跑过严格匹配 =
/// 阶梯 ①「全条件」）：②保职能丢标签 → ③丢职能（保角色/标签）→ ④全松
/// 弛（职能/角色/标签均放开）。返回 `(节点 id, 松弛级说明)`，说明字符串
/// 供调用方评论留痕。
///
/// tier 门槛**不参与松弛**（D13）：门槛是 required_profession 的函数，
/// 有职能的松弛级照常带门槛，职能被丢掉后门槛自然随行——不存在「保留
/// 门槛要求但放开职能」的中间态。
///
/// 无职能子单：② 蜕化为「保角色丢标签」（与旧 fallback 一级等价），③
/// 跳过，④ 等价旧末级（角色也放开）——旧行为完全保留。
pub fn pick_relaxed<'a>(
    base: &MatchInput<'a>,
    peers: &[PeerCandidate],
    load: &HashMap<String, usize>,
) -> Option<(String, &'static str)> {
    let prof = base
        .required_profession
        .map(str::trim)
        .filter(|s| !s.is_empty());
    fn make<'a>(
        base: &MatchInput<'a>,
        role: Option<&'a str>,
        prof: Option<&'a str>,
        tags: &'a [String],
    ) -> MatchInput<'a> {
        MatchInput {
            required_role: role,
            required_profession: prof,
            required_tags: tags,
            description: base.description,
        }
    }
    let empty_tags: Vec<String> = Vec::new();
    // ② 保职能丢标签（无职能 → 保角色丢标签，即旧①级）。
    if let Some(id) = pick_peer(
        &make(base, base.required_role, prof, &empty_tags),
        peers,
        load,
    ) {
        let label = if prof.is_some() {
            "保职能松弛标签兜底"
        } else {
            "无标签匹配节点，保角色松弛兜底"
        };
        return Some((id, label));
    }
    // ③ 丢职能（保角色/标签）——仅在有职能时是新的一步。
    if prof.is_some()
        && let Some(id) = pick_peer(
            &make(base, base.required_role, None, base.required_tags),
            peers,
            load,
        )
    {
        return Some((id, "松弛职能兜底（角色/标签保留）"));
    }
    // ④ 全松弛（职能/角色/标签均放开；与旧末级 rank(None) 等价）。
    pick_peer(&make(base, None, None, &empty_tags), peers, load)
        .map(|id| (id, "无匹配节点，全松弛兜底（职能/角色/标签均放开）"))
}

#[cfg(test)]
mod tests;
