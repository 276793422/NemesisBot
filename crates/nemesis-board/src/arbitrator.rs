//! Swarm M3：定向投递裁决器（master 侧纯逻辑，无集群依赖）。
//!
//! 裁决语义（impl-plan §5.3）：新消息落库后决定「唤醒了谁 / 跳过了谁 /
//! 为什么」，投递链路可审计：
//! - ① `@name` / `@node-id` 精准点名（id/name 均可，大小写不敏感）→ 定点投递；
//! - ② `@role:qa` 点名角色 → 该角色**全部**在线节点投递（点名角色=该角色都该知道）；
//! - ③ 频道消息无 @ → 只投主持人（master 本地直调 run_direct 裁决）；
//! - ④ issue 评论无 @ → 投该 issue 指派节点。
//!
//! 在线 → wake.post；离线 → 跳过（board.sync 补拉兜底），跳过决策带原因
//! 进 `WakePlan.skipped`。投递语义 ≠ 派活语义：这里不选「谁干活」，只决定
//! 「谁被叫回来看」。
//!
//! 节点表由调用方投影（在线 peers → [`NodeCandidate`]），本模块只做纯
//! 解析与匹配——单测零依赖。

/// 参与裁决的节点快照（调用方从集群节点表投影；含离线节点——离线目标要
/// 出现在 skipped 里而不是凭空消失）。
#[derive(Debug, Clone, PartialEq)]
pub struct NodeCandidate {
    pub id: String,
    pub name: String,
    /// 拓扑角色（coordinator/worker；小写比较）。
    pub role: String,
    /// 功能类别（qa/dev/general/...；@role: 匹配同时看本字段与 [`Self::role`]
    /// ——集群里功能性角色落 category，拓扑角色落 role，点名任意一个都该
    /// 命中）。
    pub category: String,
    pub online: bool,
}

/// 裁决输入：一条新落库的讨论消息。
#[derive(Debug, Clone)]
pub struct WakeInput<'a> {
    /// [`crate::models::thread_kind`] 词表（issue / channel）。
    pub thread_kind: &'a str,
    /// 消息原文（@ 解析的对象）。
    pub content: &'a str,
    /// 发送者节点 id（不唤醒自己；人类发送者不受此限）。
    pub sender_id: &'a str,
    /// 规则④：issue 指派节点 id（None / 空 = 未指派）。
    pub issue_assignee: Option<&'a str>,
    /// 规则③：主持人节点 id（master 本尊）。
    pub moderator_id: &'a str,
}

/// 跳过记录（审计：为什么没被唤醒）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipRecord {
    pub node_id: String,
    /// offline / sender / not_found / no_assignee / not_moderator
    pub reason: &'static str,
}

/// 裁决结果：唤醒计划。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WakePlan {
    /// 待 wake.post 的在线节点 id（按解析顺序去重）。
    pub targets: Vec<String>,
    /// 跳过决策（离线/发送者本人/点名未命中/无指派）。
    pub skipped: Vec<SkipRecord>,
    /// 规则③命中：无 @ 频道消息 → master 本地直调裁决器 agent。
    pub to_moderator: bool,
}

/// 提取消息中的 @ 点名 token（`@xxx` / `@role:yyy`）。
/// 识别边界：@ 后取连续的非空白字符；中文后紧跟的 @ 同样生效。
fn extract_mentions(content: &str) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in content.match_indices('@') {
        // 排除邮箱形态（前一字符是字母/数字时不当作点名——如 a@b.com）。
        if i > 0 {
            let prev = content[..i].chars().next_back().unwrap_or(' ');
            if prev.is_alphanumeric() || prev == '_' {
                continue;
            }
        }
        let rest = &content[i + 1..];
        let token: String = rest
            .chars()
            .take_while(|c| !(c.is_whitespace() || *c == '，' || *c == '。' || *c == '、'))
            .collect();
        if !token.is_empty() {
            out.push(token);
        }
    }
    out
}

/// 这条消息是否点名了该节点（规则①同源匹配：@id / @name，大小写不敏感）。
/// `@role:x` 角色点名按规则②语义判定：x 匹配该节点的拓扑角色或功能类别
/// 也算点名（master fan-out 时节点离线 → skip → board.sync 补拉靠这里兜住
/// 「@role:qa 我在场却没被唤醒」的缺口）。
///
/// worker 侧 board.sync 过滤与 master 裁决共用同一匹配语义（单一真相源）。
pub fn mentions_node(
    content: &str,
    node_id: &str,
    node_name: &str,
    node_role: &str,
    node_category: &str,
) -> bool {
    extract_mentions(content).iter().any(|t| {
        if let Some(role) = t.strip_prefix("role:") {
            let role_lc = role.to_lowercase();
            !role_lc.is_empty()
                && (node_role.to_lowercase() == role_lc || node_category.to_lowercase() == role_lc)
        } else {
            let t_lc = t.to_lowercase();
            t_lc == node_id.to_lowercase() || t_lc == node_name.to_lowercase()
        }
    })
}

/// 消息里有没有任何 @ 点名（master 侧决定 wake 事件标签用：
/// 无 @ + issue 指派 → assignee_comment，否则 mention）。
pub fn has_mentions(content: &str) -> bool {
    !extract_mentions(content).is_empty()
}

/// 裁决入口：解析 @ 并按四规则产出唤醒计划。
pub fn resolve_wake_targets(input: &WakeInput<'_>, nodes: &[NodeCandidate]) -> WakePlan {
    let mentions = extract_mentions(input.content);
    let mut plan = WakePlan::default();

    if mentions.is_empty() {
        // 无 @：按线程种类走规则③/④。
        if input.thread_kind == crate::models::thread_kind::CHANNEL {
            // 主持人总在本地（master 本尊），不算离线目标。
            plan.to_moderator = true;
        } else {
            // issue 评论：投指派节点。
            match input.issue_assignee.filter(|s| !s.is_empty()) {
                Some(assignee) => {
                    match nodes.iter().find(|n| n.id == assignee) {
                        Some(node) => {
                            if node.online {
                                plan.targets.push(assignee.to_string());
                            } else {
                                plan.skipped.push(SkipRecord {
                                    node_id: assignee.to_string(),
                                    reason: "offline",
                                });
                            }
                        }
                        // 指派 id 不在节点表（未发现/已移除）——诚实记录。
                        None => plan.skipped.push(SkipRecord {
                            node_id: assignee.to_string(),
                            reason: "not_found",
                        }),
                    }
                }
                None => plan.skipped.push(SkipRecord {
                    node_id: "(issue)".to_string(),
                    reason: "no_assignee",
                }),
            }
        }
        return plan;
    }

    // 有 @：规则①②可混用（"@node-b @role:qa" = 并集），按解析顺序去重。
    for token in &mentions {
        if let Some(role) = token.strip_prefix("role:") {
            let role_lc = role.to_lowercase();
            if role_lc.is_empty() {
                plan.skipped.push(SkipRecord {
                    node_id: format!("@{token}"),
                    reason: "not_found",
                });
                continue;
            }
            let mut matched = false;
            for node in nodes {
                if node.role.to_lowercase() == role_lc || node.category.to_lowercase() == role_lc {
                    matched = true;
                    push_target_or_skip(&mut plan, node, input.sender_id);
                }
            }
            if !matched {
                plan.skipped.push(SkipRecord {
                    node_id: format!("@{token}"),
                    reason: "not_found",
                });
            }
        } else {
            // 规则①：id 或 name 精确匹配（大小写不敏感）。
            let token_lc = token.to_lowercase();
            match nodes
                .iter()
                .find(|n| n.id.to_lowercase() == token_lc || n.name.to_lowercase() == token_lc)
            {
                Some(node) => push_target_or_skip(&mut plan, node, input.sender_id),
                None => plan.skipped.push(SkipRecord {
                    node_id: format!("@{token}"),
                    reason: "not_found",
                }),
            }
        }
    }
    plan
}

/// 单目标落计划：去重、跳过发送者本人、离线进 skipped。
fn push_target_or_skip(plan: &mut WakePlan, node: &NodeCandidate, sender_id: &str) {
    if plan.targets.iter().any(|t| t == &node.id)
        || plan.skipped.iter().any(|s| s.node_id == node.id)
    {
        return; // 已处理过（@ 多次点名同一节点）。
    }
    if node.id == sender_id {
        plan.skipped.push(SkipRecord {
            node_id: node.id.clone(),
            reason: "sender",
        });
        return;
    }
    if node.online {
        plan.targets.push(node.id.clone());
    } else {
        plan.skipped.push(SkipRecord {
            node_id: node.id.clone(),
            reason: "offline",
        });
    }
}

#[cfg(test)]
mod tests;
