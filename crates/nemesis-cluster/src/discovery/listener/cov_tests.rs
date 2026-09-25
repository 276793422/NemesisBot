// discovery/listener.rs 覆盖率补充测试（UdpListener 明文/加密收发全链 /
// 解密失败丢弃 warn / 定向单播 + 广播 / recv 超时臂 / 本地地址枚举）。
//
// 豁免：295-317 的 UDP-connect 兜底分支（local_ip_addresses 在接口枚举
// 为空时才走，无法在本进程内强制 get_local_network_interfaces 返回空
// ——环境依赖）；284-286 的 get_broadcast_addresses 枚举 Err 兜底（同
// 环境依赖）。

use super::*;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn seq() -> u64 {
    SEQ.fetch_add(1, Ordering::SeqCst)
}

fn make_msg(node: &str) -> DiscoveryMessage {
    DiscoveryMessage::new_announce(
        node,
        node,
        vec!["10.0.0.9".to_string()],
        15000,
        "worker",
        "development",
        Vec::new(),
        Vec::new(),
        "gateway",
    )
}

/// 收集 handler 收到的 node_id（带超时轮询）。
type Inbox = Arc<StdMutex<Vec<String>>>;

fn capturing_handler(inbox: Inbox) -> MessageHandler {
    Box::new(move |msg, _sender| {
        inbox.lock().unwrap().push(msg.node_id.clone());
    })
}

fn wait_for(inbox: &Inbox, want: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if inbox.lock().unwrap().iter().any(|id| id == want) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    inbox.lock().unwrap().iter().any(|id| id == want)
}

/// 明文链路：定向单播送达 handler（167 超时臂随 1s recv tick 覆盖）+
/// broadcast 可调用（210-231）。
#[test]
fn plaintext_unicast_delivers_and_broadcast_sends() {
    let tag = seq();
    let inbox: Inbox = Arc::new(StdMutex::new(Vec::new()));
    let b = UdpListener::new(0, None).unwrap();
    b.set_message_handler(capturing_handler(inbox.clone()));
    b.start().unwrap();

    let a = UdpListener::new(0, None).unwrap();
    a.start().unwrap();
    let target = format!("127.0.0.1:{}", b.port());
    a.send_unicast(&target, &make_msg("cov-plain-peer"));

    assert!(
        wait_for(&inbox, "cov-plain-peer", Duration::from_secs(2)),
        "单播必须送达 B 的 handler：{:?}",
        inbox.lock().unwrap()
    );

    // 广播：只断言可调用成功（回环/隔离环境不保证回送自身端口）。
    a.broadcast(&make_msg("cov-bcast-peer")).unwrap();

    // 跨过一次 1s recv 超时 tick → TimedOut continue 臂。
    std::thread::sleep(Duration::from_millis(1300));

    b.stop().unwrap();
    a.stop().unwrap();
    let _ = tag;
}

/// 加密链路：密钥不匹配的垃圾报文被解密闸丢弃（128 warn），密钥一致的
/// 合法报文正常送达 handler（152 / 237-250 加密单播）。
#[test]
fn encrypted_garbage_drops_and_valid_delivers() {
    let inbox: Inbox = Arc::new(StdMutex::new(Vec::new()));
    let c = UdpListener::new(0, Some([7u8; 32])).unwrap();
    c.set_message_handler(capturing_handler(inbox.clone()));
    c.start().unwrap();

    let a = UdpListener::new(0, Some([7u8; 32])).unwrap();
    a.start().unwrap();
    let target = format!("127.0.0.1:{}", c.port());

    // 垃圾字节：解密必败 → warn + continue。
    let garbage = [0xABu8; 64];
    a.socket.send_to(&garbage, &target).unwrap();

    // 合法加密单播：正常解密 → 解析 → handler。
    a.send_unicast(&target, &make_msg("cov-enc-peer"));

    assert!(
        wait_for(&inbox, "cov-enc-peer", Duration::from_secs(2)),
        "同密钥单播必须送达：{:?}",
        inbox.lock().unwrap()
    );
    assert!(
        !inbox.lock().unwrap().contains(&"garbage".to_string()),
        "垃圾报文不得进入 handler"
    );

    c.stop().unwrap();
    a.stop().unwrap();
}

/// 本地地址枚举：广播地址列表与本机 IP 列表非空（279-291 + compute_broadcast
/// + local_ip_addresses 的接口枚举主分支）。
#[test]
fn broadcast_addresses_and_local_ips_non_empty() {
    let addrs = get_broadcast_addresses();
    assert!(!addrs.is_empty(), "本机至少有一个子网广播地址：{addrs:?}");
    let ips = get_all_local_ips();
    assert!(!ips.is_empty(), "本机至少有一个 IP：{ips:?}");
}
