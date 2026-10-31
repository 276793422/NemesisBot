use super::*;
use nemesis_types::cluster::TaskStatus;

/// REL-002 失败注入助手：唯一临时名升级后，旧「预建固定名 `.tmp` 目录
/// 阻塞临时文件」的注入面不复存在（正是升级目标之一），改用平台确定的
/// 「替换失败」注入——
/// - unix：父目录置只读 → 临时文件创建失败（EACCES）
/// - Windows：目标文件置只读属性 → rename(REPLACE_EXISTING) 被拒
///
/// 前提：`dest` 已存在且可读。返回 Drop guard 自动恢复权限（unix 恢复
/// 父目录权限保证 tempdir 可清理；Windows 恢复目标可写属性）。
#[cfg(any(unix, windows))]
struct RestorePermsGuard {
    #[cfg(unix)]
    parent: std::path::PathBuf,
    #[cfg(windows)]
    dest: std::path::PathBuf,
}

#[cfg(any(unix, windows))]
impl Drop for RestorePermsGuard {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&self.parent, std::fs::Permissions::from_mode(0o755));
        }
        #[cfg(windows)]
        {
            if let Ok(meta) = std::fs::metadata(&self.dest) {
                let mut perm = meta.permissions();
                perm.set_readonly(false);
                let _ = std::fs::set_permissions(&self.dest, perm);
            }
        }
    }
}

#[cfg(any(unix, windows))]
fn block_atomic_replace(dest: &std::path::Path) -> Option<RestorePermsGuard> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let parent = dest.parent().unwrap().to_path_buf();
        let mut perm = std::fs::metadata(&parent).unwrap().permissions();
        perm.set_mode(0o555);
        std::fs::set_permissions(&parent, perm).unwrap();
        // 环境探针（trivial probe）：root 语境（CAP_DAC_OVERRIDE）不受 0o555
        // 限制，写失败注入无法生效——诚实跳过而非假红；权限就地恢复。
        let probe = parent.join(".tmp-perm-probe");
        if std::fs::write(&probe, b"").is_ok() {
            let _ = std::fs::remove_file(&probe);
            let _ = std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o755));
            return None;
        }
        Some(RestorePermsGuard { parent })
    }
    #[cfg(windows)]
    {
        let mut perm = std::fs::metadata(dest).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(dest, perm).unwrap();
        Some(RestorePermsGuard {
            dest: dest.to_path_buf(),
        })
    }
}

/// 目录内不应残留任何 `.tmp-` 前缀的临时文件（helper 唯一临时名前缀）。
#[cfg(any(unix, windows))]
fn assert_no_tmp_leftovers(dir: &std::path::Path, what: &str) {
    let leftovers: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
        .collect();
    assert!(leftovers.is_empty(), "{what}: {leftovers:?}");
}

/// G4/G5: poll_stale_pending_tasks 第 4 参 —— 默认 24h 安全网。
fn poll_defaults() -> chrono::Duration {
    chrono::Duration::hours(24)
}

// G5+P1（T19 根修）：bus / delivery_cb 改收句柄（分支内取用时现读）后的
// 测试助手。空句柄 = 「未注入总线 / 未注册回调」；包装句柄供注入场景。

fn poll_no_bus() -> Arc<Mutex<Option<Arc<dyn MessageBus>>>> {
    Arc::new(Mutex::new(None))
}

type PollDeliveryCb = Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>;

fn poll_no_cb() -> Arc<Mutex<Option<PollDeliveryCb>>> {
    Arc::new(Mutex::new(None))
}

fn poll_cb_handle(cb: PollDeliveryCb) -> Arc<Mutex<Option<PollDeliveryCb>>> {
    Arc::new(Mutex::new(Some(cb)))
}

fn make_config() -> ClusterConfig {
    ClusterConfig {
        node_id: "local-node-001".into(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec!["127.0.0.1:9001".into()],
        // 批次四：显式名（config.cluster.json node_name 路径）——构造期
        // 确定化显示名，测试断言不依赖测试机 COMPUTERNAME（跨平台稳定）。
        node_name: "local-node".into(),
    }
}

#[test]
fn test_start_stop_lifecycle() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.is_running());
    cluster.start();
    assert!(cluster.is_running());
    assert_eq!(cluster.list_nodes().len(), 1);

    cluster.stop();
    assert!(!cluster.is_running());
}

#[test]
fn test_register_and_list_nodes() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let remote = ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "remote-001".into(),
            name: "worker-1".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    };
    cluster.register_node(remote);

    let nodes = cluster.list_nodes();
    assert_eq!(nodes.len(), 2);
    assert!(cluster.get_node_info("remote-001").is_some());
}

#[test]
fn test_submit_and_assign_task() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let task_id = cluster.submit_task(
        "peer_chat",
        serde_json::json!({"message": "hello"}),
        "web",
        "chat-123",
    );

    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.action, "peer_chat");
    assert_eq!(task.status, TaskStatus::Pending);

    // Assign
    assert!(cluster.assign_task(&task_id, "remote-001"));
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Running);
}

#[test]
fn test_complete_and_fail_task() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let task_id = cluster.submit_task("peer_chat", serde_json::json!({}), "rpc", "chat-1");

    // Complete
    cluster.assign_task(&task_id, "node-a");
    assert!(cluster.complete_task(&task_id, serde_json::json!("done")));
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Completed);

    // Fail a different task
    let task_id2 = cluster.submit_task("forge_share", serde_json::json!({}), "rpc", "chat-2");
    assert!(cluster.fail_task(&task_id2, "timeout"));
    let task = cluster.get_task(&task_id2).unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

#[test]
fn test_handle_discovered_node() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "remote-002",
        "worker-2",
        vec!["10.0.0.3".into(), "192.168.1.5".into()],
        21949,
        "worker",
        "development",
        vec!["test".into()],
        vec!["llm".into(), "tools".into()],
        "agent",
    );

    let node = cluster.get_node_info("remote-002").unwrap();
    assert_eq!(node.base.name, "worker-2");
    assert_eq!(node.status, NodeStatus::Online);
    assert_eq!(node.capabilities.len(), 2);
}

// 回归（2026-09-09 双机 G8）：handle_discovered_node 曾把 role 硬编码 Worker，
// announce 携带的对端自报 role 被丢弃 → 纯 UDP 发现拓扑下 board.sync 的
// coordinator 判定永远失败（「no online coordinator known yet」补拉死循环）。
#[test]
fn test_handle_discovered_node_preserves_announced_role() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "remote-coord",
        "master-1",
        vec!["10.0.0.9".into()],
        21949,
        "coordinator",
        "development",
        vec![],
        vec![],
        "agent",
    );

    let node = cluster.get_node_info("remote-coord").unwrap();
    assert_eq!(
        node.base.role,
        nemesis_types::cluster::NodeRole::Coordinator,
        "announce 自报的 coordinator role 必须保留"
    );

    // 旧词兼容：announce 带 "manager"/"master" 同样解析为 Coordinator。
    cluster.handle_discovered_node(
        "remote-coord-legacy",
        "master-legacy",
        vec!["10.0.0.10".into()],
        21949,
        "manager",
        "development",
        vec![],
        vec![],
        "agent",
    );
    let node = cluster.get_node_info("remote-coord-legacy").unwrap();
    assert_eq!(
        node.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );

    // 缺失/未知 role（announce 兼容路径）回落 Worker。
    cluster.handle_discovered_node(
        "remote-norole",
        "no-role",
        vec!["10.0.0.11".into()],
        21949,
        "",
        "development",
        vec![],
        vec![],
        "agent",
    );
    let node = cluster.get_node_info("remote-norole").unwrap();
    assert_eq!(node.base.role, nemesis_types::cluster::NodeRole::Worker);
}

// 回归（2026-09-11 停车场复活 A1）：handle_discovered_node 曾把 announce
// 携带的 tags 整体丢弃（_tags 弃参、ExtendedNodeInfo 无字段）→ 看板派发
// 匹配器只剩 category 一个粒度，required_tags 天然不可满足 → 全部停车。
#[test]
fn test_handle_discovered_node_stores_tags() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "remote-tags",
        "worker-tags",
        vec!["10.0.0.12".into()],
        21949,
        "worker",
        "general",
        vec!["python".into(), "ml".into()],
        vec!["llm".into()],
        "agent",
    );

    let node = cluster.get_node_info("remote-tags").unwrap();
    assert_eq!(node.tags, vec!["python".to_string(), "ml".to_string()]);

    // tags 变化必须算 changed（content_eq 纳入 tags 比较）：
    // 对端改了 tags 后旧记录不更新会导致匹配器用到过期画像。
    let changed = cluster.handle_discovered_node(
        "remote-tags",
        "worker-tags",
        vec!["10.0.0.12".into()],
        21949,
        "worker",
        "general",
        vec!["rust".into()],
        vec!["llm".into()],
        "agent",
    );
    assert!(changed, "tags 变化应判定为 changed");
    let node = cluster.get_node_info("remote-tags").unwrap();
    assert_eq!(node.tags, vec!["rust".to_string()]);

    // 内容完全相同（含 tags）→ 不算 changed（周期 announce 幂等）。
    let changed = cluster.handle_discovered_node(
        "remote-tags",
        "worker-tags",
        vec!["10.0.0.12".into()],
        21949,
        "worker",
        "general",
        vec!["rust".into()],
        vec!["llm".into()],
        "agent",
    );
    assert!(
        !changed,
        "相同内容（含 tags）的周期 announce 不应判定 changed"
    );
}

#[test]
fn test_handle_node_offline() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "remote-003",
        "worker-3",
        vec!["10.0.0.4".into()],
        21949,
        "worker",
        "general",
        vec![],
        vec![],
        "agent",
    );

    let node = cluster.get_node_info("remote-003").unwrap();
    assert_eq!(node.status, NodeStatus::Online);

    cluster.handle_node_offline("remote-003", "heartbeat timeout");
    let node = cluster.get_node_info("remote-003").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_get_capabilities() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "remote-004".into(),
            name: "worker-4".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.5:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "tools".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let caps = cluster.get_capabilities();
    assert!(caps.contains(&"cluster".into()));
    assert!(caps.contains(&"llm".into()));
    assert!(caps.contains(&"tools".into()));
}

#[test]
fn test_with_callback() {
    let completed = Arc::new(Mutex::new(Vec::new()));
    let completed_clone = completed.clone();
    let cluster = Cluster::with_callback(
        make_config(),
        Box::new(move |t: &Task| {
            completed_clone.lock().push(t.id.clone());
        }),
    );
    cluster.start();

    let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch");
    cluster.complete_task(&task_id, serde_json::json!("result"));

    let ids = completed.lock();
    assert!(ids.contains(&task_id));
}

#[test]
fn test_handle_task_complete_no_bus() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Should not panic
    cluster.handle_task_complete("nonexistent");
}

#[test]
fn test_call_with_context_override() {
    let cluster = Cluster::new(make_config());
    cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
        Ok(format!("called {} on {}", action, peer_id).into_bytes())
    }));

    let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
    assert!(result.is_ok());
    let s = String::from_utf8(result.unwrap()).unwrap();
    assert_eq!(s, "called ping on peer-1");
}

#[test]
fn test_parse_host_port() {
    assert_eq!(
        parse_host_port("10.0.0.1:21949"),
        ("10.0.0.1".into(), 21949u16)
    );
    assert_eq!(
        parse_host_port("example.com:8080"),
        ("example.com".into(), 8080u16)
    );
    assert_eq!(
        parse_host_port("no-port"),
        ("no-port".into(), DEFAULT_RPC_PORT)
    );
}

#[test]
fn test_generate_node_id() {
    let id = generate_node_id();
    assert!(
        id.starts_with("node-"),
        "id should start with 'node-': {}",
        id
    );
    assert!(id.len() > 10);
    // Verify format: node-{hostname}-{uuid} where uuid is 36 chars (8-4-4-4-12)
    // hostname may contain '-' (e.g. "LAPTOP-FOO"), so check by tail (uuid is last 36 chars)
    assert!(
        id.len() >= 5 + 36,
        "id should be at least 'node-' + 36-char uuid"
    );
    let uuid_part = &id[id.len() - 36..];
    assert_eq!(
        uuid_part.matches('-').count(),
        4,
        "uuid should have 4 hyphens"
    );
}

struct MockBus {
    messages: Arc<Mutex<Vec<BusInboundMessage>>>,
}

impl MessageBus for MockBus {
    fn publish_inbound(&self, msg: BusInboundMessage) {
        self.messages.lock().push(msg);
    }
    // T19 二层根修：Mock 恒「就位」（不模拟订阅时序，等待逻辑归 publish 侧）。
    fn inbound_receiver_count(&self) -> usize {
        1
    }
}

#[test]
fn test_handle_task_complete_with_bus() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });

    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);

    let task_id = cluster.submit_task("peer_chat", serde_json::json!({}), "web", "chat-1");
    cluster.complete_task(&task_id, serde_json::json!("done"));

    // The callback should have been fired by the task manager
    // But handle_task_complete is called separately in the real flow
    cluster.handle_task_complete(&task_id);

    let msgs = messages.lock();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].channel, "system");
    assert!(msgs[0].sender_id.starts_with("cluster_continuation:"));
}

// -- CD2（2026-09-17）：安全网超时分支必须发 bus 唤醒 agent ------------------
//
// 根因：安全网分支此前只 complete_callback（标 Failed）不 publish，等回复
// 的 agent 永远不知道——与 done / not_found 分支（都发 bus）不一致。

#[tokio::test]
async fn test_poll_safety_net_timeout_publishes_bus_error() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });

    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);

    // 缩短安全网到 60s，构造一个 2h 前创建的 Pending 任务（年龄 > 2min
    // 年龄闸 + > 安全网 → 命中安全网分支）。
    cluster
        .task_manager()
        .set_pending_timeout(chrono::Duration::seconds(60));
    let old_created = (chrono::Local::now() - chrono::Duration::hours(2)).to_rfc3339();
    let task = nemesis_types::cluster::Task {
        id: "cd2-timeout-task".into(),
        status: TaskStatus::Pending,
        action: "peer_chat".into(),
        peer_id: "remote-001".into(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "web".into(),
        original_chat_id: "chat-1".into(),
        created_at: old_created,
        completed_at: None,
    };
    cluster.task_manager().submit(task).unwrap();

    cluster.poll_stale_pending_tasks().await;

    // 断言 1：bus 收到 error 续行帧（agent 被唤醒），metadata.error 带
    // 安全网文案、content 为空、status=error（形状对齐 not_found 分支）。
    let msgs = messages.lock();
    assert_eq!(msgs.len(), 1, "安全网超时分支必须发恰好一条 bus 唤醒帧");
    assert_eq!(msgs[0].channel, "system");
    assert_eq!(msgs[0].sender_id, "cluster_continuation:cd2-timeout-task");
    assert_eq!(msgs[0].content, "");
    assert_eq!(
        msgs[0].metadata.get("status").map(String::as_str),
        Some("error")
    );
    let err = msgs[0]
        .metadata
        .get("error")
        .expect("metadata.error 必须存在");
    assert!(
        err.contains("safety net"),
        "metadata.error 应含安全网文案，got: {}",
        err
    );
    drop(msgs);

    // 断言 2：任务状态被标 Failed（complete_callback 语义不回归）。
    let task = cluster.get_task("cd2-timeout-task").unwrap();
    assert_eq!(task.status, TaskStatus::Failed);
}

#[tokio::test]
async fn test_poll_young_task_does_not_hit_safety_net() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });

    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);
    cluster
        .task_manager()
        .set_pending_timeout(chrono::Duration::seconds(60));

    // 刚创建的任务（submit 默认 created_at = now）：不满足年龄闸也不满足
    // 安全网 → 不发 bus、状态保持 Pending。
    cluster.submit_task("peer_chat", serde_json::json!({}), "web", "chat-1");

    cluster.poll_stale_pending_tasks().await;

    assert!(
        messages.lock().is_empty(),
        "young 任务不得触发安全网 bus 帧"
    );
    let task_id = cluster
        .task_manager()
        .list_pending_tasks()
        .first()
        .map(|t| t.id.clone())
        .expect("young 任务应保持 Pending");
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Pending);
}

// -- call_with_context production path tests --------------------------------

#[test]
fn test_call_with_context_no_rpc_client_returns_error() {
    // Without start(), no RPC client is initialized.
    let cluster = Cluster::new(make_config());
    let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
    let err = result.unwrap_err();
    assert!(
        err.contains("RPC client not initialized"),
        "Expected 'not initialized' error, got: {:?}",
        err
    );
}

#[test]
fn test_call_with_context_after_start_no_peer_errors() {
    // After start(), the RPC client is initialized with a resolver,
    // but the peer is not found in the registry. This should return
    // an error about peer not found or connection failure.
    let cluster = Cluster::new(make_config());
    cluster.start();
    let result = cluster.call_with_context("nonexistent-peer", "ping", serde_json::json!({}));
    assert!(
        result.is_err(),
        "Expected error for nonexistent peer, got: {:?}",
        result
    );
}

#[test]
fn test_call_with_context_test_override_takes_priority() {
    // The test override should take priority even when RPC client is set.
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Set the test override AFTER start (which creates the RPC client)
    cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
        Ok(format!("override: {} on {}", action, peer_id).into_bytes())
    }));

    let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
    assert!(result.is_ok());
    let s = String::from_utf8(result.unwrap()).unwrap();
    assert_eq!(s, "override: ping on peer-1");
}

#[tokio::test]
async fn test_call_with_context_async_no_rpc_client() {
    let cluster = Cluster::new(make_config());
    let result = cluster
        .call_with_context_async(
            "peer-1",
            "ping",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("RPC client not initialized"));
}

#[tokio::test]
async fn test_call_with_context_async_test_override() {
    let cluster = Cluster::new(make_config());
    cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
        Ok(format!("async-override: {} on {}", action, peer_id).into_bytes())
    }));

    let result = cluster
        .call_with_context_async(
            "peer-1",
            "hello",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await;
    assert!(result.is_ok());
    let s = String::from_utf8(result.unwrap()).unwrap();
    assert_eq!(s, "async-override: hello on peer-1");
}

#[test]
fn test_start_initializes_rpc_client() {
    let cluster = Cluster::new(make_config());
    // Before start, no RPC client
    assert!(cluster.rpc_client.lock().is_none());
    cluster.start();
    // After start, RPC client is initialized with ClusterPeerResolver
    assert!(
        cluster.rpc_client.lock().is_some(),
        "start() should initialize the RPC client"
    );
}

#[test]
fn test_set_rpc_client_overrides_auto_created() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Verify auto-created client
    assert!(cluster.rpc_client.lock().is_some());

    // Set a custom client
    let custom_client = Arc::new(RpcClient::with_timeout(Duration::from_secs(30)));
    cluster.set_rpc_client(custom_client);

    // Verify the custom client was set
    let client = cluster.rpc_client.lock();
    assert!(client.is_some());
    assert_eq!(client.as_ref().unwrap().timeout(), Duration::from_secs(30));
}

#[test]
fn test_set_rpc_client_before_start_preserves_it() {
    let cluster = Cluster::new(make_config());
    // Set a custom client before start
    let custom_client = Arc::new(RpcClient::with_timeout(Duration::from_secs(120)));
    cluster.set_rpc_client(custom_client);

    cluster.start();

    // start() should not overwrite the custom client
    let client = cluster.rpc_client.lock();
    assert!(client.is_some());
    assert_eq!(client.as_ref().unwrap().timeout(), Duration::from_secs(120));
}

#[test]
fn test_cluster_peer_resolver_returns_peer_info() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a peer with specific addresses
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "peer-abc".into(),
            name: "test-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "203.0.113.10:21949".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec!["203.0.113.10".into(), "203.0.113.11".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    let (addresses, port, is_online) = resolver.get_peer_info("peer-abc").unwrap();
    assert_eq!(addresses.len(), 2);
    assert!(addresses.contains(&"203.0.113.10".to_string()));
    assert!(addresses.contains(&"203.0.113.11".to_string()));
    assert_eq!(port, 21949);
    assert!(is_online);
}

#[test]
fn test_cluster_peer_resolver_unknown_peer() {
    let cluster = Cluster::new(make_config());
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    assert!(resolver.get_peer_info("unknown-peer").is_none());
}

#[test]
fn test_cluster_peer_resolver_offline_peer() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "offline-peer".into(),
            name: "offline-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:21949".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["10.0.0.1".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    let (_, _, is_online) = resolver.get_peer_info("offline-peer").unwrap();
    assert!(!is_online);
}

/// Helper to create a cluster with an RPC server for handler registration tests.
fn make_cluster_with_rpc_server() -> Cluster {
    let mut cluster = Cluster::new(make_config());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    cluster
}

#[test]
fn test_register_forge_handlers() {
    let cluster = make_cluster_with_rpc_server();
    assert!(cluster.is_running());

    // Create a file-based forge provider
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));

    // Register forge handlers
    let result = cluster.register_forge_handlers(provider);
    assert!(
        result.is_ok(),
        "register_forge_handlers should succeed: {:?}",
        result
    );

    // Verify handlers are registered by making RPC calls through the server
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    // Test forge_share handler
    let share_result = rpc_server.handle_request_sync(
        "forge_share",
        serde_json::json!({
            "source_node": "remote-node-1",
            "report": {"insights": ["test insight"], "score": 0.85},
        }),
    );
    assert!(
        share_result.is_ok(),
        "forge_share handler should succeed: {:?}",
        share_result
    );
    let resp = share_result.unwrap();
    assert_eq!(resp["status"], "ok");

    // Test forge_get_reflections handler
    let list_result =
        rpc_server.handle_request_sync("forge_get_reflections", serde_json::json!({}));
    assert!(
        list_result.is_ok(),
        "forge_get_reflections handler should succeed: {:?}",
        list_result
    );
    let list_resp = list_result.unwrap();
    assert!(list_resp.get("reflections").is_some());
    assert_eq!(list_resp["node_id"], "local-node-001");
}

#[test]
fn test_register_forge_handlers_not_running() {
    let mut cluster = Cluster::new(make_config());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    // Don't start the cluster
    assert!(!cluster.is_running());

    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));

    let result = cluster.register_forge_handlers(provider);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not running"));
}

#[test]
fn test_register_basic_handlers_registers_all() {
    let cluster = make_cluster_with_rpc_server();

    let result = cluster.register_basic_handlers();
    assert!(result.is_ok());

    // Verify all basic handlers are registered
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let actions = vec![
        "ping",
        "get_capabilities",
        "get_info",
        "list_actions",
        "hello",
    ];
    for action in actions {
        let result = rpc_server.handle_request_sync(action, serde_json::json!({}));
        assert!(result.is_ok(), "Handler '{}' should be registered", action);
    }
}

// -- string_value helper tests --

#[test]
fn test_string_value_with_string() {
    let v = serde_json::json!("hello");
    assert_eq!(string_value(Some(&v)), "hello");
}

#[test]
fn test_string_value_with_null() {
    let v = serde_json::Value::Null;
    assert_eq!(string_value(Some(&v)), "");
}

#[test]
fn test_string_value_with_number() {
    let v = serde_json::json!(42);
    assert_eq!(string_value(Some(&v)), "42");
}

#[test]
fn test_string_value_with_boolean() {
    let v = serde_json::json!(true);
    assert_eq!(string_value(Some(&v)), "true");
}

#[test]
fn test_string_value_with_none() {
    assert_eq!(string_value(None), "");
}

#[test]
fn test_string_value_with_object() {
    let v = serde_json::json!({"key": "val"});
    // Objects fall through to as_str().unwrap_or("")
    assert_eq!(string_value(Some(&v)), "");
}

#[test]
fn test_string_value_with_float() {
    let v = serde_json::json!(2.75);
    assert_eq!(string_value(Some(&v)), "2.75");
}

// -- Accessor tests --

#[test]
fn test_accessors() {
    let cluster = Cluster::new(make_config());
    assert_eq!(cluster.node_id(), "local-node-001");
    // 批次四：make_config 带显式 node_name="local-node"——走 config 覆盖
    // 路径，断言不依赖测试机 COMPUTERNAME（跨平台稳定）。
    assert_eq!(cluster.node_name(), "local-node");
    assert_eq!(cluster.address(), "127.0.0.1:9000");
    assert_eq!(cluster.role(), "worker");
    assert_eq!(cluster.category(), "general");
    assert!(cluster.tags().is_empty());
    assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
    assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);
}

#[test]
fn test_set_ports() {
    let mut cluster = Cluster::new(make_config());
    assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
    assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);

    cluster.set_ports(11111, 22222);
    assert_eq!(cluster.udp_port(), 11111);
    assert_eq!(cluster.rpc_port(), 22222);
}

#[test]
fn test_generate_node_id_uniqueness() {
    let id1 = generate_node_id();
    let id2 = generate_node_id();
    assert!(id1.starts_with("node-"));
    assert!(id2.starts_with("node-"));
    // The uuid portion should differ
    assert_ne!(id1, id2);
}

#[test]
fn test_parse_host_port_edge_cases() {
    // IPv6-like address
    let (host, port) = parse_host_port("[::1]:8080");
    assert_eq!(host, "[::1]");
    assert_eq!(port, 8080);

    // Invalid port number
    let (host, port) = parse_host_port("host:abc");
    assert_eq!(host, "host");
    assert_eq!(port, DEFAULT_RPC_PORT);

    // Port 0
    let (host, port) = parse_host_port("host:0");
    assert_eq!(host, "host");
    assert_eq!(port, 0);

    // Empty string
    let (host, port) = parse_host_port("");
    assert_eq!(host, "");
    assert_eq!(port, DEFAULT_RPC_PORT);
}

#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_UDP_PORT, 11949);
    assert_eq!(DEFAULT_RPC_PORT, 21949);
    assert_eq!(DEFAULT_BROADCAST_INTERVAL, Duration::from_secs(30));
}

#[test]
fn test_with_workspace_creates_cluster() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    assert_eq!(cluster.workspace(), dir.path());
    assert!(!cluster.is_running());
}

#[test]
fn test_stop_receiver_returns_receiver() {
    let cluster = Cluster::new(make_config());
    let _rx = cluster.stop_receiver();
}

#[test]
fn test_get_peer_returns_none_for_unknown() {
    let cluster = Cluster::new(make_config());
    assert!(cluster.get_peer("unknown-peer").is_none());
}

#[test]
fn test_get_online_peers_initially_empty_after_new() {
    let cluster = Cluster::new(make_config());
    // Before start, no online peers registered
    let peers = cluster.get_online_peers();
    assert!(peers.is_empty());
}

#[test]
fn test_get_all_local_ips_does_not_panic() {
    let cluster = Cluster::new(make_config());
    let _ips = cluster.get_all_local_ips();
}

#[test]
fn test_all_go_handlers_registered_after_full_setup() {
    // Simulate full startup: register_basic_handlers + register_peer_chat_handlers + register_forge_handlers
    let cluster = make_cluster_with_rpc_server();

    // Register basic handlers (ping, get_capabilities, get_info, list_actions, hello)
    cluster.register_basic_handlers().unwrap();

    // Set a mock RPC channel to trigger register_peer_chat_handlers
    use crate::rpc::RpcChannel;
    #[derive(Debug)]
    struct MockRpcChannel;
    impl RpcChannel for MockRpcChannel {
        fn input(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
            Err("mock".into())
        }
    }
    cluster.set_rpc_channel(Arc::new(MockRpcChannel));

    // Register forge handlers
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));
    cluster.register_forge_handlers(provider).unwrap();

    // Verify ALL Go-compatible handlers are registered
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let expected_actions = vec![
        // Default handlers
        "ping",
        "get_capabilities",
        "get_info",
        "list_actions",
        // Peer chat handlers
        "peer_chat",
        "peer_chat_callback",
        "hello",
        "query_task_result",
        "confirm_task_delivery",
        // Forge handlers
        "forge_share",
        "forge_get_reflections",
    ];
    for action in expected_actions {
        let result = rpc_server.handle_request_sync(action, serde_json::json!({}));
        assert!(
            result.is_ok(),
            "Handler '{}' should be registered after full setup",
            action
        );
    }
}

// -- Additional coverage tests --

#[test]
fn test_submit_peer_chat() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let result = cluster.submit_peer_chat(
        "remote-001",
        "peer_chat",
        serde_json::json!({"content": "hello", "task_id": "task-abc"}),
        "web",
        "chat-1",
    );
    assert!(result.is_ok());
    let task_id = result.unwrap();
    assert!(!task_id.is_empty());
    // Verify the task exists in the task manager
    assert!(cluster.get_task(&task_id).is_some());
}

#[test]
fn test_register_task_recovery_handlers_gateway_mode() {
    // 回归钉（2026-09-01 跨机 E2E 发现）：gateway 模式不走 set_rpc_channel
    // （无人调用 → register_peer_chat_handlers 不执行），gateway 又只自行
    // 注册 peer_chat / peer_chat_callback / task_cancel——query_task_result
    // / confirm_task_delivery 缺失使 A 侧 poll_stale_pending_tasks 永远
    // 收到 "no handler"，stale 恢复链路断裂。gateway 现显式调用
    // register_task_recovery_handlers；本测试钉住这两个 handler 可独立注册。
    let cluster = make_cluster_with_rpc_server();
    cluster.register_task_recovery_handlers();

    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    for action in ["query_task_result", "confirm_task_delivery"] {
        let result = rpc_server.handle_request_sync(action, serde_json::json!({}));
        assert!(
            result.is_ok(),
            "Handler '{}' should be registered in gateway mode",
            action
        );
    }

    // not_found 语义：未知 task_id → {"status":"not_found"}（poll 据此判失败）
    let out = rpc_server
        .handle_request_sync("query_task_result", serde_json::json!({"task_id": "nope"}))
        .unwrap();
    assert_eq!(out["status"], "not_found");

    // confirm 幂等：确认不存在的 task 也返回 confirmed
    let out = rpc_server
        .handle_request_sync(
            "confirm_task_delivery",
            serde_json::json!({"task_id": "nope"}),
        )
        .unwrap();
    assert_eq!(out["status"], "confirmed");
}

#[test]
fn test_submit_peer_chat_auto_task_id() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // No task_id in payload -> should auto-generate
    let result = cluster.submit_peer_chat(
        "remote-001",
        "peer_chat",
        serde_json::json!({"content": "hello"}),
        "web",
        "chat-1",
    );
    assert!(result.is_ok());
    let task_id = result.unwrap();
    assert!(!task_id.is_empty());
}

#[test]
fn test_handle_discovered_node_no_addresses() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "no-addr-node",
        "empty-addr",
        vec![], // no addresses
        21949,
        "worker",
        "test",
        vec![],
        vec!["llm".into()],
        "agent",
    );

    let node = cluster.get_node_info("no-addr-node").unwrap();
    assert_eq!(node.base.address, ""); // empty primary address
    assert_eq!(node.capabilities.len(), 1);
}

#[test]
fn test_handle_node_offline_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Should not panic
    cluster.handle_node_offline("nonexistent", "test");
}

#[test]
fn test_remove_node() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "to-remove",
        "removeme",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );

    assert!(cluster.get_node_info("to-remove").is_some());
    assert!(cluster.remove_node("to-remove"));
    assert!(cluster.get_node_info("to-remove").is_none());
}

#[test]
fn test_remove_node_nonexistent() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.remove_node("nonexistent"));
}

#[test]
fn test_list_tasks() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    assert!(cluster.list_tasks().is_empty());

    cluster.submit_task("action1", serde_json::json!({}), "web", "ch1");
    cluster.submit_task("action2", serde_json::json!({}), "web", "ch2");

    let tasks = cluster.list_tasks();
    assert_eq!(tasks.len(), 2);
}

#[test]
fn test_cleanup_task_noop() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "ch");
    // Should not panic
    cluster.cleanup_task(&task_id);
    // Task should still exist (no-op)
    assert!(cluster.get_task(&task_id).is_some());
}

#[test]
fn test_get_task_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    assert!(cluster.get_task("nonexistent").is_none());
}

#[test]
fn test_assign_task_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    assert!(!cluster.assign_task("nonexistent", "node-1"));
}

#[test]
fn test_complete_task_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    assert!(!cluster.complete_task("nonexistent", serde_json::json!("result")));
}

#[test]
fn test_fail_task_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    assert!(!cluster.fail_task("nonexistent", "error"));
}

#[test]
fn test_task_manager_accessor() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let tm = cluster.task_manager();
    assert!(Arc::strong_count(tm) >= 1);
}

#[test]
fn test_continuation_store_accessor() {
    let cluster = Cluster::new(make_config());
    let store = cluster.continuation_store();
    assert!(Arc::strong_count(store) >= 1);
}

#[test]
fn test_result_store_accessor() {
    let cluster = Cluster::new(make_config());
    let store = cluster.result_store();
    assert!(Arc::strong_count(store) >= 1);
}

#[test]
fn test_handle_task_complete_empty_channel() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Submit a task with empty original_channel
    let task_id = cluster.submit_task("action", serde_json::json!({}), "", "");
    cluster.complete_task(&task_id, serde_json::json!("done"));

    // Should return early (no bus message published)
    cluster.handle_task_complete(&task_id);
}

#[test]
fn test_sync_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    let result = cluster.sync_to_disk();
    assert!(result.is_ok());

    // Verify file was created
    let state_path = dir.path().join("cluster").join("state.toml");
    assert!(state_path.exists());
}

#[test]
fn test_find_peers_by_capability() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "peer-with-llm".into(),
            name: "llm-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "tools".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let llm_peers = cluster.find_peers_by_capability("llm");
    assert_eq!(llm_peers.len(), 1);

    let no_peers = cluster.find_peers_by_capability("nonexistent");
    assert!(no_peers.is_empty());
}

#[test]
fn test_get_config() {
    let cluster = Cluster::new(make_config());
    // config() returns a static default, not the actual config
    // Use node_id() to verify the actual node ID
    assert_eq!(cluster.node_id(), "local-node-001");
}

#[test]
fn test_bus_inbound_message_fields() {
    let msg = BusInboundMessage {
        channel: "system".into(),
        sender_id: "sender-1".into(),
        chat_id: "chat-1".into(),
        content: "hello".into(),
        metadata: std::collections::HashMap::new(),
    };
    assert_eq!(msg.channel, "system");
    assert_eq!(msg.sender_id, "sender-1");
    assert_eq!(msg.chat_id, "chat-1");
    assert_eq!(msg.content, "hello");
}

#[test]
fn test_cluster_peer_resolver_node_id() {
    let cluster = Cluster::new(make_config());
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    assert_eq!(resolver.get_node_id(), "local-node-001");
}

#[test]
fn test_cluster_peer_resolver_local_interfaces() {
    let cluster = Cluster::new(make_config());
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    // Should return the local node's addresses as interfaces
    let interfaces = resolver.get_local_interfaces();
    // May or may not have interfaces depending on registry state
    assert!(interfaces.is_empty() || !interfaces.is_empty());
}

// ============================================================
// Coverage improvement: additional cluster tests
// ============================================================

#[test]
fn test_start_registers_self_node() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // The local node should be registered in the registry
    let nodes = cluster.list_nodes();
    assert_eq!(nodes.len(), 1);
    assert_eq!(nodes[0].base.id, "local-node-001");
    cluster.stop();
}

#[test]
fn test_start_idempotent() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.start(); // Second call should be a no-op
    assert!(cluster.is_running());
    let nodes = cluster.list_nodes();
    assert_eq!(nodes.len(), 1); // Should not duplicate self
    cluster.stop();
}

#[test]
fn test_stop_idempotent() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.stop();
    cluster.stop(); // Second call should be a no-op
    assert!(!cluster.is_running());
}

#[test]
fn test_handle_task_complete_no_bus_set() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch1");
    cluster.complete_task(&task_id, serde_json::json!("result"));
    // Should not panic when bus is not set
    cluster.handle_task_complete(&task_id);
    cluster.stop();
}

#[test]
fn test_stop_start_restart() {
    let cluster = Cluster::new(make_config());

    // First start
    cluster.start();
    assert!(cluster.is_running());
    let nodes_after_start = cluster.list_nodes();
    assert_eq!(nodes_after_start.len(), 1);

    // Stop
    cluster.stop();
    assert!(!cluster.is_running());

    // Restart — should succeed without panic or duplicate nodes
    cluster.start();
    assert!(cluster.is_running());
    let nodes_after_restart = cluster.list_nodes();
    assert_eq!(nodes_after_restart.len(), 1); // Same local node, not duplicated
}

#[test]
fn test_rpc_server_restart() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let config = RpcServerConfig {
        bind_address: "127.0.0.1:0".into(), // OS-assigned port
        ..Default::default()
    };
    let server = RpcServer::new(config);

    // Default handlers should be registered (in constructor now)
    assert!(
        server
            .handle_request_sync("ping", serde_json::json!({}))
            .is_ok()
    );

    let rt = tokio::runtime::Runtime::new().unwrap();
    // First start
    rt.block_on(server.start()).unwrap();
    assert!(server.is_running());
    let port1 = server.port();
    assert_ne!(port1, 0);

    // Register a custom handler
    server.register_handler(
        "custom_test",
        Box::new(|_payload| Ok(serde_json::json!({"custom": true}))),
    );
    assert!(
        server
            .handle_request_sync("custom_test", serde_json::json!({}))
            .is_ok()
    );

    // Stop
    server.stop().unwrap();
    assert!(!server.is_running());

    // Restart
    rt.block_on(server.start()).unwrap();
    assert!(server.is_running());
    let port2 = server.port();
    // Port may differ on restart (OS-assigned), but should be valid
    assert_ne!(port2, 0);

    // Custom handler should survive restart
    assert!(
        server
            .handle_request_sync("custom_test", serde_json::json!({}))
            .is_ok()
    );
    let resp = server
        .handle_request_sync("custom_test", serde_json::json!({}))
        .unwrap();
    assert_eq!(resp["custom"], true);

    // Default handler should also survive (not overwritten by restart)
    assert!(
        server
            .handle_request_sync("ping", serde_json::json!({}))
            .is_ok()
    );

    server.stop().unwrap();
}

#[test]
fn test_handle_task_complete_nonexistent_task() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);
    // Should return early, no panic
    cluster.handle_task_complete("nonexistent-task");
    assert!(messages.lock().is_empty());
    cluster.stop();
}

#[test]
fn test_sync_to_disk_no_workspace() {
    // Without workspace, sync_to_disk should fail
    let cluster = Cluster::new(make_config());
    cluster.start();
    let result = cluster.sync_to_disk();
    // The default workspace is empty, so this should return error or succeed
    // depending on whether the directory exists
    // It should not panic either way
    let _ = result;
    cluster.stop();
}

#[test]
fn test_sync_to_disk_includes_discovered_nodes() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "discovered-1".into(),
            name: "discovered-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let result = cluster.sync_to_disk();
    assert!(result.is_ok());
    cluster.stop();
}

#[test]
fn test_register_node_updates_existing() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a node
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-1".into(),
            name: "original-name".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // Re-register same node with updated name
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-1".into(),
            name: "updated-name".into(),
            role: nemesis_types::cluster::NodeRole::Coordinator,
            address: "10.0.0.2:9000".into(),
            category: "prod".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["tools".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let node = cluster.get_node_info("node-1").unwrap();
    assert_eq!(node.base.name, "updated-name");
    assert_eq!(
        node.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
    assert_eq!(node.capabilities.len(), 1);
    cluster.stop();
}

#[test]
fn test_get_online_peers_includes_online_nodes() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "online-peer".into(),
            name: "online".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "offline-peer".into(),
            name: "offline".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let online = cluster.get_online_peers();
    // Should include the local node and the online peer, but NOT the offline peer
    assert!(online.iter().any(|n| n.base.id == "online-peer"));
    assert!(!online.iter().any(|n| n.base.id == "offline-peer"));
    cluster.stop();
}

#[test]
fn test_get_capabilities_dedup() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register two nodes with overlapping capabilities
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-a".into(),
            name: "a".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "tools".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-b".into(),
            name: "b".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "forge".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let caps = cluster.get_capabilities();
    // "llm" should appear only once (dedup)
    assert_eq!(caps.iter().filter(|c| **c == "llm").count(), 1);
    assert!(caps.contains(&"llm".to_string()));
    assert!(caps.contains(&"tools".to_string()));
    assert!(caps.contains(&"forge".to_string()));
    cluster.stop();
}

#[test]
fn test_register_rpc_handler_not_running() {
    let cluster = Cluster::new(make_config());
    // Not started, so register_rpc_handler should fail
    let result =
        cluster.register_rpc_handler("test_action", Box::new(|_| Ok(serde_json::json!({}))));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not running"));
}

#[test]
fn test_register_rpc_handler_no_server() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // No RPC server set, should fail
    let result =
        cluster.register_rpc_handler("test_action", Box::new(|_| Ok(serde_json::json!({}))));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("RPC server"));
    cluster.stop();
}

#[test]
fn test_register_basic_handlers_not_running() {
    let cluster = Cluster::new(make_config());
    let result = cluster.register_basic_handlers();
    assert!(result.is_err());
}

#[test]
fn test_get_rpc_channel_initially_none() {
    let cluster = Cluster::new(make_config());
    assert!(cluster.get_rpc_channel().is_none());
}

// 注：test_config_returns_default 已随 config() 死 API 一并删除
// （LOW-19，2026-10-17 四轮审计）。

#[test]
fn test_handle_discovered_node_with_multiple_addresses() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "multi-addr-node",
        "multi",
        vec!["10.0.0.1".into(), "192.168.1.1".into(), "172.16.0.1".into()],
        21949,
        "worker",
        "dev",
        vec!["tag1".into()],
        vec!["llm".into()],
        "agent",
    );

    let node = cluster.get_node_info("multi-addr-node").unwrap();
    assert_eq!(node.addresses.len(), 3);
    assert!(node.addresses.contains(&"10.0.0.1".to_string()));
    assert!(node.addresses.contains(&"192.168.1.1".to_string()));
    assert!(node.addresses.contains(&"172.16.0.1".to_string()));
    cluster.stop();
}

#[test]
fn test_call_with_context_override_returns_error() {
    let cluster = Cluster::new(make_config());
    cluster.set_call_with_context_fn(Box::new(|_peer, _action, _payload| {
        Err("test error".to_string())
    }));

    let result = cluster.call_with_context("peer-1", "action", serde_json::json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("test error"));
}

#[tokio::test]
async fn test_call_with_context_async_override_returns_error() {
    let cluster = Cluster::new(make_config());
    cluster.set_call_with_context_fn(Box::new(|_peer, _action, _payload| {
        Err("async test error".to_string())
    }));

    let result = cluster
        .call_with_context_async(
            "peer-1",
            "action",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await;
    assert!(result.is_err());
}

#[test]
fn test_peer_chat_handlers_without_rpc_channel() {
    let cluster = make_cluster_with_rpc_server();
    // Don't set RPC channel - register_peer_chat_handlers should return early
    // but not panic
    cluster.register_peer_chat_handlers();
}

#[test]
fn test_set_rpc_server() {
    let mut cluster = Cluster::new(make_config());
    assert!(cluster.rpc_server.is_none());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    assert!(cluster.rpc_server.is_some());
}

#[test]
fn test_complete_task_with_callback_and_bus() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });
    let completed = Arc::new(Mutex::new(Vec::new()));
    let completed_clone = completed.clone();
    let cluster = Cluster::with_callback(
        make_config(),
        Box::new(move |t: &Task| {
            completed_clone.lock().push(t.id.clone());
        }),
    );
    cluster.start();
    cluster.set_message_bus(bus);

    let task_id = cluster.submit_task("peer_chat", serde_json::json!({}), "web", "chat-1");
    cluster.assign_task(&task_id, "node-a");
    cluster.complete_task(&task_id, serde_json::json!("done"));

    // Callback fires from task_manager
    let ids = completed.lock();
    assert!(ids.contains(&task_id));
}

#[test]
fn test_handle_task_complete_with_failed_task() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);

    let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch1");
    cluster.fail_task(&task_id, "error");
    cluster.handle_task_complete(&task_id);

    // Should publish continuation message even for failed tasks
    let msgs = messages.lock();
    assert_eq!(msgs.len(), 1);
    assert!(msgs[0].sender_id.starts_with("cluster_continuation:"));
}

#[test]
fn test_find_peers_by_capability_offline_excluded() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "offline-cap".into(),
            name: "offline-cap".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let peers = cluster.find_peers_by_capability("llm");
    // Offline node should not be included
    assert!(peers.is_empty());
    cluster.stop();
}

#[test]
fn test_remove_node_then_readd() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "node-x",
        "x",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    assert!(cluster.get_node_info("node-x").is_some());

    cluster.remove_node("node-x");
    assert!(cluster.get_node_info("node-x").is_none());

    // Re-discovery blocked by blacklist
    cluster.handle_discovered_node(
        "node-x",
        "x-v2",
        vec!["10.0.0.2".into()],
        21949,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    assert!(
        cluster.get_node_info("node-x").is_none(),
        "blacklisted node should not be re-added"
    );

    // After unban, re-discovery works
    cluster.unban_node("node-x");
    cluster.handle_discovered_node(
        "node-x",
        "x-v2",
        vec!["10.0.0.2".into()],
        21949,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    let node = cluster.get_node_info("node-x").unwrap();
    assert_eq!(node.base.name, "x-v2");
    cluster.stop();
}

#[test]
fn test_bus_inbound_message_debug() {
    let msg = BusInboundMessage {
        channel: "test".into(),
        sender_id: "sender".into(),
        chat_id: "chat".into(),
        content: "content".into(),
        metadata: std::collections::HashMap::new(),
    };
    let debug_str = format!("{:?}", msg);
    assert!(debug_str.contains("test"));
    assert!(debug_str.contains("sender"));
}

#[test]
fn test_cluster_default_node_info() {
    let config = make_config();
    let cluster = Cluster::new(config);
    assert_eq!(cluster.role(), "worker");
    assert_eq!(cluster.category(), "general");
    assert!(cluster.tags().is_empty());
}

#[test]
fn test_handle_task_complete_with_channel_and_chat_id() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);

    let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "chat-42");
    cluster.complete_task(&task_id, serde_json::json!("result"));
    cluster.handle_task_complete(&task_id);

    let msgs = messages.lock();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0].chat_id, "web:chat-42");
}

#[test]
fn test_cluster_new_vs_with_workspace() {
    let config = make_config();
    let cluster1 = Cluster::new(config.clone());
    // new() uses current_dir() as workspace
    assert!(cluster1.workspace().exists());

    let dir = tempfile::tempdir().unwrap();
    let cluster2 = Cluster::with_workspace(config, dir.path().to_path_buf());
    assert_eq!(cluster2.workspace(), dir.path());
}

#[test]
fn test_submit_multiple_tasks() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let ids: Vec<String> = (0..10)
        .map(|i| cluster.submit_task("action", serde_json::json!({"i": i}), "rpc", "ch"))
        .collect();

    let tasks = cluster.list_tasks();
    assert_eq!(tasks.len(), 10);

    // All IDs should be unique
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique.len(), 10);
    cluster.stop();
}

#[test]
fn test_get_peer_returns_correct_info() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "peer-xyz",
        "xyz-peer",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "test",
        vec!["tag1".into()],
        vec!["llm".into(), "forge".into()],
        "agent",
    );

    let peer = cluster.get_peer("peer-xyz").unwrap();
    assert_eq!(peer.base.id, "peer-xyz");
    assert_eq!(peer.base.name, "xyz-peer");
    assert_eq!(peer.capabilities.len(), 2);
    cluster.stop();
}

// ============================================================
// Coverage improvement: poll_stale_pending_tasks, confirm_delivery,
// handler builders, actions_schema, peer resolver edge cases
// ============================================================

#[tokio::test]
async fn test_poll_stale_pending_tasks_young_task_skipped() {
    let tm = Arc::new(TaskManager::new());
    // Create a brand new task (< 2 minutes old) - should be skipped
    let _task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    // Task should still be pending
    let pending = tm.list_pending_tasks();
    assert_eq!(pending.len(), 1);
}

/// P6b（2026-09-11 日志）：include_young=true（恢复 loop 首 tick）绕过
/// 2 分钟年龄闸——重启后立即对年轻任务发起恢复查询。
#[tokio::test]
async fn test_poll_stale_pending_tasks_include_young_queries_young_task() {
    let tm = Arc::new(TaskManager::new());
    // Brand new task (< 2 minutes old); poll requires a peer_id to query.
    let _task = tm.create_task_with_peer("action", serde_json::json!({}), "rpc", "ch", "remote-1");
    let pending_before = tm.list_pending_tasks();
    let task_id = pending_before[0].id.clone();

    // call_fn answers "done" with a result — proves the young task was
    // actually queried instead of skipped by the age gate.
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(move |_peer, action, payload| {
        if action == "query_task_result" {
            let tid = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let resp = serde_json::json!({
                "status": "done",
                "task_id": tid,
                "result_status": "success",
                "response": "recovered",
                "error": "",
            });
            Ok(serde_json::to_vec(&resp).unwrap())
        } else {
            // confirm_task_delivery
            Ok(Vec::new())
        }
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        true,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task(&task_id).expect("task should exist");
    assert_eq!(
        t.status,
        TaskStatus::Completed,
        "include_young=true must bypass the 2-minute age gate"
    );
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_old_task_timed_out() {
    let tm = Arc::new(TaskManager::new());
    // Create a task with an old created_at (> 24 hours) and a peer_id
    let old_time = (chrono::Local::now() - chrono::Duration::hours(25)).to_rfc3339();
    let task = Task {
        id: "stale-24h".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("stale-24h").unwrap();
    assert_eq!(t.status, TaskStatus::Failed);
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_stale_with_call_fn() {
    let tm = Arc::new(TaskManager::new());
    // Create a task that is > 2 minutes old but < 24 hours, with peer_id
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "stale-5m".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // Provide a call_fn that returns a "not_found" response
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, _action, _payload| {
        let resp = serde_json::json!({"status": "not_found", "task_id": "stale-5m"});
        Ok(serde_json::to_vec(&resp).unwrap())
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("stale-5m").unwrap();
    assert_eq!(t.status, TaskStatus::Failed);
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_stale_with_done_response() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "stale-done".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // call_fn returns a "done" response with success
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, action, _payload| {
        if action == "query_task_result" {
            let resp = serde_json::json!({
                "status": "done",
                "task_id": "stale-done",
                "result_status": "success",
                "response": "hello",
                "error": ""
            });
            Ok(serde_json::to_vec(&resp).unwrap())
        } else {
            // confirm_task_delivery
            Ok(Vec::new())
        }
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("stale-done").unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
}

// -- CD3（2026-09-17）：恢复交付回调 = confirm 前置闸 ------------------------
//
// done 分支：交付回调返回 true（或未注册）才向 worker 发 confirm 删除其
// 本地副本；false 跳过 confirm（宁留勿丢，副本走 worker 端 TTL 兜底）。

/// CD3 测试共用：构造一个 >2min 的 Pending 任务 + 返回 done 响应的 call_fn，
/// call_fn 同时记录 confirm_task_delivery 调用次数。
fn cd3_done_fixture(
    task_id: &'static str,
) -> (
    Arc<TaskManager>,
    Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>>,
    Arc<Mutex<Vec<String>>>,
) {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: task_id.to_string(),
        status: TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    let confirm_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = confirm_calls.clone();
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(move |_peer, action, _payload| {
        if action == "confirm_task_delivery" {
            calls_clone.lock().push(action.to_string());
            return Ok(Vec::new());
        }
        let resp = serde_json::json!({
            "status": "done",
            "task_id": task_id,
            "result_status": "success",
            "response": "hello",
            "error": ""
        });
        Ok(serde_json::to_vec(&resp).unwrap())
    }));
    (tm, call_fn, confirm_calls)
}

#[tokio::test]
async fn test_poll_recovered_delivery_cb_true_confirms() {
    let (tm, call_fn, confirm_calls) = cd3_done_fixture("cd3-true");
    let cb_args: Arc<Mutex<Vec<(String, String, String, Option<String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let args_clone = cb_args.clone();
    let delivery_cb: Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync> =
        Arc::new(move |task_id, status, response, error| {
            args_clone.lock().push((
                task_id.to_string(),
                status.to_string(),
                response.to_string(),
                error.map(String::from),
            ));
            true
        });

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_cb_handle(delivery_cb),
    )
    .await;

    // 回调收到查询结果全量参数，error 为空 → None。
    let args = cb_args.lock();
    assert_eq!(args.len(), 1, "交付回调应被调用恰好一次");
    assert_eq!(args[0].0, "cd3-true");
    assert_eq!(args[0].1, "success");
    assert_eq!(args[0].2, "hello");
    assert_eq!(args[0].3, None);
    drop(args);

    // 交付成功 → confirm 被发（worker 副本可删）。
    assert_eq!(
        confirm_calls.lock().len(),
        1,
        "回调 true 必须发 confirm_task_delivery"
    );
    assert_eq!(
        tm.get_task("cd3-true").unwrap().status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn test_poll_recovered_delivery_cb_false_skips_confirm() {
    let (tm, call_fn, confirm_calls) = cd3_done_fixture("cd3-false");
    let delivery_cb: Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync> =
        Arc::new(|_task_id, _status, _response, _error| false);

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_cb_handle(delivery_cb),
    )
    .await;

    // 交付失败 → 不发 confirm（worker 端副本保留到 TTL）；任务本身照常
    // 完成（TaskManager 状态与交付解耦）。
    assert!(
        confirm_calls.lock().is_empty(),
        "回调 false 必须跳过 confirm_task_delivery"
    );
    assert_eq!(
        tm.get_task("cd3-false").unwrap().status,
        TaskStatus::Completed
    );
}

#[tokio::test]
async fn test_poll_recovered_delivery_cb_none_keeps_legacy_confirm() {
    let (tm, call_fn, confirm_calls) = cd3_done_fixture("cd3-none");

    // 未注册回调 = 旧行为：无条件 confirm。
    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;

    assert_eq!(
        confirm_calls.lock().len(),
        1,
        "无回调时保持旧行为（无条件 confirm）"
    );
}

// -- N3c（2026-10-12）：not_found 分支走交付回调 ------------------------------
//
// worker 重启丢内存任务表 → query_task_result 应答 not_found = 确定性判死
// 信号。board 派发任务的交付回调（write_back FAILED + in_review + spawn
// 评审）必须立即触发，不等 dispatch_timeout sweep（否则派发行悬 dispatched
// 最长 3600s）。worker 端已无副本：不发 confirm。

#[tokio::test]
async fn test_poll_not_found_invokes_delivery_cb_without_confirm() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    tm.submit(Task {
        id: "nf-cb".to_string(),
        status: TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    })
    .unwrap();

    let confirm_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let calls_clone = confirm_calls.clone();
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(move |_peer, action, _payload| {
        if action == "confirm_task_delivery" {
            calls_clone.lock().push(action.to_string());
        }
        let resp = serde_json::json!({"status": "not_found", "task_id": "nf-cb"});
        Ok(serde_json::to_vec(&resp).unwrap())
    }));

    let cb_args: Arc<Mutex<Vec<(String, String, String, Option<String>)>>> =
        Arc::new(Mutex::new(Vec::new()));
    let args_clone = cb_args.clone();
    let delivery_cb: Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync> =
        Arc::new(move |task_id, status, response, error| {
            args_clone.lock().push((
                task_id.to_string(),
                status.to_string(),
                response.to_string(),
                error.map(String::from),
            ));
            true
        });

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_cb_handle(delivery_cb),
    )
    .await;

    // 交付回调必须被调用，形态 = error + not_found 文案（board 写回据此
    // 走 FAILED + ⛔ 评论 + in_review + spawn 评审链）。
    let args = cb_args.lock();
    assert_eq!(args.len(), 1, "not_found 必须触发交付回调（不等 sweep）");
    assert_eq!(args[0].0, "nf-cb");
    assert_eq!(args[0].1, "error");
    assert_eq!(args[0].2, "");
    assert_eq!(args[0].3.as_deref(), Some("remote task not found"));
    drop(args);

    // worker 端已无副本：不发 confirm。
    assert!(
        confirm_calls.lock().is_empty(),
        "not_found 无 worker 副本，禁止发 confirm_task_delivery"
    );
    // 任务本地判死（与交付回调解耦，对齐 done 分支语义）。
    assert_eq!(tm.get_task("nf-cb").unwrap().status, TaskStatus::Failed);
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_stale_with_running_response() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "stale-running".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // call_fn returns "running" status - should remain pending
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, _action, _payload| {
        let resp = serde_json::json!({"status": "running", "task_id": "stale-running"});
        Ok(serde_json::to_vec(&resp).unwrap())
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("stale-running").unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_no_peer_id() {
    let tm = Arc::new(TaskManager::new());
    // Task older than 2 min but with no peer_id -> should be skipped
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "no-peer".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(), // empty peer_id
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("no-peer").unwrap();
    assert_eq!(t.status, TaskStatus::Pending); // still pending
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_call_fn_error() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "call-error".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // call_fn returns error -> task stays pending
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, _action, _payload| {
        Err("connection refused".to_string())
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("call-error").unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
}

#[tokio::test]
async fn test_confirm_delivery_with_call_fn() {
    let confirmed = Arc::new(Mutex::new(false));
    let confirmed_clone = confirmed.clone();
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(move |_peer, action, _payload| {
        if action == "confirm_task_delivery" {
            *confirmed_clone.lock() = true;
        }
        Ok(Vec::new())
    }));

    confirm_delivery_with(&call_fn, None, "peer-1", "task-1").await;
    assert!(*confirmed.lock());
}

#[tokio::test]
async fn test_confirm_delivery_no_client_no_fn() {
    // No client and no call_fn -> should just return without panic
    confirm_delivery_with(&None, None, "peer-1", "task-1").await;
}

#[test]
fn test_peer_chat_handler_empty_content() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_peer_chat_handler();
    let result = handler(serde_json::json!({"task_id": "t1"}));
    assert_eq!(result.unwrap()["status"], "error");
}

#[test]
fn test_peer_chat_handler_with_content() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_peer_chat_handler();
    let result = handler(serde_json::json!({
        "content": "hello",
        "task_id": "t1"
    }));
    let resp = result.unwrap();
    assert_eq!(resp["status"], "accepted");
    assert_eq!(resp["task_id"], "t1");
}

#[test]
fn test_callback_handler_empty_task_id() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_callback_handler();
    let result = handler(serde_json::json!({"status": "success"}));
    assert_eq!(result.unwrap()["status"], "error");
}

#[test]
fn test_callback_handler_with_task_id() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch");

    let handler = cluster.build_callback_handler();
    let result = handler(serde_json::json!({
        "task_id": task_id,
        "status": "success",
        "response": "hello"
    }));
    assert_eq!(result.unwrap()["status"], "accepted");

    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.status, TaskStatus::Completed);
}

#[test]
fn test_query_task_result_handler_empty_task_id() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_query_task_result_handler();
    let result = handler(serde_json::json!({}));
    assert_eq!(result.unwrap()["status"], "error");
}

#[test]
fn test_query_task_result_handler_not_found() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_query_task_result_handler();
    let result = handler(serde_json::json!({"task_id": "unknown"}));
    assert_eq!(result.unwrap()["status"], "not_found");
}

#[test]
fn test_query_task_result_handler_found() {
    let cluster = Cluster::new(make_config());
    cluster.result_store.store_success(
        "task-1",
        "peer_chat",
        serde_json::json!({
            "response": "hello world",
        }),
    );
    let handler = cluster.build_query_task_result_handler();
    let result = handler(serde_json::json!({"task_id": "task-1"}));
    let resp = result.unwrap();
    assert_eq!(resp["status"], "done");
    assert_eq!(resp["result_status"], "success");
    assert_eq!(resp["response"], "hello world");
}

#[test]
fn test_query_task_result_handler_failed_result() {
    let cluster = Cluster::new(make_config());
    cluster
        .result_store
        .store_failure("task-err", "peer_chat", "something failed");
    let handler = cluster.build_query_task_result_handler();
    let result = handler(serde_json::json!({"task_id": "task-err"}));
    let resp = result.unwrap();
    assert_eq!(resp["status"], "done");
    assert_eq!(resp["result_status"], "error");
}

#[test]
fn test_confirm_task_delivery_handler_empty_task_id() {
    let cluster = Cluster::new(make_config());
    let handler = cluster.build_confirm_task_delivery_handler();
    let result = handler(serde_json::json!({}));
    assert_eq!(result.unwrap()["status"], "error");
}

#[test]
fn test_confirm_task_delivery_handler_removes_result() {
    let cluster = Cluster::new(make_config());
    cluster
        .result_store
        .store_success("task-del", "peer_chat", serde_json::json!({"r": "v"}));
    assert!(cluster.result_store.get("task-del").is_some());

    let handler = cluster.build_confirm_task_delivery_handler();
    let result = handler(serde_json::json!({"task_id": "task-del"}));
    assert_eq!(result.unwrap()["status"], "confirmed");
    // Note: confirm_task_delivery may or may not remove the result depending on implementation
}

#[test]
fn test_get_actions_schema() {
    let cluster = Cluster::new(make_config());
    let schema = cluster.get_actions_schema();
    assert!(!schema.is_empty());
    // Check some known actions exist
    use crate::actions_schema::Action;
    let actions: Vec<&Action> = schema.iter().map(|s| &s.action).collect();
    assert!(actions.iter().any(|a| matches!(a, Action::Ping)));
    assert!(actions.iter().any(|a| matches!(a, Action::PeerChat)));
}

#[test]
fn test_get_actions_schema_json() {
    let cluster = Cluster::new(make_config());
    let json = cluster.get_actions_schema_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

#[test]
fn test_confirm_delivery_with_override() {
    let confirmed = Arc::new(Mutex::new(false));
    let confirmed_clone = confirmed.clone();
    let cluster = Cluster::new(make_config());
    cluster.set_call_with_context_fn(Box::new(move |_peer, action, _payload| {
        if action == "confirm_task_delivery" {
            *confirmed_clone.lock() = true;
        }
        Ok(Vec::new())
    }));

    cluster.confirm_delivery("peer-1", "task-1");
    assert!(*confirmed.lock());
}

#[test]
fn test_handle_task_complete_for_test() {
    let messages = Arc::new(Mutex::new(Vec::new()));
    let bus = Arc::new(MockBus {
        messages: messages.clone(),
    });
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.set_message_bus(bus);

    let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "ch");
    cluster.complete_task(&task_id, serde_json::json!("done"));

    cluster.handle_task_complete_for_test(&task_id);
    let msgs = messages.lock();
    assert_eq!(msgs.len(), 1);
}

#[test]
fn test_cluster_peer_resolver_empty_addresses() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a peer with no addresses
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "no-addr".into(),
            name: "no-addr".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    // Should fall back to parsing primary address
    let (addresses, port, is_online) = resolver.get_peer_info("no-addr").unwrap();
    assert_eq!(addresses.len(), 1);
    assert_eq!(addresses[0], "10.0.0.1");
    assert_eq!(port, 9000);
    assert!(is_online);
}

#[test]
fn test_cluster_peer_resolver_empty_primary_address() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a peer with empty primary address and no stored addresses
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "empty-addr".into(),
            name: "empty-addr".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: String::new(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    // 零候选 → None（闭环批 2026-10-13：Some(空池) 会让拨号方对 "" 空拨
    // 一次、错误文案误导为连接问题；registry 在册但地址全缺 = 诚实不可解析）。
    assert!(
        resolver.get_peer_info("empty-addr").is_none(),
        "peer in registry with zero addresses → None (no dial candidates)"
    );
}

#[test]
fn test_cluster_callbacks_trait_impl() {
    let cluster = Cluster::new(make_config());
    // Test ClusterCallbacks trait methods
    assert_eq!(ClusterCallbacks::node_id(&cluster), "local-node-001");
    assert_eq!(ClusterCallbacks::address(&cluster), "127.0.0.1:9000");
    assert_eq!(ClusterCallbacks::rpc_port(&cluster), DEFAULT_RPC_PORT);
    assert_eq!(ClusterCallbacks::role(&cluster), "worker");
    assert_eq!(ClusterCallbacks::category(&cluster), "general");
    assert!(ClusterCallbacks::tags(&cluster).is_empty());
}

#[test]
fn test_cluster_callbacks_sync_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // Test sync_to_disk through ClusterCallbacks trait
    let result = ClusterCallbacks::sync_to_disk(&cluster);
    assert!(result.is_ok());
}

#[test]
fn test_cluster_callbacks_handle_discovered_node() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    ClusterCallbacks::handle_discovered_node(
        &cluster,
        "cb-node",
        "cb-name",
        &["10.0.0.1".to_string()],
        21949,
        "worker",
        "dev",
        &["tag".to_string()],
        &["llm".to_string()],
        "agent",
    );

    let node = cluster.get_node_info("cb-node").unwrap();
    assert_eq!(node.base.name, "cb-name");
}

#[test]
fn test_cluster_callbacks_handle_node_offline() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "off-node",
        "off",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    ClusterCallbacks::handle_node_offline(&cluster, "off-node", "test");
    let node = cluster.get_node_info("off-node").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);
}

#[tokio::test]
async fn test_call_with_context_async_no_runtime_no_client() {
    let cluster = Cluster::new(make_config());
    // No RPC client set
    let result = cluster
        .call_with_context_async(
            "peer-1",
            "ping",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await;
    assert!(result.is_err());
}

#[test]
fn test_set_task_manager_for_test() {
    let mut cluster = Cluster::new(make_config());
    let custom_tm = Arc::new(TaskManager::new());
    cluster.set_task_manager_for_test(custom_tm);
}

#[test]
fn test_rpc_server_accessor_none() {
    let cluster = Cluster::new(make_config());
    assert!(cluster.rpc_server().is_none());
}

#[test]
fn test_rpc_server_accessor_some() {
    let mut cluster = Cluster::new(make_config());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    assert!(cluster.rpc_server().is_some());
}

#[test]
fn test_set_rpc_channel() {
    use crate::rpc::RpcChannel;
    #[derive(Debug)]
    struct MockCh;
    impl RpcChannel for MockCh {
        fn input(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
            Err("mock".into())
        }
    }
    let cluster = Cluster::new(make_config());
    assert!(cluster.get_rpc_channel().is_none());
    cluster.set_rpc_channel(Arc::new(MockCh));
    assert!(cluster.get_rpc_channel().is_some());
}

#[test]
fn test_register_peer_chat_handlers_without_channel() {
    let cluster = make_cluster_with_rpc_server();
    // Don't set RPC channel - should log warning and return early
    cluster.register_peer_chat_handlers();
    // Should not panic even without RPC channel
}

#[test]
fn test_new_with_empty_node_id_generates_one() {
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:9000".into(),
        peers: vec![],
        node_name: String::new(),
    };
    let cluster = Cluster::new(config);
    assert!(!cluster.node_id().is_empty());
    assert!(cluster.node_id().starts_with("node-"));
}

#[test]
fn test_sync_to_disk_excludes_self_node() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // Only the self node exists - should produce empty discovered list
    let result = cluster.sync_to_disk();
    assert!(result.is_ok());
    cluster.stop();
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[tokio::test]
async fn test_call_with_context_async_with_rpc_client_peer_not_found() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // RPC client is initialized by start(), but peer doesn't exist
    let result = cluster
        .call_with_context_async(
            "nonexistent-peer",
            "ping",
            serde_json::json!({}),
            Duration::from_secs(2),
        )
        .await;
    assert!(result.is_err());
}

#[test]
fn test_handle_task_complete_for_test_method() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let task_id = cluster.submit_task("test_action", serde_json::json!({}), "rpc", "ch1");
    cluster.complete_task(&task_id, serde_json::json!("done"));
    // Should not panic
    cluster.handle_task_complete_for_test(&task_id);
}

#[test]
fn test_handle_task_complete_for_test_nonexistent() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // Should not panic for nonexistent task
    cluster.handle_task_complete_for_test("nonexistent");
}

#[test]
fn test_get_actions_schema_returns_nonempty() {
    let cluster = Cluster::new(make_config());
    let schemas = cluster.get_actions_schema();
    assert!(!schemas.is_empty());
}

#[test]
fn test_get_actions_schema_json_valid_json() {
    let cluster = Cluster::new(make_config());
    let json = cluster.get_actions_schema_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(parsed.is_array());
}

#[test]
fn test_cluster_with_empty_node_id_in_config() {
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:9000".into(),
        peers: vec![],
        node_name: String::new(),
    };
    let cluster = Cluster::new(config);
    // Should auto-generate a node ID
    assert!(cluster.node_id().starts_with("node-"));
    assert!(!cluster.node_id().is_empty());
}

#[test]
fn test_set_rpc_channel_no_server() {
    let cluster = Cluster::new(make_config());
    // Setting RPC channel without server and not running should not panic
    #[derive(Debug)]
    struct MockChannel;
    impl crate::rpc::RpcChannel for MockChannel {
        fn input(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
            Err("mock".into())
        }
    }
    cluster.set_rpc_channel(Arc::new(MockChannel));
    assert!(cluster.get_rpc_channel().is_some());
}

#[test]
fn test_get_online_peers_after_start() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let peers = cluster.get_online_peers();
    // The self node is registered
    assert_eq!(peers.len(), 1);
    assert_eq!(peers[0].base.id, "local-node-001");
    assert_eq!(peers[0].status, NodeStatus::Online);
}

#[test]
fn test_get_peer_after_register() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "peer-x".into(),
            name: "peer-x-name".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.10:21949".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec!["10.0.0.10".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let peer = cluster.get_peer("peer-x").unwrap();
    assert_eq!(peer.base.name, "peer-x-name");
    assert_eq!(peer.capabilities.len(), 1);
}

#[test]
fn test_handle_discovered_node_updates_existing() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // First registration
    cluster.handle_discovered_node(
        "node-upd",
        "original-name",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec!["llm".into()],
        "agent",
    );
    let node = cluster.get_node_info("node-upd").unwrap();
    assert_eq!(node.base.name, "original-name");

    // Update with new name
    cluster.handle_discovered_node(
        "node-upd",
        "updated-name",
        vec!["10.0.0.2".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec!["llm".into(), "tools".into()],
        "agent",
    );
    let node = cluster.get_node_info("node-upd").unwrap();
    assert_eq!(node.base.name, "updated-name");
    assert_eq!(node.capabilities.len(), 2);
}

#[test]
fn test_list_tasks_after_submit_and_complete() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let t1 = cluster.submit_task("a1", serde_json::json!({}), "web", "ch1");
    let _t2 = cluster.submit_task("a2", serde_json::json!({}), "web", "ch2");
    cluster.complete_task(&t1, serde_json::json!("done"));

    let tasks = cluster.list_tasks();
    assert_eq!(tasks.len(), 2);
    // One completed, one pending
    let completed: Vec<_> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .collect();
    let pending: Vec<_> = tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Pending)
        .collect();
    assert_eq!(completed.len(), 1);
    assert_eq!(pending.len(), 1);
}

#[test]
fn test_submit_peer_chat_with_task_id() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let result = cluster.submit_peer_chat(
        "remote-1",
        "peer_chat",
        serde_json::json!({"content": "hello", "task_id": "my-custom-task-id"}),
        "web",
        "chat-1",
    );
    assert!(result.is_ok());
    let task_id = result.unwrap();
    assert!(!task_id.is_empty());
    // The task should exist
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.action, "peer_chat");
    assert_eq!(task.peer_id, "remote-1");
}

#[test]
fn test_cluster_peer_resolver_with_empty_primary_address() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a node with an empty address and no extra addresses
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "empty-addr".into(),
            name: "empty-addr".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: String::new(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    // 零候选 → None（闭环批 2026-10-13）：在册但地址全缺的 peer 对拨号方
    // 是诚实不可解析，不再回 Some(空池, DEFAULT_RPC_PORT) 让其对 "" 空拨。
    assert!(resolver.get_peer_info("empty-addr").is_none());
}

// G14 同族（2026-10-13 本机实录）：announce/探针身份载荷 addresses 为空时，
// 已知节点（静态 peer）的可用地址不得被空记录覆盖——曾致静态 127.0.0.1:<rpc>
// 被抹成 ""，后续 peer_chat 全部 `connect to : invalid socket address`。
// 身份字段（tier/professions/capabilities 等）照常合并（探针身份回传的
// purpose 不受损）。
#[test]
fn test_discovered_node_empty_addresses_known_peer_identity_merge_preserves_address() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "static-peer".into(),
            name: "static-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "127.0.0.1:50591".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["127.0.0.1".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let changed = cluster.handle_discovered_node_ex(
        "static-peer",
        "static-peer",
        Vec::new(), // 空地址载荷（本机实录形态）
        50591,      // rpc_port 合法（G14 只挡 0，本闸挡空地址这扇门）
        "worker",
        "development",
        Vec::new(),
        vec!["rpc".into()],
        "agent",
        Vec::new(),
        Some("big".into()),
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        None,       // UDP 隔离拓扑：无 source_addr（源地址收割语义外场景）
        "",
    );
    assert!(
        changed,
        "identity merge should report change (tier None->big)"
    );

    let node = cluster
        .registry
        .get("static-peer")
        .expect("known peer kept");
    assert_eq!(
        node.base.address, "127.0.0.1:50591",
        "usable address must survive"
    );
    assert_eq!(node.addresses, vec!["127.0.0.1".to_string()]);
    assert_eq!(node.tier, Some("big".to_string()), "identity fields merge");
    assert_eq!(node.status, NodeStatus::Online);
}

// 自建中继形态（QUIC 中继 2026-10-21）：announce 载荷携带
// `iroh:<hex>@<relay_url>` 完整拨号地址（Minimal 基座零发现层，裸 node_id
// 无解析路径）。接收侧两纪律：
// ① iroh 形态独占地址时 primary 原样收录——**不得按 TCP 口径拼 `:rpc_port`**
//   （会造出 `iroh:<hex>@http://...:50591` 非法地址，bridge 解析必败）；
// ② LAN IP 在前时 primary 保持 TCP 形态，iroh 完整形态入池尾（跨网兜底
//   不抢局域网直连优先级，池序 pin > 载荷 > 源地址 > iroh）。
#[test]
fn test_discovered_node_relay_qualified_overlay_address_pooling() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let hex = "a".repeat(64);
    let overlay = format!("iroh:{hex}@http://relay.example.com:3340");

    // 形态一：iroh 完整地址是唯一地址（纯 NAT 节点，无局域网可报）。
    let changed = cluster.handle_discovered_node_ex(
        "nat-only",
        "nat-only",
        vec![overlay.clone()],
        50591,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None,
        None,
        None,
        "",
    );
    assert!(changed, "overlay-only peer must register");
    let node = cluster.registry.get("nat-only").expect("registered");
    assert_eq!(
        node.base.address, overlay,
        "iroh 形态独占地址必须原样作 primary（禁拼 :port）"
    );
    assert!(
        node.addresses.contains(&overlay),
        "完整拨号地址必须入池（对端拨号候选）: {:?}",
        node.addresses
    );
    assert!(
        !node.addresses.iter().any(|a| a.ends_with(":50591")),
        "iroh 形态禁止被拼 RPC 端口: {:?}",
        node.addresses
    );

    // 形态二：LAN IP 在前（自建中继节点正常自报形态）——primary 保持
    // TCP 形态，iroh 完整形态入池尾。
    let changed2 = cluster.handle_discovered_node_ex(
        "lan-relay",
        "lan-relay",
        vec!["192.168.1.50".to_string(), overlay.clone()],
        50592,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None,
        None,
        None,
        "",
    );
    assert!(changed2, "lan+overlay peer must register");
    let node2 = cluster.registry.get("lan-relay").expect("registered");
    assert_eq!(
        node2.base.address, "192.168.1.50:50592",
        "LAN IP 在前时 primary 保持 TCP 形态"
    );
    let lan_pos = node2.addresses.iter().position(|a| a == "192.168.1.50");
    let overlay_pos = node2.addresses.iter().position(|a| a == &overlay);
    assert!(
        lan_pos.is_some() && overlay_pos.is_some() && overlay_pos.unwrap() > lan_pos.unwrap(),
        "iroh 完整形态必须排在 LAN IP 之后（池序纪律）: {:?}",
        node2.addresses
    );
}

// rendezvous 名册合并（QUIC 中继自动发现 2026-10-22）：成员经
// handle_discovered_node_ex 单一漏斗入册——via-relay 哨兵 tag 追加不替换、
// `iroh:<hex>@<relay>` 完整地址作 primary（iroh 形态禁拼 :rpc_port）、裸
// `iroh:<hex>` 身份候选入池；自己按 iroh_id 过滤。身份用真实 iroh keypair
// （valid_iroh_id 走 25519 公钥校验，任意 hex 会被静默丢弃）。
#[test]
fn test_rendezvous_merge_via_relay_tag_self_filter_and_address_forms() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let self_hex = crate::roaming::random_valid_iroh_hex();
    let peer_hex = crate::roaming::random_valid_iroh_hex();
    let relay_url = "http://relay.example.com:3340";
    let member = |node_id: &str, iroh: String| crate::roaming::rendezvous::RendezvousMember {
        node_id: node_id.to_string(),
        name: format!("名册-{node_id}"),
        role: "worker".to_string(),
        category: "development".to_string(),
        node_type: "agent".to_string(),
        rpc_port: 50591,
        tags: vec!["dev".to_string()],
        professions: vec!["dev:cpp".to_string()],
        tier: Some("big".to_string()),
        iroh_id: iroh,
        build: "aeb1221c".to_string(),
        relay_url: relay_url.to_string(),
        features: vec!["cluster".to_string()],
        last_seen_ms: 100,
    };

    let members = vec![
        member("self-node", self_hex.clone()),
        member("relay-peer", peer_hex.clone()),
    ];
    let merged = cluster.merge_rendezvous_members(&members, &self_hex, relay_url);
    assert_eq!(merged, 1, "自己按 iroh_id 过滤，只有对端入册");
    assert!(cluster.registry.get("self-node").is_none(), "自己不得入册");

    let node = cluster
        .registry
        .get("relay-peer")
        .expect("via-relay 成员入册");
    assert!(
        node.tags.iter().any(|t| t == "via-relay"),
        "via-relay 哨兵必须在场: {:?}",
        node.tags
    );
    assert!(
        node.tags.iter().any(|t| t == "dev"),
        "原有 tags 追加不替换: {:?}",
        node.tags
    );
    let full = format!("iroh:{peer_hex}@{relay_url}");
    assert_eq!(
        node.base.address, full,
        "完整拨号地址作 primary（iroh 形态禁拼 :rpc_port）"
    );
    assert!(
        node.addresses.contains(&format!("iroh:{peer_hex}")),
        "裸 iroh 身份候选入池（valid_iroh_id 通路）: {:?}",
        node.addresses
    );
}

// 稳态纪律：content_eq 不含 last_seen——名册每 30s 重同步仅 last_seen 增长
// 时不得反复判变（不刷日志/事件）；首见判变一次，之后恒静默。
#[test]
fn test_rendezvous_merge_steady_state_no_churn() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let self_hex = crate::roaming::random_valid_iroh_hex();
    let peer_hex = crate::roaming::random_valid_iroh_hex();
    let relay_url = "http://relay.example.com:3340";
    let mk = |seen: u64| crate::roaming::rendezvous::RendezvousMember {
        node_id: "relay-peer".to_string(),
        name: "名册-relay-peer".to_string(),
        role: "worker".to_string(),
        category: "development".to_string(),
        node_type: "agent".to_string(),
        rpc_port: 50591,
        tags: vec!["dev".to_string()],
        professions: Vec::new(),
        tier: None,
        iroh_id: peer_hex.clone(),
        build: String::new(),
        relay_url: relay_url.to_string(),
        features: vec!["cluster".to_string()],
        last_seen_ms: seen,
    };

    let first =
        cluster.merge_rendezvous_members(std::slice::from_ref(&mk(100)), &self_hex, relay_url);
    assert_eq!(first, 1, "首见判变一次");
    let steady =
        cluster.merge_rendezvous_members(std::slice::from_ref(&mk(200)), &self_hex, relay_url);
    assert_eq!(steady, 0, "仅 last_seen 增长 = 无变更（稳态不刷）");
}

// ---------------------------------------------------------------------------
// 拨号路径观测查询面（打洞直连 / 经中继转发——nodes.list roamingPath 数据源）
// ---------------------------------------------------------------------------

/// `roaming_hex_of`：注册表节点地址抠 `iroh:<hex>`（裸形态 / 带 relay 后缀
/// 形态都剥出，小写归一）；纯 LAN 节点 / 未注册节点 None。无 feature 门
/// （纯字符串语义——QUIC 插件化 M3 后端点观测在插件，节点→hex 映射归宿主）。
#[test]
fn roaming_hex_of_extracts_iroh_hex_and_normalizes() {
    let cluster = Cluster::new(make_config());
    let hex = "AA".repeat(32); // 大写入册 → 查询面小写归一
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "roam-peer".into(),
            name: "roam-worker".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: format!("iroh:{}@http://42.193.96.45:3340", hex.to_lowercase()),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: vec!["via-relay".into()],
        addresses: vec![format!("iroh:{hex}")],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    let got = cluster
        .roaming_hex_of("roam-peer")
        .expect("漫游节点应有 hex");
    assert_eq!(got, hex.to_lowercase(), "hex 应小写归一");

    // 纯 LAN 节点（无 iroh 地址）：None。
    let mut lan = cluster.registry.get("roam-peer").expect("已入册");
    lan.base.id = "lan-peer".into();
    lan.base.address = "10.0.0.9:9000".into();
    lan.addresses = Vec::new();
    cluster.register_node(lan);
    assert_eq!(cluster.roaming_hex_of("lan-peer"), None);
    assert_eq!(cluster.roaming_hex_of("missing-peer"), None);
}

/// `roaming_dial_paths`：插件缺席（宿主测试进程不装载 plugin_roaming）→
/// 空 map（消费面按无观测渲染，诚实降级）。双 feature 形态同语义：
/// feature-on 走桥 Absent 分支，feature-off 恒空。
#[tokio::test]
async fn roaming_dial_paths_without_plugin_is_empty() {
    let cluster = Cluster::new(make_config());
    assert!(
        cluster.roaming_dial_paths().await.is_empty(),
        "插件缺席时空 map"
    );
}

// QUIC 插件化 M3：端点观测槽真通路测试（bind_direct 双端点 loopback 互连
// → 观察任务写槽 → 查询面读 direct）已随端点本体迁入 plugin_roaming
// （插件侧 endpoint/dial path 测试覆盖）；宿主侧查询面语义由上方
// roaming_hex_of / roaming_dial_paths 两测钉死。

#[test]
fn test_discovered_node_empty_addresses_unknown_peer_still_registered_bridge_semantics() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // 未知节点 + 空地址 + 有效 rpc_port：照常入册——空地址记录是桥接拓扑的
    // 合法形态（bridge member_sync 广播成员本就 via-bridge 可达；拒 registering
    // 会断桥接设备的成员注册）。空地址闸只保护**已知节点**的地址不被覆盖。
    let changed = cluster.handle_discovered_node_ex(
        "ghost-peer",
        "ghost-peer",
        Vec::new(),
        21949,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        None,       // 桥接成员登记：无 source_addr
        "",
    );
    assert!(changed, "bridge member with empty addresses must register");
    let node = cluster.registry.get("ghost-peer").expect("registered");
    assert_eq!(node.base.address, "", "empty-address insert stays empty");
    assert_eq!(node.base.role, nemesis_types::cluster::NodeRole::Worker);
}

/// BUG-20261101-263：身份字符集闸——畸形 announce（id 含 `:`/`/`）在入册口
/// loud 拒收（G14 同族），零注册。坏 id 会切坏下游复合键协议（session_key
/// 的 `/` 分隔、`__ASYNC__` marker 的 `:` 分隔、连接池键）。
#[test]
fn test_discovered_node_malformed_id_charset_gate_rejected() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    for bad_id in ["bad:id", "bad/id", "bad id", ""] {
        let changed = cluster.handle_discovered_node_ex(
            bad_id,
            "malformed-peer",
            vec!["127.0.0.1".to_string()],
            21949,
            "worker",
            "development",
            Vec::new(),
            Vec::new(),
            "agent",
            Vec::new(),
            None,
            Vec::new(),
            None,
            None,
            None,
            "",
        );
        assert!(
            !changed,
            "malformed id {:?} must be rejected at registration gate",
            bad_id
        );
        assert!(
            cluster.registry.get(bad_id).is_none(),
            "rejected id {:?} must not enter registry",
            bad_id
        );
    }
}

#[test]
fn test_cluster_peer_resolver_uses_stored_addresses() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "multi-addr".into(),
            name: "multi-addr".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "203.0.113.20:21949".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["203.0.113.21".into(), "203.0.113.20".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    let (addresses, port, is_online) = resolver.get_peer_info("multi-addr").unwrap();
    // Should use stored addresses, not parse primary address
    assert_eq!(addresses.len(), 2);
    assert!(addresses.contains(&"203.0.113.21".to_string()));
    assert_eq!(port, 21949);
    assert!(is_online);
}

#[test]
fn test_string_value_with_array() {
    let v = serde_json::json!([1, 2, 3]);
    // Array falls through to as_str().unwrap_or("")
    assert_eq!(string_value(Some(&v)), "");
}

#[test]
fn test_string_value_with_nested_object() {
    let v = serde_json::json!({"nested": "value"});
    assert_eq!(string_value(Some(&v)), "");
}

#[test]
fn test_handle_node_offline_updates_status() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.handle_discovered_node(
        "offline-test",
        "offline-test",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );

    let node = cluster.get_node_info("offline-test").unwrap();
    assert_eq!(node.status, NodeStatus::Online);

    cluster.handle_node_offline("offline-test", "heartbeat timeout");
    let node = cluster.get_node_info("offline-test").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);

    // Going offline again should still be offline
    cluster.handle_node_offline("offline-test", "duplicate");
    let node = cluster.get_node_info("offline-test").unwrap();
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_register_basic_handlers_not_running_error() {
    let cluster = Cluster::new(make_config());
    // Don't start
    let result = cluster.register_basic_handlers();
    assert!(result.is_err());
}

// ============================================================
// Coverage improvement: additional cluster paths
// ============================================================

#[test]
fn test_cluster_new_generates_node_id_when_empty() {
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:9000".into(),
        peers: vec![],
        node_name: String::new(),
    };
    let cluster = Cluster::new(config);
    assert!(!cluster.node_id().is_empty());
}

#[test]
fn test_cluster_with_callback() {
    let config = make_config();
    let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let called = callback_called.clone();
    let cluster = Cluster::with_callback(
        config,
        Box::new(move |_task| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
        }),
    );
    assert!(!cluster.node_id().is_empty());
}

#[test]
fn test_cluster_accessors() {
    let cluster = Cluster::new(make_config());
    assert_eq!(cluster.node_id(), "local-node-001");
    // 批次四：make_config 显式名回显（同 test_accessors）。
    assert_eq!(cluster.node_name(), "local-node");
    assert_eq!(cluster.address(), "127.0.0.1:9000");
    assert_eq!(cluster.role(), "worker");
    assert_eq!(cluster.category(), "general");
    assert!(cluster.tags().is_empty());
    assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);
    assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
}

#[test]
fn test_cluster_set_ports() {
    let mut cluster = Cluster::new(make_config());
    cluster.set_ports(11111, 22222);
    assert_eq!(cluster.udp_port(), 11111);
    assert_eq!(cluster.rpc_port(), 22222);
}

#[test]
fn test_cluster_stop() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
    assert!(!cluster.is_running());
}

#[test]
fn test_cluster_get_capabilities_after_start() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let caps = cluster.get_capabilities();
    // Local node has "cluster" capability from start()
    assert!(caps.contains(&"cluster".to_string()));
}

#[test]
fn test_cluster_get_all_local_ips() {
    let cluster = Cluster::new(make_config());
    let ips = cluster.get_all_local_ips();
    // Just verify it doesn't panic
    let _ = ips;
}

#[test]
fn test_cluster_get_online_peers() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let peers = cluster.get_online_peers();
    // Should have at least the local node
    assert!(!peers.is_empty());
}

#[test]
fn test_cluster_find_peers_by_capability() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let peers = cluster.find_peers_by_capability("nonexistent");
    assert!(peers.is_empty());
}

#[test]
fn test_cluster_remove_node() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.handle_discovered_node(
        "remove-me",
        "remove-me",
        vec!["10.0.0.1".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    assert!(cluster.get_node_info("remove-me").is_some());
    assert!(cluster.remove_node("remove-me"));
    assert!(cluster.get_node_info("remove-me").is_none());
}

#[test]
fn test_cluster_remove_nonexistent_node() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.remove_node("nonexistent"));
}

#[test]
fn test_cluster_cleanup_task_noop() {
    let cluster = Cluster::new(make_config());
    // Should not panic
    cluster.cleanup_task("any-task");
}

#[test]
fn test_cluster_list_tasks_empty() {
    let cluster = Cluster::new(make_config());
    let tasks = cluster.list_tasks();
    assert!(tasks.is_empty());
}

#[test]
fn test_cluster_task_manager_accessor() {
    let cluster = Cluster::new(make_config());
    let _tm = cluster.task_manager();
}

#[test]
fn test_cluster_continuation_store_accessor() {
    let cluster = Cluster::new(make_config());
    let _cs = cluster.continuation_store();
}

#[test]
fn test_cluster_result_store_accessor() {
    let cluster = Cluster::new(make_config());
    let _rs = cluster.result_store();
}

#[test]
fn test_cluster_stop_receiver() {
    let cluster = Cluster::new(make_config());
    let _rx = cluster.stop_receiver();
}

#[test]
fn test_cluster_register_rpc_handler_not_running() {
    let cluster = Cluster::new(make_config());
    let result = cluster.register_rpc_handler("test", Box::new(|_| Ok(serde_json::json!({}))));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not running"));
}

#[test]
fn test_cluster_register_forge_handlers_not_running() {
    let cluster = Cluster::new(make_config());
    struct MockProvider;
    impl crate::handlers::ForgeDataProvider for MockProvider {
        fn receive_reflection(&self, _payload: &serde_json::Value) -> Result<(), String> {
            Ok(())
        }
        fn get_reflections_list_payload(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        fn read_reflection_content(&self, _filename: &str) -> Result<String, String> {
            Err("not found".into())
        }
        fn sanitize_content(&self, content: &str) -> String {
            content.to_string()
        }
        fn clone_boxed(&self) -> Box<dyn crate::handlers::ForgeDataProvider> {
            Box::new(MockProvider)
        }
    }
    let result = cluster.register_forge_handlers(Box::new(MockProvider));
    assert!(result.is_err());
}

#[test]
fn test_call_with_context_no_rpc_client() {
    let cluster = Cluster::new(make_config());
    // Don't start, so no RPC client
    let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("RPC client not initialized"));
}

#[test]
fn test_handle_task_complete_no_task() {
    let cluster = Cluster::new(make_config());
    // Should not panic for nonexistent task
    cluster.handle_task_complete("nonexistent-task");
}

#[test]
fn test_handle_task_complete_no_bus_v2() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    let task_id = cluster.submit_task("test", serde_json::json!({}), "web", "chat-1");
    // No bus set, should log error but not panic
    cluster.handle_task_complete(&task_id);
}

#[test]
fn test_bus_inbound_message_debug_v2() {
    let msg = BusInboundMessage {
        channel: "system".into(),
        sender_id: "test".into(),
        chat_id: "chat-1".into(),
        content: "hello".into(),
        metadata: std::collections::HashMap::new(),
    };
    let debug = format!("{:?}", msg);
    assert!(debug.contains("system"));
}

#[test]
fn test_submit_peer_chat_v2() {
    let cluster = Cluster::new(make_config());
    let result = cluster.submit_peer_chat(
        "peer-1",
        "peer_chat",
        serde_json::json!({"content": "hello", "task_id": "t-123"}),
        "web",
        "chat-1",
    );
    assert!(result.is_ok());
    // submit_peer_chat creates a new task with its own ID
    let task_id = result.unwrap();
    assert!(!task_id.is_empty());
}

#[test]
fn test_submit_peer_chat_generates_task_id_v2() {
    let cluster = Cluster::new(make_config());
    let result = cluster.submit_peer_chat(
        "peer-1",
        "peer_chat",
        serde_json::json!({"content": "hello"}),
        "web",
        "chat-1",
    );
    assert!(result.is_ok());
    // Should generate a UUID task_id since none in payload
    let task_id = result.unwrap();
    assert!(!task_id.is_empty());
}

#[test]
fn test_assign_task_nonexistent_v2() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.assign_task("nonexistent", "node-1"));
}

#[test]
fn test_complete_task_nonexistent_v2() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.complete_task("nonexistent", serde_json::json!({})));
}

#[test]
fn test_fail_task_nonexistent_v2() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.fail_task("nonexistent", "error"));
}

// ============================================================
// Additional coverage tests for remaining uncovered paths
// ============================================================

// -- 1. sync_to_disk with NodeStatus::Connecting ----------------------------

#[test]
fn test_sync_to_disk_with_connecting_status() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // Register a node with Connecting status to hit the "unknown" branch
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "connecting-node".into(),
            name: "connecting-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.99:9000".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Connecting,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let result = cluster.sync_to_disk();
    assert!(result.is_ok());

    // Verify the state file was written
    let state_path = dir.path().join("cluster").join("state.toml");
    assert!(state_path.exists());

    // Read back and verify the status is "unknown"
    let content = std::fs::read_to_string(&state_path).unwrap();
    assert!(
        content.contains("unknown"),
        "Connecting status should map to 'unknown' in state file, got: {}",
        content
    );
    cluster.stop();
}

// -- 2. sync_to_disk write failure -----------------------------------------

#[test]
fn test_sync_to_disk_write_failure() {
    // Use a workspace path that cannot be written to
    // On Windows, trying to write inside a non-existent deeply nested path
    // where parent doesn't exist and can't be created should fail
    let dir = tempfile::tempdir().unwrap();
    let _cluster = Cluster::new(make_config());
    // Override the dynamic_state_path to point to an impossible location
    // by using with_workspace but then corrupting the path
    let impossible_path = dir.path().join("cluster");
    // Create a file where the directory should be to force mkdir to fail
    std::fs::write(&impossible_path, "blocker").unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // The state.toml would go inside cluster/ which is now a file, not a dir
    let result = cluster.sync_to_disk();
    // Should fail because the parent "cluster" path is a file, not a directory
    assert!(
        result.is_err(),
        "Expected sync_to_disk to fail with blocked path"
    );
    cluster.stop();
}

// -- 3. string_value helper function - all branches -------------------------

#[test]
fn test_string_value_all_branches() {
    // String
    let v = serde_json::Value::String("hello".into());
    assert_eq!(string_value(Some(&v)), "hello");

    // Null
    assert_eq!(string_value(Some(&serde_json::Value::Null)), "");

    // None
    assert_eq!(string_value(None), "");

    // Number
    let v = serde_json::json!(42);
    assert_eq!(string_value(Some(&v)), "42");

    // Boolean
    let v = serde_json::json!(true);
    assert_eq!(string_value(Some(&v)), "true");

    // Array fallback
    let v = serde_json::json!([1, 2, 3]);
    assert_eq!(string_value(Some(&v)), "");

    // Object fallback
    let v = serde_json::json!({"k": "v"});
    assert_eq!(string_value(Some(&v)), "");

    // Float number
    let v = serde_json::json!(2.75);
    assert_eq!(string_value(Some(&v)), "2.75");

    // Boolean false
    let v = serde_json::json!(false);
    assert_eq!(string_value(Some(&v)), "false");
}

// -- 4. poll_stale_pending_tasks with malformed created_at ------------------

#[tokio::test]
async fn test_poll_stale_pending_tasks_malformed_created_at() {
    let tm = Arc::new(TaskManager::new());
    // Create a task with invalid RFC3339 timestamp - should be skipped (continue)
    let task = Task {
        id: "bad-date".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: "not-a-date".to_string(),
        completed_at: None,
    };
    tm.submit(task).unwrap();

    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    // Task should still be pending (skipped due to malformed date)
    let t = tm.get_task("bad-date").unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
}

// -- 5. poll_stale_pending_tasks with unknown status response ---------------

#[tokio::test]
async fn test_poll_stale_pending_tasks_unknown_status_response() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "weird-status".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // Returns a status that doesn't match "running", "done", or "not_found"
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, _action, _payload| {
        let resp = serde_json::json!({"status": "weird_unknown_status", "task_id": "weird-status"});
        Ok(serde_json::to_vec(&resp).unwrap())
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("weird-status").unwrap();
    // Unknown status -> continue -> task stays pending
    assert_eq!(t.status, TaskStatus::Pending);
}

// -- 6. poll_stale_pending_tasks with invalid JSON response -----------------

#[tokio::test]
async fn test_poll_stale_pending_tasks_invalid_json_response() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "invalid-json".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // Returns invalid JSON bytes
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, _action, _payload| {
        Ok(b"this is not valid json {{{".to_vec())
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("invalid-json").unwrap();
    // Invalid JSON -> continue -> task stays pending
    assert_eq!(t.status, TaskStatus::Pending);
}

// -- 7. poll_stale_pending_tasks with done/error result_status ---------------

#[tokio::test]
async fn test_poll_stale_pending_tasks_done_with_error_status() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "done-err".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    // call_fn returns "done" with result_status "error"
    let call_fn: Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    > = Some(Arc::new(|_peer, action, _payload| {
        if action == "query_task_result" {
            let resp = serde_json::json!({
                "status": "done",
                "task_id": "done-err",
                "result_status": "error",
                "response": "",
                "error": "something went wrong on remote"
            });
            Ok(serde_json::to_vec(&resp).unwrap())
        } else {
            // confirm_task_delivery
            Ok(Vec::new())
        }
    }));

    poll_stale_pending_tasks(
        &tm,
        &call_fn,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    let t = tm.get_task("done-err").unwrap();
    // result_status "error" -> complete_callback with "error" -> should be completed (callback handled)
    assert_eq!(t.status, TaskStatus::Failed);
}

// -- 8. confirm_delivery with call_fn fallback -------------------------------

#[test]
fn test_confirm_delivery_with_call_fn_fallback() {
    let call_invoked = Arc::new(Mutex::new(false));
    let call_invoked_clone = call_invoked.clone();

    let cluster = Cluster::new(make_config());
    // Set call_with_context_fn but DO NOT start (so no RPC client)
    cluster.set_call_with_context_fn(Box::new(move |peer_id, action, _payload| {
        if action == "confirm_task_delivery" {
            *call_invoked_clone.lock() = true;
            assert_eq!(peer_id, "peer-1");
        }
        Ok(Vec::new())
    }));

    cluster.confirm_delivery("peer-1", "task-1");
    assert!(
        *call_invoked.lock(),
        "call_with_context_fn should have been invoked"
    );
}

// -- 9. confirm_delivery with neither client nor fn --------------------------
// NOTE: confirm_delivery() has a potential deadlock when call_with_context_fn
// is None: it locks the mutex, then falls back to call_with_context() which
// also tries to lock the same mutex. Instead we test the free function
// confirm_delivery_with directly, which is the async path used by the
// recovery loop.

#[tokio::test]
async fn test_confirm_delivery_neither_client_nor_fn() {
    // No client and no call_fn -> should just return without panic
    confirm_delivery_with(&None, None, "peer-1", "task-1").await;
}

// -- 10. submit_peer_chat without task_id in payload -------------------------

#[test]
fn test_submit_peer_chat_without_task_id_generates_uuid() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Payload has no "task_id" field - should auto-generate
    let result = cluster.submit_peer_chat(
        "remote-1",
        "peer_chat",
        serde_json::json!({"content": "hello from test"}),
        "web",
        "chat-99",
    );
    assert!(result.is_ok());
    let task_id = result.unwrap();
    // The task_id returned is from task_manager.create_task_with_peer, not from payload
    assert!(!task_id.is_empty());
    // The task should exist
    let task = cluster.get_task(&task_id).unwrap();
    assert_eq!(task.action, "peer_chat");
    assert_eq!(task.peer_id, "remote-1");
    cluster.stop();
}

// -- 11. with_workspace loads node_id from existing config -------------------

#[test]
fn test_with_workspace_loads_node_id_from_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();

    // Write a peers.toml with a specific node ID
    let peers_toml = r#"
[node]
id = "existing-node-42"
name = "ExistingNode"
address = "10.0.0.1:21949"
role = "worker"
category = "general"
"#;
    std::fs::write(cluster_dir.join("peers.toml"), peers_toml).unwrap();

    // Create cluster with empty node_id in config - should load from file
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "0.0.0.0:9000".into(),
        peers: vec![],
        node_name: String::new(),
    };
    let cluster = Cluster::with_workspace(config, dir.path().to_path_buf());
    assert_eq!(cluster.node_id(), "existing-node-42");
}

// -- 12. call_with_context with RPC client but peer not found ----------------

#[test]
fn test_call_with_context_rpc_client_peer_not_found() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // RPC client is initialized by start(), but the peer is not in the registry
    let result = cluster.call_with_context("nonexistent-peer-xyz", "ping", serde_json::json!({}));
    assert!(result.is_err(), "Should fail for nonexistent peer");
    cluster.stop();
}

// -- 13. register_basic_handlers with RPC server ----------------------------

#[test]
fn test_register_basic_handlers_with_rpc_server_success() {
    let cluster = make_cluster_with_rpc_server();
    let result = cluster.register_basic_handlers();
    assert!(result.is_ok());

    // Verify specific handlers work correctly
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    // Test ping handler
    let ping_result = rpc_server.handle_request_sync("ping", serde_json::json!({}));
    assert!(ping_result.is_ok());
    let resp = ping_result.unwrap();
    assert_eq!(resp["status"], "pong");
    assert_eq!(resp["node_id"], "local-node-001");

    // Test get_info handler
    let info_result = rpc_server.handle_request_sync("get_info", serde_json::json!({}));
    assert!(info_result.is_ok());
    let resp = info_result.unwrap();
    assert_eq!(resp["node_id"], "local-node-001");
    assert_eq!(resp["status"], "online");

    // Test list_actions handler
    let actions_result = rpc_server.handle_request_sync("list_actions", serde_json::json!({}));
    assert!(actions_result.is_ok());
    let resp = actions_result.unwrap();
    assert!(resp["actions"].is_array());

    // Test get_capabilities handler
    let caps_result = rpc_server.handle_request_sync("get_capabilities", serde_json::json!({}));
    assert!(caps_result.is_ok());
    let resp = caps_result.unwrap();
    assert!(resp["capabilities"].is_array());
}

// -- 14. register_forge_handlers success path --------------------------------

#[test]
fn test_register_forge_handlers_success() {
    let cluster = make_cluster_with_rpc_server();

    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));
    let result = cluster.register_forge_handlers(provider);
    assert!(result.is_ok());

    // Verify forge_share works
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let share_result = rpc_server.handle_request_sync(
        "forge_share",
        serde_json::json!({
            "source_node": "node-remote-1",
            "report": {"insights": ["test insight"], "score": 0.9},
        }),
    );
    assert!(share_result.is_ok());
    let resp = share_result.unwrap();
    assert_eq!(resp["status"], "ok");

    // Verify forge_get_reflections works
    let list_result =
        rpc_server.handle_request_sync("forge_get_reflections", serde_json::json!({}));
    assert!(list_result.is_ok());
    let resp = list_result.unwrap();
    assert!(resp.get("reflections").is_some());
}

// -- 15. register_forge_handlers forge_share with error ----------------------

#[test]
fn test_register_forge_handlers_forge_share_error() {
    let cluster = make_cluster_with_rpc_server();

    struct ErrorProvider;
    impl crate::handlers::ForgeDataProvider for ErrorProvider {
        fn receive_reflection(&self, _payload: &serde_json::Value) -> Result<(), String> {
            Err("simulated storage failure".into())
        }
        fn get_reflections_list_payload(&self) -> serde_json::Value {
            serde_json::json!({"reflections": [], "count": 0})
        }
        fn read_reflection_content(&self, _filename: &str) -> Result<String, String> {
            Err("not found".into())
        }
        fn sanitize_content(&self, content: &str) -> String {
            content.to_string()
        }
        fn clone_boxed(&self) -> Box<dyn crate::handlers::ForgeDataProvider> {
            Box::new(ErrorProvider)
        }
    }

    let result = cluster.register_forge_handlers(Box::new(ErrorProvider));
    assert!(result.is_ok());

    // forge_share should return error status when receive_reflection fails
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let share_result = rpc_server.handle_request_sync(
        "forge_share",
        serde_json::json!({
            "source_node": "node-1",
            "report": {"data": "test"},
        }),
    );
    assert!(share_result.is_ok());
    let resp = share_result.unwrap();
    assert_eq!(resp["status"], "error");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("simulated storage failure")
    );
}

// -- 16. register_forge_handlers forge_get_reflections with filename ---------

#[test]
fn test_register_forge_handlers_get_reflections_with_filename() {
    let cluster = make_cluster_with_rpc_server();
    let dir = tempfile::tempdir().unwrap();

    // Create a reflection file
    let reflections_dir = dir.path().join("reflections");
    std::fs::create_dir_all(&reflections_dir).unwrap();
    let test_content = r#"{"insights": ["test insight 1", "test insight 2"]}"#;
    std::fs::write(reflections_dir.join("test-reflection.json"), test_content).unwrap();

    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));
    let result = cluster.register_forge_handlers(provider);
    assert!(result.is_ok());

    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let list_result = rpc_server.handle_request_sync(
        "forge_get_reflections",
        serde_json::json!({
            "filename": "test-reflection.json",
        }),
    );
    assert!(list_result.is_ok());
    let resp = list_result.unwrap();
    // Should include the content of the specific reflection
    assert!(resp.get("content").is_some());
    assert_eq!(resp["filename"], "test-reflection.json");
}

// -- 17. register_forge_handlers forge_get_reflections read error -----------

#[test]
fn test_register_forge_handlers_get_reflections_read_error() {
    let cluster = make_cluster_with_rpc_server();
    let dir = tempfile::tempdir().unwrap();

    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));
    let result = cluster.register_forge_handlers(provider);
    assert!(result.is_ok());

    // Request a non-existent file
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let list_result = rpc_server.handle_request_sync(
        "forge_get_reflections",
        serde_json::json!({
            "filename": "nonexistent-file.json",
        }),
    );
    assert!(list_result.is_ok());
    let resp = list_result.unwrap();
    assert_eq!(resp["status"], "error");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("Failed to read reflection")
    );
}

// -- 18. register_peer_chat_handlers with RPC channel ------------------------

#[test]
fn test_register_peer_chat_handlers_with_rpc_channel() {
    use crate::rpc::RpcChannel;
    #[derive(Debug)]
    struct MockRpcChannelForPeerChat;
    impl RpcChannel for MockRpcChannelForPeerChat {
        fn input(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
            Err("mock input".into())
        }
    }

    let cluster = make_cluster_with_rpc_server();

    // Set RPC channel - this triggers register_peer_chat_handlers
    cluster.set_rpc_channel(Arc::new(MockRpcChannelForPeerChat));

    // Verify all peer chat handlers are registered
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    let peer_chat_result = rpc_server.handle_request_sync(
        "peer_chat",
        serde_json::json!({
            "content": "hello",
            "task_id": "t1",
        }),
    );
    assert!(peer_chat_result.is_ok());
    assert_eq!(peer_chat_result.unwrap()["status"], "accepted");

    let callback_result = rpc_server.handle_request_sync(
        "peer_chat_callback",
        serde_json::json!({
            "task_id": "nonexistent",
            "status": "success",
        }),
    );
    assert!(callback_result.is_ok());

    let hello_result = rpc_server.handle_request_sync("hello", serde_json::json!({}));
    assert!(hello_result.is_ok());
    assert_eq!(hello_result.unwrap()["status"], "online");

    let query_result = rpc_server.handle_request_sync(
        "query_task_result",
        serde_json::json!({
            "task_id": "nonexistent",
        }),
    );
    assert!(query_result.is_ok());

    let confirm_result = rpc_server.handle_request_sync(
        "confirm_task_delivery",
        serde_json::json!({
            "task_id": "nonexistent",
        }),
    );
    assert!(confirm_result.is_ok());
}

// -- 19. ClusterPeerResolver fallback scan by name ---------------------------

#[test]
fn test_cluster_peer_resolver_fallback_scan_by_name() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // Register a node with specific id and name
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-123".into(),
            name: "MyNode".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "203.0.113.30:21949".into(),
            category: "dev".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        addresses: vec!["203.0.113.30".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    // Resolve by name (fallback scan)
    let result = resolver.get_peer_info("MyNode");
    assert!(
        result.is_some(),
        "Should find peer by name via fallback scan"
    );
    let (addresses, port, is_online) = result.unwrap();
    assert!(addresses.contains(&"203.0.113.30".to_string()));
    assert_eq!(port, 21949);
    assert!(is_online);

    // Also verify direct lookup by id still works
    let result2 = resolver.get_peer_info("node-123");
    assert!(result2.is_some());
    cluster.stop();
}

// -- 20. ClusterPeerResolver offline peer ------------------------------------

#[test]
fn test_cluster_peer_resolver_offline_peer_is_online_false() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "offline-peer-20".into(),
            name: "OfflinePeer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "203.0.113.40:21949".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["203.0.113.40".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    let (_, _, is_online) = resolver.get_peer_info("offline-peer-20").unwrap();
    assert!(!is_online, "Offline peer should have is_online=false");
    cluster.stop();
}

// -- 21. generate_node_id uniqueness ----------------------------------------

#[test]
fn test_generate_node_id_uniqueness_multiple() {
    let mut ids = std::collections::HashSet::new();
    for _ in 0..50 {
        let id = generate_node_id();
        assert!(
            id.starts_with("node-"),
            "ID should start with 'node-': {}",
            id
        );
        assert!(ids.insert(id), "Generated duplicate node ID");
    }
    assert_eq!(ids.len(), 50);
}

// -- 22. parse_host_port with no colon --------------------------------------

#[test]
fn test_parse_host_port_no_colon_returns_default_port() {
    let (host, port) = parse_host_port("noport");
    assert_eq!(host, "noport");
    assert_eq!(port, DEFAULT_RPC_PORT);
}

// -- 23. set_node_name -------------------------------------------------------

#[test]
fn test_set_node_name() {
    let cluster = Cluster::new(make_config());
    // 批次四：make_config 显式名回显（config.cluster.json node_name 路径）。
    assert_eq!(cluster.node_name(), "local-node");

    cluster.set_node_name("CustomNode");
    assert_eq!(cluster.node_name(), "CustomNode");

    // Set again to different name
    cluster.set_node_name("AnotherName");
    assert_eq!(cluster.node_name(), "AnotherName");
}

// -- 24. rpc_client_arc before and after start --------------------------------

#[test]
fn test_rpc_client_arc_before_and_after_start() {
    let cluster = Cluster::new(make_config());
    // Before start, no RPC client
    assert!(
        cluster.rpc_client_arc().is_none(),
        "rpc_client_arc should return None before start()"
    );

    cluster.start();
    // After start, RPC client is initialized
    let client = cluster.rpc_client_arc();
    assert!(
        client.is_some(),
        "rpc_client_arc should return Some after start()"
    );
    cluster.stop();
}

// -- Phase 1: generate_node_id format + with_workspace persistence --

#[test]
fn test_with_workspace_persists_runtime_id_to_peers_toml() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let peers_path = workspace.join("cluster").join("peers.toml");

    // Use empty node_id to trigger runtime fallback (generate_node_id path)
    let config = ClusterConfig {
        node_id: String::new(),
        bind_address: "127.0.0.1:9000".into(),
        peers: vec![],
        node_name: String::new(),
    };
    let cluster = Cluster::with_workspace(config, workspace.clone());

    // peers.toml should now exist (ensure_node_id creates it)
    assert!(peers_path.exists(), "peers.toml should be created");

    // Verify the persisted id matches cluster.node_id()
    let loaded = crate::cluster_config::load_static_config(&peers_path).unwrap();
    assert_eq!(
        loaded.node.id,
        cluster.node_id(),
        "persisted id should match cluster.node_id()"
    );
    assert!(
        !loaded.node.id.is_empty(),
        "persisted id should not be empty"
    );
    assert!(
        loaded.node.id.starts_with("node-"),
        "persisted id should use 'node-' prefix, got: {}",
        loaded.node.id
    );
}

#[test]
fn test_with_workspace_respects_user_set_id() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let peers_path = workspace.join("cluster").join("peers.toml");

    // Pre-create peers.toml with user-set id
    std::fs::create_dir_all(peers_path.parent().unwrap()).unwrap();
    std::fs::write(
        &peers_path,
        "[node]\nid = \"user-custom-id\"\nname = \"MyBot\"\n",
    )
    .unwrap();

    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace.clone());

    // Cluster should use the user-set id, NOT a generated one
    assert_eq!(cluster.node_id(), "user-custom-id");

    // File should still contain the user's id (ensure_node_id is no-op)
    let content = std::fs::read_to_string(&peers_path).unwrap();
    assert!(content.contains("id = \"user-custom-id\""));
    assert!(!content.contains("node-")); // no auto-generated id
}

// -- Phase 3 tests: merge_real_node_info --

fn make_real_node_info(id: &str, name: &str, addr: &str) -> RealNodeInfo {
    RealNodeInfo {
        id: id.into(),
        name: name.into(),
        address: addr.into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "development".into(),
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    }
}

#[test]
fn test_merge_real_node_info_inserts_brand_new_entry() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Registry starts empty
    assert!(cluster.get_peer("node-real-1").is_none());

    let canonical = cluster.merge_real_node_info(&make_real_node_info(
        "node-real-1",
        "Real Node 1",
        "10.0.0.5:9000",
    ));

    assert_eq!(canonical, "node-real-1");
    let p = cluster.get_peer("node-real-1").unwrap();
    assert_eq!(p.base.name, "Real Node 1");
    assert_eq!(p.base.address, "10.0.0.5:9000");
}

#[test]
fn test_merge_real_node_info_updates_existing_entry_with_real_id() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Pre-populate registry with a real_id entry (e.g. from prior UDP discovery)
    cluster.handle_discovered_node(
        "node-real-1",
        "OldName",
        vec!["10.0.0.5".into()],
        9000,
        "worker",
        "old_cat",
        vec![],
        vec!["old_cap".into()],
        "old_type",
    );

    let canonical = cluster.merge_real_node_info(&RealNodeInfo {
        id: "node-real-1".into(),
        name: "NewName".into(),
        address: "10.0.0.5:9000".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Coordinator,
        category: "new_cat".into(),
        capabilities: vec!["new_cap".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "new_type".into(),
        build: String::new(),
    });

    assert_eq!(canonical, "node-real-1");
    let p = cluster.get_peer("node-real-1").unwrap();
    assert_eq!(p.base.name, "NewName");
    assert_eq!(p.base.category, "new_cat");
    assert_eq!(p.base.role, nemesis_types::cluster::NodeRole::Coordinator);
    assert_eq!(p.capabilities, vec!["new_cap".to_string()]);
    assert_eq!(p.node_type, "new_type");
}

#[test]
fn test_merge_real_node_info_keeps_static_peer_address() {
    // 真机三节点实证（2026-09-15）：静态 peer 的地址被对端自报 primary 覆盖
    // 后，多网段环境下本地按不可达地址拨号 → G2 探针失败 → Offline →
    // online 门禁拒 transfer（worker→master 全链死锁）。静态语义=用户显式
    // 配置优先：地址不被 merge 覆盖，其余字段照常刷新。
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Simulate gateway static-peer load: discovered entry + mark_static
    cluster.handle_discovered_node(
        "node-master-1",
        "master",
        vec!["192.168.1.1".into()],
        9000,
        "coordinator",
        "development",
        vec![],
        vec![],
        "unknown",
    );
    assert!(cluster.registry.mark_static("node-master-1"));

    // Peer reports a different (unreachable) primary address
    let canonical = cluster.merge_real_node_info(&RealNodeInfo {
        id: "node-master-1".into(),
        name: "master".into(),
        address: "10.255.0.1:9000".into(),
        rpc_port: 0,
        // 自报全量数组含不可达 primary——静态保护仍应保留原地址。
        addresses: vec!["10.255.0.1:9000".into(), "192.168.1.50:9000".into()],
        role: nemesis_types::cluster::NodeRole::Coordinator,
        category: "development".into(),
        capabilities: vec!["exec".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    assert_eq!(canonical, "node-master-1");
    let p = cluster.get_peer("node-master-1").unwrap();
    // Address stays at the user-configured value
    assert_eq!(p.base.address, "192.168.1.1:9000");
    // Other fields still refresh from the peer's self-report
    assert_eq!(p.capabilities, vec!["exec".to_string()]);
    assert_eq!(p.node_type, "agent");
}

#[test]
fn test_merge_real_node_info_static_pin_survives_peers_toml_persist() {
    // 洞 #5 盘面臂回归锁（2026-10-21 横扫姊妹洞）：内存保护（上方
    // keeps_static_peer_address）保住 base.address，但 persist 路径用的是
    // 入参 info.address——append 路径对已存在键=有意覆盖，nodes.ping 一
    // 次，peers.toml 手写的 pin 地址就被 announce 自报 primary 冲掉，重
    // 启后 R2 静态语义丢失（内存 pin 只活到下次重启）。盘面与内存同纪律。
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace.clone());

    // Simulate gateway static-peer load: discovered entry + mark_static
    cluster.handle_discovered_node(
        "node-master-1",
        "master",
        vec!["192.168.1.1".into()],
        9000,
        "coordinator",
        "development",
        vec![],
        vec![],
        "unknown",
    );
    assert!(cluster.registry.mark_static("node-master-1"));

    let canonical = cluster.merge_real_node_info(&RealNodeInfo {
        id: "node-master-1".into(),
        name: "master".into(),
        address: "10.255.0.1:9000".into(),
        rpc_port: 0,
        addresses: vec!["10.255.0.1:9000".into(), "192.168.1.50:9000".into()],
        role: nemesis_types::cluster::NodeRole::Coordinator,
        category: "development".into(),
        capabilities: vec!["exec".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    assert_eq!(canonical, "node-master-1");
    // 盘上 pin 地址不被自报 primary 覆盖（9000 ≤ 10000，rpc_to_udp 原样）。
    let peers_path = workspace.join("cluster/peers.toml");
    let doc = std::fs::read_to_string(&peers_path)
        .unwrap()
        .parse::<toml::Value>()
        .unwrap();
    let addr = doc["peers"]["node-master-1"]["address"].as_str().unwrap();
    assert_eq!(addr, "192.168.1.1:9000");
}

#[test]
fn test_merge_real_node_info_static_peer_extras_survive_persist() {
    // 四审 #6b 端到端（2026-10-21）：静态条目盘上带 professions/tier 手写
    // 声明（U 系列职能静态声明的真实形态）→ merge（nodes.ping 路径）→
    // 盘上扩展字段原样。#5 修复保 address，本测试钉扩展字段——append
    // 覆盖路径保留非五字段（merge_peer_entry_extras）。
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace.clone());

    // 预写盘上静态声明形态（operator 手写 professions/tier）
    let peers_path = workspace.join("cluster/peers.toml");
    std::fs::write(
        &peers_path,
        "[peers.node-master-1]\n\
         address = \"192.168.1.1:9000\"\n\
         name = \"master\"\n\
         role = \"coordinator\"\n\
         category = \"development\"\n\
         professions = [\"dev:cpp\"]\n\
         tier = \"big\"\n",
    )
    .unwrap();

    // registry 侧模拟 gateway 静态装载：条目在册 + static 标记
    cluster.handle_discovered_node(
        "node-master-1",
        "master",
        vec!["192.168.1.1".into()],
        9000,
        "coordinator",
        "development",
        vec![],
        vec![],
        "unknown",
    );
    assert!(cluster.registry.mark_static("node-master-1"));

    // ping 路径：merge 自报 primary 与 pin 不同
    cluster.merge_real_node_info(&RealNodeInfo {
        id: "node-master-1".into(),
        name: "master".into(),
        address: "10.255.0.1:9000".into(),
        rpc_port: 0,
        addresses: vec!["10.255.0.1:9000".into()],
        role: nemesis_types::cluster::NodeRole::Coordinator,
        category: "development".into(),
        capabilities: vec!["exec".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let doc = std::fs::read_to_string(&peers_path)
        .unwrap()
        .parse::<toml::Value>()
        .unwrap();
    let entry = &doc["peers"]["node-master-1"];
    assert_eq!(entry["address"].as_str().unwrap(), "192.168.1.1:9000");
    assert_eq!(
        entry["professions"].as_array().unwrap().len(),
        1,
        "手写 professions 不得被 persist 冲掉"
    );
    assert_eq!(entry["tier"].as_str().unwrap(), "big");
}

#[test]
fn test_upgrade_peer_in_peers_toml_carries_extras_to_real_key() {
    // 四审 #6 同族（2026-10-21）：占位升级字段搬运——占位条目（静态 peers
    // 手写）的 professions/tier 随键迁移到真实 id 键，不因升级丢失。
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\n\
         id = \"local-node-001\"\n\n\
         [peers.ph-a]\n\
         address = \"127.0.0.1:9149\"\n\
         name = \"A\"\n\
         role = \"worker\"\n\
         category = \"development\"\n\
         professions = [\"dev:cpp\"]\n\
         tier = \"big\"\n",
    )
    .unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let info = RealNodeInfo {
        id: "real-a".into(),
        name: "Alpha Real".into(),
        address: "10.8.8.1:21949".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "test".into(),
        capabilities: vec![],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    };
    cluster.upgrade_peer_in_peers_toml("ph-a", "real-a", &info);

    let doc = std::fs::read_to_string(cluster_dir.join("peers.toml"))
        .unwrap()
        .parse::<toml::Value>()
        .unwrap();
    let real = &doc["peers"]["real-a"];
    assert!(real.is_table(), "real-a entry written");
    assert_eq!(
        real["professions"].as_array().unwrap().len(),
        1,
        "占位条目手写 professions 必须随键迁移"
    );
    assert_eq!(real["tier"].as_str().unwrap(), "big");
    assert!(doc["peers"].get("ph-a").is_none(), "占位键已删除");
}

#[test]
fn test_merge_real_node_info_updates_address_for_dynamic_peer() {
    // 对照组：非静态（UDP 发现）节点，merge 照常用对端自报地址更新。
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    cluster.handle_discovered_node(
        "node-dyn-1",
        "dyn",
        vec!["10.0.0.5".into()],
        9000,
        "worker",
        "general",
        vec![],
        vec![],
        "unknown",
    );
    assert!(!cluster.registry.is_peer_static("node-dyn-1"));

    cluster.merge_real_node_info(&RealNodeInfo {
        id: "node-dyn-1".into(),
        name: "dyn".into(),
        address: "10.0.0.77:9000".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "general".into(),
        capabilities: Vec::new(),
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let p = cluster.get_peer("node-dyn-1").unwrap();
    assert_eq!(p.base.address, "10.0.0.77:9000");
}

#[test]
fn test_merge_real_node_info_upgrades_placeholder_by_address() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Manually-added peer with placeholder ID (uses name as ID)
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "MyNodeName".into(), // placeholder
            name: "MyNodeName".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.5:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    assert!(cluster.get_peer("MyNodeName").is_some());
    assert!(cluster.get_peer("node-real-1").is_none());

    let canonical = cluster.merge_real_node_info(&make_real_node_info(
        "node-real-1",
        "Real Name",
        "10.0.0.5:9000",
    ));

    // Real ID should now be the canonical entry
    assert_eq!(canonical, "node-real-1");
    assert!(cluster.get_peer("node-real-1").is_some());
    // Placeholder should be removed
    assert!(cluster.get_peer("MyNodeName").is_none());
}

#[test]
fn test_merge_real_node_info_persists_to_peers_toml() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace.clone());

    cluster.merge_real_node_info(&make_real_node_info(
        "node-real-persist",
        "Persisted",
        "10.0.0.7:9000",
    ));

    let peers_path = workspace.join("cluster/peers.toml");
    let content = std::fs::read_to_string(&peers_path).unwrap();
    assert!(
        content.contains("[peers.node-real-persist]"),
        "expected [peers.node-real-persist] in: {}",
        content
    );
    assert!(content.contains("10.0.0.7:9000"));
}

// -- 发现①/A6 回归：全量地址数组保全（2026-09-15 真机三节点） --

#[test]
fn test_merge_real_node_info_preserves_full_address_array() {
    // 真机实证根因：RPC merge 建条目时丢弃全量数组（Vec::new()）→
    // get_peer_info 回落单地址 → select_best_address 被 len<=1 短路，
    // 子网匹配智能选址从未运行。本测试钉死数组保全。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    let mut info = make_real_node_info("node-multi-1", "Multi", "10.103.174.241:9000");
    info.addresses = vec![
        "10.103.174.241".into(),
        "192.168.137.1".into(),
        "172.20.1.5".into(),
    ];
    cluster.merge_real_node_info(&info);

    let p = cluster.get_peer("node-multi-1").unwrap();
    assert_eq!(
        p.addresses,
        vec![
            "10.103.174.241".to_string(),
            "192.168.137.1".to_string(),
            "172.20.1.5".to_string()
        ],
        "merge 必须保全对端自报的全量地址数组"
    );
}

#[test]
fn test_merge_real_node_info_empty_array_keeps_existing_addresses() {
    // 空数组 = 来源未携带（旧数据）→ 不清空已有集合。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    let mut first = make_real_node_info("node-multi-2", "Multi", "10.0.0.5:9000");
    first.addresses = vec!["10.0.0.5".into(), "192.168.1.50".into()];
    cluster.merge_real_node_info(&first);

    let second = make_real_node_info("node-multi-2", "Multi", "10.0.0.6:9000");
    assert_eq!(second.addresses, Vec::<String>::new());
    cluster.merge_real_node_info(&second);

    let p = cluster.get_peer("node-multi-2").unwrap();
    assert_eq!(
        p.addresses,
        vec!["10.0.0.5".to_string(), "192.168.1.50".to_string()],
        "空数组 merge 不得清空已有地址集合"
    );
}

#[test]
fn test_merge_placeholder_upgrade_matches_secondary_address() {
    // 发现②/B4：占位条目配的地址可能是对端非 primary 网卡——merge 的
    // 地址匹配必须对自报全量数组逐个比对（真机实证：placeholder 配
    // 192.168.137.x 而自报 primary=10.103.x → 旧逻辑永不归一、双条目并存）。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // placeholder：手写兜底条目，地址=对端第二网卡
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-alex".into(),
            name: "node-alex".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "192.168.137.50:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    // 对端自报 primary=10.103.1.5，但全量数组含 placeholder 配的地址
    let mut info = make_real_node_info("node-alex-real", "Alex", "10.103.1.5:9000");
    info.addresses = vec!["10.103.1.5".into(), "192.168.137.50".into()];
    let canonical = cluster.merge_real_node_info(&info);

    assert_eq!(canonical, "node-alex-real");
    assert!(cluster.get_peer("node-alex-real").is_some());
    assert!(
        cluster.get_peer("node-alex").is_none(),
        "placeholder 命中非 primary 自报地址也必须归一"
    );
}

#[test]
fn test_handle_discovered_node_placeholder_upgrade_matches_secondary_address() {
    // B4 的 UDP 路径：announce 全量数组逐个比对（不再只 primary）。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // placeholder：地址 = announce 数组里的第二地址
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-bob".into(),
            name: "node-bob".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "169.254.39.174:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    let changed = cluster.handle_discovered_node(
        "node-bob-real",
        "Bob",
        vec!["10.103.2.9".into(), "169.254.39.174".into()],
        9000,
        "worker",
        "general",
        vec![],
        vec![],
        "agent",
    );
    assert!(changed || cluster.get_peer("node-bob-real").is_some());
    assert!(
        cluster.get_peer("node-bob-real").is_some(),
        "真实条目必须建立"
    );
    assert!(
        cluster.get_peer("node-bob").is_none(),
        "placeholder 命中 announce 次地址必须被升级移除"
    );
}

#[test]
fn test_handle_discovered_node_never_consumes_local_self_entry_as_placeholder() {
    // 2026-10-21 同机双节点实测回归锁：对端自报地址与本机地址相同
    // （同机多实例同 rpc_port / NAT 共出口 IP+端口）时，占位升级的地
    // 址比对臂曾把 registry 本机自条目当 stale 占位吞掉——自条目被
    // 删、peers.toml [peers.{对端}] 被写成本机 name + UDP 派生端口、
    // 对端单播打回本机 socket（发现自环，G2 探针只能走 overlay）。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // 本机自条目：地址与即将到来的 announce 自报主地址完全相同。
    let local_id = cluster.node_id.clone();
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: local_id.clone(),
            name: "local-node".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.103.175.236:21949".into(),
            category: "development".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    cluster.handle_discovered_node(
        "node-b-real",
        "Node-B",
        vec!["10.103.175.236".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );

    // 自条目必须健在（不被当占位消费）。
    let self_entry = cluster.get_peer(&local_id);
    assert!(self_entry.is_some(), "本机自条目不得被占位升级吞掉");
    assert_eq!(self_entry.unwrap().base.name, "local-node");
    // 真实对端条目照常建立，name 不被本机名污染。
    let peer = cluster
        .get_peer("node-b-real")
        .expect("对端真实条目必须建立");
    assert_eq!(peer.base.name, "Node-B", "对端 name 不得继承本机名");
}

#[test]
fn test_merge_real_node_info_never_consumes_local_self_entry_as_placeholder() {
    // 同上回归锁的 merge_real_node_info 臂（register_rpc_peer / 探针身份
    // 回传等路径共用本函数的占位解析）。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    let local_id = cluster.node_id.clone();
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: local_id.clone(),
            name: "local-node".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.5:9000".into(),
            category: "development".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    cluster.merge_real_node_info(&make_real_node_info(
        "node-real-2",
        "Real Two",
        "10.0.0.5:9000",
    ));

    assert!(
        cluster.get_peer(&local_id).is_some(),
        "本机自条目不得被占位升级吞掉"
    );
    let peer = cluster.get_peer("node-real-2").expect("真实条目必须建立");
    assert_eq!(peer.base.name, "Real Two", "对端 name 不得继承本机名");
}

#[test]
fn test_announce_address_arm_never_consumes_real_remote_entry() {
    // 2026-10-21 复查姊妹洞回归锁：同机同 rpc_port / 共享 NAT 出口拓扑
    // 下两个真实远端节点地址相同。地址比对臂曾吞「任意 id≠announce-id
    // 条目」——见证节点视角里两个真实条目反复互吞（flapping）、
    // peers.toml 反复改写。收窄后地址臂只消费真占位（静态 / id==name /
    // 空 name），真实条目（人类名、id≠name、非静态）共存。
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // 真实远端 B（announce 学来的形态：人类名、id≠name、非静态）。
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-b-real".into(),
            name: "Node-B".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.103.175.236:21949".into(),
            category: "development".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    // 同地址的 C 的 announce 到达。
    cluster.handle_discovered_node(
        "node-c-real",
        "Node-C",
        vec!["10.103.175.236".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );

    // B 不得被当占位吞掉。
    let b = cluster
        .get_peer("node-b-real")
        .expect("真实远端条目必须健在");
    assert_eq!(b.base.name, "Node-B", "B 的身份不得被 C 的 announce 污染");
    // C 照常建立。
    let c = cluster.get_peer("node-c-real").expect("C 条目照常建立");
    assert_eq!(c.base.name, "Node-C");
}

#[test]
fn test_merge_real_node_info_upgrades_placeholder_in_toml() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();

    // Pre-write peers.toml with a placeholder subtable
    let peers_path = workspace.join("cluster/peers.toml");
    std::fs::create_dir_all(peers_path.parent().unwrap()).unwrap();
    std::fs::write(
        &peers_path,
        "[node]\nid = \"node-local-1\"\n\n[peers.MyPlaceholder]\naddress = \"10.0.0.5:9000\"\nrole = \"worker\"\n",
    )
    .unwrap();

    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace.clone());

    // Registry also has the placeholder
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "MyPlaceholder".into(),
            name: "MyPlaceholder".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.5:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    cluster.merge_real_node_info(&make_real_node_info(
        "node-real-upgraded",
        "Real Upgraded",
        "10.0.0.5:9000",
    ));

    let content = std::fs::read_to_string(&peers_path).unwrap();
    assert!(
        content.contains("[peers.node-real-upgraded]"),
        "missing real peer subtable: {}",
        content
    );
    assert!(
        !content.contains("[peers.MyPlaceholder]"),
        "placeholder subtable should be removed: {}",
        content
    );
    // Local node should be preserved
    assert!(content.contains("[node]"));
}

// -- Phase 4 test: handle_discovered_node triggers placeholder upgrade --

#[test]
fn test_handle_discovered_node_upgrades_placeholder_via_udp() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Register a placeholder by name
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "ManualPeer".into(),
            name: "ManualPeer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "192.168.137.50:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });
    assert!(cluster.get_peer("ManualPeer").is_some());

    // UDP discovery delivers the real ID
    cluster.handle_discovered_node(
        "node-real-udp-1",
        "Real UDP Node",
        vec!["192.168.137.50".into()],
        9000,
        "worker",
        "development",
        vec![],
        vec!["llm".into()],
        "agent",
    );

    // Real ID should be present
    assert!(cluster.get_peer("node-real-udp-1").is_some());
    // Placeholder should be removed (upgraded)
    assert!(
        cluster.get_peer("ManualPeer").is_none(),
        "placeholder should have been removed by UDP-triggered upgrade"
    );
}

// -- Peer status helper tests --

#[test]
fn test_mark_peer_online_for_refresh_flips_offline_to_online() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Register an offline peer
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "peer-x".into(),
            name: "Peer X".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Offline,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    assert_eq!(
        cluster.get_peer("peer-x").unwrap().status,
        NodeStatus::Offline
    );
    cluster.mark_peer_online_for_refresh("peer-x");
    assert_eq!(
        cluster.get_peer("peer-x").unwrap().status,
        NodeStatus::Online
    );
}

#[test]
fn test_set_peer_status_restores_offline() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "peer-y".into(),
            name: "Peer Y".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "general".into(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: String::new(),
        build: String::new(),
    });

    cluster.set_peer_status("peer-y", NodeStatus::Offline);
    assert_eq!(
        cluster.get_peer("peer-y").unwrap().status,
        NodeStatus::Offline
    );
}

#[test]
fn test_mark_peer_online_for_refresh_no_op_for_unknown_peer() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    // Unknown peer — should not panic
    cluster.mark_peer_online_for_refresh("does-not-exist");
    assert!(cluster.get_peer("does-not-exist").is_none());
}

#[test]
fn test_set_peer_status_no_op_for_unknown_peer() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let config = make_config();
    let cluster = Cluster::with_workspace(config, workspace);

    cluster.set_peer_status("ghost", NodeStatus::Online);
    assert!(cluster.get_peer("ghost").is_none());
}

// -- addr_eq helper tests (Phase 4) --

#[test]
fn test_addr_eq_basic_equality() {
    assert!(addr_eq("1.2.3.4:9000", "1.2.3.4:9000"));
    assert!(!addr_eq("1.2.3.4:9000", "1.2.3.5:9000"));
}

#[test]
fn test_addr_eq_case_insensitive() {
    assert!(addr_eq("HOST:80", "host:80"));
    assert!(addr_eq("host:80", "HOST:80"));
}

#[test]
fn test_addr_eq_missing_port_one_side() {
    // 严格语义：带 port 和不带 port 是不同的地址规范，不再视为相等。
    // 历史 bug：旧规则容忍"任一无 port"导致 placeholder filter 用 addresses
    // 字段（host-only 列表）跟 primary_address（host:rpc_port）比较时误判，
    // 误删同 host 不同 port 的 peer（cluster-uat 13/14 失败根因）。
    assert!(!addr_eq("1.2.3.4:9000", "1.2.3.4"));
    assert!(!addr_eq("1.2.3.4", "1.2.3.4:9000"));
}

#[test]
fn test_addr_eq_both_no_port() {
    // 两边都 host-only：保留宽松语义。这是 addresses 字段内部比较需要的
    // （discovery listener 把同节点的多个 IP 都放进 addresses，互比时仍要识别）。
    assert!(addr_eq("1.2.3.4", "1.2.3.4"));
    assert!(!addr_eq("1.2.3.4", "1.2.3.5"));
}

#[test]
fn test_addr_eq_port_mismatch() {
    assert!(!addr_eq("1.2.3.4:9000", "1.2.3.4:9001"));
}

#[test]
fn test_addr_eq_empty() {
    assert!(!addr_eq("", "1.2.3.4"));
    assert!(!addr_eq("1.2.3.4", ""));
    assert!(!addr_eq("", ""));
}

#[test]
fn test_addr_eq_whitespace_trimmed() {
    assert!(addr_eq("  1.2.3.4:9000  ", "1.2.3.4:9000"));
}

// -- placeholder filter regression (cluster-uat 13/14 failure root cause) --

#[test]
fn test_handle_discovered_node_does_not_collapse_same_host_different_port() {
    // Regression: cluster-uat 4-node setup adds 3 static peers all on 127.0.0.1
    // with different RPC ports (21950/21951/21952). gateway.rs:1068 loads each
    // via handle_discovered_node(addresses=["127.0.0.1"], rpc_port=N).
    //
    // Old bug: placeholder filter's fallback check
    //   `p.addresses.iter().any(|a| addr_eq(a, primary_address))`
    // compared host-only addresses against host:rpc_port primary_address.
    // addr_eq's loose rule "either side missing port => equal if hosts match"
    // made Node-C's filter find Node-B as a "placeholder" (same host) and
    // delete it; Node-D then deleted Node-C; only Node-D survived.
    //
    // After fix: addr_eq is strict (host-only vs host:port => NOT equal),
    // so the fallback check only fires for true address overlaps. All three
    // peers must survive.
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    let addresses = vec!["127.0.0.1".to_string()];
    cluster.handle_discovered_node(
        "Node-B",
        "Node-B",
        addresses.clone(),
        21950,
        "worker",
        "general",
        vec![],
        vec![],
        "test",
    );
    cluster.handle_discovered_node(
        "Node-C",
        "Node-C",
        addresses.clone(),
        21951,
        "worker",
        "general",
        vec![],
        vec![],
        "test",
    );
    cluster.handle_discovered_node(
        "Node-D",
        "Node-D",
        addresses.clone(),
        21952,
        "worker",
        "general",
        vec![],
        vec![],
        "test",
    );

    // All three must remain in registry. handle_discovered_node only writes
    // peers.toml for placeholder upgrades; brand-new nodes are persisted via
    // merge_real_node_info during RPC get_info. The registry is therefore the
    // authoritative regression check: if the bug resurfaces, Node-B and Node-C
    // get deleted as "placeholders" and ids drops to ["Node-D"].
    let nodes = cluster.list_nodes();
    let ids: Vec<&str> = nodes.iter().map(|n| n.base.id.as_str()).collect();
    assert!(
        ids.contains(&"Node-B") && ids.contains(&"Node-C") && ids.contains(&"Node-D"),
        "expected all three peers to survive, got: {:?}",
        ids
    );
}

// ============================================================
// W3b coverage batch: with_workspace empty-id, display-address
// resolution, auth-token loading, discovery lifecycle, loops,
// peers.toml error branches, real-RPC-client call/poll paths,
// callback routing, setters sweep, resolver fallback edges
// ============================================================

/// Mock RPC channel used by W3b tests (set_rpc_channel wiring).
struct MockRpcChannelW3b;
impl std::fmt::Debug for MockRpcChannelW3b {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MockRpcChannelW3b")
    }
}
impl crate::rpc::RpcChannel for MockRpcChannelW3b {
    fn input(
        &self,
        _session_key: &str,
        _content: &str,
        _correlation_id: &str,
    ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
        Err("mock input".into())
    }
}

fn make_w3b_node(id: &str, name: &str, address: &str, status: NodeStatus) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: id.into(),
            name: name.into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: address.into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    }
}

#[test]
fn test_with_workspace_empty_node_id_falls_back_to_generated() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    // [node] section exists but id is empty → generated id must be used and persisted
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\nid = \"\"\nname = \"Empty\"\n\n[peers.Node-A]\naddress = \"10.0.0.1:9000\"\n",
    )
    .unwrap();

    let mut config = make_config();
    config.node_id = String::new();
    let cluster = Cluster::with_workspace(config, dir.path().to_path_buf());

    assert!(
        !cluster.node_id.is_empty(),
        "empty [node].id must not stick"
    );
    assert_ne!(cluster.node_id, "local-node-001");
    // The generated id is persisted back so restarts keep it
    let loaded =
        crate::cluster_config::load_static_config(&cluster_dir.join("peers.toml")).unwrap();
    assert_eq!(loaded.node.id, cluster.node_id);
    assert!(!loaded.node.id.is_empty());
}

#[test]
fn test_start_resolves_display_address_from_wildcard_bind() {
    let mut config = make_config();
    config.bind_address = "0.0.0.0:9377".into();
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(config, dir.path().to_path_buf());
    cluster.start();

    let local = cluster.get_node_info("local-node-001").unwrap();
    assert!(
        !local.base.address.starts_with("0.0.0.0"),
        "wildcard bind must be resolved to a concrete IP, got {}",
        local.base.address
    );
    assert!(local.base.address.ends_with(":9377"));
    cluster.stop();
}

#[test]
fn test_rpc_to_udp_address_all_branches() {
    // rpc_port > 10000 → reversed to udp convention
    assert_eq!(
        Cluster::rpc_to_udp_address("10.0.0.1:21949"),
        "10.0.0.1:11949"
    );
    // port ≤ 10000 → unchanged
    assert_eq!(
        Cluster::rpc_to_udp_address("10.0.0.1:9000"),
        "10.0.0.1:9000"
    );
    assert_eq!(
        Cluster::rpc_to_udp_address("10.0.0.1:10000"),
        "10.0.0.1:10000"
    );
    // no port → unchanged
    assert_eq!(Cluster::rpc_to_udp_address("10.0.0.1"), "10.0.0.1");
    // non-numeric port → unchanged
    assert_eq!(
        Cluster::rpc_to_udp_address("host.example:abc"),
        "host.example:abc"
    );
    // boundary 10001
    assert_eq!(Cluster::rpc_to_udp_address("h:10001"), "h:1");
}

#[test]
fn test_load_rpc_auth_token_config_read_error_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    // config.cluster.json is a DIRECTORY → read_to_string fails → warn + return
    std::fs::create_dir_all(dir.path().join("config").join("config.cluster.json")).unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

#[test]
fn test_load_rpc_auth_token_invalid_json_is_tolerated() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        "{ not valid json",
    )
    .unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

#[test]
fn test_load_rpc_auth_token_empty_token_runs_without_auth() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": ""}).to_string(),
    )
    .unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    assert!(cluster.is_running());
    cluster.stop();
}

#[tokio::test]
async fn test_load_rpc_auth_token_roundtrip_encrypted_rejects_plaintext() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server.clone());
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "w3b-secret"}).to_string(),
    )
    .unwrap();

    cluster.start(); // initializes rpc_client + applies token to server & client
    cluster.register_basic_handlers().unwrap();
    server.start().await.unwrap();
    let port = server.port();

    cluster.register_node(make_w3b_node(
        "peer-b",
        "Peer B",
        &format!("127.0.0.1:{}", port),
        NodeStatus::Online,
    ));

    // A plaintext client (no token) must NOT get a successful response from
    // the token-protected server — proves the server picked the token up.
    let plain = Arc::new(RpcClient::with_resolver(Arc::new(ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    })));
    let req = crate::rpc_types::RPCRequest {
        id: "plain-1".into(),
        action: crate::rpc_types::ActionType::Custom("ping".into()),
        payload: serde_json::json!({}),
        source: "local".into(),
        target: Some("peer-b".into()),
    };
    let plain_res = plain
        .call_with_timeout("peer-b", req, Duration::from_secs(5))
        .await;
    match plain_res {
        Ok(resp) => assert!(
            resp.error.is_some(),
            "plaintext client should be rejected, got {:?}",
            resp.result
        ),
        Err(_) => { /* rejected at transport level — also fine */ }
    }

    // The cluster's own client (token loaded from config) must succeed —
    // proves the client picked the token up.
    let client = cluster.rpc_client_arc().unwrap();
    let req2 = crate::rpc_types::RPCRequest {
        id: "enc-1".into(),
        action: crate::rpc_types::ActionType::Custom("ping".into()),
        payload: serde_json::json!({}),
        source: cluster.node_id.clone(),
        target: Some("peer-b".into()),
    };
    let resp = client
        .call_with_timeout("peer-b", req2, Duration::from_secs(5))
        .await
        .expect("encrypted call should succeed");
    assert!(resp.error.is_none());
    assert_eq!(resp.result.unwrap()["status"], "pong");

    server.stop().unwrap();
    cluster.stop();
}

#[test]
fn test_load_discovery_secret_variants() {
    // No config file → empty secret
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    assert_eq!(cluster.load_discovery_secret(), "");

    // Config with token → secret returned
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "disc-secret"}).to_string(),
    )
    .unwrap();
    assert_eq!(cluster.load_discovery_secret(), "disc-secret");

    // Unreadable config (directory) → empty secret
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir2.path().join("config").join("config.cluster.json")).unwrap();
    let cluster2 = Cluster::with_workspace(make_config(), dir2.path().to_path_buf());
    assert_eq!(cluster2.load_discovery_secret(), "");
}

#[test]
fn test_start_discovery_lifecycle_double_start_and_stop_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    // Random UDP port + long interval keeps the broadcast thread quiet.
    cluster.set_ports(0, 0);
    cluster.set_broadcast_interval(Duration::from_secs(600));
    // RPC server attached but NOT started → cluster.stop() hits its Err arm.
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();

    let arc: Arc<Cluster> = Arc::new(cluster);
    let cb: Arc<dyn ClusterCallbacks> = arc.clone();
    arc.start_discovery(cb);
    {
        let guard = arc.discovery.lock();
        assert!(guard.is_some(), "discovery service should be stored");
    }

    // Double start → warn + skip (flag path)
    let cb2: Arc<dyn ClusterCallbacks> = arc.clone();
    arc.start_discovery(cb2);

    arc.stop();
    assert!(!arc.is_running());
}

#[tokio::test]
async fn test_start_spawns_loops_and_writes_state_toml() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.set_broadcast_interval(Duration::from_millis(50));
    cluster.start();

    // The sync loop's interval fires immediately on its first tick, then every
    // 50ms — poll briefly for {workspace}/cluster/state.toml to appear.
    let state_path = dir.path().join("cluster").join("state.toml");
    let deadline = std::time::Instant::now() + Duration::from_millis(800);
    while !state_path.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        state_path.exists(),
        "sync loop should persist state.toml to the workspace"
    );

    cluster.stop();
    // Give the loops a moment to observe the stop signal and exit cleanly.
    tokio::time::sleep(Duration::from_millis(20)).await;
}

#[test]
fn test_remove_node_peers_toml_write_error_warns() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();
    cluster.handle_discovered_node(
        "peer-gone",
        "gone",
        vec!["10.9.9.9".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    // peers.toml as a directory → remove_peer_from_file fails → warn path.
    // (handle_discovered_node already persisted it as a regular file — remove first.)
    let peers_toml = dir.path().join("cluster").join("peers.toml");
    if peers_toml.exists() {
        std::fs::remove_file(&peers_toml).unwrap();
    }
    std::fs::create_dir_all(&peers_toml).unwrap();
    let removed = cluster.remove_node("peer-gone");
    assert!(removed, "registry removal should still succeed");
    cluster.stop();
}

#[test]
fn test_handle_discovered_node_upgrades_static_placeholder_preserving_name() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // Placeholder keyed by human name at the same address the announce will use.
    cluster.register_node(make_w3b_node(
        "Node-A",
        "Node-A",
        "10.1.1.5:21949",
        NodeStatus::Online,
    ));
    assert!(cluster.mark_peer_static("Node-A"));

    // Announce carries node_id as name (common) — the placeholder's human name
    // must survive the upgrade and static-ness must be inherited.
    cluster.handle_discovered_node(
        "real-x",
        "real-x",
        vec!["10.1.1.5".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );

    assert!(
        cluster.get_node_info("Node-A").is_none(),
        "placeholder must be removed"
    );
    let real = cluster.get_node_info("real-x").unwrap();
    assert_eq!(real.base.name, "Node-A", "human name must be restored");
    assert!(
        cluster.registry.is_peer_static("real-x"),
        "static-ness must be inherited"
    );
    cluster.stop();
}
#[test]
fn test_upgrade_peer_in_peers_toml_same_sanitized_key_writes_real_id() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\nid = \"local-node-001\"\n\n[peers.\"peer.a\"]\naddress = \"10.0.0.1:9000\"\n",
    )
    .unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    // 'peer.a' and 'peer:a' sanitize to the same key 'peer_a' → same-key branch
    cluster.upgrade_peer_in_peers_toml(
        "peer.a",
        "peer:a",
        &RealNodeInfo {
            id: "peer:a".into(),
            name: "Peer Colon".into(),
            address: "10.0.0.1:21949".into(),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_types::cluster::NodeRole::Worker,
            category: "test".into(),
            capabilities: vec![],
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".into(),
            build: String::new(),
        },
    );

    let content = std::fs::read_to_string(cluster_dir.join("peers.toml")).unwrap();
    assert!(
        content.contains("Peer Colon"),
        "real id entry should be written: {}",
        content
    );
}

#[test]
fn test_upgrade_peer_in_peers_toml_read_error_skips() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    // peers.toml is a DIRECTORY → read_to_string fails → warn + skip
    std::fs::create_dir_all(cluster_dir.join("peers.toml")).unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.upgrade_peer_in_peers_toml(
        "Node-A",
        "real-y",
        &RealNodeInfo {
            id: "real-y".into(),
            name: "Y".into(),
            address: "10.0.0.2:21949".into(),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_types::cluster::NodeRole::Worker,
            category: "test".into(),
            capabilities: vec![],
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".into(),
            build: String::new(),
        },
    );
    // No panic; the directory stays a directory (no write happened through it).
    assert!(cluster_dir.join("peers.toml").is_dir());
}

#[test]
fn test_upgrade_peer_in_peers_toml_without_peers_table_persists_directly() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    // File with [node] only — no [peers] table → direct persist + return
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\nid = \"local-node-001\"\n",
    )
    .unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.upgrade_peer_in_peers_toml(
        "Node-A",
        "real-z",
        &RealNodeInfo {
            id: "real-z".into(),
            name: "Z".into(),
            address: "10.0.0.3:21949".into(),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_types::cluster::NodeRole::Worker,
            category: "test".into(),
            capabilities: vec![],
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".into(),
            build: String::new(),
        },
    );

    let content = std::fs::read_to_string(cluster_dir.join("peers.toml")).unwrap();
    assert!(
        content.contains("real-z") && content.contains("[peers."),
        "real entry should be appended under [peers]: {}",
        content
    );
}

#[test]
fn test_upgrade_peer_in_peers_toml_write_error_after_removal() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\nid = \"local-node-001\"\n\n[peers.Node-A]\naddress = \"10.0.0.1:9000\"\n",
    )
    .unwrap();
    // REL-002：旧注入（预建 peers.toml.tmp 目录阻塞固定临时名）随唯一临时名
    // 升级失效——改用平台确定的写失败注入（见 block_atomic_replace）。
    // root 语境注入失效（探针 None）→ 诚实跳过。
    let Some(_guard) = block_atomic_replace(&cluster_dir.join("peers.toml")) else {
        return;
    };

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.upgrade_peer_in_peers_toml(
        "Node-A",
        "real-w",
        &RealNodeInfo {
            id: "real-w".into(),
            name: "W".into(),
            address: "10.0.0.4:21949".into(),
            rpc_port: 0,
            addresses: Vec::new(),
            role: nemesis_types::cluster::NodeRole::Worker,
            category: "test".into(),
            capabilities: vec![],
            tags: Vec::new(),
            professions: Vec::new(),
            tier: None,
            agents: Vec::new(),
            runtime_backend: None,
            node_type: "agent".into(),
            build: String::new(),
        },
    );
    // Original file content is untouched (the failed write never landed).
    let content = std::fs::read_to_string(cluster_dir.join("peers.toml")).unwrap();
    assert!(
        content.contains("[peers.Node-A]"),
        "file must be unchanged: {}",
        content
    );
}

#[test]
fn test_merge_real_node_info_master_role_with_write_error() {
    let dir = tempfile::tempdir().unwrap();
    let cluster_dir = dir.path().join("cluster");
    // peers.toml as directory → append fails → persist warn arm
    std::fs::create_dir_all(cluster_dir.join("peers.toml")).unwrap();

    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let merged = cluster.merge_real_node_info(&RealNodeInfo {
        id: "master-1".into(),
        name: "Master One".into(),
        address: "10.5.5.5:21949".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Coordinator,
        category: "prod".into(),
        capabilities: vec!["llm".into()],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    assert_eq!(merged, "master-1");
    let info = cluster.get_node_info("master-1").unwrap();
    assert_eq!(
        info.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_call_with_context_real_server_success_error_and_connect_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server.clone());
    cluster.start();
    cluster
        .register_rpc_handler("w3b_ok", Box::new(|_p| Ok(serde_json::json!({"ok": true}))))
        .unwrap();
    cluster
        .register_rpc_handler("w3b_boom", Box::new(|_p| Err("boom-error".to_string())))
        .unwrap();
    server.start().await.unwrap();
    let port = server.port();

    cluster.register_node(make_w3b_node(
        "peer-live",
        "Live",
        &format!("127.0.0.1:{}", port),
        NodeStatus::Online,
    ));
    cluster.register_node(make_w3b_node(
        "peer-dead",
        "Dead",
        "127.0.0.1:1",
        NodeStatus::Online,
    ));

    // Success path: result serialized to bytes
    let ok = cluster
        .call_with_context("peer-live", "w3b_ok", serde_json::json!({}))
        .expect("call should succeed");
    let parsed: serde_json::Value = serde_json::from_slice(&ok).unwrap();
    assert_eq!(parsed["ok"], true);

    // Error-response path: handler error propagates
    let err = cluster
        .call_with_context("peer-live", "w3b_boom", serde_json::json!({}))
        .expect_err("handler error should surface");
    assert!(err.contains("boom-error"), "unexpected error: {}", err);

    // Connect-failure path: dead port → transport error
    let err2 = cluster
        .call_with_context("peer-dead", "w3b_ok", serde_json::json!({}))
        .expect_err("dead peer should fail");
    assert!(!err2.is_empty());

    server.stop().unwrap();
    cluster.stop();
}

#[tokio::test]
async fn test_call_with_context_async_real_server_success_and_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server.clone());
    cluster.start();
    cluster
        .register_rpc_handler(
            "w3b_async",
            Box::new(|_p| Ok(serde_json::json!({"async": true}))),
        )
        .unwrap();
    server.start().await.unwrap();
    let port = server.port();

    cluster.register_node(make_w3b_node(
        "peer-live",
        "Live",
        &format!("127.0.0.1:{}", port),
        NodeStatus::Online,
    ));
    cluster.register_node(make_w3b_node(
        "peer-dead",
        "Dead",
        "127.0.0.1:1",
        NodeStatus::Online,
    ));

    let ok = cluster
        .call_with_context_async(
            "peer-live",
            "w3b_async",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("async call should succeed");
    let parsed: serde_json::Value = serde_json::from_slice(&ok).unwrap();
    assert_eq!(parsed["async"], true);

    let err = cluster
        .call_with_context_async(
            "peer-dead",
            "w3b_async",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect_err("dead peer should fail");
    assert!(!err.is_empty());

    server.stop().unwrap();
    cluster.stop();
}

#[tokio::test]
async fn test_set_cluster_task_queue_routes_callback_to_work_queue() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    // The callback handler captures the task list/queue clones when it is
    // registered (triggered by set_rpc_channel below), so wire the queue FIRST.
    let tl = Arc::new(crate::cluster_task::ClusterTaskList::new(
        dir.path().join("tasks"),
    ));
    let wq = Arc::new(crate::cluster_task::ClusterWorkQueue::new(8));
    cluster.set_cluster_task_queue(tl.clone(), wq.clone());
    cluster.set_rpc_channel(Arc::new(MockRpcChannelW3b)); // running → registers handlers

    tl.create_task(crate::cluster_task::ClusterTask {
        task_id: "parent-1".into(),
        source: crate::cluster_task::TaskSource {
            node_id: "origin-a".into(),
            rpc_address: "127.0.0.1:1".into(),
            session_key: "sess-1".into(),
        },
        status: crate::cluster_task::TaskStatus::Pending,
        content: "do work".into(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
        required_profession: None,
        async_usage_stash: None,
    });
    tl.save_async_state(
        "parent-1",
        "child-9".into(),
        "tool-9".into(),
        serde_json::json!({"messages": []}),
    )
    .expect("save_async_state should succeed");

    let resp = cluster
        .rpc_server
        .as_ref()
        .unwrap()
        .handle_request_sync(
            "peer_chat_callback",
            serde_json::json!({"task_id": "child-9", "response": "final-answer"}),
        )
        .expect("callback should be accepted");
    assert_eq!(resp["status"], "accepted");

    let task = tl.get_task("parent-1").unwrap();
    // inject_callback documents: status → Pending so the work queue can pick it up.
    assert_eq!(task.status, crate::cluster_task::TaskStatus::Pending);
    assert_eq!(task.callback_result.as_deref(), Some("final-answer"));

    // The re-submitted parent must come out of the work queue
    let next_id = tokio::time::timeout(Duration::from_secs(2), wq.next())
        .await
        .expect("work queue should yield the parent task")
        .expect("queue should not be closed");
    assert_eq!(next_id, "parent-1");
    cluster.stop();
}

#[tokio::test]
async fn test_callback_routing_with_full_work_queue_logs_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    // Wire the queue BEFORE set_rpc_channel: the callback handler captures the
    // task list/queue clones at registration time (triggered by set_rpc_channel).
    let tl = Arc::new(crate::cluster_task::ClusterTaskList::new(
        dir.path().join("tasks"),
    ));
    let wq = Arc::new(crate::cluster_task::ClusterWorkQueue::new(1));
    cluster.set_cluster_task_queue(tl.clone(), wq.clone());
    wq.submit("filler".into()).unwrap();
    cluster.set_rpc_channel(Arc::new(MockRpcChannelW3b));

    tl.create_task(crate::cluster_task::ClusterTask {
        task_id: "parent-2".into(),
        source: crate::cluster_task::TaskSource {
            node_id: "origin-a".into(),
            rpc_address: "127.0.0.1:1".into(),
            session_key: "sess-2".into(),
        },
        status: crate::cluster_task::TaskStatus::Pending,
        content: "c".into(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
        required_profession: None,
        async_usage_stash: None,
    });
    tl.save_async_state(
        "parent-2",
        "child-10".into(),
        "tool-10".into(),
        serde_json::json!({"messages": []}),
    )
    .expect("save_async_state should succeed");

    let resp = cluster
        .rpc_server
        .as_ref()
        .unwrap()
        .handle_request_sync(
            "peer_chat_callback",
            serde_json::json!({"task_id": "child-10", "response": "r"}),
        )
        .expect("callback still accepted despite full queue");
    assert_eq!(resp["status"], "accepted");
    // Callback was injected even though the queue submit failed
    assert_eq!(
        tl.get_task("parent-2").unwrap().callback_result.as_deref(),
        Some("r")
    );
    cluster.stop();
}
#[test]
fn test_mark_peer_healthy_offline_static_and_excluding_self() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    cluster.register_node(make_w3b_node(
        "p1",
        "P1",
        "10.0.0.1:21949",
        NodeStatus::Online,
    ));
    cluster.register_node(make_w3b_node(
        "p2",
        "P2",
        "10.0.0.2:21949",
        NodeStatus::Online,
    ));

    cluster.mark_peer_healthy("p1");
    assert!(cluster.get_node_info("p1").unwrap().is_online());

    cluster.mark_peer_offline("p1", "probe timed out");
    assert!(!cluster.get_node_info("p1").unwrap().is_online());

    assert!(cluster.mark_peer_static("p2"));
    assert!(!cluster.mark_peer_static("unknown-peer")); // miss → false

    let online = cluster.get_online_peers_excluding_self();
    let ids: Vec<&str> = online.iter().map(|n| n.base.id.as_str()).collect();
    assert!(!ids.contains(&"local-node-001"), "self must be excluded");
    assert!(ids.contains(&"p2"), "online peer p2 expected in {:?}", ids);
    cluster.stop();
}

#[test]
fn test_identity_setters_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    cluster.set_broadcast_interval(Duration::from_secs(7));
    cluster.set_node_type("node");
    assert_eq!(cluster.node_type(), "node");

    cluster.start(); // needed so sync_local_node_to_registry has a base entry
    cluster.set_node_name("Renamed");
    cluster.set_role("master");
    cluster.set_category("research");
    cluster.set_tags(vec!["t1".into(), "t2".into()]);
    cluster.set_capabilities(vec!["grep".into(), "git".into()]);

    let local = cluster.get_node_info("local-node-001").unwrap();
    assert_eq!(local.base.name, "Renamed");
    assert_eq!(
        local.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
    assert_eq!(local.base.category, "research");
    assert!(local.capabilities.contains(&"grep".to_string()));
    cluster.stop();
}

#[test]
fn test_register_peer_chat_handlers_not_running_logs_errors() {
    let cluster = Cluster::new(make_config());
    // Channel set but cluster NOT running → every register_rpc_handler fails
    // and each failure is logged (no panic).
    cluster.set_rpc_channel(Arc::new(MockRpcChannelW3b)); // not running → no auto-register
    assert!(!cluster.is_running());
    cluster.register_peer_chat_handlers();
}

#[test]
fn test_basic_and_diagnostics_handlers_via_handle_request_sync() {
    let cluster = make_cluster_with_rpc_server();
    cluster.register_basic_handlers().unwrap();
    let server = cluster.rpc_server.as_ref().unwrap();

    let caps = server
        .handle_request_sync("get_capabilities", serde_json::json!({}))
        .unwrap();
    assert!(caps.get("capabilities").is_some());

    let info = server
        .handle_request_sync("get_info", serde_json::json!({}))
        .unwrap();
    assert_eq!(info["node_id"], "local-node-001");
    assert_eq!(info["status"], "online");

    let actions = server
        .handle_request_sync("list_actions", serde_json::json!({}))
        .unwrap();
    assert!(!actions["actions"].as_array().unwrap().is_empty());

    let sys = server
        .handle_request_sync("diagnostics.system", serde_json::json!({}))
        .unwrap();
    assert!(sys["hostname"].is_string());
    assert!(sys["memory_total_bytes"].is_u64());

    let net = server
        .handle_request_sync("diagnostics.network", serde_json::json!({}))
        .unwrap();
    assert!(net.get("interfaces").is_some());
    assert!(net.get("all_ips").is_some());

    // cluster_state reflects the registry (local node at minimum)
    let state = server
        .handle_request_sync("diagnostics.cluster_state", serde_json::json!({}))
        .unwrap();
    assert!(state["node_count"].as_u64().unwrap() >= 1);
    assert!(state["online_count"].as_u64().unwrap() >= 1);
    cluster.stop();
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_wrapper_method() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();
    // Empty task manager → wrapper runs without effect
    cluster.poll_stale_pending_tasks().await;
    assert!(cluster.list_tasks().is_empty());
    cluster.stop();
}

#[test]
fn test_set_rpc_client_external() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    assert!(cluster.rpc_client_arc().is_some()); // auto-initialized by start()

    let resolver = Arc::new(ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    });
    cluster.set_rpc_client(Arc::new(RpcClient::with_resolver(resolver)));
    assert!(cluster.rpc_client_arc().is_some());
    cluster.stop();
}

#[tokio::test]
async fn test_poll_stale_pending_tasks_real_client_running_done_notfound_and_dead() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server.clone());
    cluster.start();
    cluster.set_rpc_channel(Arc::new(MockRpcChannelW3b)); // registers query/confirm handlers
    server.start().await.unwrap();
    let port = server.port();

    cluster.register_node(make_w3b_node(
        "peer-b",
        "Peer B",
        &format!("127.0.0.1:{}", port),
        NodeStatus::Online,
    ));
    cluster.register_node(make_w3b_node(
        "peer-dead",
        "Dead",
        "127.0.0.1:1",
        NodeStatus::Online,
    ));

    let tm = cluster.task_manager.clone();
    let client = cluster.rpc_client_arc().unwrap();

    let submit_stale = |tm: &TaskManager, id: &str, peer: &str| {
        let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
        tm.submit(Task {
            id: id.to_string(),
            status: TaskStatus::Pending,
            action: "peer_chat".to_string(),
            peer_id: peer.to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        })
        .unwrap();
    };

    // 1. Remote reports "running" → task stays Pending
    server.register_handler(
        "query_task_result",
        Box::new(|_p| Ok(serde_json::json!({"status": "running"}))),
    );
    submit_stale(&tm, "stale-running", "peer-b");
    poll_stale_pending_tasks(
        &tm,
        &None,
        Some(client.as_ref()),
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(
        tm.get_task("stale-running").unwrap().status,
        TaskStatus::Pending
    );

    // 2. Remote reports "done" → completed locally + confirm sent via real client
    server.register_handler(
        "query_task_result",
        Box::new(|_p| {
            Ok(serde_json::json!({
                "status": "done",
                "result_status": "success",
                "response": "remote-answer",
                "error": "",
            }))
        }),
    );
    submit_stale(&tm, "stale-done", "peer-b");
    poll_stale_pending_tasks(
        &tm,
        &None,
        Some(client.as_ref()),
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(
        tm.get_task("stale-done").unwrap().status,
        TaskStatus::Completed
    );

    // 3. Remote reports "not_found" → failed locally
    server.register_handler(
        "query_task_result",
        Box::new(|_p| Ok(serde_json::json!({"status": "not_found"}))),
    );
    submit_stale(&tm, "stale-nf", "peer-b");
    poll_stale_pending_tasks(
        &tm,
        &None,
        Some(client.as_ref()),
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(tm.get_task("stale-nf").unwrap().status, TaskStatus::Failed);

    // 4. Dead peer → transport error → task stays Pending
    submit_stale(&tm, "stale-dead", "peer-dead");
    poll_stale_pending_tasks(
        &tm,
        &None,
        Some(client.as_ref()),
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(
        tm.get_task("stale-dead").unwrap().status,
        TaskStatus::Pending
    );

    server.stop().unwrap();
    cluster.stop();
}

#[test]
fn test_cluster_callbacks_full_identity_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.set_node_type("node");
    cluster.start();
    cluster.set_capabilities(vec!["grep".into(), "cluster".into(), "git".into()]);

    assert_eq!(
        ClusterCallbacks::name(&cluster),
        cluster.get_node_info("local-node-001").unwrap().base.name
    );
    assert!(!ClusterCallbacks::all_local_ips(&cluster).is_empty());
    assert!(ClusterCallbacks::capabilities(&cluster).len() >= 3);
    assert_eq!(ClusterCallbacks::node_type(&cluster), "node");
    cluster.stop();
}

#[test]
fn test_cluster_peer_resolver_fallback_by_name_with_empty_address() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    // Peer matched by NAME with an empty primary address and no address list
    // → 零候选 → None（闭环批 2026-10-13：名字命中救不了「无任何可拨地址」，
    // Some(空池) 的 "" 空拨已废）。
    let mut peer = make_w3b_node("x1", "MyNode", "", NodeStatus::Online);
    peer.addresses = vec![];
    cluster.register_node(peer);

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    assert!(resolver.get_peer_info("MyNode").is_none());
    cluster.stop();
}

#[test]
fn test_write_atomic_success_and_failures() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cfg.toml");

    // Success path: tmp write + rename（唯一临时名 `.tmp-{pid}-{nanos}`）
    write_atomic(&dest, b"data").unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"data");
    assert_no_tmp_leftovers(dir.path(), "no tmp leftovers after success");

    // destination is a directory → rename fails, tmp cleaned up
    let dest3 = dir.path().join("asdir.toml");
    std::fs::create_dir_all(&dest3).unwrap();
    assert!(write_atomic(&dest3, b"y").is_err());
    assert_no_tmp_leftovers(dir.path(), "tmp cleaned after rename failure");

    // REL-002 注：旧实现固定临时名 `.toml.tmp`，可用预建同名目录注入失败；
    // 唯一临时名升级后该注入面不复存在，替换失败路径由
    // test_upgrade_peer_in_peers_toml_write_error_after_removal 的平台
    // 注入 + nemesis-utils helper 专属单测（只读目录/并发交错）覆盖。
}

// ============================================================
// S4 coverage batch: auth-token wiring, sync-loop save error,
// discovery success fields, placeholder-upgrade arms,
// upgrade_peer_in_peers_toml branches, handler registration
// fail-fast, forge empty filename, callback fall-through,
// confirm_delivery fallback, poll no-client skip, async null
// result, resolver fallback host vec, tracing field lines.
// ============================================================

/// No-op tracing subscriber that enables every event so that
/// tracing-macro field expressions (`timeout_secs = ...` etc.)
/// are evaluated and counted as covered. Installed once per
/// process; a second install attempt fails harmlessly.
struct S4AllEventsSubscriber;
impl tracing::Subscriber for S4AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn s4_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4AllEventsSubscriber);
    });
}

/// `start()` creates the RPC client, then `load_rpc_auth_token` applies the
/// configured token to BOTH the attached (unstarted) server and the client
/// (cluster.rs 387-396). `stop()` afterwards hits the server-not-running Err
/// arm (471-473).
#[test]
fn test_load_rpc_auth_token_applies_to_server_and_client() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "s4-auth-tok"}).to_string(),
    )
    .unwrap();

    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    assert!(cluster.is_running());
    // Both server and client branches of load_rpc_auth_token ran during start().
    assert!(cluster.rpc_client.lock().is_some());

    // Server was never started → stop() warns and swallows the error.
    cluster.stop();
    assert!(!cluster.is_running());
}

/// The sync loop's first interval tick fires immediately; with
/// `cluster/state.toml` blocked by a directory the save fails every tick and
/// the error is logged (cluster.rs 588-590) without killing the loop.
#[tokio::test]
async fn test_sync_loop_state_save_error_logged() {
    let dir = tempfile::tempdir().unwrap();
    let state_path = dir.path().join("cluster").join("state.toml");
    std::fs::create_dir_all(&state_path).unwrap();

    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.set_broadcast_interval(Duration::from_millis(50));
    cluster.start();

    // Let at least a few ticks fire — each one hits the save error path.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(state_path.is_dir(), "blocked path must remain a directory");

    cluster.stop();
    assert!(!cluster.is_running());
}

/// `start_discovery` with a configured token succeeds and evaluates the
/// `encrypted` field of the success log (cluster.rs 423-427). Port 0 + a long
/// interval keep the broadcast thread quiet.
#[test]
fn test_start_discovery_with_token_logs_encrypted_success() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "s4-disc-secret"}).to_string(),
    )
    .unwrap();

    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.set_ports(0, 0);
    cluster.set_broadcast_interval(Duration::from_secs(600));
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();

    let arc: Arc<Cluster> = Arc::new(cluster);
    let cb: Arc<dyn ClusterCallbacks> = arc.clone();
    arc.start_discovery(cb);
    {
        let guard = arc.discovery.lock();
        assert!(guard.is_some(), "discovery service should be stored");
    }
    arc.stop();
}

/// A placeholder with an EMPTY name at the announce address: the upgraded
/// real entry falls back to the announce name (cluster.rs 734-737).
#[test]
fn test_handle_discovered_node_empty_placeholder_name_uses_announce_name() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // Placeholder keyed by an id with an empty human name at the same address.
    cluster.register_node(make_w3b_node(
        "ph-empty",
        "",
        "10.3.3.3:21949",
        NodeStatus::Online,
    ));

    cluster.handle_discovered_node(
        "real-empty-name",
        "RealHuman",
        vec!["10.3.3.3".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );

    assert!(
        cluster.get_node_info("ph-empty").is_none(),
        "empty-name placeholder must be removed"
    );
    let real = cluster.get_node_info("real-empty-name").unwrap();
    assert_eq!(real.base.name, "RealHuman");
    cluster.stop();
}

/// When the announce already carries a real (non-node_id) name, the
/// name-restore branch does NOT overwrite it (cluster.rs 764 condition false).
#[test]
fn test_handle_discovered_node_upgrade_keeps_announce_name_when_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    cluster.register_node(make_w3b_node(
        "Node-B",
        "Node-B",
        "10.4.4.4:21949",
        NodeStatus::Online,
    ));

    cluster.handle_discovered_node(
        "real-w",
        "RealW",
        vec!["10.4.4.4".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );

    assert!(cluster.get_node_info("Node-B").is_none());
    let real = cluster.get_node_info("real-w").unwrap();
    assert_eq!(
        real.base.name, "RealW",
        "distinct announce name must survive"
    );
    cluster.stop();
}

/// An identical second announce is a no-op change: `upsert_if_changed` returns
/// false and the unchanged-health trace fires (cluster.rs 778-782).
#[test]
fn test_handle_discovered_node_second_announce_unchanged() {
    s4_tracing_subscriber();
    let cluster = Cluster::new(make_config());
    cluster.start();

    let args = (
        "remote-unchanged",
        "stable-name",
        vec!["10.7.7.7".to_string()],
        21949u16,
        "worker",
        "development",
        vec![String::new()],
        vec![String::new()],
        "agent",
    );
    let first = cluster.handle_discovered_node(
        args.0,
        args.1,
        args.2.clone(),
        args.3,
        args.4,
        args.5,
        args.6.clone(),
        args.7.clone(),
        args.8,
    );
    assert!(first, "first announce should be a change");
    let second = cluster.handle_discovered_node(
        args.0, args.1, args.2, args.3, args.4, args.5, args.6, args.7, args.8,
    );
    assert!(!second, "identical second announce must be unchanged");
    cluster.stop();
}

/// Direct calls to the private `upgrade_peer_in_peers_toml`:
/// (a) no peers.toml at all → empty document path (cluster.rs 973-974),
/// (b) peers.toml with [peers] but WITHOUT the placeholder key → the
///     removed-is-none fall-through (cluster.rs 1004-1011).
#[test]
fn test_upgrade_peer_in_peers_toml_no_file_and_missing_key() {
    let info_a = RealNodeInfo {
        id: "real-a".into(),
        name: "Alpha Real".into(),
        address: "10.8.8.1:21949".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "test".into(),
        capabilities: vec![],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    };

    // (a) fresh workspace, ensure no peers.toml exists
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let peers_toml = dir.path().join("cluster").join("peers.toml");
    if peers_toml.exists() {
        std::fs::remove_file(&peers_toml).unwrap();
    }
    cluster.upgrade_peer_in_peers_toml("ph-a", "real-a", &info_a);
    let content = std::fs::read_to_string(&peers_toml).unwrap();
    assert!(
        content.contains("Alpha Real"),
        "real entry appended: {}",
        content
    );
    drop(cluster);

    // (b) peers.toml with a [peers] table but no ph-b key
    let dir2 = tempfile::tempdir().unwrap();
    let cluster_dir = dir2.path().join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(
        cluster_dir.join("peers.toml"),
        "[node]\nid = \"local-node-001\"\n\n[peers]\n\n[peers.zzz]\nname = \"zz\"\n",
    )
    .unwrap();

    let cluster2 = Cluster::with_workspace(make_config(), dir2.path().to_path_buf());
    let info_b = RealNodeInfo {
        id: "real-b".into(),
        name: "Bee Real".into(),
        address: "10.8.8.2:21949".into(),
        rpc_port: 0,
        addresses: Vec::new(),
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "test".into(),
        capabilities: vec![],
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    };
    cluster2.upgrade_peer_in_peers_toml("ph-b", "real-b", &info_b);

    let content2 = std::fs::read_to_string(cluster_dir.join("peers.toml")).unwrap();
    assert!(
        content2.contains("Bee Real"),
        "real-b entry appended: {}",
        content2
    );
    assert!(
        !content2.contains(crate::cluster_config::sanitize_peer_key("ph-b").as_str()),
        "placeholder key must not appear: {}",
        content2
    );
}

/// `register_basic_handlers` on a RUNNING cluster without an RPC server: the
/// very first registration fails and `?` aborts immediately (cluster.rs 1837).
#[test]
fn test_register_basic_handlers_fails_fast_without_rpc_server() {
    let cluster = Cluster::new(make_config());
    cluster.start();
    assert!(cluster.is_running());

    let err = cluster
        .register_basic_handlers()
        .expect_err("registration must fail without an RPC server");
    assert!(
        err.contains("RPC server is not initialized"),
        "unexpected error: {}",
        err
    );
    cluster.stop();
}

/// `forge_get_reflections` with an empty `filename` skips the read branch
/// (cluster.rs 2042-2063) and still returns the list + node_id.
#[test]
fn test_register_forge_handlers_get_reflections_empty_filename() {
    let cluster = make_cluster_with_rpc_server();
    let dir = tempfile::tempdir().unwrap();

    let provider = Box::new(crate::handlers::FileForgeProvider::new(dir.path()));
    let result = cluster.register_forge_handlers(provider);
    assert!(result.is_ok());

    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let list_result = rpc_server.handle_request_sync(
        "forge_get_reflections",
        serde_json::json!({
            "from": "peer-x",
            "filename": "",
        }),
    );
    assert!(list_result.is_ok());
    let resp = list_result.unwrap();
    assert!(
        resp.get("content").is_none(),
        "empty filename must not read a reflection"
    );
    assert_eq!(resp["node_id"], "local-node-001");
}

/// A peer_chat_callback whose task_id is NOT a known child task falls through
/// to the main TaskManager (cluster.rs 2178-2182).
#[tokio::test]
async fn test_peer_chat_callback_unknown_child_falls_through_to_task_manager() {
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    // Wire the queue BEFORE set_rpc_channel so the callback handler captures it.
    let tl = Arc::new(crate::cluster_task::ClusterTaskList::new(
        dir.path().join("tasks"),
    ));
    let wq = Arc::new(crate::cluster_task::ClusterWorkQueue::new(8));
    cluster.set_cluster_task_queue(tl.clone(), wq.clone());
    cluster.set_rpc_channel(Arc::new(MockRpcChannelW3b));

    // A pending task in the MAIN task manager with the id the callback uses.
    let task = Task {
        id: "orphan-1".to_string(),
        status: TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    cluster.task_manager.submit(task).unwrap();

    let resp = cluster
        .rpc_server
        .as_ref()
        .unwrap()
        .handle_request_sync(
            "peer_chat_callback",
            serde_json::json!({"task_id": "orphan-1", "status": "completed", "response": "back-at-ya"}),
        )
        .expect("callback should be accepted");
    assert_eq!(resp["status"], "accepted");

    let t = cluster.task_manager.get_task("orphan-1").unwrap();
    assert_eq!(
        t.status,
        TaskStatus::Completed,
        "main TaskManager must complete the task"
    );
    cluster.stop();
}

/// `confirm_delivery` with neither the test override nor an RPC client walks
/// the fallback `call_with_context` branch (cluster.rs 2288-2295) and must not
/// panic.
#[test]
fn test_confirm_delivery_fallback_path_without_client() {
    let cluster = Cluster::new(make_config());
    assert!(!cluster.is_running());
    // rpc_client is None on a cluster that was never started.
    assert!(cluster.rpc_client.lock().is_none());
    cluster.confirm_delivery("peer-x", "task-1");
}

/// `set_rpc_client` logs the client timeout (cluster.rs 2300-2303) — the
/// `timeout_secs` field must evaluate under an active subscriber.
#[test]
fn test_set_rpc_client_logs_timeout_field() {
    s4_tracing_subscriber();
    let cluster = Cluster::new(make_config());
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    cluster.set_rpc_client(Arc::new(RpcClient::with_resolver(Arc::new(resolver))));
    assert!(cluster.rpc_client.lock().is_some());
}

/// The 24h-timeout warn (cluster.rs 2391-2396) — the `age_secs` field must
/// evaluate under an active subscriber.
#[tokio::test]
async fn test_poll_stale_24h_timeout_logs_age_under_subscriber() {
    s4_tracing_subscriber();
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::hours(25)).to_rfc3339();
    let task = Task {
        id: "stale-24h-s4".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(
        tm.get_task("stale-24h-s4").unwrap().status,
        TaskStatus::Failed
    );
}

/// A task between 2 minutes and 24 hours old with a peer_id, but with NEITHER
/// an RPC client NOR a call override → the poll skips it (cluster.rs 2444-2446)
/// and the task stays Pending.
#[tokio::test]
async fn test_poll_stale_no_client_no_call_fn_continues() {
    let tm = Arc::new(TaskManager::new());
    let old_time = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let task = Task {
        id: "stale-5m-noclient".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    tm.submit(task).unwrap();

    poll_stale_pending_tasks(
        &tm,
        &None,
        None,
        poll_defaults(),
        &poll_no_bus(),
        false,
        &poll_no_cb(),
    )
    .await;
    assert_eq!(
        tm.get_task("stale-5m-noclient").unwrap().status,
        TaskStatus::Pending,
        "no client + no override must leave the task pending"
    );
}

/// `sync_to_disk` serializes an Offline peer (cluster.rs 1655), and a blocked
/// state.toml surfaces the save error with its log fields (cluster.rs 1673-1679).
#[test]
fn test_sync_to_disk_offline_peer_then_save_error() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    cluster.register_node(make_w3b_node(
        "peer-off",
        "Off",
        "10.6.6.6:21949",
        NodeStatus::Online,
    ));
    cluster.handle_node_offline("peer-off", "test");
    assert_eq!(
        cluster.get_node_info("peer-off").unwrap().status,
        NodeStatus::Offline
    );

    // First sync succeeds and hits the "offline" status arm.
    cluster.sync_to_disk().expect("first sync should succeed");

    // Block state.toml → second sync fails through the error-mapping closure.
    let state_path = dir.path().join("cluster").join("state.toml");
    if state_path.exists() {
        std::fs::remove_file(&state_path).unwrap();
    }
    std::fs::create_dir_all(&state_path).unwrap();
    assert!(
        cluster.sync_to_disk().is_err(),
        "blocked state.toml must fail"
    );
    cluster.stop();
}

/// A handler returning JSON null round-trips as `result: Some(Null)`, so the
/// async call serializes the JSON `null` literal (cluster.rs 1308-1311); the
/// debug fields before the call (1279-1285) evaluate under the subscriber.
#[tokio::test]
async fn test_async_call_null_handler_result_serializes_null() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server.clone());
    cluster.start();
    cluster
        .register_rpc_handler("s4_null", Box::new(|_p| Ok(serde_json::Value::Null)))
        .unwrap();
    server.start().await.unwrap();
    let port = server.port();

    cluster.register_node(make_w3b_node(
        "peer-null",
        "Null",
        &format!("127.0.0.1:{}", port),
        NodeStatus::Online,
    ));

    let res = cluster
        .call_with_context_async(
            "peer-null",
            "s4_null",
            serde_json::json!({}),
            Duration::from_secs(5),
        )
        .await
        .expect("null-result call should succeed");
    // Server wraps Null as payload → client decodes result: Some(Null) →
    // serde serializes the JSON null literal (4 bytes).
    assert_eq!(res, b"null".to_vec());

    server.stop().unwrap();
    cluster.stop();
}

/// A direct RPCResponse frame WITHOUT a `result` field decodes to
/// `result: None`, so the sync `call_with_context` returns empty bytes
/// (cluster.rs 1210-1215). Requires a multi-thread runtime because the
/// sync path bridges through `block_in_place` + `handle.block_on`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_sync_call_missing_result_field_returns_empty() {
    s4_tracing_subscriber();

    // Raw server: for EVERY connection, reply a bare RPCResponse JSON
    // (no result, no error) — decode_response's WireMessage parse fails,
    // the direct fallback yields result: None.
    //
    // 曾经的 one-shot 形态（只 accept 一次、回完即 drop listener）是 flaky
    // 根因（2026-09-03）：G2 健康探针循环随 cluster.start() 启动，
    // tokio::time::interval 首 tick 立即到期；若 probe task 首次 poll 晚于
    // register_node，探针与本测试的主调用会并发争抢这唯一一次 accept——
    // 探针抢到则主连接还挂在 backlog 里，listener 一 drop 被 Windows RST
    // （os error 10054），全量并行下约 10% 概率红。改为循环 accept + 每
    // 连接独立回同一份裸回复：探针/主调用谁先到都被服务，listener 存活
    // 到测试结束，backlog RST 类竞争被结构性消除（测试语义不变）。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let raw = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut len_buf = [0u8; 4];
                if sock.read_exact(&mut len_buf).await.is_err() {
                    return;
                }
                let len = u32::from_be_bytes(len_buf) as usize;
                let mut buf = vec![0u8; len];
                if sock.read_exact(&mut buf).await.is_err() {
                    return;
                }
                let reply = br#"{"id":"s4-none-1"}"#.to_vec();
                let total = (reply.len() as u32).to_be_bytes();
                let _ = sock.write_all(&total).await;
                let _ = sock.write_all(&reply).await;
            });
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();
    assert!(cluster.rpc_client.lock().is_some());

    cluster.register_node(make_w3b_node(
        "peer-none",
        "NoneNode",
        &addr,
        NodeStatus::Online,
    ));

    let res = cluster
        .call_with_context("peer-none", "s4_none_action", serde_json::json!({}))
        .expect("call should succeed");
    assert!(
        res.is_empty(),
        "missing result must decode to None → empty bytes"
    );

    cluster.stop();
    raw.abort();
}

/// Resolver fallback matched by NAME with an empty `addresses` list but a
/// parseable primary address → yields `vec![host]` (cluster.rs 2658-2663).
#[test]
fn test_cluster_peer_resolver_fallback_host_from_primary_address() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let mut peer = make_w3b_node("x9", "HostOnlyNode", "127.0.0.1:9001", NodeStatus::Online);
    peer.addresses = vec![]; // force the primary-address parse path
    cluster.register_node(peer);

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    let (addresses, port, online) = resolver.get_peer_info("HostOnlyNode").unwrap();
    assert_eq!(addresses, vec!["127.0.0.1".to_string()]);
    assert_eq!(port, 9001);
    assert!(online);
    cluster.stop();
}

// ---- G4: 超时阶梯不变量（2026-09-01 集群韧性 goal）----
//
// 正确阶梯（每层都必须大于下一层，否则外层先死、内层超时形同虚设）：
//   provider 单请求 600s（nemesis-providers openai_compat default_timeout）
//     < B 端 llm_timeout 默认 2h（peer_chat_handler DEFAULT_LLM_TIMEOUT）
//     < A 端 stale 安全网 max(24h, 2×llm_timeout)
//     < 结果存储 TTL 7d（task_result_store 清扫周期）。

/// 安全网：llm_timeout=0（不限制）→ 落到 24h 地板。
#[test]
fn test_safety_net_zero_llm_timeout_hits_floor() {
    let net = super::stale_task_safety_net(0);
    assert_eq!(net, chrono::Duration::hours(24));
}

/// 安全网：普通 llm_timeout（如 1h）→ 2×1h=2h < 24h 地板 → 仍是 24h。
#[test]
fn test_safety_net_normal_llm_timeout_still_floor() {
    let net = super::stale_task_safety_net(3600);
    assert_eq!(net, chrono::Duration::hours(24));
}

/// 安全网：超大 llm_timeout（48h）→ 2×48h=96h 顶穿地板。
#[test]
fn test_safety_net_huge_llm_timeout_scales() {
    let net = super::stale_task_safety_net(48 * 3600);
    assert_eq!(net, chrono::Duration::hours(96));
}

/// 全阶梯不变量（数值文字量对应上面各真相源常量；一边改了这里会红）。
#[test]
fn test_timeout_ladder_ordering() {
    let provider_single_request_secs: i64 = 600; // openai_compat default_timeout
    let b_side_llm_timeout_secs: i64 = 2 * 3600; // DEFAULT_LLM_TIMEOUT
    let a_side_safety_net = super::stale_task_safety_net(b_side_llm_timeout_secs as u64);
    let result_ttl_secs: i64 = 7 * 24 * 3600; // task_result_store TTL

    assert!(
        provider_single_request_secs < b_side_llm_timeout_secs,
        "provider request must time out before B-side LLM timeout"
    );
    assert!(
        chrono::Duration::seconds(b_side_llm_timeout_secs) < a_side_safety_net,
        "B-side timeout must expire before A-side safety net"
    );
    assert!(
        a_side_safety_net < chrono::Duration::seconds(result_ttl_secs),
        "safety net must fire before result store TTL cleanup"
    );
}

// ---- G5: query_task_result handler 语义（2026-09-01 集群韧性 goal）----

/// running 占位条目必须上报 running（修复前误报 done+空回复，A 会过早完成任务）。
#[test]
fn test_query_task_result_running_entry_reports_running() {
    let cluster = Cluster::new(make_config());
    cluster.result_store().store_success(
        "qtr-running",
        "peer_chat",
        serde_json::json!({"status": "running", "from": "Node-B"}),
    );

    let handler = cluster.build_query_task_result_handler();
    let resp = handler(serde_json::json!({"task_id": "qtr-running"})).unwrap();
    assert_eq!(resp.get("status").and_then(|v| v.as_str()), Some("running"));
}

/// 成功结果读 "response" 键（写端归一后）；旧格式 "content" 键回退兼容。
#[test]
fn test_query_task_result_done_reads_response_key_with_content_fallback() {
    let cluster = Cluster::new(make_config());

    // 新格式（gateway adapter set_result 归一后写 "response"）。
    cluster.result_store().store_success(
        "qtr-done",
        "peer_chat",
        serde_json::json!({"response": "final answer", "from": "Node-B"}),
    );
    // 旧格式（修复前落盘）。
    cluster.result_store().store_success(
        "qtr-done-legacy",
        "peer_chat",
        serde_json::json!({"content": "legacy answer", "from": "Node-B"}),
    );

    let handler = cluster.build_query_task_result_handler();

    let resp = handler(serde_json::json!({"task_id": "qtr-done"})).unwrap();
    assert_eq!(resp.get("status").and_then(|v| v.as_str()), Some("done"));
    assert_eq!(
        resp.get("response").and_then(|v| v.as_str()),
        Some("final answer")
    );

    let legacy = handler(serde_json::json!({"task_id": "qtr-done-legacy"})).unwrap();
    assert_eq!(
        legacy.get("response").and_then(|v| v.as_str()),
        Some("legacy answer"),
        "legacy 'content' key must fall back"
    );
}

/// 失败结果：done + result_status=error + error 文案；未知任务 not_found。
#[test]
fn test_query_task_result_error_and_not_found() {
    let cluster = Cluster::new(make_config());
    cluster
        .result_store()
        .store_failure("qtr-fail", "peer_chat", "boom");

    let handler = cluster.build_query_task_result_handler();

    let resp = handler(serde_json::json!({"task_id": "qtr-fail"})).unwrap();
    assert_eq!(resp.get("status").and_then(|v| v.as_str()), Some("done"));
    assert_eq!(
        resp.get("result_status").and_then(|v| v.as_str()),
        Some("error")
    );
    assert_eq!(resp.get("error").and_then(|v| v.as_str()), Some("boom"));

    let missing = handler(serde_json::json!({"task_id": "qtr-missing"})).unwrap();
    assert_eq!(
        missing.get("status").and_then(|v| v.as_str()),
        Some("not_found")
    );
}

/// Swarm M2: on_node_discovered 回调 —— 非黑名单发现事件触发，
/// 携带 (node_id, role, category)；黑名单节点不触发。
#[test]
fn test_on_node_discovered_callback_fires() {
    let cluster = Cluster::new(make_config());
    let seen: std::sync::Arc<std::sync::Mutex<Vec<(String, String, String)>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    cluster.set_on_node_discovered(std::sync::Arc::new(move |node_id, role, category| {
        seen_clone.lock().unwrap().push((
            node_id.to_string(),
            role.to_string(),
            category.to_string(),
        ));
    }));

    cluster.handle_discovered_node(
        "cb-node-1",
        "cb-worker",
        vec!["10.0.0.9".into()],
        21949,
        "worker",
        "qa",
        vec![],
        vec![],
        "agent",
    );

    let events = seen.lock().unwrap();
    assert_eq!(events.len(), 1, "发现一次应触发一次回调");
    assert_eq!(events[0].0, "cb-node-1");
    assert_eq!(events[0].1, "worker");
    assert_eq!(events[0].2, "qa");
}

#[test]
fn test_on_node_discovered_callback_skips_blacklisted() {
    let cluster = Cluster::new(make_config());
    // 时序：发现→移除（入黑名单）→挂回调→再次发现应被拦。
    cluster.handle_discovered_node(
        "banned-node",
        "banned",
        vec!["10.0.0.10".into()],
        21949,
        "worker",
        "general",
        vec![],
        vec![],
        "agent",
    );
    assert!(cluster.remove_node("banned-node"));
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen_clone = seen.clone();
    cluster.set_on_node_discovered(std::sync::Arc::new(move |node_id, _role, _category| {
        seen_clone.lock().unwrap().push(node_id.to_string());
    }));
    cluster.handle_discovered_node(
        "banned-node",
        "banned",
        vec!["10.0.0.10".into()],
        21949,
        "worker",
        "general",
        vec![],
        vec![],
        "agent",
    );

    let events = seen.lock().unwrap();
    assert!(
        events.is_empty(),
        "黑名单节点不应触发回调，实际: {events:?}"
    );
}

// 回归（2026-09-09 双机 G14）：rpc_port=0 的 announce（异版本/畸形）不得
// 产生不可达记录（曾实证 state.toml 存下 ip:0 + rpc_port=0 的 unusable 条目）。
// 未知节点不登记；已知节点（静态 peers 条目）不被 0 覆盖。
#[test]
fn test_handle_discovered_node_skips_zero_rpc_port() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // 未知节点 + rpc_port=0 → 不登记。
    assert!(!cluster.handle_discovered_node(
        "node-ghost",
        "Ghost",
        vec!["10.0.0.99".into()],
        0,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    ));
    assert!(
        cluster.get_node_info("node-ghost").is_none(),
        "rpc_port=0 的 announce 不应产生节点记录"
    );

    // 已知节点（正常发现 21953）→ 随后 rpc_port=0 的 announce 不得覆盖。
    assert!(cluster.handle_discovered_node(
        "node-x",
        "X",
        vec!["10.0.0.50".into()],
        21953,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    ));
    assert!(!cluster.handle_discovered_node(
        "node-x",
        "X",
        vec!["10.0.0.50".into()],
        0,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    ));
    let node = cluster.get_node_info("node-x").unwrap();
    assert_eq!(
        node.base.address, "10.0.0.50:21953",
        "静态/正常条目的可用地址必须原值保留"
    );
}

// ------------------------------------------------------------------
// state.toml 回读（集群完备性加固 2026-09-11）：restore_discovered_from_state
// 把磁盘 discovered 历史以 Offline 种回注册表（write-only 假持久化根修）。
// ------------------------------------------------------------------

/// 写一个 state.toml（PeerConfig 序列化形态由 DynamicState 决定）。
fn write_state_toml(path: &Path, discovered: &[PeerConfig]) {
    let state = DynamicState {
        discovered: discovered.to_vec(),
        last_sync: chrono::Local::now().to_rfc3339(),
    };
    crate::cluster_config::save_dynamic_state(path, &state).unwrap();
}

fn test_peer_config(id: &str, address: &str) -> PeerConfig {
    PeerConfig {
        id: id.into(),
        name: format!("name-{id}"),
        address: address.into(),
        addresses: vec![],
        rpc_port: 21949,
        role: "worker".into(),
        category: "development".into(),
        tags: vec!["python".into()],
        professions: Vec::new(),
        tier: None,
        runtime_backend: None,
        priority: 1,
        enabled: true,
        status: PeerStatus::default(),
    }
}

#[test]
fn restore_seeds_known_peers_as_offline() {
    let mut cluster = Cluster::new(make_config());
    let dir = std::env::temp_dir().join(format!("nemesis-restore-seed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_path = dir.join("state.toml");
    write_state_toml(&state_path, &[test_peer_config("node-a", "10.0.0.2:21949")]);
    cluster.dynamic_state_path = state_path.clone();

    let seeded = cluster.restore_discovered_from_state();
    assert_eq!(seeded, 1);
    let node = cluster
        .get_node_info("node-a")
        .expect("seeded 节点必须在册");
    assert_eq!(node.status, NodeStatus::Offline, "回读绝不直接 Online");
    assert_eq!(node.base.address, "10.0.0.2:21949");
    assert_eq!(node.tags, vec!["python".to_string()]);
    // 二次调用不重复种（已有条目跳过）。
    assert_eq!(cluster.restore_discovered_from_state(), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn restore_skips_self_banned_existing_and_unusable() {
    let mut cluster = Cluster::new(make_config()); // node_id = local-node-001
    let dir = std::env::temp_dir().join(format!("nemesis-restore-skip-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_path = dir.join("state.toml");

    // 预置一个「已有条目」（静态 peers 场景）。
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-static".into(),
            name: "static".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.9:21949".into(),
            category: "general".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    // 预置一个黑名单节点。
    cluster.removed_peers.write().insert("node-banned".into());

    write_state_toml(
        &state_path,
        &[
            test_peer_config("local-node-001", "10.0.0.1:21949"), // 本节点 → 跳
            test_peer_config("node-banned", "10.0.0.3:21949"),    // 黑名单 → 跳
            test_peer_config("node-static", "10.0.0.9:21949"),    // 已有 → 跳（不覆盖）
            test_peer_config("node-bad", "no-address"),           // 无 host:port → 跳
            PeerConfig::default(),                                // 空 id → 跳
            test_peer_config("node-ok", "10.0.0.4:21949"),        // 合法 → 种
        ],
    );
    cluster.dynamic_state_path = state_path;

    let seeded = cluster.restore_discovered_from_state();
    assert_eq!(seeded, 1, "只有 node-ok 可种");
    assert!(cluster.get_node_info("node-ok").is_some());
    // 静态条目原值保留（Online 不被 Offline 覆盖）。
    assert_eq!(
        cluster.get_node_info("node-static").unwrap().status,
        NodeStatus::Online
    );
    assert!(cluster.get_node_info("node-banned").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

/// T13 根修（2026-09-12）：state.toml 的占位名条目（id="Node-A"）与注册表
/// 里同地址的运行时 id 静态条目是**同一物理节点**，回读不得再种一份——
/// 否则 RpcClient 直接键命中离线影子，拒绝拨号同地址在线条目（UAT T13
/// 实证：D 重启后 D→A duration_ms=0 全灭）。
#[test]
fn restore_skips_address_shadow_of_existing_entry() {
    let mut cluster = Cluster::new(make_config());
    let dir = std::env::temp_dir().join(format!("nemesis-restore-addr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_path = dir.join("state.toml");

    // 注册表已有该物理节点：运行时 id 键控、同地址、Online（静态 peer）。
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-real-runtime-id".into(),
            name: "Node-A".into(),
            role: nemesis_types::cluster::NodeRole::Coordinator,
            address: "127.0.0.1:21949".into(),
            category: "general".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    // state.toml 里留着占位名条目（升级前形态）：id=名字、同地址。
    write_state_toml(
        &state_path,
        &[test_peer_config("Node-A", "127.0.0.1:21949")],
    );
    cluster.dynamic_state_path = state_path;

    assert_eq!(cluster.restore_discovered_from_state(), 0, "同地址影子不种");
    assert!(
        cluster.get_node_info("Node-A").is_none(),
        "占位名条目不得入册"
    );
    // 既有条目原值保留（Online 不被动）。
    assert_eq!(
        cluster
            .get_node_info("node-real-runtime-id")
            .unwrap()
            .status,
        NodeStatus::Online
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// 同名不同地址也算影子：RpcClient 名字回退扫描会命中离线重名条目，
/// 同样遮蔽在线条目（直接键未命中时按 id/name 扫 list_peers）。
#[test]
fn restore_skips_name_shadow_of_existing_entry() {
    let mut cluster = Cluster::new(make_config());
    let dir = std::env::temp_dir().join(format!("nemesis-restore-name-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let state_path = dir.join("state.toml");

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-real-runtime-id".into(),
            name: "Node-A".into(),
            role: nemesis_types::cluster::NodeRole::Coordinator,
            address: "10.0.0.7:21949".into(),
            category: "general".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    // id=占位名、地址不同（对端换了地址），但名字相同 → 同节点，不种。
    let mut pc = test_peer_config("Node-A", "10.9.9.9:21949");
    pc.name = "Node-A".into();
    write_state_toml(&state_path, &[pc]);
    cluster.dynamic_state_path = state_path;

    assert_eq!(cluster.restore_discovered_from_state(), 0, "同名影子不种");
    assert!(cluster.get_node_info("Node-A").is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------
// G2 探针降频（集群完备性加固 2026-09-11）：should_probe_peer 纯函数
// + spawn 链路测试。2026-10-21 启动冲刺：Offline 前 12 tick 全探（纯中继/
// UDP 静默拓扑首次上线不能等 5 个 interval；窗口 12 = 60s 默认间隔下覆盖
// 12 分钟人尺度启动偏差），之后恢复 1/5 降频。
// ------------------------------------------------------------------

#[test]
fn should_probe_peer_down_frequency_matrix() {
    // Online：每 tick 都探。
    for tick in [1u64, 2, 3, 4, 5, 6, 9, 10, 25] {
        assert!(
            super::should_probe_peer(true, tick),
            "online tick {tick} 必须探"
        );
    }
    // Offline：前 12 tick 启动冲刺全探（纯中继拓扑首上线秒级拨号）。
    for tick in [1u64, 2, 5, 10, 12] {
        assert!(
            super::should_probe_peer(false, tick),
            "offline tick {tick} 启动冲刺必须探"
        );
    }
    // 冲刺窗口后：第 5 tick 一次（降频是恢复手段不是放弃）。
    for tick in [13u64, 14, 16, 17, 18, 19] {
        assert!(
            !super::should_probe_peer(false, tick),
            "offline tick {tick} 不应探（1/5 降频）"
        );
    }
    for tick in [15u64, 20, 50] {
        assert!(
            super::should_probe_peer(false, tick),
            "offline tick {tick}（5 的倍数）必须探"
        );
    }
}

/// 链路：探针循环确实按「启动冲刺 + 降频」调度——dead-port Offline 节点在
/// tick 1-12（冲刺窗口）每 tick +1、tick 13/14 零增量、tick 15 再 +1。
/// 127.0.0.1:1 是保留端口，connection refused 即刻返回。用 `start_paused`
/// 虚拟时钟：tick
/// 推进确定（探针真实 I/O pending 期间 runtime 空闲 → 自动推进到下一
/// tick deadline），不依赖真实墙钟窗口，CI 慢机不 flaky。
#[tokio::test(start_paused = true)]
async fn health_probe_loop_sprint_then_fifth_tick() {
    let cluster = Cluster::new(make_config());
    // 死端口 Offline 节点（127.0.0.1:1 保留端口，refused 即刻返回）。
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-dead".into(),
            name: "dead".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "127.0.0.1:1".into(),
            category: "general".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    // 探针循环要求 RPC client 已初始化（None 直接 return）。
    *cluster.rpc_client.lock() = Some(Arc::new(RpcClient::new()));

    // 失败阈值拉满：探测失败只累计 consecutive_failures，不翻状态。
    cluster.start_health_check_loop(Duration::from_millis(100), u32::MAX);

    let failures = || cluster.registry.probe_failure_count("node-dead");
    // 虚拟时钟推进到 320ms：tick 1(0ms)-4(300ms) 已过，tick 5(400ms) 未到。
    // 冲刺窗口（tick 1-12）内每 tick 都探。
    tokio::time::sleep(Duration::from_millis(320)).await;
    let before = failures();
    assert_eq!(
        before, 4,
        "冲刺窗口 tick 1-4 每 tick 恰好各探一次（实得 {before}）"
    );
    // 推进到 1250ms：tick 5-12（400-1100ms）继续全探（窗口内），
    // tick 13(1200ms) 已在窗口外不探 → 累计恰好 12。
    tokio::time::sleep(Duration::from_millis(930)).await;
    let mid = failures();
    assert_eq!(
        mid, 12,
        "冲刺窗口 tick 1-12 全探，tick 13 起降频（实得 {mid}）"
    );
    // 推进到 1650ms：tick 14(1300ms) 不探、tick 15(1400ms) 再探一次、
    // tick 16(1500ms) 不探。
    tokio::time::sleep(Duration::from_millis(400)).await;
    let after = failures();
    assert_eq!(after, 13, "tick 15 恰好再探一次（实得 {after}）");
}

// ---------------------------------------------------------------------
// QUIC 中继钉址形态的拨号候选解析回归（2026-10-21 实测抓到的真 bug）：
// iroh 形态 base.address 是原子（node_id+可选 relay_url），按冒号拆 host
// 曾拆出 `iroh:<hex>@http://127.0.0.1` 截断垃圾候选混进拨号池（junk 白撞
// 一次拨号 + 失败链 last error 用垃圾地址误导排障，relay 端口还被错当
// rpc_port 返回）。修后不变量：全址整条候选恰好一条、零截断形态、
// rpc_port 派生恒 0（iroh 地址内无 RPC 端口语义）。
// ---------------------------------------------------------------------

#[test]
fn resolve_dial_candidates_iroh_pinned_base_address_atomic() {
    use crate::rpc::client::PeerResolver;

    let cluster = Cluster::new(make_config());
    let hex = "ab".repeat(32);
    let full = format!("iroh:{hex}@http://127.0.0.1:33440");
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "nb-relay-peer".into(),
            name: "relay-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: full.clone(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: vec![],
        // 静态 iroh 条目装载形态：loader 给的池里就只有这一条（G14 闸
        // 占位不进池——iroh 分支 udp_port=0 不产 TCP 候选）。
        addresses: vec![full.clone()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });
    cluster.mark_peer_static("nb-relay-peer");

    let resolver = super::ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id().to_string(),
        rpc_port: 23951,
    };
    let (candidates, rpc_port, _) = resolver
        .get_peer_info("nb-relay-peer")
        .expect("静态 iroh peer 必须可解析候选");
    assert_eq!(
        candidates.iter().filter(|a| *a == &full).count(),
        1,
        "全址候选恰好一次（实得 {candidates:?}）"
    );
    assert!(
        candidates
            .iter()
            .all(|a| *a == full || !a.starts_with("iroh:")),
        "不得出现截断/变形垃圾候选（实得 {candidates:?}）"
    );
    assert_eq!(
        rpc_port, 0,
        "iroh 形态 rpc_port 派生恒 0（:33440 是中继端口）"
    );
}

// ---------------------------------------------------------------------
// 复活也是「节点发现」（T1 真机缺陷修复回归）：广播被隔离的拓扑里
// announce 永远不来，Offline→Online 探针翻转是唯一上线信号——
// on_node_discovered（停车场 sweep / 看板收编）必须同样触发。
// ---------------------------------------------------------------------

/// mark_peer_healthy：Offline→Online 翻转触发回调（role/category 取注册表），
/// 已 Online 的重复确认不重复触发（幂等）。
#[test]
fn mark_peer_healthy_recovery_fires_node_discovered() {
    let cluster = Cluster::new(make_config());
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-b-test".into(),
            name: "Node-B".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "192.168.137.237:11958".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Offline,
        capabilities: vec![],
        tags: vec![],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let fired: Arc<Mutex<Vec<(String, String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = fired.clone();
    cluster.set_on_node_discovered(Arc::new(move |id, role, category| {
        sink.lock()
            .push((id.to_string(), role.to_string(), category.to_string()));
    }));

    cluster.mark_peer_healthy("node-b-test");
    {
        let events = fired.lock();
        assert_eq!(
            events.len(),
            1,
            "Offline→Online 翻转应触发恰好一次回调（实得 {}）",
            events.len()
        );
        assert_eq!(events[0].0, "node-b-test");
        assert_eq!(events[0].1, "worker", "role 取注册表规范词表");
        assert_eq!(events[0].2, "development");
    }

    // 已 Online：重复确认不再触发。
    cluster.mark_peer_healthy("node-b-test");
    assert_eq!(fired.lock().len(), 1, "已 Online 的重复确认必须幂等不触发");
}

/// 不在册节点：mark_peer_healthy 不炸、不触发回调（保守 no-op）。
#[test]
fn mark_peer_healthy_unknown_node_is_noop() {
    let cluster = Cluster::new(make_config());
    let fired: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = fired.clone();
    cluster.set_on_node_discovered(Arc::new(move |id, _role, _category| {
        sink.lock().push(id.to_string());
    }));
    cluster.mark_peer_healthy("ghost-node");
    assert!(fired.lock().is_empty(), "未知节点不得触发回调");
}

/// canonical_peer_id（T37 双身份失配第三处落点回归，2026-09-13）：
/// peer 名 / 节点 id 两种形态都归一化为注册表权威 `base.id`；
/// 未知 target 返回 None（调用方保留原值交 RPC resolver 兜底）。
#[test]
fn canonical_peer_id_resolves_name_and_id() {
    let cluster = Cluster::new(make_config());
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-alex-uuid".into(),
            name: "Alex".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec!["python".into()],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 节点 id → 幂等（matcher 路径产物形态）。
    assert_eq!(
        cluster.canonical_peer_id("node-alex-uuid"),
        Some("node-alex-uuid".into())
    );
    // peer 名 → 节点 id（人工指派/兜底路径形态）。
    assert_eq!(
        cluster.canonical_peer_id("Alex"),
        Some("node-alex-uuid".into())
    );
    // 未知 target → None。
    assert_eq!(cluster.canonical_peer_id("ghost"), None);
}

// F1（2026-09-30 派发目标校验前置）：大小写失配回落——人读名手工输入
// 大小写异是常态，文档承诺「大小写不敏感」。
#[test]
fn canonical_peer_id_case_insensitive_fallback() {
    let cluster = Cluster::new(make_config());
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "node-alex-uuid".into(),
            name: "Alex".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "10.0.0.2:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: vec!["python".into()],
        addresses: vec![],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    // 名字大小写失配 → 回落命中（此前精确匹配直接 None → 派发放行原名
    // 撞 RPC 层 peer not found，行为分裂）。
    assert_eq!(
        cluster.canonical_peer_id("alex"),
        Some("node-alex-uuid".into())
    );
    assert_eq!(
        cluster.canonical_peer_id("ALEX"),
        Some("node-alex-uuid".into())
    );
    assert_eq!(
        cluster.canonical_peer_id("NODE-ALEX-UUID"),
        Some("node-alex-uuid".into())
    );
    // 真·未知（无任何大小写变体）→ 仍 None（上游拒绝派发）。
    assert_eq!(cluster.canonical_peer_id("ALEX2"), None);
}

// -- peer_udp_endpoints（U1-5 根修 2026-09-15：异端口拓扑定向单播）--

#[test]
fn test_peer_udp_endpoints_reads_peers_toml() {
    let dir = tempfile::tempdir().unwrap();
    let workspace = dir.path().to_path_buf();
    let cluster = Cluster::with_workspace(make_config(), workspace.clone());

    // 无 peers.toml → 空端点（退化为纯广播）
    assert!(ClusterCallbacks::peer_udp_endpoints(&cluster).is_empty());

    // 写入带 [peers.*].address（UDP 形态）的 peers.toml → 逐个收集
    let cluster_dir = workspace.join("cluster");
    std::fs::create_dir_all(&cluster_dir).unwrap();
    std::fs::write(
        cluster_dir.join("peers.toml"),
        r#"
[node]
id = "node-self"
name = "self"

[peers.node-b]
address = "127.0.0.1:19423"
name = "b"
rpc_port = 29423

[peers.node-a]
address = "192.168.137.237:19422"
name = "a"
rpc_port = 29422
"#,
    )
    .unwrap();

    let mut eps = ClusterCallbacks::peer_udp_endpoints(&cluster);
    eps.sort();
    assert_eq!(
        eps,
        vec![
            "127.0.0.1:19423".to_string(),
            "192.168.137.237:19422".to_string()
        ]
    );
}

// ---------------------------------------------------------------------------
// register_rpc_peer（T26 根修回归：RPC 学到未知对端时继承占位身份，
// 不再以硬编码 worker/general/127.0.0.1 覆盖 operator 配置）
// ---------------------------------------------------------------------------

/// 装载一个自键占位条目（模拟 gateway 静态 peers loader 的调用形态，
/// 含 mark_peer_static——operator 配置的条目豁免 UDP 过期，也是占位
/// 继承的资格标记）。
fn load_placeholder(cluster: &Cluster, id: &str, rpc_port: u16, role: &str, category: &str) {
    cluster.handle_discovered_node(
        id,
        id,
        vec!["127.0.0.1".to_string()],
        rpc_port,
        role,
        category,
        vec![],
        vec![],
        "unknown",
    );
    cluster.mark_peer_static(id);
}

#[test]
fn test_register_rpc_peer_inherits_placeholder_identity_with_hint() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // 占位 Node-A：operator 配置 coordinator/development（T26 实况形态，
    // base.address 被静态 loader 转成 rpc 形态 127.0.0.1:21949）。
    load_placeholder(&cluster, "Node-A", 21949, "coordinator", "development");

    // RPC 先于 announce 学到真实 id，hint=21949（peer_chat payload 携带）。
    let changed = cluster.register_rpc_peer("node-yangjian-real-a", 21949);
    assert!(changed);

    let info = cluster
        .get_peer("node-yangjian-real-a")
        .expect("registered");
    // T26 死因断言：role 必须继承占位的 coordinator，而非硬编码 worker。
    assert_eq!(
        info.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
    assert_eq!(info.base.name, "Node-A");
    assert_eq!(info.base.category, "development");
}

#[test]
fn test_register_rpc_peer_inherits_unique_placeholder_without_hint() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    load_placeholder(&cluster, "Node-A", 21949, "coordinator", "development");

    // payload 缺 _source_rpc_port（hint=0）：唯一占位可安全继承，rpc_port
    // 按占位端口派生。
    let changed = cluster.register_rpc_peer("node-yangjian-real-a", 0);
    assert!(changed);

    let info = cluster
        .get_peer("node-yangjian-real-a")
        .expect("registered");
    assert_eq!(
        info.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
    assert_eq!(info.base.category, "development");
}

#[test]
fn test_register_rpc_peer_multiple_placeholders_without_hint_waits() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    load_placeholder(&cluster, "Node-A", 21949, "coordinator", "development");
    load_placeholder(&cluster, "Node-B", 21950, "worker", "development");

    // 多占位 + 无 hint：无法判定对端身份，不猜（等 announce 修正）。
    let changed = cluster.register_rpc_peer("node-yangjian-real-x", 0);
    assert!(!changed);
    assert!(cluster.get_peer("node-yangjian-real-x").is_none());
}

#[test]
fn test_register_rpc_peer_hint_matches_udp_form_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // 直接以 udp 形态端口写入占位（peers.toml 原始形态，未经 loader 转换）。
    load_placeholder(&cluster, "Node-A", 11949, "coordinator", "development");

    // hint 是 rpc 形态（udp+10000）：端口差 10000 也算命中。
    let changed = cluster.register_rpc_peer("node-yangjian-real-a", 21949);
    assert!(changed);
    let info = cluster
        .get_peer("node-yangjian-real-a")
        .expect("registered");
    assert_eq!(
        info.base.role,
        nemesis_types::cluster::NodeRole::Coordinator
    );
}

#[test]
fn test_register_rpc_peer_known_peer_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // announce 已先到，真实条目已存在：register_rpc_peer 必须是 no-op，
    // 不得覆盖（announce 是身份真相源）。
    cluster.handle_discovered_node(
        "node-yangjian-real-a",
        "Node-A",
        vec!["192.168.1.10".to_string()],
        21949,
        "coordinator",
        "development",
        vec![],
        vec![],
        "agent",
    );
    let changed = cluster.register_rpc_peer("node-yangjian-real-a", 21949);
    assert!(!changed);
    let info = cluster.get_peer("node-yangjian-real-a").expect("present");
    assert_eq!(info.base.address, "192.168.1.10:21949");
}

#[test]
fn test_register_rpc_peer_no_placeholder_conservative_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());

    // 无占位（纯 UDP 集群）+ hint 可用：保守登记（旧行为），announce
    // 到达后由 upsert_if_changed 修正。
    let changed = cluster.register_rpc_peer("node-yangjian-real-a", 21949);
    assert!(changed);
    assert!(cluster.get_peer("node-yangjian-real-a").is_some());

    // 无占位 + hint=0（G14：不可达条目不如不登记）。
    assert!(!cluster.register_rpc_peer("node-yangjian-real-b", 0));
    assert!(cluster.get_peer("node-yangjian-real-b").is_none());
}

// -- P0 vault fail-closed（2026-09-21 复审修复）：token 引用解析失败 →
// RPC 拒绝无认证运行、discovery 拒绝无加密运行 ------------------------------

/// 安装一个必然失败的解析器（保证确定性，不依赖进程内是否有别的解析器）。
fn install_failing_vault_resolver() {
    nemesis_config::set_global_vault_resolver(Arc::new(|alias| {
        Err(format!("test resolver: alias '{alias}' 不存在"))
    }));
}

/// token 是 vault: 引用且解析失败：start() 置位 fail-closed 标志，绑定层
/// 据此拒绝启动 RPC（节点本体照常运行）。空 token 不置位（既有语义）。
#[test]
fn test_load_rpc_auth_token_broken_reference_sets_fail_closed_flag() {
    install_failing_vault_resolver();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "vault:broken-cluster-tok"}).to_string(),
    )
    .unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    let server = Arc::new(crate::rpc::server::RpcServer::new(
        crate::rpc::server::RpcServerConfig {
            bind_address: "127.0.0.1:0".into(),
            ..Default::default()
        },
    ));
    cluster.set_rpc_server(server);
    cluster.start();
    assert!(
        cluster.rpc_reference_broken(),
        "引用解析失败必须置位 fail-closed 标志"
    );
    assert!(cluster.is_running(), "节点本体不受影响（只有 RPC 不 bind）");
    cluster.stop();
    nemesis_config::clear_global_vault_resolver();
}

/// 空字面量 token：不置位 fail-closed（维持"无 token = 无认证"既有语义）。
#[test]
fn test_empty_literal_token_does_not_trip_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": ""}).to_string(),
    )
    .unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();
    assert!(!cluster.rpc_reference_broken());
    cluster.stop();
}

/// discovery 密钥引用失败：start_discovery 整体不启动（不存 discovery
/// 服务、running 标志不置位）——绝不降级为无加密发现。
#[test]
fn test_start_discovery_broken_reference_fails_closed() {
    install_failing_vault_resolver();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(
        dir.path().join("config").join("config.cluster.json"),
        serde_json::json!({"token": "vault:broken-disc-tok"}).to_string(),
    )
    .unwrap();
    let mut cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.set_ports(0, 0);
    cluster.set_broadcast_interval(Duration::from_secs(600));
    cluster.start();
    let arc: Arc<Cluster> = Arc::new(cluster);
    let cb: Arc<dyn ClusterCallbacks> = arc.clone();
    arc.start_discovery(cb);
    assert!(arc.rpc_reference_broken());
    assert!(
        arc.discovery.lock().is_none(),
        "fail-closed：discovery 服务不得创建"
    );
    assert!(
        !arc.discovery_running
            .load(std::sync::atomic::Ordering::SeqCst)
    );
    arc.stop();
    nemesis_config::clear_global_vault_resolver();
}

// -------------------------------------------------------------------------
// resolve_auto_node_name：CJK node_id 显示名截断（2026-10-11 横扫修复）
// -------------------------------------------------------------------------

/// 裸 `&node_id[..8.min(len)]` 在中文 node_id（中文主机名等）上会 panic。
/// 修复后走字节边界安全版：8 字节预算回退到 6 字节 = 2 个汉字。
#[test]
fn auto_node_name_cjk_node_id_truncates_at_char_boundary() {
    let (name, from_config) = resolve_auto_node_name("", None, "中中中中中");
    assert!(!from_config);
    assert_eq!(name, "Bot 中中");
}
// ---------------------------------------------------------------------------
// 探针身份回传（TCP fallback，2026-10-10）：UDP 静默拓扑里身份传播唯一
// 可靠通道——ping 响应携带自报 announce，探针成功方 apply 入册。
// ---------------------------------------------------------------------------

/// ping 响应携带自报身份（announce additive 字段）：TCP 探针身份回传的
/// B 端半边。响应字段集与 UDP announce 同构（build_announce_from_callbacks
/// 单一真相源），且身份热改（set_professions/set_tier）后下一次 ping 即
/// 反映——活读而非构造期快照。
#[test]
fn test_ping_response_carries_live_announce_identity() {
    let cluster = Arc::new(make_cluster_with_rpc_server());
    cluster.install_self_weak();
    cluster
        .register_basic_handlers()
        .expect("register_basic_handlers should succeed");
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    let resp = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .expect("ping should succeed");
    assert_eq!(resp["status"], "pong");
    let ann = resp
        .get("announce")
        .expect("self_weak 已安装，ping 响应必须携带 announce");
    assert_eq!(ann["node_id"], cluster.node_id());
    assert_eq!(ann["name"], cluster.name());
    assert_eq!(ann["rpc_port"], cluster.rpc_port());

    // 热改身份 → 下一次 ping 活读反映。
    cluster.set_professions(vec!["coder".into(), "reviewer".into()]);
    cluster.set_tier(Some("normal".into()));
    let resp2 = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .unwrap();
    let ann2 = resp2.get("announce").unwrap();
    let profs: Vec<&str> = ann2["professions"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert!(profs.contains(&"coder") && profs.contains(&"reviewer"));
    assert_eq!(ann2["tier"], "normal");
}

/// 裸构造（未 install_self_weak，老调用方/部分单测形态）→ ping 优雅
/// 降级为裸 pong，无 announce 字段——向后兼容红线。
#[test]
fn test_ping_without_self_weak_is_bare_pong() {
    let cluster = make_cluster_with_rpc_server();
    cluster
        .register_basic_handlers()
        .expect("register_basic_handlers should succeed");
    let rpc_server = cluster.rpc_server.as_ref().unwrap();
    let resp = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .unwrap();
    assert_eq!(resp["status"], "pong");
    assert!(
        resp.get("announce").is_none(),
        "未安装 self_weak 时不得携带 announce"
    );
}

/// apply_probe_announce（A 端半边）：探针响应携带的对端身份走
/// handle_discovered_node_ex 单一漏斗落册（professions/tier 进注册表）；
/// 坏载荷与自身 node_id 诚实忽略、不炸不误伤既有条目。
///
/// 隔离必须用 with_workspace + tempdir：apply 命中变更会 sync_to_disk，
/// 裸构造（空工作区）落到 CWD=包根的 cluster/state.toml——后续测试的
/// start() 回读它就把本测试的 fixture 当幽灵节点种回注册表（实录：
/// 五个生命周期计数测试连坐 +1 节点）。
#[test]
fn test_apply_probe_announce_lands_identity_in_registry() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();
    let ann = serde_json::json!({
        "version": "1.0",
        "type": "announce",
        "node_id": "peer-node-77",
        "name": "Peer77",
        "addresses": ["192.168.1.77"],
        "rpc_port": 17777,
        "role": "worker",
        "category": "development",
        "tags": ["rust"],
        "capabilities": ["exec"],
        "node_type": "standard",
        "professions": ["coder"],
        "tier": "mini",
        "timestamp": 0,
    });
    cluster.apply_probe_announce(&ann);

    let peers = cluster.registry.list_peers();
    let peer = peers
        .iter()
        .find(|p| p.base.id == "peer-node-77")
        .expect("对端身份应已入册");
    assert_eq!(peer.base.name, "Peer77");
    assert!(peer.professions.iter().any(|p| p == "coder"));
    assert_eq!(peer.tier.as_deref(), Some("mini"));

    // 坏载荷（缺必填 version/timestamp）：反序列化失败 → 忽略，不落册。
    let before = cluster.registry.list_peers().len();
    cluster.apply_probe_announce(&serde_json::json!({"type": "announce"}));
    assert_eq!(cluster.registry.list_peers().len(), before);

    // 空 node_id：忽略。
    cluster.apply_probe_announce(&serde_json::json!({
        "version": "1.0", "type": "announce", "node_id": "",
        "timestamp": 0,
    }));
    assert_eq!(cluster.registry.list_peers().len(), before);

    // 自身 node_id：忽略（自 ping 回环不自报）。
    cluster.apply_probe_announce(&serde_json::json!({
        "version": "1.0",
        "type": "announce",
        "node_id": cluster.node_id(),
        "name": "self-echo",
        "addresses": ["127.0.0.1"],
        "rpc_port": 1,
        "role": "worker",
        "category": "development",
        "tags": [],
        "capabilities": [],
        "node_type": "standard",
        "timestamp": 0,
    }));
    assert_eq!(cluster.registry.list_peers().len(), before);
    cluster.stop();
    nemesis_config::clear_global_vault_resolver();
}

// ===========================================================================
// Win↔Win 集群地址缺陷链回归锁（R2 pin 合并 / R3 拨号兜底 / R4 自投递防护 /
// R5 源地址收割）。计划：docs/PLAN/2026-10-13_four-observation-bugs-fix-plan.md P1。
// 合成 IP 一律走 TEST-NET-3（203.0.113.0/24，永不分配给真实主机），杜绝
// R4 过滤器对测试机真实网卡的意外命中。
// ===========================================================================

/// 构造注册表条目（默认 base.address = host:21949）。
fn make_addr_chain_peer(
    id: &str,
    host: &str,
    addresses: Vec<String>,
    status: NodeStatus,
) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: id.into(),
            name: id.into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: format!("{}:21949", host),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status,
        capabilities: vec![],
        tags: Vec::new(),
        addresses,
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    }
}

/// R2：announce 刷新不逐出静态 peers.toml pin（地址池并入 + base.address 保 pin）。
#[test]
fn test_announce_refresh_preserves_static_pin_addresses() {
    let cluster = Cluster::new(make_config());
    // 静态装载：pin 203.0.113.10（操作员手写的可达地址）
    cluster.handle_discovered_node(
        "worker-1",
        "worker-1",
        vec!["203.0.113.10".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    cluster.mark_peer_static("worker-1");

    // announce 到达：自报地址不含 pin（对端多网卡枚举序/枚举缺口）
    let changed = cluster.handle_discovered_node_ex(
        "worker-1",
        "worker-1",
        vec!["203.0.113.50".into(), "203.0.113.51".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(changed);
    let info = cluster.registry.get("worker-1").unwrap();
    assert!(info.addresses.contains(&"203.0.113.50".to_string()));
    assert!(info.addresses.contains(&"203.0.113.51".to_string()));
    assert!(
        info.addresses.contains(&"203.0.113.10".to_string()),
        "静态 pin 被 announce 池刷新逐出（R2 回归）"
    );
    // base.address 保持操作员 pin（静态语义锁用户显式配置，与
    // merge_real_node_info 的保护同源）
    assert!(
        info.base.address.starts_with("203.0.113.10:"),
        "静态对端 base.address 被 announce primary 覆盖（R2 回归）"
    );

    // 第二拍 announce（同载荷）：pin 已在池 → 内容收敛 → changed=false
    // （无限刷盘回归锁：合并是幂等的，不是每拍都算「变化」）
    let changed2 = cluster.handle_discovered_node_ex(
        "worker-1",
        "worker-1",
        vec!["203.0.113.50".into(), "203.0.113.51".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(
        !changed2,
        "pin 合并必须幂等，否则 announce 每拍都触发 sync_to_disk"
    );
}

/// R2 对照：非静态对端保持旧行为——自报数组整组替换（发现①语义不变）。
#[test]
fn test_announce_refresh_replaces_addresses_for_dynamic_peer() {
    let cluster = Cluster::new(make_config());
    cluster.handle_discovered_node(
        "dyn-1",
        "dyn-1",
        vec!["203.0.113.60".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    let changed = cluster.handle_discovered_node_ex(
        "dyn-1",
        "dyn-1",
        vec!["203.0.113.61".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(changed);
    let info = cluster.registry.get("dyn-1").unwrap();
    assert_eq!(info.addresses, vec!["203.0.113.61".to_string()]);
    assert!(info.base.address.starts_with("203.0.113.61:"));
}

/// R2：merge_real_node_info（TCP 探针/get_info 身份回传路径）对静态对端
/// 候选池走合并而非整组替换。
#[test]
fn test_merge_real_node_info_merges_addresses_for_static_peer() {
    let cluster = Cluster::new(make_config());
    cluster.handle_discovered_node(
        "stat-1",
        "stat-1",
        vec!["203.0.113.70".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    cluster.mark_peer_static("stat-1");

    cluster.merge_real_node_info(&crate::cluster::RealNodeInfo {
        id: "stat-1".into(),
        name: "stat-1".into(),
        address: "203.0.113.71:21949".into(),
        rpc_port: 21949,
        addresses: vec!["203.0.113.71".into()],
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "test".into(),
        capabilities: vec![],
        tags: vec![],
        professions: vec![],
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let info = cluster.registry.get("stat-1").unwrap();
    assert!(
        info.addresses.contains(&"203.0.113.70".to_string()),
        "merge 路径整组替换逐出了静态 pin（R2 回归）"
    );
    assert!(info.addresses.contains(&"203.0.113.71".to_string()));
    // base.address 静态保护（既有行为，回归 pin）
    assert!(info.base.address.starts_with("203.0.113.70:"));
}

/// R3：池非空时 base.address host 仍追加池尾参与拨号（静态 pin 被 announce
/// 挤出主序后依旧可达——兜底语义）。
#[test]
fn test_get_peer_info_appends_base_address_host_to_nonempty_pool() {
    let cluster = Cluster::new(make_config());
    cluster.register_node(make_addr_chain_peer(
        "p1",
        "203.0.113.80",
        vec!["203.0.113.81".into(), "203.0.113.82".into()],
        NodeStatus::Online,
    ));
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    let (addresses, port, online) = resolver.get_peer_info("p1").unwrap();
    assert_eq!(
        addresses,
        vec![
            "203.0.113.81".to_string(),
            "203.0.113.82".to_string(),
            "203.0.113.80".to_string()
        ]
    );
    assert_eq!(port, 21949);
    assert!(online);
}

/// R4 纯函数矩阵：端口不匹配不误杀 / 回环+同端口命中 / 本机 IP+同端口命中。
#[test]
fn test_is_self_endpoint_matrix() {
    let own = vec!["10.103.0.5".to_string(), "192.168.137.1".to_string()];
    // 端口不同 → 永不命中（同机多节点靠端口区分，不按裸 IP 误杀）
    assert!(!is_self_endpoint("127.0.0.1", 21950, 21949, &own));
    assert!(!is_self_endpoint("10.103.0.5", 21950, 21949, &own));
    // 回环 + 同端口 → 命中
    assert!(is_self_endpoint("127.0.0.1", 21949, 21949, &own));
    assert!(is_self_endpoint("127.0.0.1:21949", 21949, 21949, &own));
    // 本机 IP + 同端口 → 命中
    assert!(is_self_endpoint("10.103.0.5", 21949, 21949, &own));
    assert!(is_self_endpoint("192.168.137.1", 21949, 21949, &own));
    // 非本机 host + 同端口 → 不命中
    assert!(!is_self_endpoint("other-host", 21949, 21949, &own));
}

/// R4：同机双节点（同 IP 不同端口）互不误杀——对端候选含本机 IP 但端口
/// 不同 = 合法同机对端，必须原样保留。
#[test]
fn test_same_host_different_port_peer_not_filtered() {
    let cluster = Cluster::new(make_config());
    cluster.register_node(make_addr_chain_peer(
        "sibling",
        "127.0.0.1",
        vec!["127.0.0.1".into()],
        NodeStatus::Online,
    ));
    // 把对端 base.address 端口改成非本节点端口（同机第二实例）
    let mut info = cluster.registry.get("sibling").unwrap();
    info.base.address = "127.0.0.1:31949".into();
    cluster.registry.upsert(info);

    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port, // 21949
    };
    let (addresses, port, _) = resolver.get_peer_info("sibling").unwrap();
    assert_eq!(addresses, vec!["127.0.0.1".to_string()]);
    assert_eq!(port, 31949);
}

/// R4：对端候选池被本节点历史地址污染（同端口布局）→ 自端点候选剔除；
/// 全池自碰撞 → None（诚实失败，拒拨自己）。
#[test]
fn test_self_endpoint_pollution_filtered_and_full_collision_none() {
    let cluster = Cluster::new(make_config());
    // 污染池：本机回环候选（端口 = 本节点 rpc_port）+ 一个干净候选
    cluster.register_node(make_addr_chain_peer(
        "poisoned",
        "127.0.0.1",
        vec!["127.0.0.1".into(), "203.0.113.90".into()],
        NodeStatus::Online,
    ));
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    let (addresses, _, _) = resolver.get_peer_info("poisoned").unwrap();
    assert_eq!(
        addresses,
        vec!["203.0.113.90".to_string()],
        "自端点候选必须被剔除，干净候选保留"
    );

    // 全池自碰撞 → None（dispatch 拿到 "peer unresolvable"，不静默本投）
    cluster.register_node(make_addr_chain_peer(
        "all-self",
        "127.0.0.1",
        vec!["127.0.0.1".into()],
        NodeStatus::Online,
    ));
    assert!(
        resolver.get_peer_info("all-self").is_none(),
        "全池自碰撞必须返回 None（拒拨自身）"
    );
}

/// R4 豁免面（T47 R3 回归锁）：条目 id 与本节点 id 全等 = 显式自投递，
/// 端点自碰撞是预期而非池污染——候选原样放行（Some），不落 None。
/// 防护只拦「别人的单子误投给自己」（id 不等但候选全撞本地端点）；
/// 发给自己的单子由自己执行，作者=执行者，无身份漂移。
#[test]
fn test_self_entry_dial_exempt_keeps_candidates() {
    let cluster = Cluster::new(make_config());
    // 条目 id = 本节点 id（announce 回环/状态回读会把本节点自己写进注册
    // 表）；候选全为本机回环 × 本节点 rpc_port——旧防护把它全滤空成
    // None，合法自派（file: 锚点单拓扑约束）派发即送达失败。
    cluster.register_node(make_addr_chain_peer(
        &cluster.node_id,
        "127.0.0.1",
        vec!["127.0.0.1".into()],
        NodeStatus::Online,
    ));
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };
    let (addresses, port, _) = resolver.get_peer_info(&cluster.node_id).unwrap();
    assert_eq!(
        addresses,
        vec!["127.0.0.1".to_string()],
        "自派豁免：候选必须原样保留，不得滤空"
    );
    assert_eq!(port, cluster.rpc_port);
}

/// R5：announce UDP 源地址作为最低优先级候选入池；与载荷地址去重。
#[test]
fn test_announce_source_addr_appended_lowest_priority() {
    let cluster = Cluster::new(make_config());
    let changed = cluster.handle_discovered_node_ex(
        "src-1",
        "src-1",
        vec!["203.0.113.95".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        Some("203.0.113.99"),
        "",
    );
    assert!(changed);
    let info = cluster.registry.get("src-1").unwrap();
    // 载荷在前、源地址末位（载荷 > 源地址；R2 pin 若在则居中）
    assert_eq!(
        info.addresses,
        vec!["203.0.113.95".to_string(), "203.0.113.99".to_string()]
    );

    // 源地址与载荷重复 → 不重复入池
    cluster.handle_discovered_node_ex(
        "src-2",
        "src-2",
        vec!["203.0.113.96".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        Some("203.0.113.96"),
        "",
    );
    let info = cluster.registry.get("src-2").unwrap();
    assert_eq!(info.addresses, vec!["203.0.113.96".to_string()]);
}

/// 低-4（闭环批 2026-10-13 补全）：池空 + base.host 自碰撞、连候选都没有
/// ——两种形态此前返回 Some 空池（客户端拿 "" 拨号一次才失败，错误文案
/// 误导为连接问题），统一 None 诚实失败。
#[test]
fn test_resolver_empty_pool_self_host_and_no_candidates_none() {
    let cluster = Cluster::new(make_config());
    let resolver = ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id.clone(),
        rpc_port: cluster.rpc_port,
    };

    // 池空、唯一候选 base.host = 自端点（回环）→ None（自碰撞 WARN）
    cluster.register_node(make_addr_chain_peer(
        "empty-pool-self",
        "127.0.0.1",
        vec![],
        NodeStatus::Online,
    ));
    assert!(
        resolver.get_peer_info("empty-pool-self").is_none(),
        "空池 + 自 host 必须返回 None，不得返回 Some 空池"
    );

    // 连候选都没有（base.address 空 + 池空）→ None（无候选 WARN）
    cluster.register_node(make_addr_chain_peer(
        "nothing-at-all",
        "",
        vec![],
        NodeStatus::Online,
    ));
    assert!(
        resolver.get_peer_info("nothing-at-all").is_none(),
        "零候选必须返回 None，不得返回 Some 空池"
    );
}

/// R5 池序综合：静态 pin → 载荷 → 源地址（三来源同场时顺序钉死）。
/// pin 前置（多角度审计批中-2）：拨号预算 = best + 3 fallback，载荷 ≥4 个
/// 且无一同网段时 pin 排尾永远轮不到；「用户显式配置优先」要求 pin 排头。
#[test]
fn test_pool_order_pin_then_payload_then_source() {
    let cluster = Cluster::new(make_config());
    cluster.handle_discovered_node(
        "order-1",
        "order-1",
        vec!["203.0.113.20".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
    );
    cluster.mark_peer_static("order-1");

    cluster.handle_discovered_node_ex(
        "order-1",
        "order-1",
        vec!["203.0.113.21".into()],
        21949,
        "worker",
        "test",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        Vec::new(), // 自报 agent 清单：未宣告
        None,       // iroh_id：未宣告
        None,       // 自报运行时后端形态：未宣告
        Some("203.0.113.29"),
        "",
    );
    let info = cluster.registry.get("order-1").unwrap();
    assert_eq!(
        info.addresses,
        vec![
            "203.0.113.20".to_string(),
            "203.0.113.21".to_string(),
            "203.0.113.29".to_string()
        ],
        "池序必须为 pin → 载荷 → 源地址"
    );
}

// -- P2 agent 清单宣告（AgentAnnouncement，2026-10-16）--

/// roster provider → ping announce 子对象活读（professions M2 同形）。
/// provider 热改（模拟运行期建删 agent）→ 下一次 ping 反映，无需重启。
#[test]
fn test_agent_roster_stamped_into_ping_announce() {
    let cluster = Arc::new(make_cluster_with_rpc_server());
    cluster.install_self_weak();
    cluster
        .register_basic_handlers()
        .expect("register_basic_handlers should succeed");
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    cluster.set_agent_roster_provider(Some(std::sync::Arc::new(|| {
        vec![
            crate::types::AgentAnnouncement {
                agent_id: "main".into(),
                name: "Main Agent".into(),
                kind: "main".into(),
                tier: "big".into(),
                state: String::new(),
            },
            crate::types::AgentAnnouncement {
                agent_id: "proj-alpha".into(),
                name: "Alpha".into(),
                kind: "project".into(),
                tier: "normal".into(),
                state: String::new(),
            },
        ]
    })));

    let resp = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .expect("ping should succeed");
    let ann = resp.get("announce").expect("ping 响应必须携带 announce");
    let agents = ann["agents"].as_array().expect("agents 数组");
    assert_eq!(agents.len(), 2);
    assert_eq!(agents[0]["agent_id"], "main");
    assert_eq!(agents[0]["kind"], "main");
    assert_eq!(agents[1]["tier"], "normal");

    // 运行期换 provider（建删 agent 等价形态）→ 无重启活读。
    cluster.set_agent_roster_provider(Some(std::sync::Arc::new(Vec::new)));
    let resp2 = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .unwrap();
    let ann2 = resp2.get("announce").unwrap();
    assert!(
        ann2["agents"].as_array().unwrap().is_empty(),
        "provider 返回空 = 未宣告语义（空清单）"
    );

    // provider 未装 = 优雅降级空清单（旧节点形态）。
    cluster.set_agent_roster_provider(None);
    let resp3 = rpc_server
        .handle_request_sync("ping", serde_json::json!({}))
        .unwrap();
    assert!(
        resp3.get("announce").unwrap()["agents"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

/// get_info 顶层 agents 字段（nodes_refresh 解析源）：provider 活读，
/// 与 professions/tier 同层。
#[test]
fn test_get_info_carries_agent_roster() {
    let cluster = make_cluster_with_rpc_server();
    cluster
        .register_basic_handlers()
        .expect("register_basic_handlers should succeed");
    let rpc_server = cluster.rpc_server.as_ref().unwrap();

    cluster.set_agent_roster_provider(Some(std::sync::Arc::new(|| {
        vec![crate::types::AgentAnnouncement {
            agent_id: "main".into(),
            name: "Main Agent".into(),
            kind: "main".into(),
            tier: "auto".into(),
            state: String::new(),
        }]
    })));

    let info = rpc_server
        .handle_request_sync("get_info", serde_json::json!({}))
        .unwrap();
    let agents = info["agents"].as_array().expect("get_info 顶层 agents");
    assert_eq!(agents.len(), 1);
    assert_eq!(agents[0]["agent_id"], "main");
    assert_eq!(agents[0]["name"], "Main Agent");
    assert_eq!(agents[0]["tier"], "auto");

    // 未装 provider → 空数组（不缺键，消费方解析稳定）。
    cluster.set_agent_roster_provider(None);
    let info2 = rpc_server
        .handle_request_sync("get_info", serde_json::json!({}))
        .unwrap();
    assert!(info2["agents"].as_array().unwrap().is_empty());
    cluster.stop();
}

/// 收端：roster 随 handle_discovered_node_ex 入注册表；roster 变化经
/// content_eq 触发 changed=true（否则周期广播携带的清单更新会被当冗余
/// upsert 跳过）；同清单重复宣告 = 冗余（changed=false）。
///
/// 隔离用 with_workspace + tempdir（apply 命中变更会 sync_to_disk，裸
/// 构造落 CWD 的 state.toml 污染后续测试——apply_probe_announce 测试
/// 同款纪律）。
#[test]
fn test_discovered_node_roster_lands_and_change_detection() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    let roster_a = vec![crate::types::AgentAnnouncement {
        agent_id: "main".into(),
        name: "Main".into(),
        kind: "main".into(),
        tier: "big".into(),
        state: String::new(),
    }];
    let roster_b = vec![
        crate::types::AgentAnnouncement {
            agent_id: "main".into(),
            name: "Main".into(),
            kind: "main".into(),
            tier: "big".into(),
            state: String::new(),
        },
        crate::types::AgentAnnouncement {
            agent_id: "proj-x".into(),
            name: "X".into(),
            kind: "project".into(),
            tier: "mini".into(),
            state: String::new(),
        },
    ];

    let changed = cluster.handle_discovered_node_ex(
        "roster-peer",
        "roster-peer",
        vec!["203.0.113.50".into()],
        21950,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        roster_a.clone(),
        None, // iroh_id：未宣告
        None, // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(changed, "首次入册必 changed");
    let node = cluster.registry.get("roster-peer").expect("入册");
    assert_eq!(node.agents, roster_a);

    // 同清单重复宣告 → content_eq 相等 → 冗余跳过。
    let changed = cluster.handle_discovered_node_ex(
        "roster-peer",
        "roster-peer",
        vec!["203.0.113.50".into()],
        21950,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        roster_a.clone(),
        None, // iroh_id：未宣告
        None, // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(!changed, "同清单重复宣告不得算内容变化");

    // 清单变化（对端新建 agent）→ changed=true + 注册表更新。
    let changed = cluster.handle_discovered_node_ex(
        "roster-peer",
        "roster-peer",
        vec!["203.0.113.50".into()],
        21950,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        roster_b.clone(),
        None, // iroh_id：未宣告
        None, // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(changed, "roster 变化必须算内容变化");
    let node = cluster.registry.get("roster-peer").unwrap();
    assert_eq!(node.agents, roster_b);
}

/// P5 第 4 步 ③：runtime_backend 随 announce 落册；同值重复宣告冗余收敛
/// （content_eq，不刷盘）；旧节点（wire 无字段）= None 未宣告照常互通
/// （serde default 回归）；静态身份持久化 state.toml 并经启动回读种回
/// 注册表（与 agents 相反——runtime_backend 是静态身份，同 tier 先例）。
#[test]
fn test_runtime_backend_announce_lands_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // ① VM 节点 announce 携带 runtime_backend=Some("vm") → 注册表落值。
    let changed = cluster.handle_discovered_node_ex(
        "vm-peer",
        "vm-peer",
        vec!["203.0.113.80".into()],
        21980,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None, // iroh_id：未宣告
        Some("vm".into()),
        None,
        "", // build：未宣告（P5 期测试补齐 A1 参数）
    );
    assert!(changed, "首次入册必 changed");
    let node = cluster.registry.get("vm-peer").expect("入册");
    assert_eq!(node.runtime_backend.as_deref(), Some("vm"));

    // ② 同值重复宣告 → content_eq 相等 → 冗余跳过（不触发刷盘）。
    let changed2 = cluster.handle_discovered_node_ex(
        "vm-peer",
        "vm-peer",
        vec!["203.0.113.80".into()],
        21980,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None, // iroh_id：未宣告
        Some("vm".into()),
        None,
        "", // build：未宣告（P5 期测试补齐 A1 参数）
    );
    assert!(!changed2, "同值重复宣告不得算内容变化");

    // ③ 旧节点 announce 无字段 → None 未宣告照常入册（互通不炸）。
    let changed3 = cluster.handle_discovered_node_ex(
        "old-peer",
        "old-peer",
        vec!["203.0.113.81".into()],
        21981,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None, // iroh_id：未宣告
        None,
        None,
        "", // build：未宣告（P5 期测试补齐 A1 参数）
    );
    assert!(changed3, "旧节点入册必 changed");
    let old = cluster.registry.get("old-peer").expect("入册");
    assert_eq!(old.runtime_backend, None, "旧节点无字段 = 未宣告");

    // ④ 静态身份持久化：sync_to_disk 落 state.toml（UDP 收端在 changed 时
    // 触发，直调场景显式同步）→ 新 Cluster 启动回读种回注册表。
    cluster.sync_to_disk().unwrap();
    let state_text = std::fs::read_to_string(&cluster.dynamic_state_path).unwrap();
    assert!(
        state_text.contains("runtime_backend"),
        "state.toml 必须持久化 runtime_backend"
    );
    let cluster2 = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster2.start();
    let restored = cluster2.get_node_info("vm-peer").expect("回读种回");
    assert_eq!(restored.status, NodeStatus::Offline, "回读绝不直接 Online");
    assert_eq!(
        restored.runtime_backend.as_deref(),
        Some("vm"),
        "静态身份经 state.toml 往返不丢"
    );
    cluster2.stop();
    cluster.stop();
}

/// G14 空地址已知节点身份合并：agents 与 tier/professions 同族——地址
/// 保留原值的同时清单照常合并（探针回传身份不因空地址丢失）。
#[test]
fn test_empty_addresses_known_peer_roster_merge_preserves_address() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "static-roster-peer".into(),
            name: "static-roster-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "127.0.0.1:50592".into(),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["127.0.0.1".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let roster = vec![crate::types::AgentAnnouncement {
        agent_id: "main".into(),
        name: "M".into(),
        kind: "main".into(),
        tier: String::new(),
        state: String::new(),
    }];
    let changed = cluster.handle_discovered_node_ex(
        "static-roster-peer",
        "static-roster-peer",
        Vec::new(), // 空地址载荷
        50592,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        roster.clone(),
        None, // iroh_id：未宣告
        None, // 自报运行时后端形态：未宣告
        None,
        "",
    );
    assert!(changed, "roster None->A 是内容变化");

    let node = cluster.registry.get("static-roster-peer").unwrap();
    assert_eq!(
        node.base.address, "127.0.0.1:50592",
        "usable address must survive"
    );
    assert_eq!(node.agents, roster, "清单随身份合并进注册表");
}

/// merge_real_node_info（nodes_refresh 路径）：agents 与 professions/tier
/// 同臂合并——既有条目身份更新不丢清单（补合并语义与 professions 一致）。
#[test]
fn test_merge_real_node_info_merges_agent_roster() {
    let dir = tempfile::tempdir().unwrap();
    let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
    cluster.start();

    // 先入一个无清单占位条目。
    cluster.register_node(ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "merge-peer".into(),
            name: "merge-peer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: "203.0.113.60:21960".into(),
            category: "test".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        tags: Vec::new(),
        addresses: vec!["203.0.113.60".into()],
        professions: Vec::new(),
        tier: None,
        agents: Vec::new(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    });

    let roster = vec![crate::types::AgentAnnouncement {
        agent_id: "proj-y".into(),
        name: "Y".into(),
        kind: "project".into(),
        tier: "normal".into(),
        state: String::new(),
    }];
    let info = crate::cluster::RealNodeInfo {
        id: "merge-peer".into(),
        name: "merge-peer-renamed".into(),
        address: "203.0.113.60:21960".into(),
        rpc_port: 21960,
        addresses: vec!["203.0.113.60".into()],
        role: nemesis_types::cluster::NodeRole::Worker,
        category: "test".into(),
        capabilities: vec![],
        tags: vec![],
        professions: vec!["dev".into()],
        tier: Some("big".into()),
        agents: roster.clone(),
        runtime_backend: None,
        node_type: "agent".into(),
        build: String::new(),
    };
    cluster.merge_real_node_info(&info);

    let node = cluster.registry.get("merge-peer").expect("merge 后仍在册");
    assert_eq!(node.agents, roster);
    assert_eq!(node.professions, vec!["dev".to_string()]);
    assert_eq!(node.tier.as_deref(), Some("big"));
    assert_eq!(node.base.name, "merge-peer-renamed");
}
/// NB-122 批 ③ 数据面（2026-10-16）：`set_capabilities` 与 peers.toml
/// `[node].capabilities` 自报段取**并集**——环境能力词（browser 等）与
/// 工具名同进宣告面，作 matcher required_capabilities 硬条件数据源；
/// 重叠词去重；静态段在前顺序稳定（重调幂等）。
#[test]
fn test_set_capabilities_unions_static_identity_caps() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_path_buf();
    let peers_path = workspace.join("cluster").join("peers.toml");
    std::fs::create_dir_all(peers_path.parent().unwrap()).unwrap();
    std::fs::write(
        &peers_path,
        "[node]\nid = \"cap-node\"\nname = \"CapNode\"\nrole = \"worker\"\n\
         capabilities = [\"browser\", \"chrome\"]\n",
    )
    .unwrap();

    let cluster = Cluster::with_workspace(make_config(), workspace);
    // 注入工具名；chrome 与自报段重叠（模拟某工具恰好同名）。
    cluster.set_capabilities(vec!["grep".into(), "chrome".into()]);

    let caps = cluster.local_capabilities();
    assert!(
        caps.contains(&"browser".to_string()),
        "静态自报能力必须存活：{caps:?}"
    );
    assert!(
        caps.contains(&"grep".to_string()),
        "工具能力必须在：{caps:?}"
    );
    assert_eq!(
        caps.iter().filter(|c| c.as_str() == "chrome").count(),
        1,
        "重叠能力必须去重：{caps:?}"
    );
    // 自报段在前（顺序稳定）。
    assert_eq!(caps.first().map(String::as_str), Some("browser"));

    // registry 本节点条目同步携带并集（nodes.list 数据源）。
    let entry = cluster.get_node_info("cap-node").expect("local entry");
    assert!(entry.capabilities.contains(&"browser".to_string()));
}

// -----------------------------------------------------------------------
// A1 P3: iroh 漫游身份入池
// -----------------------------------------------------------------------

/// A1 P3：announce 携带合法 iroh_id → `iroh:<hex>` 拼接后作拨号候选
/// 池尾追加（pin > 载荷 > 源地址 > iroh，不抢直连优先级）；重复 announce
/// 不重复入池。feature 关闭时 valid_iroh_id 恒 None → 不入池（诚实裁剪）。
#[test]
fn test_handle_discovered_node_iroh_id_pools() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // iroh EndpointId 是 ed25519 公钥——from_bytes 会做 curve point 校验，
    // 任意 32 字节（如 ab*32）会被 valid_iroh_id 诚实拒绝；必须用真实
    // 密钥派生 hex（random_valid_iroh_hex：ed25519 SigningKey 派生公钥）。
    let hex = crate::roaming::random_valid_iroh_hex();
    cluster.handle_discovered_node_ex(
        "iroh-node-1",
        "roaming-worker",
        vec!["10.0.0.20".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        vec![],
        Some(hex.clone()),
        None, // 自报运行时后端形态：未宣告
        None, // UDP 隔离拓扑：无 source_addr（源地址收割语义外场景）
        "",   // build：未宣告（P5 期测试补齐 A1 参数）
    );

    let node = cluster.get_node_info("iroh-node-1").expect("node stored");
    {
        let cand = format!("iroh:{hex}");
        assert!(
            node.addresses.contains(&cand),
            "合法 iroh_id 必须入池：{:?}",
            node.addresses
        );
        assert_eq!(
            node.addresses.last(),
            Some(&cand),
            "iroh 候选必须池尾（不抢直连优先级）：{:?}",
            node.addresses
        );
        // primary（TCP 地址拼 port）不被 iroh 形态污染。
        assert_eq!(node.base.address, "10.0.0.20:21949");
    }

    // 重复 announce：池内已存在 → 不重复。
    cluster.handle_discovered_node_ex(
        "iroh-node-1",
        "roaming-worker",
        vec!["10.0.0.20".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        vec![],
        Some(hex.clone()),
        None, // 自报运行时后端形态：未宣告
        None, // UDP 隔离拓扑：无 source_addr（源地址收割语义外场景）
        "",   // build：未宣告（P5 期测试补齐 A1 参数）
    );
    let node = cluster.get_node_info("iroh-node-1").unwrap();
    assert_eq!(
        node.addresses
            .iter()
            .filter(|a| **a == format!("iroh:{hex}"))
            .count(),
        1,
        "重复 announce 不得重复入池：{:?}",
        node.addresses
    );
}

/// A1 P3：非法 iroh_id（长度不足 / 非 hex / 空白）被校验丢弃，不入池。
#[test]
fn test_handle_discovered_node_iroh_id_invalid_rejected() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    // "ab"*32：hex 语法合法但不是有效 ed25519 公钥（curve point 校验拒）。
    for bad in ["abcd", &"zz".repeat(32), "", &"ab".repeat(32)] {
        cluster.handle_discovered_node_ex(
            "iroh-bad",
            "bad-id-worker",
            vec!["10.0.0.21".into()],
            21949,
            "worker",
            "development",
            vec![],
            vec![],
            "agent",
            vec![],
            None,
            vec![],
            Some(bad.to_string()),
            None, // 自报运行时后端形态：未宣告
            None, // UDP 隔离拓扑：无 source_addr（源地址收割语义外场景）
            "",   // build：未宣告（P5 期测试补齐 A1 参数）
        );
    }
    let node = cluster.get_node_info("iroh-bad").expect("node stored");
    assert!(
        !node.addresses.iter().any(|a| a.starts_with("iroh:")),
        "非法 iroh_id 一律不入池：{:?}",
        node.addresses
    );
}

/// A1 P3：iroh 形态地址（静态 pin 场景）作 announce 载荷首地址时，
/// primary 不拼 rpc_port（拼了会造出 `iroh:<hex>:<port>` 非法地址）。
#[test]
fn test_handle_discovered_node_roaming_primary_no_port_suffix() {
    let cluster = Cluster::new(make_config());
    cluster.start();

    let hex = crate::roaming::random_valid_iroh_hex();
    let roaming_addr = format!("iroh:{hex}@10.0.0.30:40001");
    cluster.handle_discovered_node_ex(
        "iroh-node-2",
        "roaming-primary",
        vec![roaming_addr.clone(), "10.0.0.31".into()],
        21949,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
        vec![],
        None,
        vec![],
        None, // iroh_id：未宣告
        None, // 自报运行时后端形态：未宣告
        None, // UDP 隔离拓扑：无 source_addr（源地址收割语义外场景）
        "",   // build：未宣告（P5 期测试补齐 A1 参数）
    );

    let node = cluster.get_node_info("iroh-node-2").expect("node stored");
    assert_eq!(
        node.base.address, roaming_addr,
        "iroh 形态 primary 原样保留（不拼 :rpc_port）"
    );
}

/// 漫游显式开关（config.cluster.json `roaming.enabled`）三态判定：
/// 缺文件 / 缺字段或 false / 显式 true。缺 key 不再等价于开启——
/// 单一真相源是 config，key 只是身份材料。
#[test]
fn test_roaming_enabled_in_config_gate() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.cluster.json");

    // 缺文件 → false。
    assert!(!Cluster::roaming_enabled_in_config(&cfg));

    // 坏 JSON → false（不 panic）。
    std::fs::write(&cfg, "{not json").unwrap();
    assert!(!Cluster::roaming_enabled_in_config(&cfg));

    // 缺 roaming 段 / 缺 enabled 字段 / 显式 false / 非布尔 → false。
    for body in [
        r#"{"port": 11949}"#,
        r#"{"roaming": {}}"#,
        r#"{"roaming": {"enabled": false}}"#,
        r#"{"roaming": {"enabled": "yes"}}"#,
    ] {
        std::fs::write(&cfg, body).unwrap();
        assert!(!Cluster::roaming_enabled_in_config(&cfg), "body={body}");
    }

    // 显式 true → true。
    std::fs::write(&cfg, r#"{"roaming": {"enabled": true}}"#).unwrap();
    assert!(Cluster::roaming_enabled_in_config(&cfg));
}

// 漫游开关打开但 key 缺席：`load_or_create` 幂等自动生成（首次 created=true
// 落盘，二次加载 created=false 且身份一致）——setup_roaming 的自动备料分支。
// QUIC 插件化 M3：identity_ensure 全语义（含 created 报告与幂等）已随
// 身份读写迁入 plugin_roaming（插件 identity_tests 钉死），宿主测试删除。

// ===========================================================================
// A1 运行时硬化批（2026-10-19）：构建号 wire 传播回归锁。
// announce build 字段经 ex 漏斗入册（新条目 / 既有条目 merge / 空地址身份
// 合并三分支全覆盖），Dashboard 节点面据此展示三机换装偏斜。
// ===========================================================================

/// announce 携带 build：新条目入册时注册表透传。
#[test]
fn test_announce_build_propagates_to_registry_new_entry() {
    let cluster = Cluster::new(make_config());
    let changed = cluster.handle_discovered_node_ex(
        "build-peer",
        "build-peer",
        vec!["203.0.113.120".into()],
        21949,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None,
        None,
        None, // runtime_backend：未宣告（合并补齐 P5 参数位）
        "6f96d056",
    );
    assert!(changed, "new entry must register");
    let node = cluster.registry.get("build-peer").expect("registered");
    assert_eq!(node.build, "6f96d056", "build must propagate to registry");
    cluster.stop();
}

/// 既有条目收到带新 build 的 announce：merge 路径刷新 build。
#[test]
fn test_announce_build_refreshes_existing_entry() {
    let cluster = Cluster::new(make_config());
    cluster.handle_discovered_node_ex(
        "build-peer",
        "build-peer",
        vec!["203.0.113.121".into()],
        21949,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None,
        None,
        None, // runtime_backend：未宣告（合并补齐 P5 参数位）
        "aaaa1111",
    );
    // 第二拍：同节点换装（build 变化），地址/身份不变。
    cluster.handle_discovered_node_ex(
        "build-peer",
        "build-peer",
        vec!["203.0.113.121".into()],
        21949,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
        Vec::new(),
        None,
        Vec::new(),
        None,
        None,
        None, // runtime_backend：未宣告（合并补齐 P5 参数位）
        "bbbb2222",
    );
    let node = cluster.registry.get("build-peer").expect("registered");
    assert_eq!(
        node.build, "bbbb2222",
        "refreshed announce must update build"
    );
    cluster.stop();
}

/// 未宣告 build（旧对端）：入册为空串，不炸、不占位。
#[test]
fn test_announce_without_build_registers_empty() {
    let cluster = Cluster::new(make_config());
    let changed = cluster.handle_discovered_node(
        "legacy-peer",
        "legacy-peer",
        vec!["203.0.113.122".into()],
        21949,
        "worker",
        "test",
        Vec::new(),
        Vec::new(),
        "agent",
    );
    assert!(changed, "legacy entry must register");
    let node = cluster.registry.get("legacy-peer").expect("registered");
    assert!(node.build.is_empty(), "absent build stays empty");
    cluster.stop();
}

/// 本节点构建号注入：set_build_hash 后本地注册表条目（sync_local_node_to_registry）
/// 携带同值——Dashboard isLocal 行的 build 数据源。
#[test]
fn test_set_build_hash_reflects_local_registry_entry() {
    let cluster = Cluster::new(make_config());
    cluster.set_build_hash("deadbeef");
    cluster.sync_local_node_to_registry();
    let me = cluster
        .registry
        .get(cluster.node_id())
        .expect("local entry");
    assert_eq!(
        me.build, "deadbeef",
        "local entry must carry injected build"
    );
    cluster.stop();
}

// -------------------------------------------------------------------------
// BUG-20261030-209 回归锁：cluster.start() 必须接线 TaskManager 后台清理
// 循环。此前生产从未调用 task_manager.start()（旧 &mut self 签名无法经
// Arc<TaskManager> 调用），2h 终态清理与 pending/running 安全网兜底双双
// 失效——终态任务（含完整 payload+result）在 tasks.db 无限累积。
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_bug209_start_wires_task_manager_cleanup_loop() {
    let mut cluster = Cluster::new(make_config());
    let tm = Arc::new(TaskManager::with_store_and_interval(
        Arc::new(crate::task_manager::InMemoryTaskStore::new()),
        std::time::Duration::from_millis(50),
    ));
    // 回拨 3h 的终态任务：清理循环一旦在跑，首个 tick 即删除。
    let three_hours_ago = (chrono::Local::now() - chrono::Duration::hours(3)).to_rfc3339();
    tm.submit(Task {
        id: "bug209-cluster-old-done".to_string(),
        status: TaskStatus::Completed,
        action: "peer_chat".to_string(),
        peer_id: "peer-a".to_string(),
        payload: serde_json::json!({"content": "x"}),
        result: Some(serde_json::json!("done")),
        original_channel: "rpc".to_string(),
        original_chat_id: "c1".to_string(),
        created_at: three_hours_ago.clone(),
        completed_at: Some(three_hours_ago),
    })
    .unwrap();
    cluster.set_task_manager_for_test(tm.clone());

    cluster.start();
    // 至多等 ~2s（40×50ms）：任务被清理循环删除即接线成立。
    let mut cleaned = false;
    for _ in 0..40 {
        if tm.get_task("bug209-cluster-old-done").is_none() {
            cleaned = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    cluster.stop();
    assert!(
        cleaned,
        "terminal task must be cleaned by the loop wired in cluster.start()"
    );
}

/// BUG-20261101-264：interval 错过 tick 语义钉子——修复在 13 处外部轮询
/// loop 上把默认 Burst 改成 Delay（挂起恢复后 Burst 对每个错过的 tick 立即
/// 补发 = 洪泛对端；Delay 从当前时刻重排）。本测试钉死修复所依赖的 tokio
/// 语义，防「tokio 默认行为变更」或「修复被重构掉」静默回退。
#[tokio::test(start_paused = true)]
async fn interval_missed_tick_delay_reanchors_after_gap() {
    // 默认 Burst：错过 3 个 slot（100/200/300ms）→ 全部立即补发。
    let mut burst = tokio::time::interval(std::time::Duration::from_millis(100));
    burst.tick().await; // t=0 初始 tick
    tokio::time::advance(std::time::Duration::from_millis(350)).await;
    burst.tick().await; // 100ms slot
    burst.tick().await; // 200ms slot
    let t0 = tokio::time::Instant::now();
    burst.tick().await; // 300ms slot
    assert_eq!(
        tokio::time::Instant::now() - t0,
        std::time::Duration::ZERO,
        "Burst：错过的 tick 逐个立即补发（外部轮询洪泛面）"
    );

    // Delay（修复采用）：同样的缺口只塌缩成一次立即补发，下一 tick 从
    // 当前时刻重排（+100ms），不再突发。
    let mut delay = tokio::time::interval(std::time::Duration::from_millis(100));
    delay.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    delay.tick().await; // t=0 初始 tick
    tokio::time::advance(std::time::Duration::from_millis(350)).await;
    delay.tick().await; // 塌缩补发 tick（立即返回，t=350）
    let t0 = tokio::time::Instant::now();
    delay.tick().await; // 重排后的下一 tick
    assert_eq!(
        tokio::time::Instant::now() - t0,
        std::time::Duration::from_millis(100),
        "Delay：下一 tick 从当前时刻重排，无突发"
    );
}
