//! M5（2026-10-10 集群无人值守硬化）— remote_approval（worker 侧）测试。
//!
//! 覆盖：装饰器转发（内层裁决生效 + ask_closed 上行不炸）、内层失败不产生
//! 上行且 Err 透传、pending 直通、truncate_chars、spawn 上行在裸 Cluster
//! （无在线 coordinator）静默跳过。跨网络行为（真双节点转发）归 cluster-uat
//! U 系列（文件锁推迟，见计划 §6.5 诚实边界）。

use super::*;
use std::sync::Mutex as StdMutex;

/// 内层 responder 桩：记录 respond 调用；`fail_ids` 命中的请求诚实 Err
/// （模拟「裁决真相源 = unknown」路径）。
struct MockResponder {
    calls: StdMutex<Vec<(String, bool, bool)>>,
    fail_ids: Vec<String>,
}

impl MockResponder {
    fn new(fail_ids: Vec<String>) -> Self {
        Self {
            calls: StdMutex::new(Vec::new()),
            fail_ids,
        }
    }
}

impl ApprovalResponder for MockResponder {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        _note: Option<String>,
    ) -> Result<bool, String> {
        if self.fail_ids.iter().any(|f| f == request_id) {
            return Err(format!("unknown approval request: {request_id}"));
        }
        self.calls
            .lock()
            .unwrap()
            .push((request_id.to_string(), approved, always));
        Ok(true)
    }

    fn pending(&self) -> Vec<serde_json::Value> {
        vec![serde_json::json!({"request_id": "mock-pending"})]
    }
}

fn bare_cluster() -> Arc<Cluster> {
    Arc::new(Cluster::new(
        nemesis_cluster::types::ClusterConfig::default(),
    ))
}

#[tokio::test]
async fn decorator_forwards_verdict_and_fires_ask_closed() {
    let cluster = bare_cluster();
    let mock = Arc::new(MockResponder::new(vec![]));
    let decorated = RemoteNotifyResponder::new(mock.clone(), cluster.clone());
    let delivered = decorated
        .respond("req-decorator-1", true, false, Some("ok".into()))
        .expect("inner respond must succeed");
    assert!(delivered);
    // 内层收到裁决（always 原样透传——钉死 false 是 master 下行 handler
    // 的职责，装饰器不二次改写）。guard 在块内释放，不跨下面的 await。
    {
        let calls = mock.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], ("req-decorator-1".to_string(), true, false));
    }
    // ask_closed 上行：裸 Cluster 无在线 coordinator → 静默 Ok（spawn 的
    // 任务即使先跑完也不炸、不影响已生效裁决）。
    tokio::task::yield_now().await;
}

#[tokio::test]
async fn decorator_inner_failure_propagates_without_panic() {
    let cluster = bare_cluster();
    let mock = Arc::new(MockResponder::new(vec!["req-gone".to_string()]));
    let decorated = RemoteNotifyResponder::new(mock, cluster);
    let err = decorated
        .respond("req-gone", true, false, None)
        .expect_err("unknown id must propagate Err");
    assert!(err.contains("unknown approval request"), "err: {err}");
}

#[test]
fn decorator_pending_passes_through() {
    let mock: Arc<dyn ApprovalResponder> = Arc::new(MockResponder::new(vec![]));
    assert_eq!(mock.pending().len(), 1); // 桩自身语义先钉住
    let decorated = RemoteNotifyResponder::new(mock, bare_cluster());
    assert_eq!(decorated.pending().len(), 1);
}

/// spawn 上行在裸 Cluster（无在线 coordinator）静默跳过：Ok 路径，无 panic、
/// 无 warn（成功语义——本节点即 master 的正常形态）。
#[tokio::test]
async fn spawn_register_with_no_coordinator_is_silent_noop() {
    let cluster = bare_cluster();
    let notice = crate::channel_approval::BoardAskNotice {
        request_id: "req-no-coord-1".to_string(),
        operation: "process_exec".to_string(),
        target: "cargo build".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        timeout_secs: 300,
        chat_id: "cluster_rpc:node-a/board:42".to_string(),
    };
    spawn_remote_ask_register(cluster, &notice);
    // fire-and-forget：任务内部 Ok(()) 静默返回；这里只等调度点跑一遍。
    tokio::task::yield_now().await;
}

/// ask_closed 同形态（裁决摘除上行的 no-op 路径）。
#[tokio::test]
async fn spawn_ask_closed_with_no_coordinator_is_silent_noop() {
    spawn_remote_ask_closed(bare_cluster(), "req-no-coord-2", false, None);
    tokio::task::yield_now().await;
}

#[test]
fn truncate_chars_char_boundary_safe() {
    assert_eq!(truncate_chars("short", 10), "short");
    assert_eq!(truncate_chars("exact10_!", 9), "exact10_!");
    let long = "汉".repeat(30);
    let cut = truncate_chars(&long, 10);
    assert_eq!(cut.chars().count(), 11); // 10 + 省略号
    assert!(cut.ends_with('…'));
}
