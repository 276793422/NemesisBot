//! matcher 纯函数测试（Swarm M1）：硬条件过滤 / 加分 / 负载排序 / 确定性。

use super::*;

fn peer(id: &str, role: &str, tags: &[&str], caps: &[&str]) -> PeerCandidate {
    PeerCandidate {
        id: id.to_string(),
        name: format!("node-{id}"),
        role: role.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
    }
}

fn load(pairs: &[(&str, usize)]) -> HashMap<String, usize> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn no_constraints_prefers_least_loaded() {
    let peers = vec![
        peer("a", "worker", &[], &[]),
        peer("b", "worker", &[], &[]),
        peer("c", "manager", &[], &[]),
    ];
    // 无任何约束：全员候选，负载最低优先。
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            description: "",
        },
        &peers,
        &load(&[("a", 3), ("b", 0), ("c", 9)]),
    );
    assert_eq!(ranked[0].0, "b", "零约束时最闲节点优先");
    assert_eq!(ranked.len(), 3, "零约束不排除任何节点");
}

#[test]
fn required_role_is_a_hard_filter() {
    let peers = vec![
        peer("dev1", "worker", &["rust"], &[]),
        peer("qa1", "worker", &["testing"], &[]),
    ];
    // 需要 qa 角色但无人自报 qa → 空（诚实留 backlog，不硬塞 worker）。
    let ranked = rank_peers(
        &MatchInput {
            required_role: Some("qa"),
            required_tags: &[],
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert!(ranked.is_empty(), "角色无人命中必须空结果");
}

#[test]
fn required_role_matches_case_insensitively() {
    let peers = vec![peer("w1", "Worker", &[], &[])];
    let ranked = rank_peers(
        &MatchInput {
            required_role: Some("worker"),
            required_tags: &[],
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1);
}

#[test]
fn required_tags_are_a_hard_filter() {
    let peers = vec![
        peer("rusty", "worker", &["rust", "backend"], &[]),
        peer("webby", "worker", &["frontend"], &[]),
    ];
    let tags = vec!["rust".to_string()];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &tags,
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].0, "rusty");
}

#[test]
fn tag_hits_outrank_capability_bonus() {
    let peers = vec![
        peer(
            "taggy",
            "worker",
            &["rust", "backend", "cli"],
            &["rust", "docker"],
        ),
        peer(
            "cappy",
            "worker",
            &["rust"],
            &["rust", "docker", "git", "cargo"],
        ),
    ];
    let tags = vec!["rust".to_string(), "backend".to_string()];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &tags,
            description: "用 rust 重写 cli 工具并打 docker 镜像",
        },
        &peers,
        &load(&[]),
    );
    // 两人都过硬条件（都有 rust 标签）。taggy：2 标签命中 ×100 = 200 +
    // cap（rust、docker）2×5=10 → 210；cappy：1 命中 = 100 + 10 → 110。
    // 多一个标签命中的权重增量必须压倒任何加分差异。
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].0, "taggy");
    assert_eq!(ranked[0].1, 210);
    assert_eq!(ranked[1].1, 110);
}

#[test]
fn capability_bonus_is_capped() {
    let caps = ["aa", "bb", "cc", "dd", "ee"];
    let peers = vec![peer("p", "worker", &[], &caps)];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            description: "aa bb cc dd ee 全命中",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked[0].1, 15, "加分封顶 15（3 hit × 5）");
}

#[test]
fn same_score_prefers_lighter_node() {
    let peers = vec![
        peer("busy", "worker", &["rust"], &[]),
        peer("idle", "worker", &["rust"], &[]),
    ];
    let tags = vec!["rust".to_string()];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &tags,
            description: "",
        },
        &peers,
        &load(&[("busy", 2), ("idle", 0)]),
    );
    assert_eq!(ranked[0].0, "idle", "同分选负载低者");
    // pick_peer 便捷封装取同一人。
    let picked = pick_peer(
        &MatchInput {
            required_role: None,
            required_tags: &tags,
            description: "",
        },
        &peers,
        &load(&[("busy", 2), ("idle", 0)]),
    );
    assert_eq!(picked.as_deref(), Some("idle"));
}

#[test]
fn empty_peers_yields_empty_result() {
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            description: "x",
        },
        &[],
        &load(&[]),
    );
    assert!(ranked.is_empty());
    assert!(
        pick_peer(
            &MatchInput {
                required_role: None,
                required_tags: &[],
                description: "x"
            },
            &[],
            &load(&[])
        )
        .is_none()
    );
}

#[test]
fn missing_load_entry_defaults_to_zero() {
    let peers = vec![peer("unknown", "worker", &[], &[])];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            description: "",
        },
        &peers,
        &load(&[]), // 负载表缺该节点
    );
    assert_eq!(ranked[0].1, 0, "缺负载条目按 0 处理，不得 panic");
}

#[test]
fn ranking_is_deterministic_for_full_ties() {
    // 完全同分同载 → id 字典序兜底，两次调用结果一致。
    let peers = vec![
        peer("zz", "worker", &[], &[]),
        peer("aa", "worker", &[], &[]),
    ];
    let input = MatchInput {
        required_role: None,
        required_tags: &[],
        description: "",
    };
    let empty = load(&[]);
    let r1 = rank_peers(&input, &peers, &empty);
    let r2 = rank_peers(&input, &peers, &empty);
    assert_eq!(r1, r2);
    assert_eq!(r1[0].0, "aa");
}
