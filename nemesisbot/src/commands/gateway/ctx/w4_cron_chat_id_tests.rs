//! [`crate::commands::gateway::ctx::cron_fire_chat_id`] 单测（BUG-20261119-197
//! 回归锁）：Opt2 实时推送的通道闸。核心断言——非 web 通道不得被
//! `web:<ws_conn_id>` 劫持。

use super::cron_fire_chat_id;

/// web 通道 + 会话绑定在 → Opt2 生效（web:<ws_conn_id> 实时推送）。
#[test]
fn web_channel_with_binding_uses_router_target() {
    let chat_id = cron_fire_chat_id(
        "web",
        "agent:main:session:c1",
        Some("web:ws-42".to_string()),
        Some("@fallback"),
    );
    assert_eq!(chat_id, "web:ws-42");
}

/// web 通道 + 会话无绑定 → 回落 `to`（历史投递，软失败语义既有）。
#[test]
fn web_channel_without_binding_falls_back_to_to() {
    let chat_id = cron_fire_chat_id("web", "agent:main:session:c1", None, Some("web:broadcast"));
    assert_eq!(chat_id, "web:broadcast");
}

/// 核心（BUG-20261119-197）：非 web 通道 + 会话绑定在 → 不得被 Opt2
/// 劫持，恒走 `to`。旧形态无通道闸时本测试红（chat_id 被劫持成
/// "web:ws-42"，telegram 侧投递必败）。
#[test]
fn non_web_channel_is_never_hijacked_by_router_target() {
    let chat_id = cron_fire_chat_id(
        "telegram",
        "agent:main:session:c1",
        Some("web:ws-42".to_string()),
        Some("@user"),
    );
    assert_eq!(chat_id, "@user", "telegram 目标不得被 web tab 绑定劫持");
}

/// 非 web 通道 + 无绑定 → `to`（同 fallback）。
#[test]
fn non_web_channel_without_binding_uses_to() {
    let chat_id = cron_fire_chat_id("discord", "agent:main:session:c1", None, Some("chan-9"));
    assert_eq!(chat_id, "chan-9");
}

/// 空 session_key → 无 Opt2 语义（绑定为 None），恒 `to`。
#[test]
fn empty_session_key_skips_opt2() {
    let chat_id = cron_fire_chat_id("web", "", Some("web:ws-42".to_string()), Some("x"));
    assert_eq!(chat_id, "x");
}

/// `to` 缺失 → 空串（出站侧 fail-soft 的既有形态，不 panic）。
#[test]
fn missing_to_yields_empty_string() {
    let chat_id = cron_fire_chat_id("web", "agent:main:session:c1", None, None);
    assert_eq!(chat_id, "");
}
