// discovery.rs 覆盖率补充测试（handler 过期丢弃 / 时钟漂移 warn /
// bye 离线 + sync_to_disk 失败臂 / stop 的 bye 广播失败 + 端点单播 /
// send_announce_direct 广播失败臂 + 端点单播 / send_announce_with 单播环）。
//
// 豁免：746-748（send_announce_with 的 to_bytes 序列化失败臂——消息结构
// 全可序列化字段，infallible）；756-757（AES-GCM encrypt_data 失败臂——
// 合法 32B 密钥下 RNG/加密不可注入失败）。

use super::*;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn seq() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst)
}

/// 记录型 callbacks：handle_discovered / offline 入列，sync_to_disk 恒败，
/// 端点可配——分别驱动 573 sync 失败臂与 646/714/779 单播环。
struct CovCallbacks {
    node_id: String,
    discovered: StdMutex<Vec<String>>,
    offline: StdMutex<Vec<String>>,
    endpoints: Vec<String>,
}

impl CovCallbacks {
    fn new(node_id: &str, endpoints: Vec<String>) -> Arc<Self> {
        Arc::new(Self {
            node_id: node_id.into(),
            discovered: StdMutex::new(Vec::new()),
            offline: StdMutex::new(Vec::new()),
            endpoints,
        })
    }

    fn wait_for(list: &StdMutex<Vec<String>>, want: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if list.lock().unwrap().iter().any(|s| s == want) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        list.lock().unwrap().iter().any(|s| s == want)
    }
}

impl ClusterCallbacks for CovCallbacks {
    fn node_id(&self) -> String {
        self.node_id.clone()
    }
    fn name(&self) -> String {
        self.node_id.clone()
    }
    fn address(&self) -> String {
        "127.0.0.1:0".into()
    }
    fn rpc_port(&self) -> u16 {
        0
    }
    fn all_local_ips(&self) -> Vec<String> {
        get_all_local_ips()
    }
    fn role(&self) -> String {
        "worker".into()
    }
    fn category(&self) -> String {
        "development".into()
    }
    fn tags(&self) -> Vec<String> {
        Vec::new()
    }
    fn capabilities(&self) -> Vec<String> {
        Vec::new()
    }
    fn handle_discovered_node(
        &self,
        node_id: &str,
        _name: &str,
        _addresses: &[String],
        _rpc_port: u16,
        _role: &str,
        _category: &str,
        _tags: &[String],
        _capabilities: &[String],
        _node_type: &str,
    ) -> bool {
        self.discovered.lock().unwrap().push(node_id.to_string());
        true
    }
    fn handle_node_offline(&self, node_id: &str, _reason: &str) {
        self.offline.lock().unwrap().push(node_id.to_string());
    }
    fn sync_to_disk(&self) -> Result<(), String> {
        Err("cov sync failure".into())
    }
    fn peer_udp_endpoints(&self) -> Vec<String> {
        self.endpoints.clone()
    }
}

fn announce_with_ts(node: &str, ts: i64) -> DiscoveryMessage {
    let mut msg = DiscoveryMessage::new_announce(
        node,
        node,
        vec!["10.0.0.9".to_string()],
        15000,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "gateway",
    );
    msg.timestamp = ts;
    msg
}

/// handler 臂：过期消息丢弃（不进 discover）、未来时间戳触发漂移 warn 且
/// 正常发现、bye 触发 handle_node_offline + sync_to_disk 失败臂。
#[test]
fn handler_expired_drift_and_bye_sync_failure_arms() {
    let tag = seq();
    let cb = CovCallbacks::new("cov-self", Vec::new());
    let mut config = DiscoveryConfig::with_encryption(0, Duration::from_secs(60), "");
    config.port = 0;
    let svc = DiscoveryService::new(cb.clone() as Arc<dyn ClusterCallbacks>, config).unwrap();
    svc.start().unwrap();

    let port = svc.listener.port();
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let target = format!("127.0.0.1:{port}");
    let now = chrono::Utc::now().timestamp();

    // ① 过期（age 200 > 120 阈值）→ 丢弃，不入 discovered。
    let expired = announce_with_ts("cov-expired", now - 200);
    sock.send_to(&expired.to_bytes().unwrap(), &target).unwrap();

    // ② 未来时间戳（drift 300 > 60）→ 漂移 warn 臂 + 正常发现。
    let drifted = announce_with_ts("cov-drifted", now + 300);
    sock.send_to(&drifted.to_bytes().unwrap(), &target).unwrap();

    assert!(
        CovCallbacks::wait_for(&cb.discovered, "cov-drifted", Duration::from_secs(2)),
        "漂移 announce 应正常发现：{:?}",
        cb.discovered.lock().unwrap()
    );
    assert!(
        !cb.discovered
            .lock()
            .unwrap()
            .contains(&"cov-expired".to_string()),
        "过期消息必须被丢弃"
    );

    // ③ bye → handle_node_offline + sync_to_disk 失败臂。
    let bye = DiscoveryMessage::new_bye("cov-drifted");
    sock.send_to(&bye.to_bytes().unwrap(), &target).unwrap();

    assert!(
        CovCallbacks::wait_for(&cb.offline, "cov-drifted", Duration::from_secs(2)),
        "bye 应触发离线回调"
    );

    svc.stop().unwrap();
    let _ = tag;
}

/// stop：bye 广播失败臂（listener 先行停止 → broadcast Err）+ 端点单播环。
#[test]
fn stop_broadcast_bye_failure_and_unicast_endpoints() {
    let cb = CovCallbacks::new("cov-self2", vec!["127.0.0.1:1".to_string()]);
    let mut config = DiscoveryConfig::with_encryption(0, Duration::from_secs(60), "");
    config.port = 0;
    let svc = DiscoveryService::new(cb.clone() as Arc<dyn ClusterCallbacks>, config).unwrap();
    svc.start().unwrap();

    // 先单独停掉 listener → stop() 里的 bye 广播走失败臂，随后的端点单播
    // 照常成环（send_unicast 对未运行 listener 静默跳过）。
    svc.listener.stop().unwrap();
    let _ = svc.stop();
}

/// send_announce_direct：listener 未启动 → 广播失败臂 + 端点单播环成行。
#[test]
fn send_announce_direct_broadcast_failure_and_unicast() {
    let cb = CovCallbacks::new("cov-self3", vec!["127.0.0.1:1".to_string()]);
    let listener = UdpListener::new(0, None).unwrap();
    // 未 start() → broadcast 返回 NotConnected → 失败臂。
    send_announce_direct(&listener, cb.as_ref());
}

/// send_announce_with：裸 socket 广播 + 端点单播环（779-780）。
#[test]
fn send_announce_with_unicast_loop() {
    let cb = CovCallbacks::new("cov-self4", vec!["127.0.0.1:1".to_string()]);
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    sock.set_broadcast(true).unwrap();
    send_announce_with(&sock, 15000, None, cb.as_ref());
}
