// cluster.rs 覆盖率补充测试（RPC 全栈往返臂——真实 client→假对端的
// Frame 协议字节流）：call_with_context 的 sync 错误臂、
// call_with_context_async 的 error / result-None 臂，poll_stale_pending_tasks
// 的 rpc_client 查询路径（error → continue、result None → continue）。
//
// 假对端骨架与 rpc/client/client_extra_tests.rs 的 EchoServer 同款
// （tokio task + async listener，随 Drop abort）——std::thread 版假对端
// 在 multi_thread 测试运行时下实测会把 client 的 spawn_blocking 写入
// 饿死（连接建立后请求帧永不到达，外层 timeout 烧满），不要改回去。
//
// 应答格式：**裸 RPCResponse JSON**（{"id","result"[,"error"]}）——
// decode_response 先试 WireMessage 解析（缺 version/from/to/action 等
// 必填字段必败），落 RPCResponse 直解分支，正好驱动 error / result-None
// 两形态。error 字段只在该有错时写入：`"error": ""` 会被反序列化成
// Some("") 误判为远端错误。

use super::*;
use crate::rpc::client::RpcClient;
use std::collections::VecDeque;
use std::sync::Mutex as StdMutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 响应脚本条目：(result, error)。error = None 时响应里**不带** error 键。
type ScriptReply = (Option<serde_json::Value>, Option<String>);

/// tokio 假对端：按脚本逐条应答（每次连接消费一条，脚本耗尽后回
/// result-null 兜底，不退出 accept 循环）。
struct FakePeer {
    _handle: tokio::task::JoinHandle<()>,
}

impl Drop for FakePeer {
    fn drop(&mut self) {
        self._handle.abort();
    }
}

async fn spawn_fake_rpc_peer(script: Arc<StdMutex<VecDeque<ScriptReply>>>) -> (FakePeer, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut len_buf = [0u8; 4];
            if sock.read_exact(&mut len_buf).await.is_err() {
                return;
            }
            let len = u32::from_be_bytes(len_buf) as usize;
            let mut body = vec![0u8; len];
            if sock.read_exact(&mut body).await.is_err() {
                return;
            }
            // 只取请求 id（请求恒为 WireMessage 形态），应答按脚本。
            let req: crate::transport::conn::WireMessage = match serde_json::from_slice(&body) {
                Ok(w) => w,
                Err(_) => return,
            };
            let (result, error) = script.lock().unwrap().pop_front().unwrap_or((None, None));
            let mut resp = serde_json::json!({ "id": req.id, "result": result });
            if let Some(e) = error {
                resp["error"] = serde_json::json!(e);
            }
            let payload = serde_json::to_vec(&resp).unwrap();
            let mut out = (payload.len() as u32).to_be_bytes().to_vec();
            out.extend_from_slice(&payload);
            if sock.write_all(&out).await.is_err() {
                return;
            }
            let _ = sock.flush().await;
            // 半关闭：告诉对端本连接响应已完（client 是一连接一请求一响应）。
            let _ = sock.shutdown().await;
        }
    });
    (FakePeer { _handle: handle }, port)
}

/// 带假对端的装配：Cluster 注册 Online 条目指向假对端端口，导出 registry。
/// （ClusterPeerResolver 拨号地址 = base.address 原样端口，无 +10000 推导。）
async fn rig_with_fake_peer(
    script: Arc<StdMutex<VecDeque<ScriptReply>>>,
    tag: &str,
) -> (Cluster, FakePeer) {
    let (fake, port) = spawn_fake_rpc_peer(script).await;
    let root =
        std::env::temp_dir().join(format!("nmb-cluster-netcov-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let config = ClusterConfig {
        node_id: "cov-net-node".into(),
        bind_address: "127.0.0.1:0".into(),
        peers: Vec::new(),
        node_name: "CovNet".into(),
    };
    let cluster = Cluster::with_workspace(config, root);
    let mut peer = ExtendedNodeInfo {
        base: nemesis_types::cluster::NodeInfo {
            id: "fake-peer".into(),
            name: "FakePeer".into(),
            role: nemesis_types::cluster::NodeRole::Worker,
            address: format!("127.0.0.1:{port}"),
            category: "development".into(),
            last_seen: chrono::Local::now().to_rfc3339(),
        },
        status: NodeStatus::Online,
        capabilities: Vec::new(),
        tags: Vec::new(),
        addresses: vec!["127.0.0.1".into()],
        node_type: "gateway".into(),
    };
    peer.base.address = format!("127.0.0.1:{port}");
    cluster.register_node(peer);
    (cluster, fake)
}

fn net_client(cluster: &Cluster) -> Arc<RpcClient> {
    Arc::new(RpcClient::with_resolver(Arc::new(ClusterPeerResolver {
        registry: cluster.registry.clone(),
        node_id: cluster.node_id().to_string(),
    })))
}

/// call_with_context（sync bridge 臂，1793-1804）：错误响应 → Err 透传。
/// block_in_place 要求 multi_thread 运行时。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rpc_sync_call_with_context_error_arm() {
    let script = Arc::new(StdMutex::new(VecDeque::from(vec![(
        Some(serde_json::Value::Null),
        Some("cov-failure".to_string()),
    )])));
    let (cluster, _fake) = rig_with_fake_peer(script, "sync-err").await;

    let client = net_client(&cluster);
    cluster.set_rpc_client(client);

    let err = cluster
        .call_with_context("fake-peer", "query_task_result", serde_json::json!({}))
        .unwrap_err();
    assert!(err.contains("cov-failure"), "{err}");
}

/// call_with_context_async 双形态（1888-1916）：error → Err 透传；
/// result None → Ok(空载荷)。
#[tokio::test]
async fn rpc_async_error_then_null_result_arms() {
    let script = Arc::new(StdMutex::new(VecDeque::from(vec![
        (
            Some(serde_json::Value::Null),
            Some("cov-failure".to_string()),
        ), // 错误臂
        (Some(serde_json::Value::Null), None), // result None → Ok(空)
    ])));
    let (cluster, _fake) = rig_with_fake_peer(script, "async-arms").await;

    let client = net_client(&cluster);
    cluster.set_rpc_client(client);

    // 错误响应 → Err 透传（1890-1897）。
    let err = cluster
        .call_with_context_async(
            "fake-peer",
            "query_task_result",
            serde_json::json!({}),
            std::time::Duration::from_secs(5),
        )
        .await
        .unwrap_err();
    assert!(err.contains("cov-failure"), "{err}");

    // result null → 无 error + None result → Ok(空)（1899-1907）。
    let ok = cluster
        .call_with_context_async(
            "fake-peer",
            "query_task_result",
            serde_json::json!({}),
            std::time::Duration::from_secs(5),
        )
        .await
        .expect("None result 是合法响应");
    assert!(ok.is_empty(), "None result → 空载荷：{ok:?}");
}

/// poll 的 rpc_client 查询路径：error 响应 → warn + continue；result None
/// → continue——两条都保持 Pending，不改任务状态（3407-3416）。
#[tokio::test]
async fn poll_stale_rpc_error_and_null_result_keep_pending() {
    let script = Arc::new(StdMutex::new(VecDeque::from(vec![
        (
            Some(serde_json::Value::Null),
            Some("query refused by cov".to_string()),
        ), // task A：error → continue
        (Some(serde_json::Value::Null), None), // task B：result None → continue
    ])));
    let (cluster, _fake) = rig_with_fake_peer(script, "poll-arms").await;
    let client = net_client(&cluster);

    let tm = Arc::new(TaskManager::new());
    for id in ["poll-err", "poll-null"] {
        let task = Task {
            id: id.to_string(),
            status: nemesis_types::cluster::TaskStatus::Pending,
            action: "peer_chat".to_string(),
            peer_id: "fake-peer".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "chat-1".to_string(),
            created_at: (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339(),
            completed_at: None,
        };
        tm.submit(task).unwrap();
    }

    poll_stale_pending_tasks(
        &tm,
        &None,
        Some(client.as_ref()),
        chrono::Duration::hours(24),
        None,
        false,
        None,
    )
    .await;

    for id in ["poll-err", "poll-null"] {
        assert_eq!(
            tm.get_task(id).unwrap().status,
            nemesis_types::cluster::TaskStatus::Pending,
            "{id} 两个 continue 臂都不改状态"
        );
    }
}
