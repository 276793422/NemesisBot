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

/// 轮询出站广播直到拿到下一条（测试用，30s 兜底超时）。
/// 30s 而非 5s：全量 nemesisbot 并行测试（630s 级满载）抢 CPU 时，
/// watcher spawn → inbound 路由 → outbound 广播链路实测会超 5s（2026-09-07
/// flake 加到 5s 后 2026-09-08 全量仍命中一次，watcher_rejects_cross_chat_reply
/// panic 在本函数窗口）；隔离复跑 6/6 绿确认为负载时序非逻辑回归。
async fn next_outbound(
    rx: &mut tokio::sync::broadcast::Receiver<OutboundMessage>,
) -> OutboundMessage {
    loop {
        match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await {
            Ok(Ok(msg)) => return msg,
            Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
            _ => panic!("expected outbound message within 30s"),
        }
    }
}

const CTX_IM: fn() -> ApprovalContext = || ctx_of("telegram", "chat-9");

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn channel_card_roundtrip_approve() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    mgr.spawn_watcher();
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
                // 120s 而非 30s：全量并行满载下 std::thread 调度 + watcher
                // 轮询可能慢于 30s（2026-09-08 全量 flake，ask 超时自动拒绝
                // 打穿 approved 断言）；本测试断言批复回传路径，不是超时
                // 拒绝，放宽只影响失败时的等待时长。
                120,
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
    mgr.spawn_watcher();
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
                // 120s 而非 30s：与 roundtrip 同理（2026-09-08 全量满载下
                // 5s 出站窗口 flake 后全家族统一加宽）；本测试断言 /deny
                // 裁决路径，不是超时拒绝，放宽只影响失败时的等待时长。
                120,
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

// ---------------------------------------------------------------------------
// ⑥ 订阅先于 spawn 的竞态回归（2026-09-08 全量三次复发根修）
// ---------------------------------------------------------------------------

// 回执在 watcher 首次 poll 前发布必须仍被处理。旧实现订阅在任务内进行：
// `tokio::spawn` 只入队、订阅等首次 poll 才生效，窗口期发布的 /approve 对
// 尚不存在的订阅者被 broadcast 直接丢弃（bus warn `no inbound receivers`），
// ask 等满超时被误拒——加宽超时窗口无解（消息根本没进 watcher）。修后：
// 先订阅（回执进 broadcast 缓冲）→ 发布回执 → 再启动主循环 → 缓冲回执被
// 处理、裁决送达。接收者在 send 前创建即必达，broadcast 语义保证确定性。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approve_published_before_watcher_first_poll_is_delivered() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    let mut out_rx = bus.subscribe_outbound();

    // 先订阅：此刻起回执只进缓冲，不依赖 watcher 何时被 poll。
    let rx = mgr.subscribe_inbound();

    let mgr_for_ask = mgr.clone();
    let ask = std::thread::spawn(move || {
        mgr_for_ask
            .request_approval_sync_ctx(
                "req-uuid-cc33dd44",
                "file_write",
                "/tmp/x",
                "HIGH",
                "pre-subscribe race",
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

    // watcher 尚未启动：此刻发布回执（旧实现在等价时序下永久丢弃此消息）。
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id}")));

    // 再启动主循环——缓冲中的回执必须被处理。
    tokio::spawn(mgr.clone().watcher_with_rx(rx));

    let verdict = ask.join().unwrap();
    assert!(verdict.approved);
    let notice = next_outbound(&mut out_rx).await;
    assert!(notice.content.contains(&id));
    assert!(notice.content.contains("已批准"));
}

// ---------------------------------------------------------------------------
// ⑦ Lagged 容错（2026-09-22 审查 REL-001）
// ---------------------------------------------------------------------------

// 旧实现 `while let Ok(msg) = rx.recv().await` 把 `Lagged` 当通道终结：缓冲
// 冲掉未消费消息的瞬间 watcher 永久退出，此后所有 `/approve` 无人消费，审批
// 全部等满超时误拒。修后 Lagged 记 warn 继续，仅 Closed 退出。容量 1 的 bus
// 确定性触发 Lagged：订阅后不启动 watcher、连发 3 条噪声（容量 1 只留最后
// 1 条，seq 缺口 ≥2）→ watcher 首次 recv 必得 Lagged，随后的 /approve 仍须
// 被裁决。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_survives_inbound_lag_and_keeps_resolving() {
    let bus = Arc::new(MessageBus::with_capacity(1));
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    let mut out_rx = bus.subscribe_outbound();

    // 先订阅（接收者在 send 前创建即必达进缓冲），此刻不启动主循环。
    let rx = mgr.subscribe_inbound();

    let mgr_for_ask = mgr.clone();
    let ask = std::thread::spawn(move || {
        mgr_for_ask
            .request_approval_sync_ctx(
                "req-uuid-l4g00001",
                "file_write",
                "/tmp/x",
                "HIGH",
                "lag regression",
                120,
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

    // 3 条噪声冲爆容量 1 的缓冲（旧实现在 watcher 启动后第一条 recv 即
    // Lagged 退出）。
    for i in 0..3 {
        bus.publish_inbound(inbound("telegram", "chat-9", &format!("noise-{i}")));
    }

    // 启动主循环：容量 1 下 /approve 已覆盖最后一条噪声，首次 recv 必得
    // Lagged（旧实现在此退出 → /approve 无人消费 → ask 超时误拒），继续后
    // 下一条 recv 直接拿到 /approve 并正常裁决。
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id}")));
    tokio::spawn(mgr.clone().watcher_with_rx(rx));

    let verdict = ask.join().unwrap();
    assert!(
        verdict.approved,
        "watcher must survive lag and resolve the reply"
    );
    let notice = next_outbound(&mut out_rx).await;
    assert!(notice.content.contains(&id));
    assert!(notice.content.contains("已批准"));
}

// ---------------------------------------------------------------------------
// wave4 追加（coverage）：残余臂——诚实 Err 双入口、组合分流 IM 臂 +
// 无上下文臂 + is_running、编号冲突加长环、watcher「未找到」与
// 「等待方已弃等」竞态兜底、runtime 内 block_in_place 等待臂。
// （pending 锁中毒臂与 Closed 退出臂不可从公共面触达：前者需毒化
// Mutex，后者 watcher 自持 Arc<Self> → bus 恒存活。）
// ---------------------------------------------------------------------------

/// 卡片「编号: 」行提取。
fn extract_id(card: &str) -> String {
    card.lines()
        .find(|l| l.starts_with("编号: "))
        .map(|l| l["编号: ".len()..].trim().to_string())
        .expect("card has id line")
}

/// 无来源上下文的 ask 落到通道管理器 = 编程错误信号 → 诚实 Err。
#[test]
fn channel_request_without_ctx_is_honest_error() {
    let bus = Arc::new(MessageBus::new());
    let mgr = ChannelApprovalManager::new(bus);
    let err = mgr
        .request_approval_sync("req-x", "op", "t", "HIGH", "why", 5)
        .expect_err("no-ctx ask must be rejected honestly");
    assert!(err.contains("requires origin context"), "err: {err}");
}

/// 空 chat_id 的上下文无法寻址对话 → 诚实 Err，且早退在挂表/发卡之前。
#[test]
fn channel_request_empty_chat_id_is_error() {
    let bus = Arc::new(MessageBus::new());
    let mut out_rx = bus.subscribe_outbound();
    let mgr = ChannelApprovalManager::new(bus);
    let err = mgr
        .request_approval_sync_ctx(
            "req-y",
            "op",
            "t",
            "HIGH",
            "why",
            5,
            &ctx_of("telegram", ""),
        )
        .expect_err("empty chat_id must be rejected");
    assert!(err.contains("empty chat_id"), "err: {err}");
    assert!(
        out_rx.try_recv().is_err(),
        "早退必须发生在发卡之前，不得有出站"
    );
}

/// 组合管理器残余臂：is_running 委托、无上下文 ask → 默认管理器、
/// IM 上下文 → 通道卡片全链（默认管理器不得被 IM 请求触达）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn composite_routes_im_to_channel_and_no_ctx_to_default() {
    let bus = Arc::new(MessageBus::new());
    let mock = Arc::new(RecordingMock::default());
    let channel = Arc::new(ChannelApprovalManager::new(bus.clone()));
    let composite = Arc::new(CompositeApprovalManager::new(mock.clone(), channel.clone()));
    assert!(composite.is_running(), "composite delegates is_running");

    // 无上下文 ask → 默认管理器（现状不变）。
    composite
        .request_approval_sync("r0", "op", "t", "HIGH", "why", 5)
        .unwrap();
    assert_eq!(mock.calls.load(std::sync::atomic::Ordering::SeqCst), 1);

    // IM 上下文 → 通道卡片全链。
    channel.spawn_watcher();
    let mut out_rx = bus.subscribe_outbound();
    let c = composite.clone();
    let ask = std::thread::spawn(move || {
        c.request_approval_sync_ctx(
            "req-uuid-im515151",
            "file_write",
            "/tmp/x",
            "HIGH",
            "via composite",
            120,
            &CTX_IM(),
        )
        .unwrap()
    });
    let card = next_outbound(&mut out_rx).await;
    assert_eq!(card.channel, "telegram");
    let id = extract_id(&card.content);
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id}")));
    let v = ask.join().unwrap();
    assert!(v.approved);
    assert_eq!(
        mock.calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "IM 上下文不得漏到默认管理器"
    );
}

/// 编号冲突 → 逐级加长：第一条挂表后，同 8 位前缀的第二条请求自动换用
/// 全量 request_id 当编号，两条互不干扰、各自可批复。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn id_collision_lengthens_short_id() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    mgr.spawn_watcher();
    let mut out_rx = bus.subscribe_outbound();

    let m1 = mgr.clone();
    let ask1 = std::thread::spawn(move || {
        m1.request_approval_sync_ctx("collide-aaaa", "op", "t", "HIGH", "first", 120, &CTX_IM())
            .unwrap()
    });
    let card1 = next_outbound(&mut out_rx).await;
    let id1 = extract_id(&card1.content);
    assert_eq!(id1, "collide-", "首请求取 request_id 前 8 位");

    // 第二条同前缀（第一条已挂表——卡片晚于挂表发出）：8 位候选撞车 →
    // 加长到 request_id 全量（长度 12）。
    let m2 = mgr.clone();
    let ask2 = std::thread::spawn(move || {
        m2.request_approval_sync_ctx("collide-bbbb", "op", "t", "HIGH", "second", 120, &CTX_IM())
            .unwrap()
    });
    let card2 = next_outbound(&mut out_rx).await;
    let id2 = extract_id(&card2.content);
    assert_ne!(id1, id2, "撞车必须换编号");
    assert_eq!(id2, "collide-bbbb", "加长后用 request_id 全量");

    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id1}")));
    bus.publish_inbound(inbound("telegram", "chat-9", &format!("/approve {id2}")));
    assert!(ask1.join().unwrap().approved);
    assert!(ask2.join().unwrap().approved);
}

/// 未知编号回执 → watcher 回「未找到」通知（可能已超时或已处理）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_unknown_id_gets_not_found_notice() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    mgr.spawn_watcher();
    let mut out_rx = bus.subscribe_outbound();

    bus.publish_inbound(inbound("telegram", "chat-9", "/approve deadbeef"));
    let notice = next_outbound(&mut out_rx).await;
    assert_eq!(notice.channel, "telegram");
    assert_eq!(notice.chat_id, "chat-9");
    assert!(
        notice.content.contains("未找到待审批请求 deadbeef"),
        "notice: {}",
        notice.content
    );
}

/// 等待方已弃等（超时摘牌前回执已到）的竞态兜底：tx.send Err → watcher
/// 静默 continue，不回执通知（摘牌归超时方）。直挂一个接收端已丢弃的
/// pending（测试子模块可触私有结构）确定性命中该臂。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_silently_skips_when_waiter_already_gave_up() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    mgr.spawn_watcher();
    let mut out_rx = bus.subscribe_outbound();

    let (tx, rx) = mpsc::channel::<ApprovalVerdict>();
    drop(rx);
    mgr.pending.lock().unwrap().insert(
        "deadc0de".to_string(),
        ChannelPendingEntry {
            channel: "telegram".to_string(),
            chat_id: "chat-9".to_string(),
            sender_id: "u1".to_string(),
            created_at: std::time::Instant::now(),
            tx,
        },
    );
    bus.publish_inbound(inbound("telegram", "chat-9", "/approve deadc0de"));
    let res = tokio::time::timeout(Duration::from_millis(700), out_rx.recv()).await;
    assert!(
        res.is_err(),
        "弃等竞态不得回执通知，got {:?}",
        res.map(|r| r.map(|m| m.content))
    );
}

/// runtime 上下文内直接 ask → Handle::try_current() 命中 → block_in_place
/// 等待臂：阻塞 ask 占住当前 worker，watcher + 解析任务在其余 worker 上
/// 完成裁决回路。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_inside_runtime_uses_block_in_place_and_resolves() {
    let bus = Arc::new(MessageBus::new());
    let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
    mgr.spawn_watcher();
    let mut out_rx = bus.subscribe_outbound();

    // 解析任务：等卡片 → 原对话批准。
    let bus_for_resolver = bus.clone();
    let resolver = tokio::spawn(async move {
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(30), out_rx.recv())
                .await
                .expect("card within 30s")
                .expect("outbound ok");
            if let Some(line) = msg.content.lines().find(|l| l.starts_with("编号: ")) {
                let id = line["编号: ".len()..].trim().to_string();
                bus_for_resolver.publish_inbound(inbound(
                    "telegram",
                    "chat-9",
                    &format!("/approve {id}"),
                ));
                return;
            }
        }
    });

    let verdict = mgr
        .request_approval_sync_ctx(
            "req-inplace-01",
            "file_write",
            "/tmp/x",
            "HIGH",
            "in-runtime",
            30,
            &CTX_IM(),
        )
        .unwrap();
    assert!(verdict.approved);
    resolver.await.unwrap();
}

// ===========================================================================
// wave5 round2 batch-2（2026-09-25）：watcher 对非裁决消息（噪声）的静默
// 跳过臂——publish 一条无 /approve //deny 前缀的普通入站消息，watcher 不得
// 产出任何出站、不得摘任何牌。
// ===========================================================================

mod w5b2 {
    use super::*; // inbound / ChannelApprovalManager / MessageBus / Arc / Duration 同源

    /// 噪声消息 → parse_reply None → continue 臂：无出站、无 panic。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w5_watcher_ignores_non_reply_noise() {
        let bus = Arc::new(MessageBus::new());
        let mgr = Arc::new(ChannelApprovalManager::new(bus.clone()));
        mgr.spawn_watcher();
        let mut out_rx = bus.subscribe_outbound();

        // 普通 chatter（无审批动词）→ watcher 静默跳过。
        bus.publish_inbound(inbound("telegram", "chat-9", "今天天气不错"));
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(out_rx.try_recv().is_err(), "噪声不得触发任何出站卡片/通知");
    }
}
