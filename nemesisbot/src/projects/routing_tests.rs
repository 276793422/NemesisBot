//! L6++ G3 bus 路由六矩阵红线测试（impl plan M3 验收门，2026-09-08）。
//!
//! 真 `MessageBus` + 真 `ProjectLoopManager`，项目 loop 用裸 mpsc 通道注入
//! （`insert_test_channel`——路由测试只验证转发/丢弃语义，不需要真 loop
//! 消费）。六矩阵：① 项目消息→项目收、主桥 skip；② 主会话消息→主桥
//! 放行、调度丢弃；③ system 消息仅主 loop（即使带项目归属）；④ 同项目
//! 并发两会话各自排队不串；⑤ A 项目消息绝不进 B 项目通道；⑥ 索引未
//! 预热但 meta 带绑定（fork 形态）→ sidecar 回读命中并回填。
//! 另附：不可用诚实出站错误、start_routing 幂等（唯一订阅）。

use std::sync::Arc;
use std::time::Duration;

use super::manager::{route_decision, RouteDecision};
use super::registry;
use crate::agent_factory::SharedResources;
use crate::projects::manager::ProjectLoopManager;
use nemesis_types::channel::InboundMessage;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn unique_home(tag: &str) -> std::path::PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nb_proj_route_{tag}_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        seq
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// mini 档模型 config（manager 构造路径需要 config.json 存在——与
/// manager/tests.rs 同形态）。
fn write_mini_model_config(home: &std::path::Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [ {
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        } ]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn msg(channel: &str, session_key: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: "u1".to_string(),
        chat_id: format!("web:{}", session_key.rsplit(':').next().unwrap_or("x")),
        content: "hello".to_string(),
        media: Vec::new(),
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

/// 真 bus + 真 manager（不 spawn 真项目 loop；通道按需注入）。返回
/// (bus, mgr, home) 供 sidecar 预置类用例定位 workspace。
fn fixture(
    tag: &str,
) -> (
    Arc<nemesis_bus::MessageBus>,
    Arc<ProjectLoopManager>,
    std::path::PathBuf,
) {
    let home = unique_home(tag);
    write_mini_model_config(&home);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        ..Default::default()
    });
    let sess_dir = home.join("sessions");
    std::fs::create_dir_all(&sess_dir).unwrap();
    let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
        &sess_dir,
    ));
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    let mgr = Arc::new(ProjectLoopManager::new(shared, store, bus.clone()));
    (bus, mgr, home)
}

/// 矩阵断言辅助：`rx` 在超时内应收到消息（收到 = 项目 loop「收到」）。
async fn expect_recv(
    rx: &mut tokio::sync::mpsc::Receiver<InboundMessage>,
    what: &str,
) -> InboundMessage {
    tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .unwrap_or_else(|_| panic!("timeout waiting for {what}"))
        .expect("channel closed")
}

/// 矩阵断言辅助：`rx` 应保持为空（调度丢弃 = 不转发）。
async fn expect_empty(rx: &mut tokio::sync::mpsc::Receiver<InboundMessage>, what: &str) {
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        rx.try_recv().is_err(),
        "{what}: message must NOT be forwarded"
    );
}

// ---------------------------------------------------------------------------
// 矩阵 ①：项目会话消息 → 项目 loop 收到、主桥 skip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix1_project_session_message_reaches_project_loop_and_main_bridge_skips() {
    let (bus, mgr, _home) = fixture("m1");
    let pid = "p_m1aaaaa";
    let key = "agent:main:session:proj1";
    mgr.remember_session(key, pid);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid, tx);
    mgr.start_routing();

    let m = msg("web", key);
    bus.publish_inbound(m.clone());
    let got = expect_recv(&mut rx, "project channel").await;
    assert_eq!(got.session_key, key, "matrix1: project loop receives it");

    // 主桥 skip：谓词命中（主 loop 桥会丢弃该消息，不与项目 loop 竞争）。
    assert!(
        mgr.bridge_should_skip(&m),
        "matrix1: main bridge must skip project-bound messages"
    );
}

// ---------------------------------------------------------------------------
// 矩阵 ②：主会话消息 → 主桥放行、项目调度丢弃
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix2_main_session_message_goes_to_main_and_dispatcher_drops() {
    let (bus, mgr, _home) = fixture("m2");
    let pid = "p_m2aaaaa";
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid, tx);
    mgr.start_routing();

    // 无归属会话（未 remember、无 sidecar）。
    let m = msg("web", "agent:main:session:free1");
    bus.publish_inbound(m.clone());
    expect_empty(&mut rx, "matrix2 dispatcher").await;
    assert!(
        !mgr.bridge_should_skip(&m),
        "matrix2: main bridge must consume unbound-session messages"
    );
}

// ---------------------------------------------------------------------------
// 矩阵 ③：cluster_continuation（system）→ 仅主 loop 消费（即使带项目归属）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix3_system_messages_only_main_loop_even_when_project_bound() {
    let (bus, mgr, _home) = fixture("m3");
    let pid = "p_m3aaaaa";
    let key = "agent:main:session:proj3";
    mgr.remember_session(key, pid);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid, tx);
    mgr.start_routing();

    // cluster_continuation 形态：channel=system，sender 带续行前缀，
    // session_key 恰好是项目绑定会话。
    let mut m = msg("system", key);
    m.sender_id = format!("{}{}", nemesis_types::constants::CLUSTER_CONTINUATION_PREFIX, "task1");
    bus.publish_inbound(m.clone());
    expect_empty(&mut rx, "matrix3 dispatcher").await;
    // 主桥不 skip（系统回灌语义只属于主 loop）；纯函数三值自洽。
    assert!(!mgr.bridge_should_skip(&m), "matrix3: main bridge consumes system messages");
    assert_eq!(
        route_decision(&m, Some(pid)),
        RouteDecision::System,
        "matrix3: route_decision classifies system as System regardless of ownership"
    );
}

// ---------------------------------------------------------------------------
// 矩阵 ④：同一项目并发两会话各自排队不串
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix4_two_concurrent_sessions_same_project_queue_without_interleave() {
    let (bus, mgr, _home) = fixture("m4");
    let pid = "p_m4aaaaa";
    let key_a = "agent:main:session:s4a";
    let key_b = "agent:main:session:s4b";
    mgr.remember_session(key_a, pid);
    mgr.remember_session(key_b, pid);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid, tx);
    mgr.start_routing();

    // 同项目两会话各自消息：都进同一项目通道（busy/queue 在 loop 侧天然
    // per-session），顺序保持（单调度任务串行 + mpsc FIFO）。
    bus.publish_inbound(msg("web", key_a));
    bus.publish_inbound(msg("web", key_b));
    let first = expect_recv(&mut rx, "matrix4 first").await;
    let second = expect_recv(&mut rx, "matrix4 second").await;
    assert_eq!(first.session_key, key_a, "matrix4: FIFO order preserved");
    assert_eq!(second.session_key, key_b);
    assert_ne!(first.session_key, second.session_key, "matrix4: sessions stay distinct");
}

// ---------------------------------------------------------------------------
// 矩阵 ⑤：A 项目消息绝不进 B 项目 mpsc
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix5_project_a_message_never_enters_project_b_channel() {
    let (bus, mgr, _home) = fixture("m5");
    let pid_a = "p_m5aaaaa";
    let pid_b = "p_m5bbbbb";
    let key_a = "agent:main:session:only_a";
    let key_b = "agent:main:session:only_b";
    mgr.remember_session(key_a, pid_a);
    mgr.remember_session(key_b, pid_b);
    let (tx_a, mut rx_a) = tokio::sync::mpsc::channel(8);
    let (tx_b, mut rx_b) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid_a, tx_a);
    mgr.insert_test_channel(pid_b, tx_b);
    mgr.start_routing();

    bus.publish_inbound(msg("web", key_a));
    let got = expect_recv(&mut rx_a, "matrix5 A channel").await;
    assert_eq!(got.session_key, key_a);
    expect_empty(&mut rx_b, "matrix5 B channel").await;
}

// ---------------------------------------------------------------------------
// 矩阵 ⑥：索引未预热但 meta 带绑定（fork 形态）→ sidecar 回读命中并回填
// ---------------------------------------------------------------------------

#[tokio::test]
async fn matrix6_sidecar_fallback_hits_and_backfills_index() {
    let (_bus, mgr, home) = fixture("m6");
    let pid = "p_m6aaaaa";
    let key = "agent:main:session:forked1";
    let stem = super::manager::stem_from_session_key(key);
    assert_eq!(stem, "agent_main_session_forked1");

    // 预置 meta sidecar（fork 继承绑定但绕过 sessions.create 的形态——
    // 索引未预热：不 remember）。
    let logs_dir = home.join("workspace").join("logs").join("session_logs");
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(
        logs_dir.join(format!("{stem}.meta.json")),
        serde_json::json!({ "title": "fork", "project_id": pid }).to_string(),
    )
    .unwrap();

    // 第一次查询：索引 miss → sidecar 回读命中。
    assert_eq!(mgr.owner_of(key).as_deref(), Some(pid), "matrix6: fallback hits sidecar");

    // 删 sidecar 后再查：仍命中 → 证明第一次命中已回填内存索引。
    std::fs::remove_file(logs_dir.join(format!("{stem}.meta.json"))).unwrap();
    assert_eq!(
        mgr.owner_of(key).as_deref(),
        Some(pid),
        "matrix6: backfilled index survives sidecar removal"
    );

    // forget_session 摘除后：miss（对话组）。
    assert!(mgr.forget_session(key), "forget must report removed binding");
    assert_eq!(mgr.owner_of(key), None, "after forget: unbound");
}

// ---------------------------------------------------------------------------
// 附加：归属命中但目标不可用 → 诚实出站错误（不静默丢）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unroutable_project_session_gets_honest_outbound_error() {
    let (bus, mgr, _home) = fixture("unroutable");
    let pid = "p_gone001";
    let key = "agent:main:session:deadproj";
    mgr.remember_session(key, pid);
    // 不注入通道（= inactive/已移除/未运行形态）。
    mgr.start_routing();

    let mut out_rx = bus.subscribe_outbound();
    bus.publish_inbound(msg("web", key));
    let out = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timeout waiting for error outbound")
        .expect("outbound closed");
    assert_eq!(out.channel, "web", "error goes back to the source channel");
    assert_eq!(out.chat_id, format!("web:deadproj"), "error addresses the source chat");
    assert!(
        out.content.contains("当前不可用"),
        "error text must be honest about unavailability: {}",
        out.content
    );
}

// ---------------------------------------------------------------------------
// 附加：start_routing 幂等（全进程唯一 1 个项目调度订阅）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn start_routing_is_idempotent_single_subscription() {
    let (bus, mgr, _home) = fixture("idem");
    let pid = "p_idem001";
    let key = "agent:main:session:once";
    mgr.remember_session(key, pid);
    let (tx, mut rx) = tokio::sync::mpsc::channel(8);
    mgr.insert_test_channel(pid, tx);
    mgr.start_routing();
    mgr.start_routing(); // 第二次调用必须被幂等闸挡住
    mgr.start_routing();

    bus.publish_inbound(msg("web", key));
    let got = expect_recv(&mut rx, "idempotent routing").await;
    assert_eq!(got.session_key, key);
    // 若存在第二个订阅，同一消息会被转发两次。
    expect_empty(&mut rx, "idempotent duplicate").await;
}

// ---------------------------------------------------------------------------
// 附加：registry 条目名进不可用错误文案（诚实提示项目名而非裸 id）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unroutable_error_mentions_project_display_name() {
    let (bus, mgr, home) = fixture("named");
    let project_dir = home.join("proj_named");
    std::fs::create_dir_all(&project_dir).unwrap();
    let entry = registry::create_project(
        mgr.registry_path(),
        &home.join("workspace"),
        "我的小项目",
        project_dir.to_str().unwrap(),
        4,
    )
    .expect("create project");
    let key = "agent:main:session:named1";
    mgr.remember_session(key, &entry.id);
    mgr.start_routing();

    let mut out_rx = bus.subscribe_outbound();
    bus.publish_inbound(msg("web", key));
    let out = tokio::time::timeout(Duration::from_secs(2), out_rx.recv())
        .await
        .expect("timeout waiting for error outbound")
        .expect("outbound closed");
    assert!(
        out.content.contains("我的小项目"),
        "error must show project display name: {}",
        out.content
    );
}
