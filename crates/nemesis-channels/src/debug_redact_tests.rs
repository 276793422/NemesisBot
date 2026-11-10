//! SEC-002 Debug 脱敏回归——通道侧（BUG-20261111-068 横向排查）。
//!
//! 通道配置结构在 gateway 装配期持有已解析明文凭据，本测试把「Debug
//! 输出不含凭据明文」钉死为回归网：构造带特征凭据值 →
//! `format!("{:?}")` → 断言明文不在场 + `<redacted>` 在场（plain 字段
//! 保留诊断价值）。每测试按同款 feature 门控（通道默认关，feature
//! 缺席时测试编译期消失）。
//!
//! 纪律（debug_redact.rs 模块注释同步）：新增带凭据字段的通道配置结构
//! 必须走 `impl_redacted_debug!` 或手写 Debug，并进本文件补一条断言。

use crate::debug_redact::{debug_secret_option, debug_secret_value};

/// 特征凭据值——若 Debug 泄露必然原样出现。
const SECRET: &str = "SK-SECRET-VALUE-9f8e7d6c5b4a";

/// 断言一个 Debug 输出既不泄露明文、又保留 `<redacted>` 形态。
fn assert_redacted(debug_out: &str, what: &str) {
    assert!(
        !debug_out.contains(SECRET),
        "{what}: Debug 输出泄露凭据明文: {debug_out}"
    );
    assert!(
        debug_out.contains("<redacted>"),
        "{what}: Debug 输出缺 <redacted> 脱敏形态: {debug_out}"
    );
}

#[test]
fn web_channel_config_debug_redacts_auth_token() {
    let c = crate::web::WebChannelConfig {
        auth_token: SECRET.into(),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "WebChannelConfig");
    // plain 字段保留诊断价值（web/tests.rs 既有断言同款）。
    assert!(
        out.contains("127.0.0.1"),
        "WebChannelConfig: host 应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "websocket")]
fn websocket_channel_config_debug_redacts_auth_token() {
    let c = crate::websocket::WebSocketChannelConfig {
        auth_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "WebSocketChannelConfig");
}

#[test]
#[cfg(feature = "telegram")]
fn telegram_config_debug_redacts_token_and_proxy() {
    let c = crate::telegram::TelegramConfig {
        token: SECRET.into(),
        proxy: Some(format!("socks5://u:{SECRET}@p:1080")),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "TelegramConfig");
    assert!(
        out.contains("api.telegram.org"),
        "TelegramConfig: api_base 应原样输出: {out}"
    );
    // alternate `{:?}` 与 `{:?}` 走同一 impl——一并钉死。
    let pretty = format!("{c:#?}");
    assert!(
        !pretty.contains(SECRET),
        "TelegramConfig alternate: 泄露: {pretty}"
    );
}

#[test]
#[cfg(feature = "email")]
fn email_config_debug_redacts_auth_material() {
    let c = crate::email::EmailConfig {
        imap_username: SECRET.into(),
        imap_password: SECRET.into(),
        smtp_username: Some(SECRET.into()),
        smtp_password: Some(SECRET.into()),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert!(
        out.matches("<redacted>").count() >= 4,
        "EmailConfig: 四个认证材料字段都应脱敏: {out}"
    );
    assert_redacted(&out, "EmailConfig");
    assert!(
        out.contains("INBOX"),
        "EmailConfig: plain 字段应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "slack")]
fn slack_config_debug_redacts_both_tokens() {
    let c = crate::slack::SlackConfig {
        bot_token: SECRET.into(),
        app_token: SECRET.into(),
        allow_from: vec![],
    };
    let out = format!("{c:?}");
    assert!(
        out.matches("<redacted>").count() >= 2,
        "SlackConfig: 双 token 都应脱敏: {out}"
    );
    assert_redacted(&out, "SlackConfig");
}

#[test]
#[cfg(feature = "feishu")]
fn feishu_config_debug_redacts_three_secrets() {
    let c = crate::feishu::FeishuConfig {
        app_id: "cli_a".into(),
        app_secret: SECRET.into(),
        verification_token: SECRET.into(),
        encrypt_key: SECRET.into(),
        allow_from: vec![],
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "FeishuConfig");
    assert!(
        out.contains("cli_a"),
        "FeishuConfig: app_id 应原样输出: {out}"
    );
}

#[test]
fn webhook_inbound_config_debug_redacts_api_key() {
    let c = crate::webhook_inbound::WebhookInboundConfig {
        api_key: SECRET.into(),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "WebhookInboundConfig");
    assert!(
        out.contains(":9090"),
        "WebhookInboundConfig: listen_addr 应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "discord")]
fn discord_config_debug_redacts_token() {
    let c = crate::discord::DiscordConfig {
        token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "DiscordConfig");
}

#[test]
#[cfg(feature = "line")]
fn line_config_debug_redacts_secrets() {
    let c = crate::line::LineConfig {
        channel_access_token: SECRET.into(),
        channel_secret: SECRET.into(),
        webhook_host: String::new(),
        webhook_port: 18791,
        webhook_path: "/webhook/line".into(),
        allow_from: vec![],
    };
    let out = format!("{c:?}");
    assert!(
        out.matches("<redacted>").count() >= 2,
        "LineConfig: token 与 secret 都应脱敏: {out}"
    );
    assert_redacted(&out, "LineConfig");
    assert!(
        out.contains("18791"),
        "LineConfig: plain 字段应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "tencent")]
fn qq_config_debug_redacts_app_secret() {
    let c = crate::qq::QQConfig {
        app_secret: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "QQConfig");
}

#[test]
#[cfg(feature = "dingtalk")]
fn dingtalk_config_debug_redacts_client_secret() {
    let c = crate::dingtalk::DingTalkConfig {
        client_secret: SECRET.into(),
        allow_from: vec![],
        client_id: String::new(),
    };
    assert_redacted(&format!("{c:?}"), "DingTalkConfig");
}

#[test]
#[cfg(feature = "onebot")]
fn onebot_config_debug_redacts_access_token() {
    let some = crate::onebot::OneBotConfig {
        access_token: Some(SECRET.into()),
        ws_url: "ws://127.0.0.1:6700".into(),
        reconnect_interval: 5,
        group_trigger_prefix: vec![],
        allow_from: vec![],
    };
    let out = format!("{some:?}");
    assert_redacted(&out, "OneBotConfig(Some)");
    let none = crate::onebot::OneBotConfig {
        access_token: None,
        ..some
    };
    let out_none = format!("{none:?}");
    assert!(
        !out_none.contains("<redacted>"),
        "OneBotConfig(None): 未配置不应出脱敏形态: {out_none}"
    );
    assert!(
        out_none.contains("<empty>"),
        "OneBotConfig(None): 应为 <empty>: {out_none}"
    );
}

#[test]
#[cfg(feature = "matrix")]
fn matrix_config_debug_redacts_access_token() {
    let c = crate::matrix::MatrixConfig {
        homeserver: "https://matrix.org".into(),
        user_id: "@bot:matrix.org".into(),
        access_token: SECRET.into(),
        room_id: Some("!room:matrix.org".into()),
        allow_from: vec![],
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "MatrixConfig");
    // plain Option 字段（room_id）原样输出。
    assert!(
        out.contains("!room"),
        "MatrixConfig: room_id 应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "mastodon")]
fn mastodon_config_debug_redacts_access_token() {
    let c = crate::mastodon::MastodonConfig {
        server: "https://mastodon.social".into(),
        access_token: SECRET.into(),
        allow_from: vec![],
    };
    assert_redacted(&format!("{c:?}"), "MastodonConfig");
}

#[test]
#[cfg(feature = "bluesky")]
fn bluesky_config_debug_redacts_password() {
    let c = crate::bluesky::BlueskyConfig {
        server: "https://bsky.social".into(),
        handle: "nb.bsky.social".into(),
        password: SECRET.into(),
        did: None,
        poll_interval: 10,
        allow_from: vec![],
    };
    assert_redacted(&format!("{c:?}"), "BlueskyConfig");
}

#[test]
#[cfg(feature = "mqtt")]
fn mqtt_config_debug_redacts_username_password() {
    let c = crate::mqtt::MqttChannelConfig {
        username: SECRET.into(),
        password: SECRET.into(),
        broker_port: 1883,
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "MqttChannelConfig");
    assert!(
        out.contains("1883"),
        "MqttChannelConfig: plain 字段应原样输出: {out}"
    );
}

#[test]
#[cfg(feature = "nostr")]
fn nostr_config_debug_redacts_private_key() {
    let c = crate::nostr::NostrConfig {
        relays: vec!["wss://relay.damus.io".into()],
        private_key: SECRET.into(),
        allow_from: vec![],
        reconnect_secs: 5,
    };
    assert_redacted(&format!("{c:?}"), "NostrConfig");
}

#[test]
#[cfg(feature = "mattermost")]
fn mattermost_config_debug_redacts_bot_token() {
    let c = crate::mattermost::MattermostConfig {
        base_url: "https://mm.example.com".into(),
        bot_token: SECRET.into(),
        allow_from: vec![],
        channels: vec![],
    };
    assert_redacted(&format!("{c:?}"), "MattermostConfig");
}

#[test]
#[cfg(feature = "irc")]
fn irc_config_debug_redacts_password() {
    let some = crate::irc::IRCConfig {
        password: Some(SECRET.into()),
        server: "irc.libera.chat:6697".into(),
        use_tls: true,
        nick: "nb".into(),
        channel: "#nemesisbot".into(),
        allow_from: vec![],
        reconnect_backoff_secs: 5,
        max_reconnect_backoff_secs: 300,
    };
    let out = format!("{some:?}");
    assert_redacted(&out, "IRCConfig(Some)");
    let none = crate::irc::IRCConfig {
        password: None,
        ..some
    };
    let out_none = format!("{none:?}");
    assert!(
        !out_none.contains("<redacted>"),
        "IRCConfig(None): 未配置不应出脱敏形态: {out_none}"
    );
}

#[test]
#[cfg(feature = "wecom")]
fn wecom_config_debug_redacts_url_token_aeskey_and_webhooks_map() {
    use std::collections::HashMap;
    let mut c = crate::wecom::WeComConfig {
        webhook_url: format!("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={SECRET}"),
        token: SECRET.into(),
        encoding_aes_key: SECRET.into(),
        ..Default::default()
    };
    // 空表：无秘密，webhook_url 已脱敏。
    let out_empty = format!("{c:?}");
    assert!(
        !out_empty.contains(SECRET),
        "WeComConfig: 空表态不应泄露: {out_empty}"
    );
    c.webhooks = {
        let mut m = HashMap::new();
        m.insert("chat-a".to_string(), format!("https://hook?key={SECRET}"));
        m
    };
    let out = format!("{c:?}");
    assert!(
        !out.contains(SECRET),
        "WeComConfig: webhooks map 值泄露 webhook key: {out}"
    );
    assert!(out.contains("<redacted>"), "WeComConfig: 缺脱敏形态: {out}");
}

#[test]
#[cfg(feature = "wechat")]
fn wechat_config_debug_redacts_token() {
    let c = crate::wechat::WeChatConfig {
        token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "WeChatConfig");
}

#[test]
#[cfg(feature = "whatsapp")]
fn whatsapp_config_debug_redacts_api_key_and_proxy() {
    let c = crate::whatsapp::WhatsAppConfig {
        api_key: Some(SECRET.into()),
        proxy: Some(format!("http://user:{SECRET}@proxy:8080")),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "WhatsAppConfig");
    assert!(
        !out.contains("user:"),
        "WhatsAppConfig: proxy userinfo 泄露: {out}"
    );
}

#[test]
fn helpers_empty_vs_nonempty() {
    assert_eq!(debug_secret_value(""), "<empty>");
    assert_eq!(debug_secret_value("x"), "<redacted>");
    assert_eq!(debug_secret_option(None), "<empty>");
    assert_eq!(debug_secret_option(Some("")), "<empty>");
    assert_eq!(debug_secret_option(Some("x")), "<redacted>");
}
