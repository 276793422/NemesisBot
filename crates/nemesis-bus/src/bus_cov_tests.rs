//! MessageBus 发送失败分支补充测试（订阅者全部先行 drop → send Err →
//! dropped 计数器递增）。

use super::*;
use nemesis_types::channel::OutboundMessage;

fn inbound() -> InboundMessage {
    InboundMessage::user_ingress(
        "test".to_string(),
        "u".to_string(),
        "c".to_string(),
        "m".to_string(),
    )
    .with_session_key("test:c".to_string())
}

fn outbound() -> OutboundMessage {
    OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c".to_string(),
        content: "m".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    }
}

/// inbound：接收者存在但已全部 drop → broadcast send Err → dropped 计数。
#[tokio::test]
async fn inbound_send_error_increments_dropped() {
    let bus = MessageBus::new();
    {
        let _rx = bus.subscribe_inbound();
        // drop 前 receiver_count = 1
    } // _rx 在此 drop → receiver_count 归 0
    bus.publish_inbound(inbound());
    assert_eq!(bus.no_receiver_dropped_inbound(), 1);
}

/// outbound：同型路径（send Err → warn + no_receiver_dropped_outbound 递增）。
#[tokio::test]
async fn outbound_send_error_increments_dropped() {
    let bus = MessageBus::new();
    {
        let _rx = bus.subscribe_outbound();
    }
    bus.publish_outbound(outbound());
    assert_eq!(bus.no_receiver_dropped_outbound(), 1);
}

/// 无接收者直接发布 → no-receiver 分支（不进 channel）。
#[test]
fn publish_without_receivers_counts_dropped() {
    let bus = MessageBus::new();
    bus.publish_inbound(inbound());
    bus.publish_outbound(outbound());
    assert_eq!(bus.no_receiver_dropped_inbound(), 1);
    assert_eq!(bus.no_receiver_dropped_outbound(), 1);
}

/// 关闭后发布被拒（closed 分支）。
#[test]
fn closed_bus_rejects_publish() {
    let bus = MessageBus::new();
    let _rx = bus.subscribe_inbound();
    let _rx2 = bus.subscribe_outbound();
    assert!(!bus.is_closed());
    bus.close();
    assert!(bus.is_closed());
    bus.publish_inbound(inbound());
    bus.publish_outbound(outbound());
    assert_eq!(bus.no_receiver_dropped_inbound(), 0);
    assert_eq!(bus.no_receiver_dropped_outbound(), 0);
}

/// 订阅者计数与 fan-out warn-once 旗标（第二次订阅触发 warned）。
#[test]
fn subscriber_count_and_sender_helpers() {
    let bus = MessageBus::new();
    assert_eq!(bus.inbound_subscriber_count(), 0);
    assert_eq!(bus.outbound_subscriber_count(), 0);
    let _rx = bus.subscribe_inbound();
    let _rx2 = bus.subscribe_inbound(); // 第二个订阅者 → fan-out warn 一次
    let _ox = bus.subscribe_outbound();
    let _ox2 = bus.subscribe_outbound();
    assert_eq!(bus.inbound_subscriber_count(), 2);
    assert_eq!(bus.outbound_subscriber_count(), 2);
    // sender 克隆通路：绕过 bus 直接 send 也能送达
    let tx = bus.inbound_sender();
    let mut rx = bus.subscribe_inbound();
    tx.send(inbound()).expect("direct send");
    assert_eq!(rx.try_recv().expect("recv").channel, "test");
    let _ = bus.outbound_sender();
}

/// 订阅 info 日志的 `total receivers` 必须是**订阅后**
/// 总数——旧实现打新增前计数，首个订阅者打 total=0，恒差 1，排障时订阅
/// 拓扑口径失真。
///
/// 断言方式是子进程自调（current_exe + --exact 重入自身，env 旗标走子分
/// 支）：tracing 的 callsite interest 是**进程级**一次性缓存，同二进制的
/// 其他并行测试会以无订阅者状态先注册这些 info! 调用点（interest=never
/// 被永久缓存），线程局部捕获订阅者收不到事件（并行下捕获缓冲区
/// 恒空，rebuild_interest_cache 与并行首注册有清-抢竞态也救不了）；子进
/// 程里本测试是唯一测试，调用点首注册必然命中捕获订阅者——cargo test
/// 并行与 nextest 单测进程两种语义下都确定性成立。
#[test]
fn subscribe_log_reports_post_subscription_total() {
    // 子分支：新进程内唯一测试 → 安装捕获订阅者后触发订阅，日志回显 stdout。
    if std::env::var_os("NB_BUS_CAPLOG_CHILD").is_some() {
        use std::sync::Arc;
        use std::sync::Mutex as StdMutex;

        struct SharedBuf(Arc<StdMutex<Vec<u8>>>);
        impl std::io::Write for SharedBuf {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let buf = Arc::new(StdMutex::new(Vec::new()));
        let writer_buf = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_ansi(false)
            .with_writer(move || SharedBuf(writer_buf.clone()))
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let bus = MessageBus::new();
            let _rx = bus.subscribe_inbound(); // 首个入站订阅者 → total=1
            let _rx2 = bus.subscribe_inbound(); // 第二个入站订阅者 → total=2
            let _ox = bus.subscribe_outbound(); // 首个出站订阅者 → total=1
        });
        println!(
            "NB_CAPLOG<<<{}>>>",
            String::from_utf8(buf.lock().unwrap().clone()).unwrap()
        );
        return;
    }

    // 父分支：拉起仅含本测试的子进程，校验捕获日志。
    let exe = std::env::current_exe().expect("current_exe");
    let out = std::process::Command::new(exe)
        .args([
            "bus_cov_tests::subscribe_log_reports_post_subscription_total",
            "--exact",
            "--nocapture",
        ])
        .env("NB_BUS_CAPLOG_CHILD", "1")
        .output()
        .expect("spawn child test process");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let captured = stdout
        .split("NB_CAPLOG<<<")
        .nth(1)
        .and_then(|s| s.split(">>>").next())
        .unwrap_or_default();
    assert!(
        out.status.success(),
        "子进程测试失败：{}\n--- stdout ---\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        captured.contains("New inbound subscriber, total receivers=1"),
        "首个入站订阅者应打 total=1，捕获日志：{captured}"
    );
    assert!(
        captured.contains("New outbound subscriber, total receivers=1"),
        "首个出站订阅者应打 total=1，捕获日志：{captured}"
    );
    assert!(
        captured.contains("New inbound subscriber, total receivers=2"),
        "第二个入站订阅者应打 total=2，捕获日志：{captured}"
    );
}

// —— msg_class 检测网自证锁（2026-11-21 msg_class 架构 B2/B3，§11）——
// 三条逃逸路（B3 收窄标记表）× panic 态 + warn 态放行 + 纯净 User 放行。

/// panic 态：User class + 任一内部形状标记 → 当场炸（catch_unwind 断言，
/// 不污染进程）。三臂覆盖标记表三族：channel=system / sender 前缀 /
/// metadata 章。
#[test]
fn semantic_assert_panics_on_user_class_with_internal_marker() {
    let bus = MessageBus::new().with_semantic_assert(true);
    let mut esc_channel = inbound();
    esc_channel.channel = "system".to_string();
    let mut esc_sender = inbound();
    esc_sender.sender_id = "cron:job-1".to_string();
    let mut esc_meta = inbound();
    esc_meta
        .metadata
        .insert("system_notice".to_string(), "events".to_string());
    for (name, m) in [
        ("channel=system", esc_channel),
        ("sender cron:*", esc_sender),
        ("metadata:system_notice", esc_meta),
    ] {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            bus.publish_inbound(m.clone());
        }));
        assert!(
            result.is_err(),
            "User class + {name} 必须炸出（with_semantic_assert(true)）"
        );
    }
}

/// warn 态（默认构造）：User class + 标记 → 发布照常成功（兜底观测不拦
/// 运行）；Notice class 携带同款标记 = 检测网早退（构造器家族产物合法，
/// 不误伤）。
#[test]
fn semantic_assert_warn_mode_publishes_through() {
    let bus = MessageBus::new(); // 默认 warn 态
    let mut m = inbound();
    m.metadata
        .insert("cron_job_id".to_string(), "j".to_string());
    let mut rx = bus.subscribe_inbound();
    bus.publish_inbound(m);
    assert!(
        rx.try_recv().is_ok(),
        "warn 态不得拦截发布（消息必须进通道）"
    );
    let notice = InboundMessage::notice(
        nemesis_types::channel::NoticeKind::Events,
        "web",
        "c",
        "s",
        "x",
    );
    bus.publish_inbound(notice);
}

/// 纯净 User（无任何内部形状标记）在 panic 态也恒放行——检测网只抓内部
/// 生产者逃逸，不误伤正常用户消息。
#[test]
fn semantic_assert_clean_user_class_passes() {
    let bus = MessageBus::new().with_semantic_assert(true);
    let mut rx = bus.subscribe_inbound();
    bus.publish_inbound(inbound());
    assert!(rx.try_recv().is_ok(), "纯净 User 恒放行");
}
