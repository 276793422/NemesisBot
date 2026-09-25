//! nb_bus handler 单测（master 侧：信封路由 / 幂等 / 额度 / sync；
//! worker 侧：wake.post 投递 / 下行 seq 幂等 / sync 过滤。
//! 不启动集群网络——Cluster::new 纯内存形态，RPC client 缺省 →
//! wake 投递路径安全 no-op）。

use super::*;
use std::sync::OnceLock;

use nemesis_board::quota::{QuotaConfig, QuotaLedger};
use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::types::ClusterConfig;

fn make_deps(store: Arc<nemesis_board::BoardStore>, quota: Arc<QuotaLedger>) -> MasterBusDeps {
    let cluster = Cluster::new(ClusterConfig {
        node_id: "node-master".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    });
    MasterBusDeps {
        store,
        workspace: std::env::temp_dir().join("board-bus-test-ws"),
        quota,
        cluster: Arc::new(cluster),
        moderator_loop: Arc::new(OnceLock::new()),
    }
}

fn temp_store(name: &str) -> (Arc<nemesis_board::BoardStore>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-board-bustest-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let store = nemesis_board::BoardStore::open(&dir.join("board.db"), "NB").expect("open store");
    store.ensure_default_channels().unwrap();
    (Arc::new(store), dir)
}

fn quota(thread_cap: u32) -> Arc<QuotaLedger> {
    Arc::new(QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: thread_cap,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 0,
    }))
}

/// 上行 comment.post 信封（worker 侧会构造的形状）。
fn post_payload(
    client_msg_id: &str,
    thread_kind: &str,
    thread_id: i64,
    content: &str,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "ns": "board", "op": "comment.post", "corr_id": "c-1",
        "body": {
            "client_msg_id": client_msg_id,
            "thread": {"kind": thread_kind, "id": thread_id},
            "sender": {"type": "agent", "id": "node-b"},
            "content": content,
            "reply_to": null,
            "kind_tag": "text",
        }
    })
}

fn parse_reply(
    reply: Result<serde_json::Value, String>,
) -> (bool, Option<String>, serde_json::Value) {
    let v = reply.expect("handler never errs");
    let ok = v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false);
    let code = v
        .get("error")
        .and_then(|e| e.get("code"))
        .and_then(|c| c.as_str())
        .map(|s| s.to_string());
    (ok, code, v)
}

#[test]
fn test_rejects_bad_envelope_and_unknown_routes() {
    let (store, dir) = temp_store("route");
    let deps = make_deps(store.clone(), quota(8));

    // 缺 ns/op → bad_envelope。
    let (ok, code, _) = parse_reply(handle_nb_bus(&deps, serde_json::json!({"v": 1})));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::BAD_ENVELOPE)
    );

    // 版本不符 → bad_envelope。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 99, "ns": "board", "op": "sync"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::BAD_ENVELOPE)
    );

    // 未知 ns / op → 各自错误码。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "file", "op": "read"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS)
    );

    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "nope"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_OP)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_comment_post_validation_errors() {
    let (store, dir) = temp_store("validation");
    let deps = make_deps(store.clone(), quota(8));

    // 缺 client_msg_id / thread / sender / content 逐个 VALIDATION。
    for payload in [
        serde_json::json!({"v": 1, "ns": "board", "op": "comment.post", "body": {}}),
        serde_json::json!({"v": 1, "ns": "board", "op": "comment.post",
            "body": {"client_msg_id": "x"}}),
        serde_json::json!({"v": 1, "ns": "board", "op": "comment.post",
            "body": {"client_msg_id": "x", "thread": {"kind": "channel", "id": 1}}}),
        serde_json::json!({"v": 1, "ns": "board", "op": "comment.post",
            "body": {"client_msg_id": "x", "thread": {"kind": "channel", "id": 1},
                "sender": {"type": "agent", "id": "node-b"}}}),
    ] {
        let (ok, code, _) = parse_reply(handle_nb_bus(&deps, payload));
        assert!(!ok);
        assert_eq!(
            code.as_deref(),
            Some(nemesis_cluster::envelope::error_code::VALIDATION)
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_comment_post_happy_path_and_idempotent() {
    let (store, dir) = temp_store("happy");
    let deps = make_deps(store.clone(), quota(8));
    let ch = store.get_channel_by_name("#dev").unwrap().unwrap();

    let payload = post_payload("u-1", "channel", ch.id, "第一条");
    let (ok, code, body) = parse_reply(handle_nb_bus(&deps, payload));
    assert!(ok, "code={code:?}");
    assert!(body.get("body").and_then(|b| b.get("seq")).is_some());

    // 落库可见。
    let msgs = store.list_channel_messages(ch.id, 0, 100).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].content, "第一条");

    // 同 id 重发 → 相同首响，不重复落库（G12）。
    let (ok2, _, body2) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("u-1", "channel", ch.id, "第一条"),
    ));
    assert!(ok2);
    assert_eq!(body.get("body"), body2.get("body"));
    assert_eq!(store.list_channel_messages(ch.id, 0, 100).unwrap().len(), 1);

    // 异步 wake 任务安全结束（无 RPC client → no-op 不 panic）。
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_comment_post_quota_denied() {
    let (store, dir) = temp_store("quota");
    let deps = make_deps(store.clone(), quota(1));
    let ch = store.get_channel_by_name("#dev").unwrap().unwrap();

    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("q-1", "channel", ch.id, "第一条"),
    ));
    assert!(ok);
    // 线程额度=1 → 第二条 quota_exhausted（消息不落库）。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("q-2", "channel", ch.id, "第二条"),
    ));
    assert!(!ok);
    assert_eq!(code.as_deref(), Some("quota_exhausted"));
    assert_eq!(store.list_channel_messages(ch.id, 0, 100).unwrap().len(), 1);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_sync_returns_ledger_entries() {
    let (store, dir) = temp_store("sync");
    let deps = make_deps(store.clone(), quota(8));
    let ch = store.get_channel_by_name("#dev").unwrap().unwrap();
    let issue = store
        .create_issue(nemesis_board::models::NewIssue {
            title: "同步".to_string(),
            ..Default::default()
        })
        .unwrap();
    let _ = handle_nb_bus(&deps, post_payload("s-1", "channel", ch.id, "频道一"));
    let _ = handle_nb_bus(&deps, post_payload("s-2", "issue", issue.id, "评论一"));

    let (ok, code, body) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "sync", "body": {"since_seq": 0}}),
    ));
    assert!(ok, "code={code:?}");
    let messages = body
        .get("body")
        .and_then(|b| b.get("messages"))
        .and_then(|m| m.as_array())
        .expect("messages array");
    assert_eq!(messages.len(), 2);
    assert_eq!(
        body["body"]["latest_seq"],
        body["body"]["messages"][1]["seq"]
    );

    // since_seq 游标：只取增量。
    let (ok, _, body) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "sync", "body": {"since_seq": 1}}),
    ));
    assert!(ok);
    assert_eq!(body["body"]["messages"].as_array().unwrap().len(), 1);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Worker 侧（G4 wake.post 下行 + G8 sync 过滤）
// ---------------------------------------------------------------------------

/// worker 侧组装依赖（含活跃 inbox——调用方需要时自行 set_sender）。
fn make_worker_deps(self_id: &str) -> (WorkerBusDeps, Arc<crate::cluster_agent::DiscussionInbox>) {
    let inbox = Arc::new(crate::cluster_agent::DiscussionInbox::new());
    let deps = WorkerBusDeps {
        self_node_id: self_id.to_string(),
        node_name: "Node-Alpha".to_string(),
        node_role: "worker".to_string(),
        node_category: "dev".to_string(),
        inbox: inbox.clone(),
        wake_state: Arc::new(WorkerWakeState::new()),
    };
    (deps, inbox)
}

/// master wake.post 下行信封（build_wake_envelope 的形状）。
fn wake_payload(
    thread_kind: &str,
    thread_id: i64,
    seq: i64,
    content: &str,
    from_node: &str,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "ns": "board", "op": "wake.post", "corr_id": "w-1",
        "_rpc": {"from": from_node},
        "body": {
            "seq": seq,
            "event": "mention",
            "thread": {"kind": thread_kind, "id": thread_id, "title": "标题",
                "messages": [{"sender": "node-master", "content": "早", "at": 1}]},
            "new_message": {"sender": "node-master", "content": content, "at": 2},
            "reply_hint": {"reply_to": seq, "max_turns_left": 5},
        }
    })
}

#[test]
fn test_worker_wake_state_seq_semantics() {
    let state = WorkerWakeState::new();
    // 新线程：更大 seq 才要处理。
    assert!(state.peek("issue:1", 3));
    assert!(!state.peek("issue:1", 0));
    state.commit("issue:1", 3);
    // commit 后同 seq / 更小 seq 幂等丢弃；标记参与线程。
    assert!(!state.peek("issue:1", 3));
    assert!(!state.peek("issue:1", 2));
    assert!(state.is_participated("issue:1"));
    assert!(!state.is_participated("channel:9"));
    // 水位单调推进（回退值不生效）。
    state.advance_watermark(10);
    state.advance_watermark(5);
    assert_eq!(state.watermark(), 10);
}

#[tokio::test]
async fn test_worker_wake_post_happy_duplicate_and_routing() {
    let (deps, inbox) = make_worker_deps("node-b");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    inbox.set_sender(tx);

    // happy path：ok + queued:true，事件完整落箱（含 _rpc.from 注入的 from_node）。
    let reply = handle_worker_nb_bus(
        &deps,
        wake_payload("issue", 7, 4, "@node-b 看这个", "node-master"),
    );
    let v = reply.expect("handler never errs");
    assert!(v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    assert_eq!(v["body"]["queued"], serde_json::json!(true));
    let event = rx.recv().await.expect("event delivered");
    assert_eq!(event.thread_kind, "issue");
    assert_eq!(event.thread_id, 7);
    assert_eq!(event.seq, 4);
    assert_eq!(event.event, "mention");
    assert_eq!(event.from_node, "node-master");
    assert_eq!(event.new_content, "@node-b 看这个");
    assert_eq!(event.max_turns_left, 5);
    assert_eq!(event.reply_to, Some(4));
    assert_eq!(event.messages.len(), 1);

    // 同 seq 重发（RPC 层重传）→ duplicate:true，不再入队（G12 下行侧）。
    let reply = handle_worker_nb_bus(
        &deps,
        wake_payload("issue", 7, 4, "@node-b 看这个", "node-master"),
    );
    let v = reply.expect("handler never errs");
    assert!(v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    assert_eq!(v["body"]["duplicate"], serde_json::json!(true));
    assert!(rx.try_recv().is_err(), "duplicate must not enqueue");

    // 更大 seq 正常入队；worker 收 ns/op 错误码与 master 同表。
    let reply = handle_worker_nb_bus(&deps, wake_payload("issue", 7, 5, "again", "node-master"));
    assert!(reply.unwrap().get("ok").unwrap().as_bool().unwrap());
    assert!(rx.try_recv().is_ok());
    let (ok, code) = {
        let v = handle_worker_nb_bus(
            &deps,
            serde_json::json!({"v": 1, "ns": "file", "op": "read"}),
        )
        .unwrap();
        (
            v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false),
            v.pointer("/error/code")
                .and_then(|c| c.as_str())
                .map(String::from),
        )
    };
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS)
    );
}

#[test]
fn test_worker_wake_post_validation_errors() {
    let (deps, _inbox) = make_worker_deps("node-b");
    // 逐个抽掉必填字段 → VALIDATION。
    for body in [
        serde_json::json!({}),
        serde_json::json!({"thread": {"id": 1}}),
        serde_json::json!({"thread": {"kind": "issue", "id": 1}}),
        serde_json::json!({"thread": {"kind": "issue", "id": 1},
            "new_message": {"content": "x"}}),
        serde_json::json!({"thread": {"kind": "issue", "id": 1},
            "new_message": {"sender": "m", "content": "x"}}),
    ] {
        let payload = serde_json::json!({"v": 1, "ns": "board", "op": "wake.post", "body": body});
        let v = handle_worker_nb_bus(&deps, payload).expect("handler never errs");
        assert!(!v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
        assert_eq!(
            v.pointer("/error/code").and_then(|c| c.as_str()),
            Some(nemesis_cluster::envelope::error_code::VALIDATION),
            "body={body}"
        );
    }
}

#[tokio::test]
async fn test_worker_wake_post_send_failure_not_committed() {
    let (deps, inbox) = make_worker_deps("node-b");
    // 无活跃 loop（cluster 停了）→ send 失败 → UNAVAILABLE，且**不记账**
    // （peek 仍为真：下一拍 board.sync / 重发可重处理）。
    let v = handle_worker_nb_bus(&deps, wake_payload("issue", 7, 4, "hi", "node-master"))
        .expect("handler never errs");
    assert!(!v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    assert_eq!(
        v.pointer("/error/code").and_then(|c| c.as_str()),
        Some(nemesis_cluster::envelope::error_code::UNAVAILABLE)
    );
    assert!(
        deps.wake_state.peek("issue:7", 4),
        "send failure must not commit"
    );

    // loop 起来之后同一 seq 重发 → 正常入队（证明上面确实没记账）。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    inbox.set_sender(tx);
    let v = handle_worker_nb_bus(&deps, wake_payload("issue", 7, 4, "hi", "node-master"))
        .expect("handler never errs");
    assert!(v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false));
    assert!(rx.try_recv().is_ok());
}

#[test]
fn test_sync_entry_targets_me() {
    let (deps, _inbox) = make_worker_deps("node-b");

    // 自己的发言永不唤醒自己。
    assert!(!sync_entry_targets_me(
        &deps,
        "@node-b 自问自答",
        "node-b",
        "channel:1"
    ));
    // 提到 id / name（大小写不敏感）→ 真。
    assert!(sync_entry_targets_me(
        &deps,
        "请 @node-b 确认",
        "node-master",
        "channel:1"
    ));
    assert!(sync_entry_targets_me(
        &deps,
        "@node-alpha 帮忙看下",
        "node-master",
        "channel:1"
    ));
    // @role: 命中拓扑角色（worker）或功能类别（dev）→ 真。
    assert!(sync_entry_targets_me(
        &deps,
        "@role:worker 都来看",
        "node-master",
        "channel:1"
    ));
    assert!(sync_entry_targets_me(
        &deps,
        "@role:dev 集合",
        "node-master",
        "channel:1"
    ));
    // @role: 未命中类别 → 假。
    assert!(!sync_entry_targets_me(
        &deps,
        "@role:qa 看这里",
        "node-master",
        "channel:1"
    ));
    // 无提及且未参与 → 假。
    assert!(!sync_entry_targets_me(
        &deps,
        "大家辛苦了",
        "node-master",
        "channel:1"
    ));
    // 参与过的线程（wake 处理过）→ 无提及也真（G8「我参与的线程」）。
    deps.wake_state.commit("channel:2", 1);
    assert!(sync_entry_targets_me(
        &deps,
        "后续讨论",
        "node-master",
        "channel:2"
    ));
}

/// G8 游标推进回归（2026-09-18 T26 根因）：sync 第一拍跑在 cluster agent
/// loop 就绪之前 → inbox.send 失败。此前 advance_watermark 无条件推进
/// latest → 该条目被跳过且永不重拉，补拉机制自己吞掉 wake 事件，离线
/// 韧性失效。修复后失败条目前停住，下一拍重拉重试。
#[test]
fn test_sync_advance_target_holds_back_on_undelivered() {
    // 无失败 → 推进到 master latest（见过的不重拉）。
    assert_eq!(sync_advance_target(None, 10), 10);
    assert_eq!(sync_advance_target(None, 0), 0);
    // 有失败条目（seq=10）→ 停在它之前，下一拍 since_seq=9 重拉重试。
    assert_eq!(sync_advance_target(Some(10), 10), 9);
    // 多个失败条目取最小（乱序防御）。
    assert_eq!(sync_advance_target(Some(12), 15), 11);
    // 失败条目 seq 异常大于 latest（响应乱序）→ 不越过 latest。
    assert_eq!(sync_advance_target(Some(15), 10), 10);
}

// -------------------------------------------------------------------------
// 批次 E：dashboard 本地发言入口（post_discussion_locally / LocalDiscussionIngress）
// -------------------------------------------------------------------------

use nemesis_board::service::DiscussionIngress as _;

#[tokio::test]
async fn test_local_post_ingress_happy_and_idempotent() {
    let (store, dir) = temp_store("local-post");
    let deps = make_deps(store.clone(), quota(8));
    let ingress = LocalDiscussionIngress { deps };

    let first = ingress
        .post(
            &Actor::admin("zoo"),
            thread_kind::CHANNEL,
            1,
            "dash-1",
            "大家好",
            None,
            "text",
        )
        .expect("local post ok");
    assert!(first["message_id"].as_i64().unwrap() > 0);
    assert!(first["seq"].as_i64().unwrap() > 0);

    // 落库：channel_message 一行，sender=admin/zoo。
    let msgs = store.list_channel_messages(1, 0, 10).unwrap();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].sender.kind, "admin");
    assert_eq!(msgs[0].sender.id, "zoo");
    assert_eq!(msgs[0].content, "大家好");

    // 同 client_msg_id 重发 → 缓存首响，不重复落库（G12）。
    let second = ingress
        .post(
            &Actor::admin("zoo"),
            thread_kind::CHANNEL,
            1,
            "dash-1",
            "大家好",
            None,
            "text",
        )
        .expect("duplicate returns cached first response");
    assert_eq!(first, second);
    assert_eq!(store.list_channel_messages(1, 0, 10).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn test_local_post_quota_denial_maps_to_message() {
    let (store, dir) = temp_store("local-quota");
    let deps = make_deps(store.clone(), quota(1));
    let ingress = LocalDiscussionIngress { deps };

    ingress
        .post(
            &Actor::admin("zoo"),
            thread_kind::CHANNEL,
            1,
            "d-1",
            "第一条",
            None,
            "text",
        )
        .expect("first post within quota");
    let err = ingress
        .post(
            &Actor::agent("node-b"),
            thread_kind::CHANNEL,
            1,
            "d-2",
            "第二条",
            None,
            "text",
        )
        .expect_err("thread quota exhausted → denied");
    assert!(err.starts_with("[quota_exhausted]"), "got: {err}");
    // 拒绝不落库（护栏语义：被拦下的发言不留痕于消息表）。
    assert_eq!(store.list_channel_messages(1, 0, 10).unwrap().len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

// =======================================================================
// G8（2026-09-09）：WorkerWakeState 持久化——重启零重放
// =======================================================================

#[test]
fn test_wake_state_load_missing_file_starts_empty() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-wakestate-g8-{}-missing",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board_wake_state.json");

    // 无文件 → 空账起步，不炸；peek 按未见过处理。
    let state = WorkerWakeState::load_or_create(path.clone());
    assert!(state.peek("t1", 5), "missing snapshot → all seqs fresh");
    assert_eq!(state.watermark(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_wake_state_commit_persists_and_survives_reload() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-wakestate-g8-{}-reload",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board_wake_state.json");

    // 实例 1：commit + advance。
    let state = WorkerWakeState::load_or_create(path.clone());
    state.commit("t1", 7);
    state.commit("t2", 3);
    state.advance_watermark(9);
    assert!(path.exists(), "commit must persist snapshot");

    // 实例 2（模拟重启）：水位接续——旧 seq 不再当作新鲜。
    let reloaded = WorkerWakeState::load_or_create(path.clone());
    assert!(
        !reloaded.peek("t1", 7),
        "old seq must not replay after restart"
    );
    assert!(
        !reloaded.peek("t1", 3),
        "older seq must not replay after restart"
    );
    assert!(reloaded.peek("t1", 8), "newer seq is still fresh");
    assert!(!reloaded.peek("t2", 3));
    assert_eq!(reloaded.watermark(), 9, "watermark survives restart");
    // participated 从 threads.keys() 重建。
    assert!(reloaded.is_participated("t1"));
    assert!(reloaded.is_participated("t2"));
    assert!(!reloaded.is_participated("t3"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_wake_state_corrupt_snapshot_starts_empty() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-wakestate-g8-{}-corrupt",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board_wake_state.json");
    std::fs::write(&path, "{not valid json").unwrap();

    // 损坏快照 → 空账起步（诚实重放比卡死安全），不 panic。
    let state = WorkerWakeState::load_or_create(path.clone());
    assert!(state.peek("t1", 1));
    // 后续 commit 能覆盖损坏文件恢复正常。
    state.commit("t1", 2);
    let reloaded = WorkerWakeState::load_or_create(path);
    assert!(!reloaded.peek("t1", 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn test_wake_state_new_is_pure_memory() {
    // new()（None 路径）：commit 不落盘、无副作用。
    let dir = std::env::temp_dir().join(format!("nemesis-wakestate-g8-{}-mem", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("board_wake_state.json");

    let state = WorkerWakeState::new();
    state.commit("t1", 5);
    state.advance_watermark(6);
    assert!(!path.exists(), "new() must not write any file");
    assert!(!state.peek("t1", 5));
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// goal P2/D0b：task.started 上报 → dispatch running（queued vs executing 分界）。
// ---------------------------------------------------------------------------

#[test]
fn test_task_started_marks_dispatch_running() {
    let (store, _dir) = temp_store("task-started");
    let deps = make_deps(store.clone(), quota(8));
    let actor = Actor::admin("test");

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "t".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("tk-1", issue.id, "node-b", &actor)
        .unwrap();

    let payload = serde_json::json!({
        "v": 1, "ns": "task", "op": "started", "corr_id": "c1",
        "_rpc": { "from": "node-b" },
        "body": { "task_id": "tk-1" }
    });
    let (ok, _, body) = parse_reply(handle_nb_bus(&deps, payload));
    assert!(ok, "started 应成功");
    assert_eq!(
        body.pointer("/body/running"),
        Some(&serde_json::json!(true))
    );
    assert_eq!(
        store.get_dispatch("tk-1").unwrap().unwrap().state,
        "running"
    );

    // 重复上报（已 running，非 dispatched）→ 诚实 failure。
    let payload2 = serde_json::json!({
        "v": 1, "ns": "task", "op": "started", "corr_id": "c2",
        "_rpc": { "from": "node-b" },
        "body": { "task_id": "tk-1" }
    });
    let (ok2, _, _) = parse_reply(handle_nb_bus(&deps, payload2));
    assert!(!ok2, "重复上报应被拒");

    // 伪造来源（worker 不匹配）→ failure。
    let issue2 = store
        .create_issue(nemesis_board::NewIssue {
            title: "t2".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("tk-2", issue2.id, "node-b", &actor)
        .unwrap();
    let payload3 = serde_json::json!({
        "v": 1, "ns": "task", "op": "started", "corr_id": "c3",
        "_rpc": { "from": "node-evil" },
        "body": { "task_id": "tk-2" }
    });
    let (ok3, _, _) = parse_reply(handle_nb_bus(&deps, payload3));
    assert!(!ok3, "来源与派发 worker 不匹配应被拒");
}

// ===========================================================================
// Coverage 追加（2026-09-24）：master handler 入口/校验臂、delivery.files
// 全链（校验矩阵 + 落盘 + INTERNAL 臂）、额度拒绝映射、wake 计划投影与
// 投递臂、主持人裁决（脚本化 LLM 直调 process_direct）、wake 信封组装、
// worker 路由臂、持久化失败、sync 前置臂。
// 主持人裁决测试触达真实 AgentLoop::process_direct → chat_log 单例按
// home env 解析（进程全局态）→ 持 GLOBAL_STATE_LOCK + EnvHomeGuard 隔离
// （#[cfg(windows)]，同 commands/session::tests 先例）。
// ===========================================================================

use nemesis_cluster::rpc::client::RpcClient;
use nemesis_cluster::types::{ExtendedNodeInfo, NodeStatus};
use nemesis_types::cluster::{NodeInfo, NodeRole};

/// 注册一个内存节点到集群注册表（wake 计划投影 / sync 目标的数据源）。
/// 地址固定 127.0.0.1:1（保留端口，连接必被拒——投递失败臂不依赖网络）。
fn register_node(cluster: &Cluster, id: &str, role: NodeRole, category: &str, online: bool) {
    cluster.register_node(ExtendedNodeInfo {
        base: NodeInfo {
            id: id.to_string(),
            name: id.to_string(),
            role,
            address: "127.0.0.1:1".to_string(),
            category: category.to_string(),
            last_seen: String::new(),
        },
        status: if online {
            NodeStatus::Online
        } else {
            NodeStatus::Offline
        },
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        node_type: "agent".to_string(),
    });
}

/// 自定 sender 的 comment.post 上行（post_payload 固定 node-b，投影测试
/// 需要 sender ≠ assignee）。
fn comment_payload(
    client_msg_id: &str,
    sender: &str,
    thread_kind: &str,
    thread_id: i64,
    content: &str,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "ns": "board", "op": "comment.post", "corr_id": "c-x",
        "body": {
            "client_msg_id": client_msg_id,
            "thread": {"kind": thread_kind, "id": thread_id},
            "sender": {"type": "agent", "id": sender},
            "content": content,
            "reply_to": null,
            "kind_tag": "text",
        }
    })
}

/// delivery.files 上行（task_id None → body 缺字段臂）。
fn delivery_payload(
    task_id: Option<&str>,
    from: &str,
    files: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "ns": "task", "op": "delivery.files", "corr_id": "d-1",
        "_rpc": {"from": from},
        "body": {"task_id": task_id, "files": files}
    })
}

/// 手搓 MasterBusDeps（make_deps 的 workspace 固定共享目录；落盘测试要
/// 独立 workspace）。
fn deps_with_workspace(
    store: Arc<nemesis_board::BoardStore>,
    workspace: std::path::PathBuf,
    q: Arc<QuotaLedger>,
) -> MasterBusDeps {
    let cluster = Cluster::new(ClusterConfig {
        node_id: "node-master".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    });
    MasterBusDeps {
        store,
        workspace,
        quota: q,
        cluster: Arc::new(cluster),
        moderator_loop: Arc::new(OnceLock::new()),
    }
}

/// 脚本化主持人 LLM：按序弹出回复，耗尽后回落 `fallback`（形态同
/// board_review::tests::ScriptedLlm）。
struct ScriptedModeratorLlm {
    script: std::sync::Mutex<std::collections::VecDeque<String>>,
    fallback: String,
}

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for ScriptedModeratorLlm {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        let raw = self
            .script
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .pop_front()
            .unwrap_or_else(|| self.fallback.clone());
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: raw,
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 把脚本化 provider 装进主持人后置装配桥。
#[cfg(windows)] // Windows-form helper (Linux nightly: excluded, 2026-09-02 sweep)
fn attach_scripted_moderator(deps: &MasterBusDeps, replies: &[&str], fallback: &str) {
    let llm = ScriptedModeratorLlm {
        script: std::sync::Mutex::new(replies.iter().map(|s| s.to_string()).collect()),
        fallback: fallback.to_string(),
    };
    let _ = deps
        .moderator_loop
        .set(Arc::new(nemesis_agent::r#loop::AgentLoop::new(
            Box::new(llm),
            nemesis_agent::types::AgentConfig::default(),
        )));
}

/// 主持人裁决测试的隔离前置（在每个 windows-gated async 测试体内联展开）：
/// `let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner());` + TempDir home +
/// `crate::tests::EnvHomeGuard::point_at(&home)`——三个守卫绑定到测试函数
/// 作用域，跨 await 存活（spawn 的裁决任务在锁内被 current_thread 轮询，
/// board_review::tests 同款纪律）。

#[test]
fn sweep_master_handler_entry_and_task_started_validation_arms() {
    let (store, dir) = temp_store("handler-entry");
    let _ = tracing_subscriber::fmt::try_init();

    // 入口转发（Box::new 闭包）+ ns=task 未知 op。
    let handler = build_master_nb_bus_handler(make_deps(store.clone(), quota(8)));
    let (ok, code, _) = parse_reply(handler(
        serde_json::json!({"v": 1, "ns": "task", "op": "nope"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_OP)
    );

    // ns=bogus → UNKNOWN_NS（task 分支之外）。
    let deps = make_deps(store.clone(), quota(8));
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "bogus", "op": "x"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS)
    );

    // task.started 缺 task_id → VALIDATION。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "task", "op": "started", "_rpc": {"from": "node-b"}, "body": {}}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::VALIDATION)
    );

    // 有 task_id 缺 _rpc.from → VALIDATION（伪造不了的发送者身份）。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "task", "op": "started", "body": {"task_id": "tk-9"}}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::VALIDATION)
    );

    // happy（tracing subscriber 已装 → debug! 格式参数行真实执行）。
    let actor = Actor::admin("test");
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "t".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("tk-h", issue.id, "node-b", &actor)
        .unwrap();
    let (ok, _, body) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "task", "op": "started", "_rpc": {"from": "node-b"}, "body": {"task_id": "tk-h"}}),
    ));
    assert!(ok, "在途派发 + 来源匹配应成功");
    assert_eq!(
        body.pointer("/body/running"),
        Some(&serde_json::json!(true))
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_delivery_files_validation_matrix_and_happy_path() {
    use base64::Engine as _;
    let (store, dir) = temp_store("delivery");
    let ws = std::env::temp_dir().join(format!("nemesis-board-dl-ws-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let deps = deps_with_workspace(store.clone(), ws.clone(), quota(8));

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "交付".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("df-1", issue.id, "node-b", &Actor::admin("test"))
        .unwrap();

    // 缺 task_id → VALIDATION。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(None, "node-b", serde_json::json!([])),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::VALIDATION)
    );

    // 来源 worker 不匹配 / 无在途派发 → 同一 VALIDATION 臂。
    for (from, task) in [("node-evil", "df-1"), ("node-b", "df-ghost")] {
        let (ok, code, _) = parse_reply(handle_nb_bus(
            &deps,
            delivery_payload(
                Some(task),
                from,
                serde_json::json!([{"name": "a.txt", "content_b64": "aGk="}]),
            ),
        ));
        assert!(!ok, "{from}/{task} 应被拒");
        assert_eq!(
            code.as_deref(),
            Some(nemesis_cluster::envelope::error_code::VALIDATION)
        );
    }

    // files 空 / 超上限（21 个）。
    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(Some("df-1"), "node-b", serde_json::json!([])),
    ));
    assert!(!ok);
    let many: Vec<_> = (0..21)
        .map(|i| serde_json::json!({"name": format!("f{i}.txt"), "content_b64": "aGk="}))
        .collect();
    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(Some("df-1"), "node-b", serde_json::json!(many)),
    ));
    assert!(!ok, "21 个文件应超上限");

    // base64 解码失败。
    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(
            Some("df-1"),
            "node-b",
            serde_json::json!([{"name": "a.txt", "content_b64": "!!!不是b64"}]),
        ),
    ));
    assert!(!ok);

    // 单文件超 8MB。
    let big = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 8 * 1024 * 1024 + 1]);
    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(
            Some("df-1"),
            "node-b",
            serde_json::json!([{"name": "big.bin", "content_b64": big}]),
        ),
    ));
    assert!(!ok);

    // 非法文件名（basename 以点开头 / 空名）。
    for name in [".env", "dir/", "dir\\"] {
        let (ok, _, _) = parse_reply(handle_nb_bus(
            &deps,
            delivery_payload(
                Some("df-1"),
                "node-b",
                serde_json::json!([{"name": name, "content_b64": "aGk="}]),
            ),
        ));
        assert!(!ok, "非法文件名 {name:?} 应被拒");
    }

    // happy：两个文件（含子路径 → basename），落盘 + 登记 + 系统评论留痕。
    let before = store.list_comments(issue.id).unwrap().len();
    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(
            Some("df-1"),
            "node-b",
            serde_json::json!([
                {"name": "report.txt", "content_b64": base64::engine::general_purpose::STANDARD.encode(b"hello report")},
                {"name": "sub/data.json", "content_b64": base64::engine::general_purpose::STANDARD.encode(b"{}")},
            ]),
        ),
    ));
    assert!(ok, "合法交付应成功");
    assert_eq!(v.pointer("/body/stored"), Some(&serde_json::json!(2)));
    let files_dir = ws
        .join("board")
        .join("files")
        .join(format!("issue_{}", issue.id));
    let written: Vec<_> = std::fs::read_dir(&files_dir).unwrap().collect();
    assert_eq!(written.len(), 2, "应落盘两个文件");
    let comments = store.list_comments(issue.id).unwrap();
    assert_eq!(comments.len(), before + 1, "应追加一条系统评论");
    assert!(
        comments
            .last()
            .unwrap()
            .content
            .contains("交付回传 2 个文件")
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&ws);
}

#[test]
fn sweep_delivery_files_internal_dir_create_failure() {
    use base64::Engine as _;
    let (store, dir) = temp_store("delivery-internal");
    // workspace 指向一个普通文件 → create_dir_all 必败 → INTERNAL 落盘失败。
    let ws_file =
        std::env::temp_dir().join(format!("nemesis-board-wsfile-{}.txt", std::process::id()));
    std::fs::write(&ws_file, "not a dir").unwrap();
    let deps = deps_with_workspace(store.clone(), ws_file.clone(), quota(8));

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "t".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .insert_dispatch("df-2", issue.id, "node-b", &Actor::admin("test"))
        .unwrap();
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        delivery_payload(
            Some("df-2"),
            "node-b",
            serde_json::json!([{"name": "a.txt", "content_b64": base64::engine::general_purpose::STANDARD.encode(b"x")}]),
        ),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::INTERNAL)
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&ws_file);
}

#[tokio::test]
async fn sweep_comment_post_missing_thread_id_and_rate_limit_denied() {
    let _ = tracing_subscriber::fmt::try_init();
    let (store, dir) = temp_store("rate-limit");
    let rate = Arc::new(QuotaLedger::new(QuotaConfig {
        max_agent_turns_per_thread: 8,
        hourly_budget_per_node: 0,
        rate_limit_per_min: 1,
    }));
    let deps = make_deps(store.clone(), rate);
    let channel_id = store.list_channels().unwrap()[0].id;

    // thread.kind 有、thread.id 缺 → missing thread.id。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "comment.post", "corr_id": "q-0",
            "body": {"client_msg_id": "q0", "thread": {"kind": "channel"},
                "sender": {"type": "agent", "id": "node-b"}, "content": "x"}}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::VALIDATION)
    );

    // 第一条过闸；1/min 限速下第二条拒绝（rate_limited）。
    let (ok1, _, _) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("q1", "channel", channel_id, "第一条"),
    ));
    assert!(ok1, "第一条应过闸");
    let (ok2, code2, _) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("q2", "channel", channel_id, "第二条"),
    ));
    assert!(!ok2, "限速第二条应被拒");
    assert_eq!(
        code2.as_deref(),
        Some(nemesis_cluster::envelope::error_code::RATE_LIMITED)
    );
    // 拒绝不落库。
    assert_eq!(
        store
            .list_channel_messages(channel_id, 0, i64::MAX)
            .unwrap()
            .len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sweep_issue_assignee_wake_projection_and_no_rpc_skip() {
    let (store, dir) = temp_store("wake-proj");
    let deps = make_deps(store.clone(), quota(8));
    register_node(&deps.cluster, "node-b", NodeRole::Worker, "qa", true);
    register_node(&deps.cluster, "node-c", NodeRole::Worker, "qa", false);

    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "指派".into(),
            ..Default::default()
        })
        .unwrap();
    store
        .assign_issue(
            issue.id,
            Some(nemesis_board::AssignmentType::Worker),
            Some("node-b".to_string()),
            &Actor::agent("node-master"),
        )
        .unwrap();

    // 无 @ + issue 指派 → 定点 assignee（在线者），不推主持人。
    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("w1", "node-x", "issue", issue.id, "进度如何？"),
    ));
    assert!(ok);
    assert_eq!(
        v.pointer("/body/wake/woke"),
        Some(&serde_json::json!(["node-b"]))
    );
    assert_eq!(
        v.pointer("/body/wake/to_moderator"),
        Some(&serde_json::json!(false))
    );

    // 投递任务跑在 rpc client 缺席臂（warn + return，无副作用）。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sweep_thread_quota_exhausted_suppresses_delivery() {
    let (store, dir) = temp_store("quota-exhaust");
    // 线程额度 1：首帖过闸用掉唯一一档 → 投递任务撞 turns_left==0 提前返回。
    let deps = make_deps(store.clone(), quota(1));
    let channel_id = store.list_channels().unwrap()[0].id;
    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        post_payload("e1", "channel", channel_id, "占用额度"),
    ));
    assert!(ok, "首帖应过闸");
    assert_eq!(
        v.pointer("/body/wake/to_moderator"),
        Some(&serde_json::json!(true))
    );
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sweep_wake_delivery_rpc_failure_is_warn_not_fatal() {
    let (store, dir) = temp_store("wake-rpc");
    let deps = make_deps(store.clone(), quota(8));
    deps.cluster.set_rpc_client(Arc::new(RpcClient::new()));
    register_node(&deps.cluster, "node-b", NodeRole::Worker, "qa", true);

    // 600 字描述 → wake 信封 prd_summary 截断臂。
    let long_desc = "长".repeat(600);
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "RPC失败".into(),
            description: long_desc,
            ..Default::default()
        })
        .unwrap();
    store
        .assign_issue(
            issue.id,
            Some(nemesis_board::AssignmentType::Worker),
            Some("node-b".to_string()),
            &Actor::agent("node-master"),
        )
        .unwrap();

    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("r1", "node-x", "issue", issue.id, "看一下"),
    ));
    assert!(ok);
    assert_eq!(
        v.pointer("/body/wake/woke"),
        Some(&serde_json::json!(["node-b"]))
    );
    // 投递任务：wake 信封组装（issue 分支 + 截断）→ rpc.call 被拒 → warn。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_build_wake_envelope_issue_and_channel_branches() {
    let (store, dir) = temp_store("wake-envelope");
    let channel_id = store.list_channels().unwrap()[0].id;

    // issue 分支：title + prd_summary（300 字描述 → 截断到 500 不触发省略号）。
    let issue = store
        .create_issue(nemesis_board::NewIssue {
            title: "信封".into(),
            description: "描".repeat(300),
            ..Default::default()
        })
        .unwrap();
    // 线程上下文需要至少一条评论。
    store
        .add_comment(nemesis_board::NewComment {
            issue_id: issue.id,
            author: Actor::new("agent", "node-x"),
            content: "你好".to_string(),
            parent_id: None,
            ctype: nemesis_board::CommentType::Discussion,
        })
        .unwrap();
    let ctx_issue = make_wake_ctx("issue", issue.id);
    let env = build_wake_envelope(&store, &ctx_issue, 5, "assignee_comment").unwrap();
    assert_eq!(env["ok"], serde_json::json!(true));
    assert_eq!(env["op"], serde_json::json!("wake.post"));
    assert_eq!(env["body"]["event"], serde_json::json!("assignee_comment"));
    assert_eq!(env["body"]["thread"]["title"], serde_json::json!("信封"));
    assert!(
        env["body"]["thread"]["prd_summary"]
            .as_str()
            .unwrap()
            .chars()
            .count()
            == 300
    );
    assert_eq!(
        env["body"]["new_message"]["sender"],
        serde_json::json!("node-x")
    );
    assert_eq!(
        env["body"]["reply_hint"]["max_turns_left"],
        serde_json::json!(5)
    );
    assert_eq!(env["body"]["seq"], serde_json::json!(9));

    // issue 不存在 → title/prd_summary 空串诚实降级。
    let ctx_ghost = make_wake_ctx("issue", 999_999);
    let env = build_wake_envelope(&store, &ctx_ghost, 1, "mention").unwrap();
    assert_eq!(env["body"]["thread"]["title"], serde_json::json!(""));

    // channel 分支：title = 频道名（#前缀），prd_summary 恒空。
    let ctx_channel = make_wake_ctx("channel", channel_id);
    let env = build_wake_envelope(&store, &ctx_channel, 7, "moderator_call").unwrap();
    let want = format!("#{}", store.list_channels().unwrap()[0].name);
    assert_eq!(env["body"]["thread"]["title"], serde_json::json!(want));
    assert_eq!(env["body"]["thread"]["prd_summary"], serde_json::json!(""));

    // 线程上下文：主持人纯文本形态（标题行 + 消息行）。
    let text = build_thread_context_text(&store, &ctx_issue).unwrap();
    assert!(text.starts_with("# Thread (issue "), "got: {text}");
    assert!(text.contains("- node-x: 你好"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 测试侧 WakeContext 构造（生产结构未实现 Clone，不能动生产代码）。
#[allow(dead_code)] // 多分支各用一处；统一构造器防字段漂移
fn make_wake_ctx(thread_kind: &str, thread_id: i64) -> WakeContext {
    WakeContext {
        thread_kind: thread_kind.to_string(),
        thread_id,
        sender: Actor::new("agent", "node-x"),
        content: "你好".to_string(),
        reply_to: Some(3),
        seq: 9,
        at: 42,
    }
}

#[test]
fn sweep_truncate_chars_passthrough_boundary_and_ellipsis() {
    assert_eq!(truncate_chars("短文本", 10), "短文本");
    assert_eq!(truncate_chars("", 5), "");
    let long = "abcdef";
    assert_eq!(truncate_chars(long, 3), "abc…");
    // 多字节 char boundary 安全（中文逐字截断 + 省略号）。
    let cn = "甲乙丙丁";
    assert_eq!(truncate_chars(cn, 2), "甲乙…");
    assert_eq!(truncate_chars(cn, 4), cn);
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sweep_channel_post_moderator_silent_adds_no_comment() {
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);
    let (store, dir) = temp_store("mod-silent");
    let deps = make_deps(store.clone(), quota(8));
    attach_scripted_moderator(&deps, &[], "[SILENT]");
    let channel_id = store.list_channels().unwrap()[0].id;

    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("m1", "node-x", "channel", channel_id, "大家好"),
    ));
    assert!(ok);
    assert_eq!(
        v.pointer("/body/wake/to_moderator"),
        Some(&serde_json::json!(true))
    );

    // spawn 的裁决任务：process_direct 回 [SILENT] → 静默不落评论。
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let msgs = store
        .list_channel_messages(channel_id, 0, i64::MAX)
        .unwrap();
    assert_eq!(msgs.len(), 1, "[SILENT] 不得落评论");
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sweep_moderator_reply_posts_comment_and_wakes_mentioned_node() {
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);
    let (store, dir) = temp_store("mod-reply");
    let deps = make_deps(store.clone(), quota(8));
    deps.cluster.set_rpc_client(Arc::new(RpcClient::new()));
    register_node(&deps.cluster, "node-b", NodeRole::Worker, "qa", true);
    attach_scripted_moderator(&deps, &[], "@node-b 请复述这条消息");
    let channel_id = store.list_channels().unwrap()[0].id;

    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("m1", "node-x", "channel", channel_id, "谁是负责人？"),
    ));
    assert!(ok);

    // 等主持人回复落库（rpc 下行必被拒端口拒绝 → 只 warn）。
    let mut reply_seen = false;
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if store
            .list_channel_messages(channel_id, 0, i64::MAX)
            .unwrap()
            .len()
            >= 2
        {
            reply_seen = true;
            break;
        }
    }
    assert!(reply_seen, "主持人回复应落库");
    let msgs = store
        .list_channel_messages(channel_id, 0, i64::MAX)
        .unwrap();
    assert_eq!(msgs[1].sender.id, "node-master", "回复 origin=主持人节点");
    assert!(msgs[1].content.contains("@node-b"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sweep_moderator_reply_without_mentions_posts_and_stops() {
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);
    let (store, dir) = temp_store("mod-nomention");
    let deps = make_deps(store.clone(), quota(8));
    attach_scripted_moderator(&deps, &[], "已阅，无需行动。");
    let channel_id = store.list_channels().unwrap()[0].id;

    let (ok, _, _) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("m1", "node-x", "channel", channel_id, "例行同步"),
    ));
    assert!(ok);

    let mut posted = false;
    for _ in 0..60 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if store
            .list_channel_messages(channel_id, 0, i64::MAX)
            .unwrap()
            .len()
            >= 2
        {
            posted = true;
            break;
        }
    }
    assert!(posted, "无 @ 回复也应落库");
    // 无点名 → plan 空 → 不再往下走（不碰 rpc）。
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        store
            .list_channel_messages(channel_id, 0, i64::MAX)
            .unwrap()
            .len(),
        2
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sweep_mention_of_master_routes_to_local_adjudication() {
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);
    let (store, dir) = temp_store("mod-self");
    let deps = make_deps(store.clone(), quota(8));
    deps.cluster.set_rpc_client(Arc::new(RpcClient::new()));
    // 注册与 master 同 id 的内存节点（@ 点名主持人自己 → 本地裁决）。
    register_node(
        &deps.cluster,
        "node-master",
        NodeRole::Coordinator,
        "dev",
        true,
    );
    attach_scripted_moderator(&deps, &[], "[SILENT]");
    let channel_id = store.list_channels().unwrap()[0].id;

    let (ok, _, v) = parse_reply(handle_nb_bus(
        &deps,
        comment_payload("m1", "node-x", "channel", channel_id, "@node-master 请裁决"),
    ));
    assert!(ok);
    assert_eq!(
        v.pointer("/body/wake/woke"),
        Some(&serde_json::json!(["node-master"]))
    );
    assert_eq!(
        v.pointer("/body/wake/to_moderator"),
        Some(&serde_json::json!(false))
    );

    // 投递任务：target == self → need_moderator（不发 wake.post 给自己）→ 静默。
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let msgs = store
        .list_channel_messages(channel_id, 0, i64::MAX)
        .unwrap();
    assert_eq!(msgs.len(), 1, "主持人静默不得落评论");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sweep_worker_bad_envelope_unknown_routes_and_wake_validation() {
    let (wdeps, _inbox) = make_worker_deps("node-b");

    // 缺 ns → bad_envelope（fallback 回带空 ns / 原 op）。
    let (ok, code, v) = parse_reply(handle_worker_nb_bus(
        &wdeps,
        serde_json::json!({"v": 1, "op": "wake.post"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::BAD_ENVELOPE)
    );
    assert_eq!(
        v["ns"],
        serde_json::json!(""),
        "fallback 应尽力回带路由字段"
    );

    // ns=file → UNKNOWN_NS；board + 未知 op → UNKNOWN_OP。
    let (ok, code, _) = parse_reply(handle_worker_nb_bus(
        &wdeps,
        serde_json::json!({"v": 1, "ns": "file", "op": "read"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS)
    );
    let (ok, code, _) = parse_reply(handle_worker_nb_bus(
        &wdeps,
        serde_json::json!({"v": 1, "ns": "board", "op": "flush"}),
    ));
    assert!(!ok);
    assert_eq!(
        code.as_deref(),
        Some(nemesis_cluster::envelope::error_code::UNKNOWN_OP)
    );

    // wake.post 缺 thread.id → VALIDATION。
    let (ok, _, _) = parse_reply(handle_worker_nb_bus(
        &wdeps,
        serde_json::json!({"v": 1, "ns": "board", "op": "wake.post", "corr_id": "w",
            "_rpc": {"from": "node-master"},
            "body": {"seq": 1, "thread": {"kind": "issue"},
                "new_message": {"sender": "node-master", "content": "x"}}}),
    ));
    assert!(!ok, "缺 thread.id 应被拒");

    // 缺 new_message.content → VALIDATION。
    let (ok, _, _) = parse_reply(handle_worker_nb_bus(
        &wdeps,
        serde_json::json!({"v": 1, "ns": "board", "op": "wake.post", "corr_id": "w2",
            "_rpc": {"from": "node-master"},
            "body": {"seq": 2, "thread": {"kind": "issue", "id": 1},
                "new_message": {"sender": "node-master"}}}),
    ));
    assert!(!ok, "缺 new_message.content 应被拒");
}

#[test]
fn sweep_wake_state_persist_failure_continues_in_memory() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-wakestate-persistfail-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // 快照路径的父级是普通文件 → create_dir_all 失败 → warn 不上抛。
    let blocked = dir.join("blocked.txt");
    std::fs::write(&blocked, "not a dir").unwrap();

    let state = WorkerWakeState::load_or_create(blocked.join("state.json"));
    state.commit("t1", 7); // persist 失败只 warn；内存账照常推进。
    assert!(!state.peek("t1", 7), "commit 后同 seq 幂等");
    state.advance_watermark(9);
    assert_eq!(state.watermark(), 9);
    assert!(state.is_participated("t1"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn sweep_worker_sync_once_requires_coordinator_then_rpc() {
    let _ = tracing_subscriber::fmt::try_init();
    let (wdeps, _inbox) = make_worker_deps("node-b");
    let cluster = Cluster::new(ClusterConfig {
        node_id: "node-b".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: String::new(),
    });

    // 无任何节点 → 找不到在线 coordinator。
    let err = worker_sync_once(&wdeps, &cluster).await.unwrap_err();
    assert!(err.contains("no online coordinator"), "got: {err}");

    // 只有自己是 coordinator → 防御性跳过（master 形态误装 worker 通道）。
    register_node(&cluster, "node-b", NodeRole::Coordinator, "dev", true);
    let err = worker_sync_once(&wdeps, &cluster).await.unwrap_err();
    assert!(err.contains("no online coordinator"), "got: {err}");

    // 有在线 coordinator 但无 rpc client → rpc unavailable。
    register_node(&cluster, "node-master", NodeRole::Coordinator, "dev", true);
    let err = worker_sync_once(&wdeps, &cluster).await.unwrap_err();
    assert!(err.contains("rpc client unavailable"), "got: {err}");

    // load_or_create 的 tracing 行（subscriber 已装 → 格式参数真实执行）。
    let dir =
        std::env::temp_dir().join(format!("nemesis-wakestate-tracing-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state = WorkerWakeState::load_or_create(dir.join("state.json"));
    state.commit("issue:1", 4);
    assert!(!state.peek("issue:1", 4));
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// wave4 追加（coverage）：worker_sync_once 成功路径（此前只有前置错误臂）
// + spawn_worker_sync_loop 常驻任务 + run_moderator 残余臂（loop 未就绪 /
// 额度归零静默收敛 / 无 rpc client）。
//
// RPC 真回环：本机 mock master（4 字节大端长度前缀 + WireMessage JSON，
// 与 nemesis-cluster 帧协议同构；client_extra_tests::spawn_response_server
// 同款手法）——worker_sync_once 的 rpc.call 走真实 TCP 往返，响应载荷 =
// EnvelopeResponse::to_json 形态（{ok:true, body:{latest_seq, messages}}）。
// RpcClient::with_timeout(10s) 兜底（默认 60min 超时不可进测试）。
// ===========================================================================

/// 固定地址解析器：对端恒在线、恒指 mock master（dial 地址与端口测试自定）。
struct FixedPeerResolver {
    addr: String,
    port: u16,
}

impl nemesis_cluster::rpc::client::PeerResolver for FixedPeerResolver {
    fn get_peer_info(&self, _peer: &str) -> Option<(Vec<String>, u16, bool)> {
        Some((vec![self.addr.clone()], self.port, true))
    }
    fn get_local_interfaces(&self) -> Vec<nemesis_cluster::rpc::client::LocalNetworkInterface> {
        vec![]
    }
    fn get_node_id(&self) -> String {
        "node-b".into()
    }
}

/// mock master：accept 一条连接 → 读请求帧 → 回 sync 成功信封（id 回显——
/// 客户端按 id 关联响应，错 id 会挂到超时）。返回监听端口。
async fn spawn_sync_mock_server(latest_seq: i64, messages: serde_json::Value) -> u16 {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut len_buf = [0u8; 4];
        if sock.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        if sock.read_exact(&mut buf).await.is_err() {
            return;
        }
        // 回显请求 id（关联锚点）。
        let req_id = serde_json::from_slice::<serde_json::Value>(&buf)
            .ok()
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
            .unwrap_or_default();
        let wire = nemesis_cluster::transport::conn::WireMessage {
            version: "1.0".into(),
            id: req_id,
            msg_type: "response".into(),
            from: "node-master".into(),
            to: "node-b".into(),
            action: "nb_bus".into(),
            payload: serde_json::json!({
                "v": 1, "ok": true, "ns": "board", "op": "sync", "corr_id": "mock",
                "body": {"latest_seq": latest_seq, "messages": messages}
            }),
            timestamp: chrono::Local::now().timestamp(),
            error: String::new(),
        };
        let json = serde_json::to_vec(&wire).unwrap();
        let total = (json.len() as u32).to_be_bytes();
        let _ = sock.write_all(&total).await;
        let _ = sock.write_all(&json).await;
        let _ = sock.flush().await;
        // 连接随即关闭（单次调用语义；连接池复用失败会重连，不在路径上）。
    });
    port
}

/// 组装带 mock master 的 worker 侧环境：集群注册表登记在线 coordinator +
/// 带固定解析器的 RPC client（10s 超时兜底）。返回 (deps, inbox, rx)。
async fn worker_sync_env(
    port: u16,
) -> (
    WorkerBusDeps,
    Arc<nemesis_cluster::cluster::Cluster>,
    Arc<crate::cluster_agent::DiscussionInbox>,
    tokio::sync::mpsc::UnboundedReceiver<nemesis_types::cluster::DiscussionEvent>,
) {
    use nemesis_cluster::types::{ExtendedNodeInfo, NodeStatus};
    use nemesis_types::cluster::NodeInfo;
    let cluster = Arc::new(Cluster::new(ClusterConfig {
        node_id: "node-b".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: vec![],
        node_name: "Node-Alpha".to_string(),
    }));
    cluster.register_node(ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-master".to_string(),
            name: "node-master".to_string(),
            role: NodeRole::Coordinator,
            address: format!("127.0.0.1:{port}"),
            category: "dev".to_string(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        node_type: "agent".to_string(),
    });
    let client = Arc::new(RpcClient::with_resolver(Arc::new(FixedPeerResolver {
        addr: "127.0.0.1".to_string(),
        port,
    })));
    cluster.set_rpc_client(client);

    let inbox = Arc::new(crate::cluster_agent::DiscussionInbox::new());
    let deps = WorkerBusDeps {
        self_node_id: "node-b".to_string(),
        node_name: "Node-Alpha".to_string(),
        node_role: "worker".to_string(),
        node_category: "dev".to_string(),
        inbox: inbox.clone(),
        wake_state: Arc::new(WorkerWakeState::new()),
    };
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    inbox.set_sender(tx);
    (deps, cluster, inbox, rx)
}

/// sync 台账条目（worker_sync_once 消费的 master 形态）。
fn sync_entry(seq: i64, kind: &str, tid: i64, sender: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "seq": seq, "thread_kind": kind, "thread_id": tid,
        "sender_id": sender, "content": content, "created_at": 111
    })
}

/// 成功路径全链：@ 提名入队（commit + 水位推进到 latest）、无提名过滤、
/// 自己的发言过滤。断言事件字段（sync_backfill 最小信息形态）与幂等水位。
#[tokio::test]
async fn sweep_worker_sync_once_happy_path_enqueues_and_advances() {
    let _ = tracing_subscriber::fmt::try_init();
    let port = spawn_sync_mock_server(
        5,
        serde_json::json!([
            sync_entry(3, "issue", 7, "node-master", "@node-b 请处理"),
            sync_entry(4, "channel", 2, "node-master", "普通闲聊"),
            sync_entry(5, "channel", 9, "node-b", "我自己的话")
        ]),
    )
    .await;
    let (wdeps, cluster, _inbox, mut rx) = worker_sync_env(port).await;

    let enqueued = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        worker_sync_once(&wdeps, &cluster),
    )
    .await
    .expect("sync 往返必须在 15s 内完成（mock 卡死快速失败）")
    .expect("sync 应成功");
    assert_eq!(
        enqueued, 1,
        "只有 @node-b 一条入队（闲聊过滤 + 自己的发言过滤）"
    );

    let ev = rx.try_recv().expect("补拉事件应已入箱");
    assert_eq!(ev.event, "sync_backfill");
    assert_eq!(ev.seq, 3);
    assert_eq!(ev.thread_kind, "issue");
    assert_eq!(ev.thread_id, 7);
    assert_eq!(ev.new_sender, "node-master");
    assert!(ev.new_content.contains("@node-b"));
    assert_eq!(ev.from_node, "node-master");
    assert_eq!(ev.max_turns_left, u32::MAX, "补拉事件不设轮次上限");
    assert!(ev.thread_title.is_empty(), "补拉不带线程历史");
    assert!(rx.try_recv().is_err(), "其余两条被过滤，不得入箱");

    // 水位推进到 master latest；已处理 seq 幂等。
    assert_eq!(wdeps.wake_state.watermark(), 5);
    assert!(
        !wdeps.wake_state.peek("issue:7", 3),
        "已 commit 的 seq 不得重跑"
    );
    assert!(wdeps.wake_state.peek("issue:7", 4), "更大的 seq 仍要处理");
}

/// 投递失败（inbox 无接收端）→ 游标只推进到失败条目之前（T26 根因修复：
/// 补拉机制不得自己吞掉 wake 事件）。
#[tokio::test]
async fn sweep_worker_sync_undelivered_holds_watermark() {
    let _ = tracing_subscriber::fmt::try_init();
    let port = spawn_sync_mock_server(
        9,
        serde_json::json!([
            sync_entry(3, "issue", 7, "node-master", "@node-b 甲"),
            sync_entry(4, "channel", 2, "node-master", "@node-b 乙")
        ]),
    )
    .await;
    let (wdeps, cluster, _inbox, rx) = worker_sync_env(port).await;
    // 接收端已关闭（cluster agent loop 刚退出）→ inbox.send 全数失败。
    drop(rx);

    let enqueued = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        worker_sync_once(&wdeps, &cluster),
    )
    .await
    .expect("sync 往返必须在 15s 内完成")
    .expect("sync 应成功");
    assert_eq!(enqueued, 0);
    // 两条都失败：first_undelivered=3 → advance = min(3-1, 9) = 2。
    assert_eq!(
        wdeps.wake_state.watermark(),
        2,
        "游标必须停在失败条目之前，下一拍重拉重试"
    );
    assert!(
        wdeps.wake_state.peek("issue:7", 3),
        "失败条目未 commit，可重拉"
    );
}

/// 常驻补拉任务：首拍立即执行（interval 首 tick 即时）→ 经 mock master
/// 补投 @ 提名事件进 inbox。
#[tokio::test]
async fn sweep_spawn_worker_sync_loop_first_tick_backfills() {
    let _ = tracing_subscriber::fmt::try_init();
    let port = spawn_sync_mock_server(
        3,
        serde_json::json!([sync_entry(3, "issue", 7, "node-master", "@node-b 走起")]),
    )
    .await;
    let (wdeps, cluster, _inbox, mut rx) = worker_sync_env(port).await;

    spawn_worker_sync_loop(wdeps.clone(), cluster.clone());
    // 首拍即时；轮询等事件落箱（上限 3s）。
    let mut got = None;
    for _ in 0..60 {
        if let Ok(ev) = rx.try_recv() {
            got = Some(ev);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let ev = got.expect("常驻补拉任务首拍必须把 @ 提名事件补进 inbox");
    assert_eq!(ev.event, "sync_backfill");
    assert_eq!(ev.seq, 3);
    assert_eq!(wdeps.wake_state.watermark(), 3);
}

/// run_moderator 残余臂：① agent loop 未就绪 → 静默跳过；② 回复落库后
/// 线程额度归零 → 静默收敛（不再投递）；③ 有提名但 rpc client 缺席 →
/// 诚实收敛。进程内全离线。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
#[tokio::test]
#[allow(clippy::await_holding_lock)]
async fn sweep_run_moderator_not_ready_quota_zero_and_no_rpc() {
    use nemesis_board::arbitrator::NodeCandidate;
    let _lock = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let _env = crate::tests::EnvHomeGuard::point_at(&home);

    let nodes = vec![NodeCandidate {
        id: "node-b".to_string(),
        name: "node-b".to_string(),
        role: "worker".to_string(),
        category: "qa".to_string(),
        online: true,
    }];
    let ctx = || WakeContext {
        thread_kind: "channel".to_string(),
        thread_id: 0, // 由调用方按 store 实际频道替换
        sender: Actor::new("agent", "node-x"),
        content: "问题".to_string(),
        reply_to: None,
        seq: 1,
        at: 42,
    };

    // ① loop 未就绪 → Ok + 不落任何消息。
    let (store, dir) = temp_store("mod-notready");
    let deps = deps_with_workspace(store.clone(), dir.join("ws"), quota(8));
    let ch0 = store.list_channels().unwrap()[0].id;
    let d = DepsForTask {
        store: store.clone(),
        quota: quota(8),
        cluster: deps.cluster.clone(),
        moderator_loop: deps.moderator_loop.clone(),
    };
    let mut c = ctx();
    c.thread_id = ch0;
    let r = run_moderator(d, c, &nodes, "node-master").await;
    assert!(r.is_ok(), "loop 未就绪必须静默 Ok：{r:?}");
    assert_eq!(
        store.list_channel_messages(ch0, 0, i64::MAX).unwrap().len(),
        0,
        "未就绪不得落消息"
    );
    let _ = std::fs::remove_dir_all(&dir);

    // ② 回复带 @ 提名但线程额度归零 → 落库后静默收敛（不再投递）。
    let (store, dir) = temp_store("mod-quota0");
    let deps = deps_with_workspace(store.clone(), dir.join("ws"), quota(0));
    attach_scripted_moderator(&deps, &[], "@node-b 已收到");
    let ch0 = store.list_channels().unwrap()[0].id;
    let d = DepsForTask {
        store: store.clone(),
        quota: quota(0),
        cluster: deps.cluster.clone(),
        moderator_loop: deps.moderator_loop.clone(),
    };
    let mut c = ctx();
    c.thread_id = ch0;
    let r = run_moderator(d, c, &nodes, "node-master").await;
    assert!(r.is_ok(), "额度归零必须静默 Ok：{r:?}");
    let msgs = store.list_channel_messages(ch0, 0, i64::MAX).unwrap();
    assert_eq!(msgs.len(), 1, "主持人回复已落库");
    assert_eq!(msgs[0].sender.id, "node-master");
    assert!(msgs[0].content.contains("@node-b"));
    let _ = std::fs::remove_dir_all(&dir);

    // ③ 回复带 @ 提名、额度充足、但 rpc client 缺席 → 诚实收敛不炸。
    let (store, dir) = temp_store("mod-norpc");
    let deps = deps_with_workspace(store.clone(), dir.join("ws"), quota(8));
    attach_scripted_moderator(&deps, &[], "@node-b 请继续");
    let ch0 = store.list_channels().unwrap()[0].id;
    let d = DepsForTask {
        store: store.clone(),
        quota: quota(8),
        cluster: deps.cluster.clone(),
        moderator_loop: deps.moderator_loop.clone(),
    };
    let mut c = ctx();
    c.thread_id = ch0;
    let r = run_moderator(d, c, &nodes, "node-master").await;
    assert!(r.is_ok(), "rpc 缺席必须静默 Ok：{r:?}");
    assert_eq!(
        store.list_channel_messages(ch0, 0, i64::MAX).unwrap().len(),
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ===========================================================================
// wave5 round2（2026-09-25）：delivery.files 落盘写失败臂（目录只读 →
// fs::write 失败 → INTERNAL「写入失败」）、post_discussion_locally 的
// Store Err 映射臂（空白 content 穿过核心直达 store 校验）、
// build_thread_context_text 纯函数（issue 评论线程 / channel 消息线程 /
// 标题回退三形态）、WorkerWakeState::load_or_create 盘上快照恢复 + commit
// 持久化（生产 worker 形态；测试此前只用纯内存 new()）。
// ===========================================================================

mod w5r2 {
    use super::*;

    /// delivery.files：目录已存在但目标名被同名**目录**占用 →
    /// create_dir_all Ok、fs::write 必败 → INTERNAL「写入失败」（区别于
    /// 目录自建失败臂）。stored_name = `{Utc 毫秒}_a.txt`——写入发生在
    /// 布阵**之后**，漂移单向为正：单侧深窗 [t0-100, t0+8000] 覆盖
    /// 「布阵耗时 ≤8s」的全部落点（±300ms 双侧窗会被布阵自身耗时漂出，
    /// 已实测偶发漏接）。
    #[test]
    fn w5_delivery_files_write_fail_readonly_dir() {
        use base64::Engine as _;
        let (store, dir) = temp_store("w5-dl-readonly");
        let ws = tempfile::tempdir().unwrap();
        let deps = deps_with_workspace(store.clone(), ws.path().to_path_buf(), quota(8));

        let issue = store
            .create_issue(nemesis_board::NewIssue {
                title: "撞名交付".into(),
                ..Default::default()
            })
            .unwrap();
        store
            .insert_dispatch("w5-dl-1", issue.id, "node-b", &Actor::admin("test"))
            .unwrap();

        // 预建落盘目录 + 目标名同名目录阵（时间戳碰撞注入，单侧深窗）。
        let files_dir = ws
            .path()
            .join("board")
            .join("files")
            .join(format!("issue_{}", issue.id));
        std::fs::create_dir_all(&files_dir).unwrap();
        let t0 = chrono::Utc::now().timestamp_millis();
        for ms in t0 - 100..=t0 + 8000 {
            std::fs::create_dir(files_dir.join(format!("{ms}_a.txt"))).unwrap();
        }

        let (ok, code, _) = parse_reply(handle_nb_bus(
            &deps,
            delivery_payload(
                Some("w5-dl-1"),
                "node-b",
                serde_json::json!([{"name": "a.txt", "content_b64": base64::engine::general_purpose::STANDARD.encode(b"x")}]),
            ),
        ));
        assert!(!ok, "同名目录占用写入路径必须失败");
        assert_eq!(
            code.as_deref(),
            Some(nemesis_cluster::envelope::error_code::INTERNAL)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// post_discussion_locally：空白 content 穿过核心（幂等预检/额度后）
    /// 直达 store 校验 Err → PostError::Store → 人读 Err 映射臂。
    #[test]
    fn w5_post_locally_store_err_arm() {
        let (store, dir) = temp_store("w5-post-store-err");
        let deps = make_deps(store.clone(), quota(8));
        let err = post_discussion_locally(
            &deps,
            &Actor::agent("node-b"),
            "issue",
            1,
            "w5-blank-1",
            "   ",
            None,
            "text",
        )
        .expect_err("空白 content 必须 Store Err");
        assert!(err.contains("content"), "错误应来自 store 校验: {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// build_thread_context_text：issue 线程（评论投影 + 标题行）与
    /// channel 线程（消息投影）两形态 + 超 20 条截尾语义。
    #[test]
    fn w5_build_thread_context_text_issue_and_channel() {
        let (store, dir) = temp_store("w5-ctx-text");
        let issue = store
            .create_issue(nemesis_board::NewIssue {
                title: "上下文标题甲".into(),
                ..Default::default()
            })
            .unwrap();
        for i in 0..25 {
            store
                .add_comment(nemesis_board::NewComment {
                    issue_id: issue.id,
                    author: Actor::agent(&format!("node-{i}")),
                    content: format!("评论{i}"),
                    parent_id: None,
                    ctype: nemesis_board::CommentType::Comment,
                })
                .unwrap();
        }
        let ctx_issue = WakeContext {
            thread_kind: "issue".to_string(),
            thread_id: issue.id,
            sender: Actor::agent("node-b"),
            content: "触发词".to_string(),
            reply_to: None,
            seq: 1,
            at: 0,
        };
        let text = build_thread_context_text(&store, &ctx_issue).unwrap();
        assert!(
            text.contains("上下文标题甲"),
            "issue 线程必须带标题: {text}"
        );
        assert!(text.contains("评论24"), "截尾必须保留最新评论");
        assert!(!text.contains("评论0\n"), "最旧评论应被截掉");

        // channel 线程形态。
        let ch = store.get_channel_by_name("#dev").unwrap().unwrap();
        store
            .append_channel_message(nemesis_board::NewChannelMessage {
                channel_id: ch.id,
                sender: Actor::agent("node-b"),
                content: "频道消息一".to_string(),
                parent_id: None,
                mtype: String::new(),
            })
            .unwrap();
        let ctx_ch = WakeContext {
            thread_kind: "channel".to_string(),
            thread_id: ch.id,
            sender: Actor::agent("node-b"),
            content: "频道触发".to_string(),
            reply_to: None,
            seq: 1,
            at: 0,
        };
        let text2 = build_thread_context_text(&store, &ctx_ch).unwrap();
        assert!(
            text2.contains("频道消息一"),
            "channel 投影必须可见: {text2}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// WorkerWakeState::load_or_create：盘上合法快照恢复（水位 + 参与集）
    /// → peek/commit/advance_watermark → 快照文件回写。
    #[test]
    fn w5_wake_state_snapshot_restore_and_persist() {
        let tmp = tempfile::tempdir().unwrap();
        let snap = tmp.path().join("wake-snap.json");
        std::fs::write(
            &snap,
            r#"{"threads": {"issue:7": 5, "channel:2": 9}, "watermark": 42}"#,
        )
        .unwrap();

        let state = WorkerWakeState::load_or_create(snap.clone());
        assert_eq!(state.watermark(), 42, "全局游标必须从盘上恢复");
        assert!(!state.peek("issue:7", 5), "已处理 seq 不得重放");
        assert!(state.peek("issue:7", 6), "新 seq 必须放行");
        assert!(state.is_participated("channel:2"), "参与集必须恢复");

        // commit → 线程水位推进 + 快照回写（盘上可见新水位）。
        state.commit("issue:7", 6);
        let reloaded = WorkerWakeState::load_or_create(snap.clone());
        assert!(
            !reloaded.peek("issue:7", 6),
            "commit 后新 seq 必须入账（持久化）"
        );
        state.advance_watermark(50);
        assert_eq!(state.watermark(), 50);
    }
}
