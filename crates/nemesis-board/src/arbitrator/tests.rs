//! 裁决器单测（纯函数，零依赖）。

use super::*;

fn node(id: &str, name: &str, role: &str, online: bool) -> NodeCandidate {
    NodeCandidate {
        id: id.to_string(),
        name: name.to_string(),
        role: role.to_string(),
        category: String::new(),
        online,
    }
}

fn input<'a>(thread_kind: &'a str, content: &'a str, sender: &'a str) -> WakeInput<'a> {
    WakeInput {
        thread_kind,
        content,
        sender_id: sender,
        issue_assignee: None,
        moderator_id: "master",
    }
}

/// 规则①：@name / @id 均可命中，大小写不敏感；离线目标进 skipped。
#[test]
fn test_rule1_direct_mention_by_id_and_name() {
    let nodes = vec![
        node("node-b", "Builder", "worker", true),
        node("node-c", "QA-1", "worker", false),
    ];
    // @id 精准命中在线节点。
    let plan = resolve_wake_targets(&input("channel", "@node-b 请看这条", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);
    assert!(plan.skipped.is_empty());
    assert!(!plan.to_moderator);

    // @name 也命中（大小写不敏感）。
    let plan = resolve_wake_targets(&input("channel", "@builder 在吗", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);

    // 离线节点：不在 targets，进 skipped(reason=offline)。
    let plan = resolve_wake_targets(&input("channel", "@node-c 看看", "admin"), &nodes);
    assert!(plan.targets.is_empty());
    assert_eq!(
        plan.skipped,
        vec![SkipRecord {
            node_id: "node-c".to_string(),
            reason: "offline"
        }]
    );
}

/// 规则②：@role:qa 该角色全投；category 与 role 都参与匹配（集群里
/// 功能性角色落 category）；空/未命中角色诚实记 not_found。
#[test]
fn test_rule2_role_mention_fans_out() {
    let nodes = vec![
        node("qa-1", "QA One", "QA", true),
        node("qa-2", "QA Two", "qa", true),
        node("dev-1", "Dev", "worker", true),
        node("qa-3", "QA Offline", "qa", false),
    ];
    let plan = resolve_wake_targets(&input("channel", "@role:QA 都过一遍", "admin"), &nodes);
    assert_eq!(
        plan.targets,
        vec!["qa-1", "qa-2"],
        "角色全投（大小写不敏感）"
    );
    assert_eq!(
        plan.skipped,
        vec![SkipRecord {
            node_id: "qa-3".to_string(),
            reason: "offline"
        }]
    );

    // category 匹配：拓扑角色都是 worker，功能类别 qa 在 category 里。
    let nodes = vec![
        NodeCandidate {
            id: "w1".to_string(),
            name: "W1".to_string(),
            role: "worker".to_string(),
            category: "qa".to_string(),
            online: true,
        },
        NodeCandidate {
            id: "w2".to_string(),
            name: "W2".to_string(),
            role: "worker".to_string(),
            category: "development".to_string(),
            online: true,
        },
    ];
    let plan = resolve_wake_targets(&input("channel", "@role:qa 关注一下", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["w1"], "category 命中");

    // 未命中角色 → not_found。
    let nodes = vec![node("dev-1", "Dev", "worker", true)];
    let plan = resolve_wake_targets(&input("channel", "@role:security 看看", "admin"), &nodes);
    assert!(plan.targets.is_empty());
    assert_eq!(plan.skipped[0].reason, "not_found");
}

/// 规则③：频道消息无 @ → 只投主持人（to_moderator）。
#[test]
fn test_rule3_channel_without_mention_goes_to_moderator() {
    let nodes = vec![node("node-b", "Builder", "worker", true)];
    let plan = resolve_wake_targets(&input("channel", "大家怎么看？", "admin"), &nodes);
    assert!(plan.targets.is_empty());
    assert!(plan.to_moderator);

    // issue 线程不走规则③。
    let plan = resolve_wake_targets(&input("issue", "补充说明", "admin"), &nodes);
    assert!(!plan.to_moderator);
}

/// 规则④：issue 评论无 @ → 投指派节点；未指派/未知指派诚实记录。
#[test]
fn test_rule4_issue_comment_routes_to_assignee() {
    let nodes = vec![
        node("node-b", "Builder", "worker", true),
        node("node-x", "Ghost", "worker", true),
    ];
    let mut inp = input("issue", "这个 bug 我复现了", "admin");
    inp.issue_assignee = Some("node-b");
    let plan = resolve_wake_targets(&inp, &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);

    // 未指派 → no_assignee。
    let plan = resolve_wake_targets(&input("issue", "补充", "admin"), &nodes);
    assert_eq!(plan.skipped[0].reason, "no_assignee");

    // 指派 id 不在节点表 → not_found。
    let mut inp = input("issue", "补充", "admin");
    inp.issue_assignee = Some("gone-node");
    let plan = resolve_wake_targets(&inp, &nodes);
    assert_eq!(plan.skipped[0].reason, "not_found");
}

/// 规则④发送者排除（BUG-20261025-17）：worker 在自己被派的单上发进度/
/// 交付评论（无 @）→ 指派节点就是发送者本人，按 push_target_or_skip 同
/// 语义 skip(sender)——否则 worker 被自己的消息唤醒，自问自答循环烧线程
/// 额度。人类发送者 id 不在节点表，天然不受影响（同测对照）。
#[test]
fn test_rule4_assignee_is_sender_skipped_not_woken() {
    let nodes = vec![node("node-b", "Builder", "worker", true)];

    // worker 本人评论自己被派的单：不唤醒，诚实记 sender。
    let mut inp = input("issue", "进度更新：模块 A 已完成，交付评论见下。", "node-b");
    inp.issue_assignee = Some("node-b");
    let plan = resolve_wake_targets(&inp, &nodes);
    assert!(plan.targets.is_empty(), "发送者本人不得被规则④唤醒");
    assert_eq!(
        plan.skipped,
        vec![SkipRecord {
            node_id: "node-b".to_string(),
            reason: "sender"
        }]
    );
    assert!(!plan.to_moderator);

    // 对照：同指派、人类发送者（admin 不在节点表）照常唤醒 worker。
    let mut inp = input("issue", "辛苦了，请继续。", "admin");
    inp.issue_assignee = Some("node-b");
    let plan = resolve_wake_targets(&inp, &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);
    assert!(plan.skipped.is_empty());
}

/// ①②可混用（并集、按解析顺序去重）；不唤醒发送者本人。
#[test]
fn test_mixed_mentions_dedupe_and_skip_sender() {
    let nodes = vec![
        node("node-b", "Builder", "worker", true),
        node("qa-1", "QA", "qa", true),
        node("node-s", "Self", "worker", true),
    ];
    // 同一节点被 @ 两次只投一次；发送者本人不唤醒；@role:worker 命中
    // 两个 worker 之一已在 targets（去重）、另一个是 sender（跳过）。
    let plan = resolve_wake_targets(
        &input(
            "channel",
            "@node-b @Builder @node-s @role:worker 来",
            "node-s",
        ),
        &nodes,
    );
    assert_eq!(plan.targets, vec!["node-b"]);
    assert_eq!(
        plan.skipped,
        vec![SkipRecord {
            node_id: "node-s".to_string(),
            reason: "sender"
        }]
    );
}

/// 邮箱形态不误判为点名；未命中点名叫 not_found 不静默。
#[test]
fn test_mention_extraction_edges() {
    let nodes = vec![node("node-b", "Builder", "worker", true)];
    // 邮箱：字母后的 @ 不算点名 → 无 @ 走规则③。
    let plan = resolve_wake_targets(
        &input("channel", "发到 admin@example.com 去吧", "admin"),
        &nodes,
    );
    assert!(plan.targets.is_empty());
    assert!(plan.to_moderator);
    assert!(plan.skipped.is_empty());

    // 中文标点截断：@node-b，后面的逗号不属于名字。
    let plan = resolve_wake_targets(&input("channel", "@node-b，看下", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);

    // @ 不存在的节点 → not_found（审计可见，不静默吞）。
    let plan = resolve_wake_targets(&input("channel", "@nobody 在吗", "admin"), &nodes);
    assert_eq!(plan.skipped[0].reason, "not_found");
    assert_eq!(plan.skipped[0].node_id, "@nobody");
}

/// worker 侧 board.sync 过滤共用的 [`mentions_node`]（与规则①②同一匹配
/// 语义——单一真相源）与 [`has_mentions`] 事件标签判定。
#[test]
fn test_mentions_node_and_has_mentions() {
    // @id / @name 大小写不敏感。
    assert!(mentions_node(
        "请 @Node-B 看下",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    assert!(mentions_node(
        "@BUILDER 在吗",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    // 未点名的他人不命中。
    assert!(!mentions_node(
        "@node-c 看下",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    // @role: 命中拓扑角色或功能类别。
    assert!(mentions_node(
        "@role:worker 集合",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    assert!(mentions_node(
        "@role:DEV 集合",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    assert!(!mentions_node(
        "@role:qa 集合",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    // @role: 空角色不命中（不是无条件真）。
    assert!(!mentions_node(
        "@role: 看下",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    // 邮箱形态不算点名。
    assert!(!mentions_node(
        "发给 b@node-b.com",
        "node-b",
        "Builder",
        "worker",
        "dev"
    ));
    // has_mentions：@ 存在性（master 决定 wake 事件标签）。
    assert!(has_mentions("@node-b 看下"));
    assert!(has_mentions("@role:qa 过一遍"));
    assert!(!has_mentions("大家辛苦了"));
    assert!(!has_mentions("邮箱 a@b.com 就好"));
}

// ---------------------------------------------------------------------------
// goal P1/F2：@all 群发（用户拍板计入）。
// ---------------------------------------------------------------------------

/// @all：唤醒全部在线其他节点；发送者本人排除；离线 → skipped(offline)。
#[test]
fn test_at_all_wakes_online_others_excludes_sender_and_offline() {
    let nodes = vec![
        node("master", "Zoo", "manager", true),
        node("node-b", "Alex", "worker", true),
        node("node-c", "Bob", "worker", false),
    ];
    let plan = resolve_wake_targets(&input("channel", "@all 注意一下", "master"), &nodes);
    // 在线其他节点全部唤醒（大小写无关，大小写不敏感解析后 token="all"）。
    assert_eq!(plan.targets, vec!["node-b"]);
    assert!(!plan.to_moderator);
    // 离线节点进 skipped（offline），发送者进 skipped（sender）。
    assert!(
        plan.skipped
            .iter()
            .any(|s| s.node_id == "node-c" && s.reason == "offline")
    );
    assert!(
        plan.skipped
            .iter()
            .any(|s| s.node_id == "master" && s.reason == "sender")
    );
}

/// @all 大小写不敏感（@ALL 同样命中）。
#[test]
fn test_at_all_case_insensitive() {
    let nodes = vec![
        node("master", "Zoo", "manager", true),
        node("node-b", "Alex", "worker", true),
    ];
    let plan = resolve_wake_targets(&input("channel", "@ALL 集合", "master"), &nodes);
    assert_eq!(plan.targets, vec!["node-b"]);
}

/// mentions_node 对 @all 恒真（board.sync 补拉兜底的同源匹配：离线期间
/// 错过的 @all，上线补拉时按此过滤入队）。
#[test]
fn test_mentions_node_at_all_matches_any_node() {
    assert!(mentions_node(
        "@all 集合",
        "node-b",
        "Alex",
        "worker",
        "general"
    ));
    assert!(mentions_node("@All 集合", "node-c", "Bob", "worker", "qa"));
    // 非 @all 消息不误命中。
    assert!(!mentions_node(
        "大家注意",
        "node-b",
        "Alex",
        "worker",
        "general"
    ));
}

// ---------------------------------------------------------------------------
// 名册感知 @ 解析：含空格节点名可被 @整体点名（2026-10-23 用户实录：
// "@Product Manager" 旧解析截成 token "Product" → not_found 无法点名）。
// ---------------------------------------------------------------------------

/// @含空格名：resolve 直击（大小写不敏感）；发送者排除照旧。
#[test]
fn test_mention_with_spaces_in_name() {
    let nodes = vec![
        node("master", "Zoo", "manager", true),
        node("node-pm", "Product Manager", "worker", true),
        node("node-dev", "Dev", "worker", true),
    ];
    let plan = resolve_wake_targets(
        &input("channel", "@Product Manager 你看一下", "master"),
        &nodes,
    );
    assert_eq!(plan.targets, vec!["node-pm"]);
    assert!(plan.skipped.is_empty());

    // 大小写不敏感 + 中文紧跟边界。
    let plan = resolve_wake_targets(
        &input("channel", "@product manager，在吗", "master"),
        &nodes,
    );
    assert_eq!(plan.targets, vec!["node-pm"]);

    // @id 形态（id 无空格）照旧。
    let plan = resolve_wake_targets(&input("channel", "@node-pm 看下", "master"), &nodes);
    assert_eq!(plan.targets, vec!["node-pm"]);
}

/// 最长匹配优先："Product" 与 "Product Manager" 并存时 @Product Manager
/// 命中后者；@Product 命中前者。
#[test]
fn test_mention_longest_match_wins() {
    let nodes = vec![
        node("n1", "Product", "worker", true),
        node("n2", "Product Manager", "worker", true),
    ];
    let plan = resolve_wake_targets(
        &input("channel", "@Product Manager 请回应", "admin"),
        &nodes,
    );
    assert_eq!(plan.targets, vec!["n2"]);
    let plan = resolve_wake_targets(&input("channel", "@Product 请回应", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["n1"]);
    // 词边界优雅降级："@Product Managers" 的长名匹配被词边界拒绝（后跟
    // 字母），回落到词边界命中的短名 "Product"——与旧 token 行为一致。
    let plan = resolve_wake_targets(&input("channel", "@Product Managers 来", "admin"), &nodes);
    assert_eq!(plan.targets, vec!["n1"]);
}

/// 词边界保护：名字后紧跟字母/数字 = 嵌在更长词里，不当作点名
/// （"@Developer" 咬不住 "Dev"、"@Managerial" 咬不住 "Manager"）。
#[test]
fn test_mention_word_boundary_no_partial_bite() {
    let nodes = vec![
        node("n1", "Dev", "worker", true),
        node("n2", "Manager", "worker", true),
    ];
    let plan = resolve_wake_targets(&input("channel", "@Developer 请回应", "admin"), &nodes);
    assert!(plan.targets.is_empty());
    assert_eq!(plan.skipped[0].reason, "not_found");
    let plan = resolve_wake_targets(&input("channel", "@Managerial 汇报", "admin"), &nodes);
    assert!(plan.targets.is_empty());
}

/// mentions_node（worker sync 兜底）同源：本节点名含空格可整体命中。
#[test]
fn test_mentions_node_multi_word_name() {
    // 本节点是 Product Manager：@全名命中。
    assert!(mentions_node(
        "@Product Manager 请回应",
        "node-pm",
        "Product Manager",
        "worker",
        "general"
    ));
    // 他人的 @ 不命中本节点。
    assert!(!mentions_node(
        "@Product Manager 请回应",
        "node-dev",
        "Dev",
        "worker",
        "general"
    ));
    // 单词名/role:/all 回落路径不回归。
    assert!(mentions_node(
        "@Dev 在吗",
        "node-dev",
        "Dev",
        "worker",
        "general"
    ));
    assert!(mentions_node(
        "@role:worker 集合",
        "node-dev",
        "Dev",
        "worker",
        "general"
    ));
    // 邮箱形态排除不回归（a@b.com 不是点名）。
    assert!(!mentions_node(
        "发到 a@Product Manager.com",
        "node-pm",
        "Product Manager",
        "worker",
        "general"
    ));
}
