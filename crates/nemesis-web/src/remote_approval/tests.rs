//! M5（2026-10-10 集群无人值守硬化）— remote_approval 单元测试。
//!
//! 覆盖：回执解析四态、廉价预检、注册表登记/查/摘/TTL 惰性清理/短号冲突
//! 加长、快照排序与字段、回源 relay 的 false 路径与失败留痕分支（真实
//! BoardStore + 裸 Cluster：rpc 未启动 → send_verdict 诚实 Err → ❌ 评论）。

use super::*;

/// 临时 BoardStore（board.rs tests 同款形态）。
fn temp_store(tag: &str) -> (std::path::PathBuf, nemesis_board::BoardStore) {
    let dir = std::env::temp_dir().join(format!(
        "nb_remote_approval_{tag}_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    (dir, store)
}

#[test]
fn parse_approval_reply_four_states() {
    // 命中（approve/deny 各一）。
    assert_eq!(
        parse_approval_reply("/approve a1b2c3d4"),
        Some((true, "a1b2c3d4"))
    );
    assert_eq!(
        parse_approval_reply("/deny a1b2c3d4"),
        Some((false, "a1b2c3d4"))
    );
    // 首尾空白容忍。
    assert_eq!(
        parse_approval_reply("  /approve xyz  \n"),
        Some((true, "xyz"))
    );
    // 不命中：无前缀 / 相近前缀 / 空号 / 带空白 id。
    assert_eq!(parse_approval_reply("hello /approve x"), None);
    assert_eq!(parse_approval_reply("/approved x"), None);
    assert_eq!(parse_approval_reply("/approve   "), None);
    assert_eq!(parse_approval_reply("/approve x y"), None);
    assert_eq!(parse_approval_reply(""), None);
}

#[test]
fn looks_like_approval_reply_cheap_gate() {
    assert!(looks_like_approval_reply("/approve abc"));
    assert!(looks_like_approval_reply("  /deny abc"));
    assert!(!looks_like_approval_reply("hello /approve abc"));
    assert!(!looks_like_approval_reply("/approved abc"));
    assert!(!looks_like_approval_reply("/approv"));
}

#[test]
fn registry_register_resolve_remove_roundtrip() {
    let reg = RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    };
    let entry = RemoteAskEntry {
        request_id: "11111111-aaaa-bbbb-cccc-dddddddddddd".into(),
        worker_node: "node-b".into(),
        operation: "process_exec".into(),
        target: "cargo build".into(),
        risk_level: "HIGH".into(),
        reason: "test ask".into(),
        issue_number: Some(7),
        timeout_secs: 300,
        created_at: std::time::Instant::now(),
    };
    let short = reg.register(entry.clone());
    assert_eq!(short, "11111111");
    // 短号与完整 id 双形态解析。
    assert_eq!(reg.resolve(&short).unwrap().worker_node, "node-b");
    assert_eq!(
        reg.resolve(entry.request_id.as_str()).unwrap().operation,
        "process_exec"
    );
    // 快照行字段。
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(
        snap[0].get("short_id").and_then(|v| v.as_str()),
        Some(short.as_str())
    );
    assert_eq!(
        snap[0].get("issue_number").and_then(|v| v.as_i64()),
        Some(7)
    );
    assert!(
        snap[0]
            .get("remaining_secs")
            .and_then(|v| v.as_u64())
            .is_some()
    );
    // 摘除：按完整 id 摘除返回短号；二次摘除 = None（幂等）。
    assert_eq!(
        reg.remove_by_request_id(&entry.request_id).as_deref(),
        Some(short.as_str())
    );
    assert_eq!(reg.remove_by_request_id(&entry.request_id), None);
    assert!(reg.resolve(&short).is_none());
}

#[test]
fn registry_short_id_collision_lengthens() {
    let reg = RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    };
    let mk = |rid: &str| RemoteAskEntry {
        request_id: rid.into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 300,
        created_at: std::time::Instant::now(),
    };
    let full_a = "abcdef12-1111-1111-1111-111111111111";
    let full_b = "abcdef12-2222-2222-2222-222222222222";
    let key_a = reg.register(mk(full_a));
    assert_eq!(key_a, "abcdef12");
    let key_b = reg.register(mk(full_b));
    // 8 位撞键 → 加长到 16。
    assert_eq!(key_b, "abcdef12-2222-22");
    // 长键可解析；短号仍归第一条。
    assert_eq!(reg.resolve(&key_b).unwrap().request_id, full_b);
    assert_eq!(reg.resolve("abcdef12").unwrap().request_id, full_a);
    // 同 request_id 重登 = 覆盖返回同键（防御性幂等）。
    assert_eq!(reg.register(mk(full_b)), key_b);
    assert_eq!(reg.snapshot().len(), 2);
}

#[test]
fn registry_ttl_sweep_expires_stale_entries() {
    let reg = RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    };
    // created_at 回拨 130s + timeout 60 → TTL = 120s < elapsed → 过期。
    // Instant::checked_sub 平台不支持（None）时诚实 SKIP（测试纪律）。
    let stale_created = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(130));
    let Some(stale_created) = stale_created else {
        eprintln!("SKIP: Instant::checked_sub unsupported on this platform");
        return;
    };
    reg.register(RemoteAskEntry {
        request_id: "stale-request-id-1".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 60,
        created_at: stale_created,
    });
    // 存活条目（未过期）不受清扫影响。
    reg.register(RemoteAskEntry {
        request_id: "fresh-request-id-2".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 300,
        created_at: std::time::Instant::now(),
    });
    assert!(
        reg.resolve("stale-request-id-1").is_none(),
        "expired entry must sweep"
    );
    assert!(reg.resolve("fresh-request-id-2").is_some());
    assert_eq!(reg.snapshot().len(), 1);
}

#[test]
fn snapshot_sorts_by_remaining_secs() {
    let reg = RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    };
    reg.register(RemoteAskEntry {
        request_id: "long-ttl-request".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 600,
        created_at: std::time::Instant::now(),
    });
    reg.register(RemoteAskEntry {
        request_id: "short-ttl-request".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 30,
        created_at: std::time::Instant::now(),
    });
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 2);
    let first = snap[0].get("request_id").and_then(|v| v.as_str());
    assert_eq!(first, Some("short-ttl-request"), "soonest-to-expire first");
}

// 2026-10-15 真机「刷新页面闪审批卡」回归锁：worker 侧超时自动拒生效后、
// 注册表 +60s 兜底未扫前（裁决窗口外、TTL 窗口内），remote_list/snapshot
// 不得再吐死条目（前端种卡即自摘=闪卡）；resolve 仍可命中（迟到裁决/
// ask_closed 竞态安全面不变）。
#[test]
fn snapshot_hides_entries_past_decision_window() {
    let reg = RemoteAskRegistry {
        inner: Mutex::new(HashMap::new()),
    };
    // elapsed=330s：timeout 300（裁决窗口已过）< TTL 360（sweep 未及）。
    let backdated = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(330));
    let Some(backdated) = backdated else {
        eprintln!("SKIP: Instant::checked_sub unsupported on this platform");
        return;
    };
    reg.register(RemoteAskEntry {
        request_id: "zombie-window-request".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 300,
        created_at: backdated,
    });
    reg.register(RemoteAskEntry {
        request_id: "alive-request".into(),
        worker_node: "w".into(),
        operation: "op".into(),
        target: "t".into(),
        risk_level: "LOW".into(),
        reason: String::new(),
        issue_number: None,
        timeout_secs: 300,
        created_at: std::time::Instant::now(),
    });
    let snap = reg.snapshot();
    assert_eq!(snap.len(), 1, "dead entry must not list: {snap:?}");
    assert_eq!(
        snap[0].get("request_id").and_then(|v| v.as_str()),
        Some("alive-request")
    );
    // 竞态安全面：条目仍在表，迟到的 resolve 照常命中。
    assert!(reg.resolve("zombie-window-request").is_some());
}

/// 回源 relay 的两条 false 路径（解析不命中 / 短号不在注册表）——不触
/// store/cluster 即返回 false，杂散同形评论零影响。
#[tokio::test]
async fn relay_ignores_non_matching_comments() {
    // 解析不命中。
    let (dir, store) = temp_store("relay_false_parse");
    let cluster =
        nemesis_cluster::cluster::Cluster::new(nemesis_cluster::types::ClusterConfig::default());
    assert!(!try_remote_approval_reply(&store, &cluster, 1, "普通讨论内容").await);
    // 前缀命中但短号未登记。
    assert!(
        !try_remote_approval_reply(&store, &cluster, 1, "/approve deadbeef").await,
        "unknown short id must be a no-op"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 回源 relay 的失败留痕分支：短号在注册表、但裸 Cluster 无 RPC 客户端 →
/// send_verdict 诚实 Err → 返回 true + ❌ 系统评论落库（条目保留可重发）。
#[tokio::test]
async fn relay_posts_failure_receipt_on_rpc_unavailable() {
    let (dir, store) = temp_store("relay_fail_receipt");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "remote approval relay".into(),
            ..Default::default()
        })
        .unwrap();
    let cluster =
        nemesis_cluster::cluster::Cluster::new(nemesis_cluster::types::ClusterConfig::default());
    registry().register(RemoteAskEntry {
        request_id: "receipt-test-request-0001".into(),
        worker_node: "node-x".into(),
        operation: "process_exec".into(),
        target: "rm -rf /".into(),
        risk_level: "CRITICAL".into(),
        reason: "relay test".into(),
        issue_number: Some(issue.id),
        timeout_secs: 300,
        created_at: std::time::Instant::now(),
    });
    let acted = try_remote_approval_reply(&store, &cluster, issue.id, "/approve receipt-").await;
    assert!(acted, "registered short id must trigger the relay");
    let comments = store.list_comments(issue.id).unwrap();
    let receipt = comments
        .iter()
        .find(|c| c.content.contains("批复转发失败"))
        .expect("failure receipt comment must be posted");
    assert!(
        receipt.content.contains("node-x"),
        "receipt names the worker"
    );
    assert_eq!(receipt.ctype, nemesis_board::CommentType::System);
    // 失败路径条目保留（可重发）。
    assert!(registry().resolve("receipt-test-request-0001").is_some());
    let _ = std::fs::remove_dir_all(&dir);
    registry().remove_by_request_id("receipt-test-request-0001");
}
