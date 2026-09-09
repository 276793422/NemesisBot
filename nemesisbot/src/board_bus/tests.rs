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
    });
    MasterBusDeps {
        store,
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
fn post_payload(client_msg_id: &str, thread_kind: &str, thread_id: i64, content: &str) -> serde_json::Value {
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

fn parse_reply(reply: Result<serde_json::Value, String>) -> (bool, Option<String>, serde_json::Value) {
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
    assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::BAD_ENVELOPE));

    // 版本不符 → bad_envelope。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 99, "ns": "board", "op": "sync"}),
    ));
    assert!(!ok);
    assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::BAD_ENVELOPE));

    // 未知 ns / op → 各自错误码。
    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "file", "op": "read"}),
    ));
    assert!(!ok);
    assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS));

    let (ok, code, _) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "nope"}),
    ));
    assert!(!ok);
    assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::UNKNOWN_OP));
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
        assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::VALIDATION));
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
    let (ok2, _, body2) = parse_reply(handle_nb_bus(&deps, post_payload("u-1", "channel", ch.id, "第一条")));
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

    let (ok, _, _) = parse_reply(handle_nb_bus(&deps, post_payload("q-1", "channel", ch.id, "第一条")));
    assert!(ok);
    // 线程额度=1 → 第二条 quota_exhausted（消息不落库）。
    let (ok, code, _) = parse_reply(handle_nb_bus(&deps, post_payload("q-2", "channel", ch.id, "第二条")));
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
    let _ = handle_nb_bus(
        &deps,
        post_payload("s-2", "issue", issue.id, "评论一"),
    );

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
    assert_eq!(body["body"]["latest_seq"], body["body"]["messages"][1]["seq"]);

    // since_seq 游标：只取增量。
    let (ok, _, body) = parse_reply(handle_nb_bus(
        &deps,
        serde_json::json!({"v": 1, "ns": "board", "op": "sync", "body": {"since_seq": 1}}),
    ));
    assert!(ok);
    assert_eq!(
        body["body"]["messages"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
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
    let reply = handle_worker_nb_bus(&deps, wake_payload("issue", 7, 4, "@node-b 看这个", "node-master"));
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
    let reply = handle_worker_nb_bus(&deps, wake_payload("issue", 7, 4, "@node-b 看这个", "node-master"));
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
            v.pointer("/error/code").and_then(|c| c.as_str()).map(String::from),
        )
    };
    assert!(!ok);
    assert_eq!(code.as_deref(), Some(nemesis_cluster::envelope::error_code::UNKNOWN_NS));
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
    assert!(deps.wake_state.peek("issue:7", 4), "send failure must not commit");

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
    assert!(!sync_entry_targets_me(&deps, "@node-b 自问自答", "node-b", "channel:1"));
    // 提到 id / name（大小写不敏感）→ 真。
    assert!(sync_entry_targets_me(&deps, "请 @node-b 确认", "node-master", "channel:1"));
    assert!(sync_entry_targets_me(&deps, "@node-alpha 帮忙看下", "node-master", "channel:1"));
    // @role: 命中拓扑角色（worker）或功能类别（dev）→ 真。
    assert!(sync_entry_targets_me(&deps, "@role:worker 都来看", "node-master", "channel:1"));
    assert!(sync_entry_targets_me(&deps, "@role:dev 集合", "node-master", "channel:1"));
    // @role: 未命中类别 → 假。
    assert!(!sync_entry_targets_me(&deps, "@role:qa 看这里", "node-master", "channel:1"));
    // 无提及且未参与 → 假。
    assert!(!sync_entry_targets_me(&deps, "大家辛苦了", "node-master", "channel:1"));
    // 参与过的线程（wake 处理过）→ 无提及也真（G8「我参与的线程」）。
    deps.wake_state.commit("channel:2", 1);
    assert!(sync_entry_targets_me(&deps, "后续讨论", "node-master", "channel:2"));
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
    assert!(!reloaded.peek("t1", 7), "old seq must not replay after restart");
    assert!(!reloaded.peek("t1", 3), "older seq must not replay after restart");
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
    let dir = std::env::temp_dir().join(format!(
        "nemesis-wakestate-g8-{}-mem",
        std::process::id()
    ));
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
