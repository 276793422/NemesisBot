//! SEC-002 Debug 脱敏回归（BUG-20261111-068）。
//!
//! 通道/模型配置结构含 token / secret / password / access_token /
//! private_key / proxy（可带 userinfo 凭据）等字段，derive(Debug) 会把
//! 明文凭据带进 Debug/panic/错误上下文日志。WebChannelConfig 与
//! ModelConfig.api_key 已有手写脱敏 Debug（SEC-002 审查），但同族 19 个
//! 结构漏网。本测试把「Debug 输出不含凭据明文」钉死为回归网：每个结构
//! 构造带特征凭据值 → `format!("{:?}")` → 断言明文不在场 + `<redacted>`
//! 在场。
//!
//! 纪律（宏注释同步）：新增带凭据字段的配置结构必须走 `impl_redacted_debug!`
//! 或手写 Debug，并进本文件补一条断言。

use crate::*;

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
fn whatsapp_config_debug_redacts_api_key_and_proxy() {
    let c = WhatsAppConfig {
        api_key: SECRET.into(),
        proxy: format!("http://user:{SECRET}@proxy:8080"),
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
fn telegram_config_debug_redacts_token_and_proxy() {
    let c = TelegramConfig {
        token: SECRET.into(),
        proxy: format!("socks5://u:{SECRET}@p:1080"),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "TelegramConfig");
}

#[test]
fn feishu_config_debug_redacts_three_secrets() {
    let c = FeishuConfig {
        app_secret: SECRET.into(),
        encrypt_key: SECRET.into(),
        verification_token: SECRET.into(),
        app_id: "cli_a".into(),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "FeishuConfig");
    // plain 字段保留诊断价值。
    assert!(
        out.contains("cli_a"),
        "FeishuConfig: app_id 应原样输出: {out}"
    );
}

#[test]
fn discord_config_debug_redacts_token() {
    let c = DiscordConfig {
        token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "DiscordConfig");
}

#[test]
fn qq_config_debug_redacts_app_secret() {
    let c = QqConfig {
        app_secret: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "QqConfig");
}

#[test]
fn dingtalk_config_debug_redacts_client_secret() {
    let c = DingTalkConfig {
        client_secret: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "DingTalkConfig");
}

#[test]
fn slack_config_debug_redacts_both_tokens() {
    let c = SlackConfig {
        bot_token: SECRET.into(),
        app_token: SECRET.into(),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert!(
        out.matches("<redacted>").count() >= 2,
        "SlackConfig: 双 token 都应脱敏: {out}"
    );
    assert_redacted(&out, "SlackConfig");
}

#[test]
fn line_config_debug_redacts_secret_and_access_token() {
    let c = LineConfig {
        channel_secret: SECRET.into(),
        channel_access_token: SECRET.into(),
        webhook_port: 18791,
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "LineConfig");
    assert!(
        out.contains("18791"),
        "LineConfig: plain 字段应原样输出: {out}"
    );
}

#[test]
fn onebot_config_debug_redacts_access_token() {
    let c = OneBotConfig {
        access_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "OneBotConfig");
}

#[test]
fn websocket_channel_config_debug_redacts_auth_token() {
    // 与 WebChannelConfig 同字段名隔壁结构（BUG-20261111-068 最直接的
    // 不对称漏网：同一审查轮次只救了 web 通道）。
    let c = WebSocketChannelConfig {
        auth_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "WebSocketChannelConfig");
}

#[test]
fn matrix_config_debug_redacts_access_token() {
    let c = MatrixConfig {
        access_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "MatrixConfig");
}

#[test]
fn irc_config_debug_redacts_password() {
    let c = IrcConfig {
        password: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "IrcConfig");
}

#[test]
fn mastodon_config_debug_redacts_access_token() {
    let c = MastodonConfig {
        access_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "MastodonConfig");
}

#[test]
fn bluesky_config_debug_redacts_password() {
    let c = BlueskyConfig {
        password: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "BlueskyConfig");
}

#[test]
fn wecom_config_debug_redacts_url_token_aeskey_and_webhooks_map() {
    let mut c = WeComConfig {
        webhook_url: format!("https://qyapi.weixin.qq.com/cgi-bin/webhook/send?key={SECRET}"),
        token: SECRET.into(),
        encoding_aes_key: SECRET.into(),
        ..Default::default()
    };
    // 空表：无秘密，原样 {:?}；非空：整体 <redacted>。
    let out_empty = format!("{c:?}");
    assert!(
        !out_empty.contains(SECRET),
        "WeComConfig: 空表态不应泄露（webhook_url 已脱敏）: {out_empty}"
    );
    c.webhooks
        .insert("chat-a".into(), format!("https://hook?key={SECRET}"));
    let out = format!("{c:?}");
    assert!(
        !out.contains(SECRET),
        "WeComConfig: webhooks map 值泄露 webhook key: {out}"
    );
    assert!(out.contains("<redacted>"), "WeComConfig: 缺脱敏形态: {out}");
}

#[test]
fn mattermost_config_debug_redacts_bot_token() {
    let c = MattermostConfig {
        bot_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "MattermostConfig");
}

#[test]
fn nostr_config_debug_redacts_private_key() {
    let c = NostrConfig {
        private_key: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "NostrConfig");
}

#[test]
fn mqtt_config_debug_redacts_username_and_password() {
    let c = MqttConfig {
        username: SECRET.into(),
        password: SECRET.into(),
        broker_port: 1883,
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "MqttConfig");
    assert!(
        out.contains("1883"),
        "MqttConfig: plain 字段应原样输出: {out}"
    );
}

#[test]
fn wechat_config_debug_redacts_token() {
    let c = WeChatConfig {
        token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "WeChatConfig");
}

#[test]
fn model_config_debug_redacts_api_key_and_proxy() {
    let c = ModelConfig {
        model_name: "test".into(),
        model: "test/testai-1.1".into(),
        api_key: SECRET.into(),
        proxy: format!("http://user:{SECRET}@proxy:8080"),
        ..Default::default()
    };
    let out = format!("{c:?}");
    assert_redacted(&out, "ModelConfig");
    assert!(
        !out.contains("user:"),
        "ModelConfig: proxy userinfo 泄露: {out}"
    );
}

#[test]
fn web_channel_config_debug_redacts_auth_token() {
    // SEC-002 首批已修形态——纳入回归网防退化。
    let c = WebChannelConfig {
        auth_token: SECRET.into(),
        ..Default::default()
    };
    assert_redacted(&format!("{c:?}"), "WebChannelConfig");
}

#[test]
fn debug_secret_value_empty_vs_nonempty() {
    assert_eq!(debug_secret_value(""), "<empty>");
    assert_eq!(debug_secret_value("x"), "<redacted>");
}

#[test]
fn alternate_debug_format_also_redacts() {
    // `{:#?}`（pretty alternate）与 `{:?}` 走同一 impl——一并钉死。
    let c = TelegramConfig {
        token: SECRET.into(),
        ..Default::default()
    };
    let out = format!("{c:#?}");
    assert!(
        !out.contains(SECRET),
        "TelegramConfig alternate: 泄露: {out}"
    );
}

#[test]
fn whole_config_debug_does_not_leak_channel_secrets() {
    // 威胁模型本尊：整个 Config 段落被 `{:?}` 打印（panic/错误上下文）。
    let cfg = Config {
        channels: ChannelsConfig {
            telegram: TelegramConfig {
                token: SECRET.into(),
                ..Default::default()
            },
            nostr: NostrConfig {
                private_key: SECRET.into(),
                ..Default::default()
            },
            ..Default::default()
        },
        model_list: vec![ModelConfig {
            model_name: "test".into(),
            model: "test/testai-1.1".into(),
            api_key: SECRET.into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    let out = format!("{cfg:?}");
    assert!(!out.contains(SECRET), "Config 整段 Debug 泄露凭据: {out}");
    assert!(
        out.contains("<redacted>"),
        "Config 整段 Debug 缺脱敏形态: {out}"
    );
}
