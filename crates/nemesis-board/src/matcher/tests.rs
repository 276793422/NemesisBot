//! matcher 纯函数测试（Swarm M1）：硬条件过滤 / 加分 / 负载排序 / 确定性。

use super::*;

fn peer(id: &str, role: &str, tags: &[&str], caps: &[&str]) -> PeerCandidate {
    prof_peer(id, role, tags, caps, &[], None)
}

fn prof_peer(
    id: &str,
    role: &str,
    tags: &[&str],
    caps: &[&str],
    professions: &[&str],
    tier: Option<&str>,
) -> PeerCandidate {
    PeerCandidate {
        id: id.to_string(),
        name: format!("node-{id}"),
        role: role.to_string(),
        tags: tags.iter().map(|s| s.to_string()).collect(),
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        professions: professions.iter().map(|s| s.to_string()).collect(),
        tier: tier.map(str::to_string),
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
            required_profession: None,
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
                required_profession: None,
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
            required_profession: None,
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
        required_profession: None,
        description: "",
    };
    let empty = load(&[]);
    let r1 = rank_peers(&input, &peers, &empty);
    let r2 = rank_peers(&input, &peers, &empty);
    assert_eq!(r1, r2);
    assert_eq!(r1[0].0, "aa");
}

// -----------------------------------------------------------------------
// 职能匹配（集群专业职能框架 M2）：硬条件 3（精确命中）+ 硬条件 4（tier 闸）
// -----------------------------------------------------------------------

#[test]
fn required_profession_exact_match_is_a_hard_filter() {
    let peers = vec![
        prof_peer("dev", "worker", &[], &[], &["dev"], Some("normal")),
        prof_peer("cpp", "worker", &[], &[], &["dev:cpp"], Some("big")),
    ];
    // 精确匹配无继承：要 dev:cpp 时只有宣告 dev:cpp 的节点入选。
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("dev:cpp"),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].0, "cpp");
    // 反向亦然：要 dev 时宣告 dev:cpp 的不命中（无继承）。
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("dev"),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].0, "dev");
}

#[test]
fn required_profession_matches_case_insensitively() {
    let peers = vec![prof_peer(
        "n",
        "worker",
        &[],
        &[],
        &["Dev:CPP"],
        Some("big"),
    )];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("dev:cpp"),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1, "手写大小写差异归一命中");
}

#[test]
fn required_profession_empty_string_means_unconstrained() {
    let peers = vec![prof_peer("n", "worker", &[], &[], &[], None)];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("  "),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1, "空白职能 = 条件跳过");
}

#[test]
fn unknown_profession_slug_blocks_everyone() {
    let peers = vec![prof_peer("n", "worker", &[], &[], &["dev"], Some("big"))];
    // 合法但目录外（未来用户专业）→ 无人命中 → 空（诚实留 backlog）。
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("dev:cuda"),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert!(ranked.is_empty());
}

#[test]
fn tier_gate_blocks_known_lower_tier_and_fails_open_on_unknown() {
    let peers = vec![
        // 已知档位低于门槛（architecture min_tier=big）：排除。
        prof_peer(
            "small",
            "worker",
            &[],
            &[],
            &["architecture"],
            Some("normal"),
        ),
        // 未知档位（None，旧节点）：fail-open 放行（D3）。
        prof_peer("legacy", "worker", &[], &[], &["architecture"], None),
    ];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: Some("architecture"),
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1, "已知低档被 tier 闸排除，未知档 fail-open");
    assert_eq!(ranked[0].0, "legacy");
}

#[test]
fn tier_gate_does_not_apply_without_profession() {
    // 无职能需求时节点 tier 与匹配无关（门槛是职能的函数）。
    let peers = vec![prof_peer("n", "worker", &[], &[], &[], Some("mini"))];
    let ranked = rank_peers(
        &MatchInput {
            required_role: None,
            required_tags: &[],
            required_profession: None,
            description: "",
        },
        &peers,
        &load(&[]),
    );
    assert_eq!(ranked.len(), 1);
}

// -----------------------------------------------------------------------
// D13 松弛阶梯：②保职能丢标签 → ③丢职能（保角色/标签）→ ④全松弛；
// tier 门槛随职能走、不单独松弛。
// -----------------------------------------------------------------------

#[test]
fn relaxed_ladder_rung2_keeps_profession_drops_tags() {
    let peers = vec![prof_peer(
        "prof",
        "worker",
        &["rust"],
        &[],
        &["dev:cpp"],
        Some("big"),
    )];
    let tags = vec!["windrv".to_string()];
    let base = MatchInput {
        required_role: Some("worker"),
        required_tags: &tags,
        required_profession: Some("dev:cpp"),
        description: "",
    };
    // 严格匹配落空（windrv 标签与节点 rust 无交集；tags 硬条件是交集
    // 非空语义）→ ②保职能丢标签命中同一人。
    assert!(rank_peers(&base, &peers, &load(&[])).is_empty());
    let (id, label) = pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Role).expect("②级应命中");
    assert_eq!(id, "prof");
    assert_eq!(label, "保职能松弛标签兜底");
}

#[test]
fn relaxed_ladder_rung3_drops_profession_keeps_role_tags() {
    let peers = vec![prof_peer(
        "generic",
        "worker",
        &["rust"],
        &[],
        &[],
        Some("big"),
    )];
    let tags = vec!["rust".to_string()];
    let base = MatchInput {
        required_role: Some("worker"),
        required_tags: &tags,
        required_profession: Some("dev:cpp"),
        description: "",
    };
    // 严格（职能未宣告）与②（②也需要职能命中）都落空 → ③丢职能保角色/标签。
    let (id, label) = pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Role).expect("③级应命中");
    assert_eq!(id, "generic");
    assert_eq!(label, "松弛职能兜底（角色/标签保留）");
}

#[test]
fn relaxed_ladder_rung4_full_relaxation() {
    let peers = vec![prof_peer("anyone", "worker", &[], &[], &[], Some("mini"))];
    let tags = vec!["nobody-has-this".to_string()];
    let base = MatchInput {
        required_role: Some("worker"),
        required_tags: &tags,
        required_profession: Some("dev:cpp"),
        description: "",
    };
    // Role 档（2026-09-29 三档化）④不可达：只有④能救的局诚实返回 None。
    assert!(
        pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Role).is_none(),
        "Role 档不得走到④全松弛（角色纪律保留）"
    );
    let (id, label) = pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Full).expect("④级应命中");
    assert_eq!(id, "anyone");
    assert_eq!(label, "无匹配节点，全松弛兜底（职能/角色/标签均放开）");
}

#[test]
fn relaxed_ladder_without_profession_preserves_old_behavior() {
    let peers = vec![prof_peer("w", "worker", &["rust"], &[], &[], None)];
    let tags = vec!["windrv".to_string()];
    let base = MatchInput {
        required_role: Some("worker"),
        required_tags: &tags,
        required_profession: None,
        description: "",
    };
    // 无职能：②蜕化为旧「保角色丢标签」一级，③ 跳过。
    let (id, label) =
        pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Role).expect("旧①级应命中");
    assert_eq!(id, "w");
    assert_eq!(label, "无标签匹配节点，保角色松弛兜底");
}

#[test]
fn relaxed_ladder_never_relaxes_profession_via_tier() {
    // tier 闸在松弛级照常生效：唯一候选 tier 已知且低于 min_tier →
    // ②保职能级不因松弛放行（D13：tier 不参与松弛），最终落到④丢职能。
    let peers = vec![prof_peer(
        "small",
        "worker",
        &[],
        &[],
        &["architecture"],
        Some("normal"), // architecture min_tier=big → 闸住
    )];
    let base = MatchInput {
        required_role: Some("worker"),
        required_tags: &[],
        required_profession: Some("architecture"),
        description: "",
    };
    let (id, label) =
        pick_relaxed(&base, &peers, &load(&[]), RelaxDepth::Full).expect("松弛阶梯应命中");
    assert_eq!(id, "small");
    assert_ne!(
        label, "保职能松弛标签兜底",
        "②保职能级必须被 tier 闸挡住：不允许经松弛把门槛降级绕过（D13）"
    );
}

#[test]
fn relaxed_ladder_returns_none_when_nothing_matches() {
    let base = MatchInput {
        required_role: None,
        required_tags: &[],
        required_profession: None,
        description: "",
    };
    assert!(pick_relaxed(&base, &[], &load(&[]), RelaxDepth::Full).is_none());
}
