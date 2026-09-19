//! 批次四（2026-09-19）：节点显示名测试。
//!
//! 覆盖 goal 测试面：解析链优先级、COMPUTERNAME 注入（经纯函数参数
//! 注入——env 是进程全局且 Rust 2024 `set_var` 是 unsafe，并行测试不能
//! 真注入，`resolve_auto_node_name` 因此拆成三元纯函数 + env 薄包装）、
//! 撞名后缀收敛、config 显式覆盖 / 撞名免疫。

use super::*;

// -- 解析链纯函数（三元直测，零 env 依赖） ------------------------------------

#[test]
fn resolve_chain_config_explicit_locked() {
    // config.cluster.json node_name 非空 → 用它 + 显式锁定（免疫撞名后缀）。
    let (name, locked) = resolve_auto_node_name("MyBot", Some("DESKTOP-X".into()), "node-aaa");
    assert_eq!(name, "MyBot");
    assert!(locked);
}

#[test]
fn resolve_chain_config_whitespace_falls_through() {
    // 纯空白 = 未配置（trim 后为空）→ 落 hostname 链。
    let (name, locked) = resolve_auto_node_name("   ", Some("DESKTOP-X".into()), "node-aaa");
    assert_eq!(name, "DESKTOP-X");
    assert!(!locked);
}

#[test]
fn resolve_chain_hostname_auto() {
    // config 空 + hostname 可用 → 用 hostname（自动名，可被撞名收敛）。
    let (name, locked) = resolve_auto_node_name("", Some("DESKTOP-X".into()), "node-aaa");
    assert_eq!(name, "DESKTOP-X");
    assert!(!locked);
}

#[test]
fn resolve_chain_bot_fallback() {
    // config 空 + hostname 不可用（纯容器）→ `Bot {id8}` 兜底。
    let (name, locked) = resolve_auto_node_name("", None, "local-node-001");
    assert_eq!(name, "Bot local-no");
    assert!(!locked);
}

#[test]
fn resolve_chain_short_node_id_no_panic() {
    // node_id 短于 8 位（畸形但防御性容忍）→ 截断不 panic。
    let (name, _) = resolve_auto_node_name("", None, "");
    assert_eq!(name, "Bot ");
}

#[test]
fn resolve_chain_config_beats_hostname() {
    // 优先级：config 显式 > hostname（即使两者都给）。
    let (name, locked) = resolve_auto_node_name("MyBot", Some("DESKTOP-X".into()), "node-aaa");
    assert_eq!(name, "MyBot");
    assert!(locked);
}

#[test]
fn env_wrapper_matches_pure_core() {
    // env 薄包装 = 纯函数核（实际注入 hostname_display_name() 结果）——
    // 两条路径等价性，防将来包装里塞进额外逻辑。
    let (a, la) = resolve_node_name_from_env("MyBot", "node-aaa");
    let (b, lb) = resolve_auto_node_name("MyBot", hostname_display_name(), "node-aaa");
    assert_eq!(a, b);
    assert_eq!(la, lb);
}

// -- 撞名后缀收敛（Cluster 层，经 handle_discovered_node） --------------------

/// 空显式名构造 → Windows/Linux 测试机走 hostname 链（locked=false），
/// 随后 set_node_name 预置本地名（不碰 locked 标志）——撞名收敛可触发。
fn make_config_auto_name() -> ClusterConfig {
    ClusterConfig {
        node_id: "local-node-001".into(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec!["127.0.0.1:9001".into()],
        node_name: String::new(),
    }
}

/// 显式名构造（config.cluster.json node_name 路径，locked=true）。
fn make_config_explicit_name() -> ClusterConfig {
    ClusterConfig {
        node_id: "local-node-001".into(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec!["127.0.0.1:9001".into()],
        node_name: "local-node".into(),
    }
}

fn announce(cluster: &Cluster, node_id: &str, name: &str) -> bool {
    cluster.handle_discovered_node(
        node_id,
        name,
        vec!["10.0.0.9".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    )
}

#[test]
fn name_suffix_uses_last_segment_head() {
    // 生产 id 形态：末段 = uuid → 后缀有区分度（真机彩排 2026-09-19：
    // 「前 4 位」恒为 "node-" 使双方收敛出相同名字，故取末段前 4）。
    assert_eq!(
        name_suffix_from_node_id("node-yangjian-6280d9dd-231a"),
        "231a"
    );
    // 畸形/短 id 防御：无 '-' 取全串前 4；空串取空。
    assert_eq!(name_suffix_from_node_id("ab"), "ab");
    assert_eq!(name_suffix_from_node_id(""), "");
    // 测试 id 形态：末段 "001" → "001"。
    assert_eq!(name_suffix_from_node_id("local-node-001"), "001");
}

#[test]
fn collision_suffix_applied_on_duplicate_name() {
    let cluster = Cluster::new(make_config_auto_name());
    cluster.start();

    // 预置：本地自动名（locked=false）→ 手动改名模拟与对端同名的现场。
    cluster.set_node_name("DESKTOP-X");
    assert_eq!(cluster.node_name(), "DESKTOP-X");

    // 对端 announce 同名异 id → 本地名追加 `-` + 自己 id 末段前 4 位。
    assert!(announce(&cluster, "remote-999", "DESKTOP-X"));
    let expected = format!("DESKTOP-X-{}", name_suffix_from_node_id(cluster.node_id()));
    assert_eq!(cluster.node_name(), expected);
}

#[test]
fn collision_ignored_when_config_explicit() {
    let cluster = Cluster::new(make_config_explicit_name()); // 显式 "local-node" → locked
    cluster.start();
    assert!(
        cluster
            .node_name_locked
            .load(std::sync::atomic::Ordering::Relaxed)
    );

    // 对端 announce 同名异 id → 显式名免疫，不变。
    assert!(announce(&cluster, "remote-999", "local-node"));
    assert_eq!(cluster.node_name(), "local-node");
}

#[test]
fn collision_ignored_for_self_announce() {
    let cluster = Cluster::new(make_config_auto_name());
    cluster.start();
    cluster.set_node_name("DESKTOP-X");

    // id 过滤：自己（announce 回环/重放）同名不触发收敛。
    assert!(announce(&cluster, "local-node-001", "DESKTOP-X"));
    assert_eq!(cluster.node_name(), "DESKTOP-X");
}

#[test]
fn collision_ignored_for_empty_announced_name() {
    let cluster = Cluster::new(make_config_auto_name());
    cluster.start();
    cluster.set_node_name("DESKTOP-X");

    // 畸形 announce（空名）不触发收敛。
    assert!(announce(&cluster, "remote-999", ""));
    assert_eq!(cluster.node_name(), "DESKTOP-X");
}

#[test]
fn collision_two_sides_converge_to_distinct_names() {
    // 真机彩排回归（2026-09-19）：生产 id 形如 node-{host}-{uuid}，「前
    // 4 位」恒 "node" → 双方各自加缀后仍同名。末段前 4 后必须不同。
    let a = Cluster::new(ClusterConfig {
        node_id: "node-yangjian-6280d9dd-231a-49c7".into(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec![],
        node_name: String::new(),
    });
    let b = Cluster::new(ClusterConfig {
        node_id: "node-yangjian-9db0c5c9-f4cc-4c89".into(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec![],
        node_name: String::new(),
    });
    a.set_node_name("YANGJIAN");
    b.set_node_name("YANGJIAN");

    // 各自收到对方的同名 announce。
    assert!(announce(&a, b.node_id(), "YANGJIAN"));
    assert!(announce(&b, a.node_id(), "YANGJIAN"));

    let name_a = a.node_name();
    let name_b = b.node_name();
    assert_ne!(name_a, name_b, "收敛后双方名字必须可区分");
    assert_eq!(name_a, "YANGJIAN-49c7"); // node-yangjian-6280d9dd-231a-49c7 末段
    assert_eq!(name_b, "YANGJIAN-4c89"); // node-yangjian-9db0c5c9-f4cc-4c89 末段
}

#[test]
fn collision_only_fires_once_per_name() {
    let cluster = Cluster::new(make_config_auto_name());
    cluster.start();
    cluster.set_node_name("DESKTOP-X");

    // 第一次撞名：DESKTOP-X → DESKTOP-X-{末段前4}。
    assert!(announce(&cluster, "remote-999", "DESKTOP-X"));
    let suffixed = format!("DESKTOP-X-{}", name_suffix_from_node_id(cluster.node_id()));
    assert_eq!(cluster.node_name(), suffixed);

    // 第二次同名 announce：本地名已带后缀 ≠ announced name → 不再追加
    // （后缀不叠罗汉；且后续 announce 携带新名后对端也收敛到同结论）。
    // 返回值断言：重复 announce 内容无变化 → false（handle_discovered_node
    // 返回「是否有更新」），这里只关心名字不被二次加缀。
    let _ = announce(&cluster, "remote-999", "DESKTOP-X");
    assert_eq!(cluster.node_name(), suffixed);
}
