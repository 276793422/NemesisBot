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

use super::inject_chat_id;

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
