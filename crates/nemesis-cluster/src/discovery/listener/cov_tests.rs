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

/// 加密链路：密钥不匹配的报文被解密闸**静默丢弃**（不进 handler、日常零
/// WARN——token 失配=安全边界正常工作，账本走 tracker），密钥一致的合法
/// 报文正常送达 handler；tracker 摘要按来源归并计数。
#[test]
fn encrypted_garbage_drops_and_valid_delivers() {
    let inbox: Inbox = Arc::new(StdMutex::new(Vec::new()));
    let c = UdpListener::new(0, Some([7u8; 32])).unwrap();
    c.set_message_handler(capturing_handler(inbox.clone()));
    c.start().unwrap();

    let a = UdpListener::new(0, Some([7u8; 32])).unwrap();
    a.start().unwrap();
    let target = format!("127.0.0.1:{}", c.port());

    // 垃圾字节：解密必败 → 记入 tracker 静默丢弃。
    let garbage = [0xABu8; 64];
    a.socket.send_to(&garbage, &target).unwrap();
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

    // tracker 账本：两帧垃圾同源（同 IP），归并成一个来源、drops=2，
    // 且合法报文不计入丢弃。
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let s = c.decrypt_drop_summary();
        if s.total_drops >= 2 {
            assert_eq!(s.total_drops, 2, "只统计解密失败帧");
            assert_eq!(
                s.sources.len(),
                1,
                "同 IP 来源必须归并（按 IP 而非 addr，避免端口漂移重复首见）：{:?}",
                s.sources
            );
            assert_eq!(s.sources[0].drops, 2);
            assert_eq!(s.sources[0].addr, "127.0.0.1");
            break;
        }
        assert!(Instant::now() < deadline, "tracker 未记录到丢弃帧：{s:?}");
        std::thread::sleep(Duration::from_millis(10));
    }

    c.stop().unwrap();
    a.stop().unwrap();
}

/// tracker 单元语义：同源静默累积（不重复首见）、异源分开计条、
/// cap 16 淘汰最旧来源。
#[test]
fn tracker_dedupes_same_source_and_evicts_oldest() {
    let t = DecryptDropTracker::default();
    let a = Ipv4Addr::new(10, 0, 0, 1);
    let b = Ipv4Addr::new(10, 0, 0, 2);

    t.record(a);
    t.record(a);
    t.record(b);
    let s = t.summary();
    assert_eq!(s.total_drops, 3);
    assert_eq!(s.sources.len(), 2);
    let sa = s.sources.iter().find(|e| e.addr == "10.0.0.1").unwrap();
    let sb = s.sources.iter().find(|e| e.addr == "10.0.0.2").unwrap();
    assert_eq!(sa.drops, 2);
    assert_eq!(sb.drops, 1);
    // 按首见时间升序：a 先见排前。
    assert!(s.sources[0].addr == "10.0.0.1");

    // cap 淘汰：灌满 16 + 1 个源，最旧（10.0.0.1）被淘汰，总量计数保留。
    for i in 0..16u32 {
        let ip = Ipv4Addr::new(10, 1, (i >> 8) as u8, (i & 0xff) as u8);
        t.record(ip);
    }
    let s2 = t.summary();
    assert_eq!(s2.sources.len(), DecryptDropTracker::MAX_SOURCES);
    assert!(
        !s2.sources.iter().any(|e| e.addr == "10.0.0.1"),
        "最旧来源应被环形淘汰：{:?}",
        s2.sources
            .iter()
            .map(|e| e.addr.clone())
            .collect::<Vec<_>>()
    );
    assert_eq!(s2.total_drops, 19, "总量计数独立于环形淘汰");
    // 被淘汰源再出现 = 新条目（会再记一条首见 DEBUG，量级可控）。
    t.record(a);
    let s3 = t.summary();
    assert!(s3.sources.iter().any(|e| e.addr == "10.0.0.1"));
    assert_eq!(s3.sources.len(), DecryptDropTracker::MAX_SOURCES);
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
