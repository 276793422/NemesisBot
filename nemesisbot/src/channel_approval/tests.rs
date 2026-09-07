// K4 (b)：IM 通道审批卡测试。
//
// 覆盖面：
// ① 回执语法 `parse_reply`——approve/deny 双动词、非回执消息不匹配、
//    空 id / 带空格 id 拒绝；
// ② 审批卡渲染 `render_card`——编号/操作/目标/批复指令齐全；
// ③ 组合分流 `CompositeApprovalManager`——web 与空上下文走默认管理器
//    （现状不变），IM 通道走通道管理器；
// ④ 端到端往返（真 MessageBus）：ctx ask → 卡片出站 → watcher 收回执
//    → 裁决回传（approved）+ 结果通知；
// ⑤ 超时自动拒绝：无回执等满 timeout → Ok(denied) + 超时通知出站。
//
// 阻塞等待用 `block_in_place`，要求多线程 runtime——测试一律
// `#[tokio::test(flavor = "multi_thread")]`。

use super::*;
use nemesis_types::channel::InboundMessage;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------------------
// ① 回执语法
// ---------------------------------------------------------------------------

#[test]
fn parse_reply_approve_and_deny() {
    assert_eq!(
        parse_reply("/approve ab12cd34"),
        Some(("approve", "ab12cd34"))
    );
    assert_eq!(
        parse_reply("  /deny  ab12cd34 "),
        Some(("deny", "ab12cd34"))
    );
    assert_eq!(parse_reply("同意 ab12cd34"), None);
    assert_eq!(parse_reply("/approve"), None);
    assert_eq!(parse_reply("/approve ab cd"), None);
    assert_eq!(parse_reply("帮我批准一下"), None);
}

// ---------------------------------------------------------------------------
// ② 审批卡渲染
// ---------------------------------------------------------------------------

#[test]
fn render_card_contains_all_fields() {
    let card = ChannelApprovalManager::render_card(
        "ab12cd34",
        "file_write",
        "/etc/hosts",
        "HIGH",
        "policy: outside workspace",
        300,
    );
    assert!(card.contains("编号: ab12cd34"));
    assert!(card.contains("操作: file_write"));
    assert!(card.contains("目标: /etc/hosts"));
    assert!(card.contains("风险: HIGH"));
    assert!(card.contains("原因: policy: outside workspace"));
    assert!(card.contains("/approve ab12cd34"));
    assert!(card.contains("/deny ab12cd34"));
    assert!(card.contains("300 秒"));
}

// ---------------------------------------------------------------------------
// ③ 组合分流
// ---------------------------------------------------------------------------

/// 记录调用的默认管理器 mock。
#[derive(Default)]
struct RecordingMock {
    calls: std::sync::atomic::AtomicUsize,
}

impl ApprovalManager for RecordingMock {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<ApprovalVerdict, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(ApprovalVerdict::approved())
    }
}

fn ctx_of(channel: &str, chat: &str) -> ApprovalContext {
    ApprovalContext {
        channel: channel.to_string(),
        chat_id: chat.to_string(),
        sender_id: "u1".to_string(),
    }
}

#[test]
fn composite_routes_web_and_empty_to_default() {
    let mock = Arc::new(RecordingMock::default());
    let bus = Arc::new(MessageBus::new());
    let composite =
        CompositeApprovalManager::new(mock.clone(), Arc::new(ChannelApprovalManager::new(bus)));

    // web 通道 → 默认。
    let v = composite
        .request_approval_sync_ctx("r1", "op", "t", "HIGH", "why", 5, &ctx_of("web", "c1"))
        .unwrap();
    assert!(v.approved);
    // 无 channel → 默认。
    composite
        .request_approval_sync_ctx("r2", "op", "t", "HIGH", "why", 5, &ctx_of("", "c1"))
        .unwrap();
    // 无 chat → 默认。
    composite
        .request_approval_sync_ctx("r3", "op", "t", "HIGH", "why", 5, &ctx_of("telegram", ""))
        .unwrap();
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 3);
}

// ---------------------------------------------------------------------------
// ④/⑤ 端到端往返 + 超时（真 MessageBus）
// ---------------------------------------------------------------------------

fn inbound(channel: &str, chat: &str, content: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: "user-1".to_string(),
        chat_id: chat.to_string(),
        content: content.to_string(),
        media: Vec::new(),
        session_key: String::new(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

/// 轮询出站广播直到拿到下一条（测试用，5s 兜底超时）。
/// 5s 而非 1s：全量 workspace 并行测试抢 CPU 时，watcher spawn → inbound
/// 路由 → outbound 广播链路实测会超 1s（watcher_rejects_cross_chat_reply
/// flake，2026-09-07）；隔离复跑 6/6 绿确认为负载时序非逻辑回归。
async fn next_outbound(
    rx: &mut tokio::sync::broadcast::Receiver<OutboundMessage>,
) -> OutboundMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Ok(msg)) => return msg,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => panic!("expected outbound message within 5s"),
        }
    }
}

const CTX_IM: fn() -> ApprovalContext = || ctx_of("telegram", "chat-9");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_card_roundtrip_approve() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    tokio::spawn(mgr.clone().watcher());
    let mut out_rx = bus.subscribe_outbound();

    // 阻塞 ask 放独立线程（block_in_place 需在 runtime worker 内——线程里
    // 直接 recv 更简单）。
    let mgr_for_ask = mgr.clone();
    let ask = std::thread::spawn(move || {
        mgr_for_ask
            .request_approval_sync_ctx(
                "req-uuid-ab12cd34",
                "file_write",
                "/tmp/x",
                "HIGH",
                "test reason",
                30,
                &CTX_IM(),
            )
            .unwrap()
    });

    // 卡片出站（通道 + 对话正确）。
    let card = next_outbound(&mut out_rx).await;
    assert_eq!(card.channel, "telegram");
    assert_eq!(card.chat_id, "chat-9");
    // 编号 = request_id 前 8 位。
    assert!(card.content.contains("编号: req-uuid"));
    let id_line = card
        .content
        .lines()
        .find(|l| l.starts_with("编号: "))
        .expect("card has id line");
    let id = id_line["编号: ".len()..].trim().to_string();

    // 用户同通道同对话批复 → watcher 裁决 + 结果通知。
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id}")));
    let verdict = ask.join().unwrap();
    assert!(verdict.approved);

    let notice = next_outbound(&mut out_rx).await;
    assert!(notice.content.contains(&id));
    assert!(notice.content.contains("已批准"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_card_timeout_denies_and_notifies() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    let mut out_rx = bus.subscribe_outbound();

    let mgr_for_ask = mgr.clone();
    let ask = std::thread::spawn(move || {
        mgr_for_ask
            .request_approval_sync_ctx(
                "req-uuid-9988aabb",
                "process_exec",
                "rm -rf /",
                "CRITICAL",
                "test timeout",
                1, // 1s 超时
                &CTX_IM(),
            )
            .unwrap()
    });

    // 卡片先出站；超时后裁决=拒绝 + 超时通知。
    let card = next_outbound(&mut out_rx).await;
    assert!(card.content.contains("编号: "));
    let verdict = ask.join().unwrap();
    assert!(!verdict.approved);

    let notice = next_outbound(&mut out_rx).await;
    assert!(notice.content.contains("超时"));
    assert!(notice.content.contains("已自动拒绝"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_rejects_cross_chat_reply() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    tokio::spawn(mgr.clone().watcher());
    let mut out_rx = bus.subscribe_outbound();

    let mgr_for_ask = mgr.clone();
    let ask = std::thread::spawn(move || {
        mgr_for_ask
            .request_approval_sync_ctx(
                "req-uuid-11223344",
                "file_write",
                "/tmp/x",
                "HIGH",
                "test",
                // 30s 而非 5s：全量并行负载下 std::thread 调度可能慢于
                // next_outbound 的窗口；本测试断言的是 /deny 裁决路径，
                // 不是超时拒绝，放宽 ask 超时只影响失败时的等待时长。
                30,
                &CTX_IM(),
            )
            .unwrap()
    });

    let card = next_outbound(&mut out_rx).await;
    let id_line = card
        .content
        .lines()
        .find(|l| l.starts_with("编号: "))
        .expect("card has id line");
    let id = id_line["编号: ".len()..].trim().to_string();

    // 别的对话批复 → watcher 回诚实警告，裁决不受影响。
    bus.publish_inbound(inbound("telegram", "chat-OTHER", &format!("/approve {id}")));
    let warn = next_outbound(&mut out_rx).await;
    assert_eq!(warn.chat_id, "chat-OTHER");
    assert!(warn.content.contains("不属于本对话"));

    // 原对话批复仍有效。
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/deny {id}")));
    let verdict = ask.join().unwrap();
    assert!(!verdict.approved);
}
