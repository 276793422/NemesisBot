// arbitrator.rs 覆盖率补充测试（规则④指派节点离线 147-151 / `@role:` 空
// 角色名跳过臂 174-178）。

use super::*;
use crate::models::thread_kind;

fn node(id: &str, name: &str, role: &str, online: bool) -> NodeCandidate {
    NodeCandidate {
        id: id.to_string(),
        name: name.to_string(),
        role: role.to_string(),
        category: String::new(),
        online,
    }
}

/// 规则④：issue 评论无 @ 且指派节点离线 → skipped(offline)（147-151）。
#[test]
fn rule4_offline_assignee_goes_to_skipped() {
    let nodes = [node("n1", "甲", "worker", false)];
    let input = WakeInput {
        thread_kind: thread_kind::ISSUE,
        content: "没有点名的评论",
        sender_id: "master",
        issue_assignee: Some("n1"),
        moderator_id: "master",
    };
    let plan = resolve_wake_targets(&input, &nodes);
    assert!(plan.targets.is_empty());
    assert_eq!(
        plan.skipped,
        vec![SkipRecord {
            node_id: "n1".to_string(),
            reason: "offline",
        }]
    );
}

/// `@role:`（冒号后为空）→ 跳过该 token（174-178），不拦同消息里其他
/// 有效点名。
#[test]
fn empty_role_token_is_skipped_but_other_tokens_resolve() {
    let nodes = [node("n1", "甲", "worker", true)];
    let input = WakeInput {
        thread_kind: thread_kind::CHANNEL,
        content: "@role: @n1 看一下",
        sender_id: "master",
        issue_assignee: None,
        moderator_id: "master",
    };
    let plan = resolve_wake_targets(&input, &nodes);
    // 空 role token 进 skipped，n1 照常定点唤醒。
    assert!(plan.skipped.iter().any(|s| s.node_id == "@role:"));
    assert!(plan.targets.iter().any(|t| t == "n1"));
}
