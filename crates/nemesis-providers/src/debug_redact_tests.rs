//! SEC-002（BUG-20261111-068 家族 providers 侧补漏）回归锁。
//!
//! 六个持明文凭据的 provider 结构（ProviderSelection / FactoryConfig /
//! AnthropicConfig / CodexConfig / HttpProviderConfig / OpenAICompatConfig）
//! 不得 derive(Debug)：`{:?}` / `{:#?}` 输出必须脱敏 api_key / proxy /
//! headers map，同时保留 plain 字段（base_url / model 等）的诊断价值。
//! 威胁模型见 [`crate::debug_redact`] 模块文档。

use crate::anthropic::AnthropicConfig;
use crate::codex::CodexConfig;
use crate::codex_credentials::{CodexCliAuth, CodexCliTokens, CodexCredentials};
use crate::factory::{FactoryConfig, ProviderSelection, ProviderType};
use crate::http_provider::HttpProviderConfig;
use crate::openai_compat::OpenAICompatConfig;

const KEY: &str = "sk-secret-DO-NOT-LEAK";
const PROXY: &str = "http://user:pass@proxy.corp:3128";
const HEADER_VAL: &str = "Bearer private-header-token";

/// 通用断言：Debug 输出无明文凭据、有脱敏标记、plain 字段保留。
fn assert_redacted(dbg: &str, secrets: &[&str], plain_kept: &[&str], ctx: &str) {
    for s in secrets {
        assert!(
            !dbg.contains(s),
            "[{}] 明文凭据泄漏进 Debug 输出: {}",
            ctx,
            dbg
        );
    }
    assert!(
        dbg.contains("<redacted>"),
        "[{}] 无 <redacted> 脱敏标记（可能漏接宏）: {}",
        ctx,
        dbg
    );
    for p in plain_kept {
        assert!(
            dbg.contains(p),
            "[{}] plain 字段 `{}` 应保留诊断价值: {}",
            ctx,
            p,
            dbg
        );
    }
}

#[test]
fn provider_selection_debug_redacts_api_key() {
    let sel = ProviderSelection {
        provider_type: ProviderType::Anthropic,
        api_key: KEY.to_string(),
        api_base: "https://api.anthropic.com".to_string(),
        model: "claude-sonnet".to_string(),
        workspace: ".".to_string(),
        connect_mode: String::new(),
        account_id: String::new(),
        timeout_secs: 600,
    };
    let dbg = format!("{:?}", sel);
    assert_redacted(
        &dbg,
        &[KEY],
        &["Anthropic", "claude-sonnet", "api.anthropic.com"],
        "ProviderSelection",
    );
    // alternate 形态（dbg!("{:#?}") / panic 展开）同口径
    let pretty = format!("{:#?}", sel);
    assert!(!pretty.contains(KEY), "alternate 形态泄漏: {}", pretty);
    // 空密钥 → <empty>（非 <redacted>）
    let sel_empty = ProviderSelection {
        api_key: String::new(),
        ..sel
    };
    let dbg_empty = format!("{:?}", sel_empty);
    assert!(
        dbg_empty.contains("<empty>"),
        "空密钥应显示 <empty>: {}",
        dbg_empty
    );
}

#[test]
fn factory_config_debug_redacts_key_proxy_headers() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("Authorization".to_string(), HEADER_VAL.to_string());
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: KEY.to_string(),
        api_base: "https://api.anthropic.com".to_string(),
        workspace: ".".to_string(),
        connect_mode: String::new(),
        account_id: "org-1".to_string(),
        protocol: "anthropic".to_string(),
        headers,
        timeout_secs: 600,
        proxy: PROXY.to_string(),
        stream_mode: String::new(),
        cache_breakpoints: true,
        extra_body: serde_json::json!({"temperature": 0.5}),
    };
    let dbg = format!("{:?}", cfg);
    // proxy 是 String 形态（可带 userinfo 凭据），同走 secret
    assert_redacted(
        &dbg,
        &[KEY, PROXY, "user:pass", HEADER_VAL],
        &["claude-sonnet", "org-1", "anthropic"],
        "FactoryConfig",
    );
    let pretty = format!("{:#?}", cfg);
    assert!(
        !pretty.contains(KEY) && !pretty.contains(PROXY) && !pretty.contains(HEADER_VAL),
        "alternate 形态泄漏: {}",
        pretty
    );
    // Default（空值全脱敏为 <empty>，保 Clone/Default 派生不变）
    let dbg_default = format!("{:?}", FactoryConfig::default());
    assert!(
        dbg_default.contains("<empty>"),
        "空 Default 应 <empty>: {}",
        dbg_default
    );
    assert!(
        !dbg_default.contains("<redacted>"),
        "空 Default 不该有 <redacted>: {}",
        dbg_default
    );
}

#[test]
fn anthropic_config_debug_redacts() {
    let cfg = AnthropicConfig {
        api_key: KEY.to_string(),
        base_url: "https://api.anthropic.com".to_string(),
        default_model: "claude-sonnet".to_string(),
        timeout_secs: 600,
        proxy: Some(PROXY.to_string()),
        stream_mode: String::new(),
        cache_breakpoints: false,
    };
    let dbg = format!("{:?}", cfg);
    assert_redacted(
        &dbg,
        &[KEY, PROXY],
        &["claude-sonnet", "api.anthropic.com"],
        "AnthropicConfig",
    );
    let pretty = format!("{:#?}", cfg);
    assert!(
        !pretty.contains(KEY) && !pretty.contains(PROXY),
        "alternate 形态泄漏: {}",
        pretty
    );
}

#[test]
fn codex_config_debug_redacts() {
    let cfg = CodexConfig {
        api_key: KEY.to_string(),
        account_id: "org-1".to_string(),
        default_model: "gpt-5.2".to_string(),
        base_url: "https://chatgpt.com/backend-api/codex".to_string(),
        enable_web_search: true,
        timeout_secs: 600,
        proxy: Some(PROXY.to_string()),
        stream_mode: String::new(),
    };
    let dbg = format!("{:?}", cfg);
    assert_redacted(&dbg, &[KEY, PROXY], &["gpt-5.2", "org-1"], "CodexConfig");
}

#[test]
fn http_provider_config_debug_redacts_key_proxy_headers() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("X-Api-Key".to_string(), HEADER_VAL.to_string());
    let cfg = HttpProviderConfig {
        name: "http-compat".to_string(),
        base_url: "https://api.vendor.com/v1".to_string(),
        api_key: KEY.to_string(),
        default_model: "vendor-model".to_string(),
        timeout_secs: 600,
        headers,
        proxy: Some(PROXY.to_string()),
        preserve_prefix: false,
        stream_mode: String::new(),
        extra_body: serde_json::Value::Null,
    };
    let dbg = format!("{:?}", cfg);
    assert_redacted(
        &dbg,
        &[KEY, PROXY, HEADER_VAL],
        &["vendor-model", "api.vendor.com"],
        "HttpProviderConfig",
    );
    // 空表形态：headers <empty> 且无 <redacted> 误报（无秘密可隐）
    let mut cfg_empty = cfg.clone();
    cfg_empty.headers = std::collections::HashMap::new();
    cfg_empty.api_key = String::new();
    cfg_empty.proxy = None;
    let dbg_empty = format!("{:?}", cfg_empty);
    assert!(
        dbg_empty.contains("<empty>"),
        "全空配置应显示 <empty>: {}",
        dbg_empty
    );
    assert!(
        !dbg_empty.contains("<redacted>"),
        "全空配置不该有 <redacted>: {}",
        dbg_empty
    );
}

#[test]
fn openai_compat_config_debug_redacts() {
    let cfg = OpenAICompatConfig {
        name: "zhipu".to_string(),
        base_url: "https://open.bigmodel.cn/api/paas/v4".to_string(),
        api_key: KEY.to_string(),
        default_model: "glm-4.7".to_string(),
        timeout_secs: 600,
        proxy: Some(PROXY.to_string()),
    };
    let dbg = format!("{:?}", cfg);
    assert_redacted(
        &dbg,
        &[KEY, PROXY],
        &["glm-4.7", "bigmodel.cn"],
        "OpenAICompatConfig",
    );
    let pretty = format!("{:#?}", cfg);
    assert!(
        !pretty.contains(KEY) && !pretty.contains(PROXY),
        "alternate 形态泄漏: {}",
        pretty
    );
}

#[test]
fn redact_helpers_three_states() {
    use crate::debug_redact::{debug_secret_map, debug_secret_option, debug_secret_value};
    // value：空 <empty> / 非空 <redacted>
    assert_eq!(debug_secret_value(""), "<empty>");
    assert_eq!(debug_secret_value(KEY), "<redacted>");
    // option：None / Some("") <empty>；Some(非空) <redacted>
    assert_eq!(debug_secret_option(None), "<empty>");
    assert_eq!(debug_secret_option(Some("")), "<empty>");
    assert_eq!(debug_secret_option(Some(KEY)), "<redacted>");
    // map：空表 <empty> / 非空 <redacted>（整体脱敏不逐键判断）
    let empty_map: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    assert_eq!(debug_secret_map(&empty_map), "<empty>");
    let mut one = std::collections::HashMap::new();
    one.insert("Authorization".to_string(), HEADER_VAL.to_string());
    assert_eq!(debug_secret_map(&one), "<redacted>");
}

/// BUG-20261118-171：codex_credentials 三结构（auth.json 解析形态 +
/// 运行时持有形态，2026-05 即存在、同 crate 清扫漏网）与六结构同锁。
/// OAuth access/refresh token 脱敏；account_id 非凭据归 plain（与
/// ProviderSelection 同口径）；CodexCliAuth 的嵌套 tokens 经子结构
/// 脱敏 Debug 传递。
#[test]
fn codex_credentials_family_debug_redacts_tokens() {
    let tokens = CodexCliTokens {
        access_token: "access-DO-NOT-LEAK".to_string(),
        refresh_token: "refresh-DO-NOT-LEAK".to_string(),
        account_id: "acct-visible".to_string(),
    };
    let dbg = format!("{:?}", tokens);
    assert_redacted(
        &dbg,
        &["access-DO-NOT-LEAK", "refresh-DO-NOT-LEAK"],
        &["acct-visible"],
        "CodexCliTokens",
    );

    let auth = CodexCliAuth { tokens };
    let dbg = format!("{:?}", auth);
    assert_redacted(
        &dbg,
        &["access-DO-NOT-LEAK", "refresh-DO-NOT-LEAK"],
        &["acct-visible"],
        "CodexCliAuth",
    );
    let pretty = format!("{:#?}", auth);
    assert!(
        !pretty.contains("access-DO-NOT-LEAK") && !pretty.contains("refresh-DO-NOT-LEAK"),
        "alternate 形态泄漏: {}",
        pretty
    );

    let creds = CodexCredentials {
        access_token: KEY.to_string(),
        account_id: "acct-visible".to_string(),
        expires_at: std::time::SystemTime::UNIX_EPOCH,
    };
    let dbg = format!("{:?}", creds);
    assert_redacted(&dbg, &[KEY], &["acct-visible"], "CodexCredentials");
    let pretty = format!("{:#?}", creds);
    assert!(!pretty.contains(KEY), "alternate 形态泄漏: {}", pretty);
}
