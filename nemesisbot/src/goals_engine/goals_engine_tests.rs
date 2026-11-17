//! `inject_chat_id` 回归锁（BUG-20261117-goals-echo）。
//!
//! 背景：goals 引擎 CONTINUE 注入消息的 chat_id 曾直接填裸 session_key
//! （`agent:main:session:<id>`），agent 回复出站同 chat_id 被 WebChannel
//! 拒投（只认 `web:<conn>` 形态）——bench `goal_gate` 流 B 的效果面断言
//! 90s 超时。修复 = 注入 chat_id 经 conv_router 解析活跃 web 连接
//! （委托 `post_agent::resolve_inject_chat_id` 单一真相源，events/loops
//! 注入器同款），无绑定回落 `web:<会话id>` 通道原生形态（死会话软失败
//! 语义，回复仍落历史）。
//!
//! 三义钉死：
//! 1. 有绑定 → target 解析出绑定值（`web:<conn>` 实时形态）；
//! 2. 无绑定 → 回落 `web:<会话id>` 通道原生形态（历史投递，不 panic 不空串）；
//! 3. 绑定更新（latest-wins）→ target 跟进新值（tab 重开场景）。

use super::{inject_chat_id, note_outbound_lagged};

/// 会话键形态（与生产注入路径一致的裸 session_key 输入）。
const SESSION_KEY: &str = "agent:main:session:goal-echo-test";

#[test]
fn bound_session_resolves_to_web_chat_id() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind(SESSION_KEY, "web:conn-abc");
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(chat_id, "web:conn-abc", "有绑定必须解析出 web:<conn> 形态");
}

#[test]
fn unbound_session_falls_back_to_channel_native_form() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(
        chat_id, "web:goal-echo-test",
        "无绑定回落 web:<会话id> 通道原生形态（死会话软失败：回复仍落历史）"
    );
}

#[test]
fn rebinding_follows_latest_bind() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind(SESSION_KEY, "web:conn-old");
    assert_eq!(inject_chat_id(&router, SESSION_KEY), "web:conn-old");
    // tab 关闭重开：新连接重新 bind（latest-wins），注入必须跟进新值。
    router.bind(SESSION_KEY, "web:conn-new");
    assert_eq!(inject_chat_id(&router, SESSION_KEY), "web:conn-new");
}

#[test]
fn other_session_binding_does_not_leak() {
    let router = std::sync::Arc::new(nemesis_web::ConvRouter::new());
    router.bind("agent:main:session:other", "web:conn-other");
    let chat_id = inject_chat_id(&router, SESSION_KEY);
    assert_eq!(
        chat_id, "web:goal-echo-test",
        "其他会话的绑定不得串到本会话（会话键隔离）"
    );
}

// ============================================================
// 出站 Lagged 记账（BUG-20261117-076）
// ============================================================

/// Lagged 丢帧必须先入 bus 出站丢失账本再 warn——`lagged_outbound()`
/// 指标完整性（bus 记账协议要求全部订阅端入账；本 watcher 此前只 warn
/// 是家族横扫的唯一漏网点）。突变验证：摘掉 helper 内 record 调用本测
/// 必红。
#[tokio::test]
async fn outbound_lagged_records_into_bus_accounting() {
    let bus = std::sync::Arc::new(nemesis_bus::MessageBus::with_capacity(2));
    let mut rx = bus.subscribe_outbound();
    // 容量 2 灌 4 条（订阅在先，发布不丢）→ 接收端首帧必报 Lagged(2)。
    for i in 0..4 {
        bus.publish_outbound(nemesis_types::channel::OutboundMessage::new(
            "web",
            "chat-lag",
            &format!("m{i}"),
        ));
    }
    let n = match rx.recv().await {
        Ok(_) => panic!("超容量灌入 + 零消费的首帧 recv 必报 Lagged"),
        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => n,
        Err(e) => panic!("unexpected recv error: {e:?}"),
    };
    assert_eq!(n, 2, "容量 2 灌 4 条：首帧 recv 必报 Lagged(2)");
    assert_eq!(bus.lagged_outbound(), 0, "处理前账本必须还是 0");

    note_outbound_lagged(&bus, n);
    assert_eq!(
        bus.lagged_outbound(),
        2,
        "Lagged(n) 必须入 bus 出站丢失账本（不只 warn）"
    );
}
