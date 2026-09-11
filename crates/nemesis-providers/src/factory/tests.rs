use super::*;

#[test]
fn test_resolve_anthropic() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet-4-5".to_string(),
        api_key: "test-key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
    assert_eq!(sel.model, "claude-sonnet-4-5");
    assert_eq!(sel.api_base, "https://api.anthropic.com");
}

#[test]
fn test_resolve_claude_cli() {
    let cfg = FactoryConfig {
        llm_ref: "claude-cli/claude-code".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_codex_cli() {
    let cfg = FactoryConfig {
        llm_ref: "codex-cli/codex".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::CodexCli);
}

#[test]
fn test_resolve_github_copilot() {
    let cfg = FactoryConfig {
        llm_ref: "copilot/gpt-4.1".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
}

#[test]
fn test_resolve_http_compat() {
    let cfg = FactoryConfig {
        llm_ref: "deepseek/deepseek-chat".to_string(),
        api_key: "test-key".to_string(),
        api_base: "https://api.deepseek.com/v1".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
}

#[test]
fn test_resolve_http_compat_no_key() {
    let cfg = FactoryConfig {
        llm_ref: "deepseek/deepseek-chat".to_string(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("no API key"));
}

#[test]
fn test_resolve_openai() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-4o".to_string(),
        api_key: "test-key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Codex);
}

#[test]
fn test_resolve_empty_ref() {
    let cfg = FactoryConfig {
        llm_ref: String::new(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_create_provider_anthropic() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "test-key".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "anthropic");
}

#[test]
fn test_create_provider_http() {
    let cfg = FactoryConfig {
        llm_ref: "deepseek/chat".to_string(),
        api_key: "test-key".to_string(),
        api_base: "https://api.deepseek.com/v1".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "http-compat");
}

#[test]
fn test_create_provider_claude_cli() {
    let cfg = FactoryConfig {
        llm_ref: "claude-cli/claude-code".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "claude-cli");
}

#[test]
fn test_create_provider_codex_cli() {
    let cfg = FactoryConfig {
        llm_ref: "codex-cli/default".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "codex-cli");
}

#[test]
fn test_create_provider_copilot() {
    let cfg = FactoryConfig {
        llm_ref: "copilot/gpt-4.1".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "github-copilot");
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_factory_config_default() {
    let cfg = FactoryConfig::default();
    assert!(cfg.llm_ref.is_empty());
    assert!(cfg.api_key.is_empty());
    assert!(cfg.api_base.is_empty());
    assert!(cfg.workspace.is_empty());
    assert!(cfg.connect_mode.is_empty());
    assert!(cfg.account_id.is_empty());
    assert!(cfg.headers.is_empty());
}

#[test]
fn test_resolve_provider_selection_anthropic_default_base() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.api_base, "https://api.anthropic.com");
}

#[test]
fn test_resolve_provider_selection_anthropic_custom_base() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "key".to_string(),
        api_base: "https://custom-proxy.com".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.api_base, "https://custom-proxy.com");
}

#[test]
fn test_resolve_provider_selection_openai_default_base() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-4o".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.api_base, "https://chatgpt.com/backend-api/codex");
}

#[test]
fn test_resolve_provider_selection_workspace_default() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.workspace, ".");
}

#[test]
fn test_resolve_provider_selection_workspace_custom() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude".to_string(),
        api_key: "key".to_string(),
        workspace: "/custom/workspace".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.workspace, "/custom/workspace");
}

#[test]
fn test_resolve_claude_code_alias() {
    let cfg = FactoryConfig {
        llm_ref: "claude-code/claude".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_claudecodec_alias() {
    let cfg = FactoryConfig {
        llm_ref: "claudecodec/default".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_codex_code_alias() {
    let cfg = FactoryConfig {
        llm_ref: "codex-code/default".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::CodexCli);
}

#[test]
fn test_resolve_github_copilot_alias() {
    let cfg = FactoryConfig {
        llm_ref: "github_copilot/gpt-4".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
}

#[test]
fn test_create_provider_codex() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-4o".to_string(),
        api_key: "test-key".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "codex");
}

#[test]
fn test_provider_selection_debug() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    let debug_str = format!("{:?}", sel);
    assert!(debug_str.contains("Anthropic"));
}

#[test]
fn test_provider_type_equality() {
    assert_eq!(ProviderType::HttpCompat, ProviderType::HttpCompat);
    assert_ne!(ProviderType::HttpCompat, ProviderType::Anthropic);
}

#[test]
fn test_resolve_whitespace_only_ref() {
    let cfg = FactoryConfig {
        llm_ref: "   ".to_string(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_resolve_http_compat_with_key_and_base() {
    let cfg = FactoryConfig {
        llm_ref: "zhipu/glm-4".to_string(),
        api_key: "test-key".to_string(),
        api_base: "https://open.bigmodel.cn/api/paas/v4".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
    assert_eq!(sel.model, "glm-4");
    assert_eq!(sel.api_base, "https://open.bigmodel.cn/api/paas/v4");
}

#[test]
fn test_resolve_http_compat_missing_key_error_message() {
    let cfg = FactoryConfig {
        llm_ref: "somevendor/model-x".to_string(),
        api_base: "https://api.vendor.com".to_string(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
    let err_msg = result.unwrap_err();
    assert!(err_msg.contains("no API key"));
    assert!(err_msg.contains("somevendor"));
    assert!(err_msg.contains("model-x"));
}

#[test]
fn test_resolve_provider_selection_preserves_api_key() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "sk-ant-test123".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.api_key, "sk-ant-test123");
}

#[test]
fn test_resolve_provider_selection_preserves_connect_mode() {
    let cfg = FactoryConfig {
        llm_ref: "copilot/gpt-4".to_string(),
        connect_mode: "ide".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.connect_mode, "ide");
}

#[test]
fn test_resolve_provider_selection_preserves_account_id() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-4o".to_string(),
        api_key: "test".to_string(),
        account_id: "org-12345".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.account_id, "org-12345");
}

#[test]
fn test_factory_config_with_headers() {
    let mut headers = std::collections::HashMap::new();
    headers.insert("X-Custom".to_string(), "value".to_string());
    let cfg = FactoryConfig {
        llm_ref: "deepseek/chat".to_string(),
        api_key: "test".to_string(),
        api_base: "https://api.deepseek.com".to_string(),
        headers,
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "http-compat");
}

#[test]
fn test_create_provider_http_with_key() {
    let cfg = FactoryConfig {
        llm_ref: "test/testai-1.1".to_string(),
        api_key: "test-key".to_string(),
        api_base: "http://127.0.0.1:8080/v1".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "http-compat");
    assert_eq!(provider.default_model(), "testai-1.1");
}

#[test]
fn test_provider_type_copy() {
    // Verify ProviderType is Copy (can be used multiple times)
    let pt = ProviderType::Anthropic;
    let pt2 = pt;
    assert_eq!(pt, pt2);
}

#[test]
fn test_provider_selection_debug_format() {
    let sel = ProviderSelection {
        provider_type: ProviderType::Anthropic,
        api_key: "key".to_string(),
        api_base: "https://api.anthropic.com".to_string(),
        model: "claude-3".to_string(),
        workspace: ".".to_string(),
        connect_mode: String::new(),
        account_id: String::new(),
        timeout_secs: 0,
    };
    let debug = format!("{:?}", sel);
    assert!(debug.contains("Anthropic"));
    assert!(debug.contains("claude-3"));
}

// --- 回归锁（2026-09-11 生产实证）：裸模型名不再默认路由 Codex ---
//
// dashboard 曾把裸 model 字段直接当 llm_ref 传入 factory；factory 又把无前缀
// 名默认成 provider=openai → CodexProvider（POST {base}/responses + 模型重映射
// gpt-5.2），第三方 OpenAI 兼容端点全被打错路。现在裸名（含 provider 解析为空
// 的 "/name" 形态）默认 HttpCompat；显式 "openai/x" 前缀行为不变。

#[test]
fn test_resolve_bare_model_name_is_http_compat() {
    let cfg = FactoryConfig {
        llm_ref: "glm-5.3-flash".to_string(),
        api_key: "k".to_string(),
        api_base: "http://127.0.0.1:15721".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
    assert_eq!(sel.model, "glm-5.3-flash");
    assert_eq!(sel.api_base, "http://127.0.0.1:15721");
}

#[test]
fn test_resolve_leading_slash_ref_is_http_compat() {
    // provider_name 解析为空时启动路径会拼出 "/name"——同样必须落 HttpCompat。
    let cfg = FactoryConfig {
        llm_ref: "/my-fine-tune".to_string(),
        api_key: "k".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
}

#[test]
fn test_resolve_openai_prefixed_still_codex() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-5.2".to_string(),
        api_key: "k".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Codex);
}

// --- LLM 协议选择器（2026-09-11）：显式 protocol 钉死 wire 协议，显式 > 推断 ---

#[test]
fn test_explicit_protocol_anthropic_overrides_openai_prefix() {
    // 条目写 "openai/gpt-x" 但显式 protocol=anthropic → 必须走 Claude 消息协议。
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-x".to_string(),
        api_key: "k".to_string(),
        protocol: "anthropic".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
    assert_eq!(
        sel.api_base, "https://api.anthropic.com",
        "空 base 补 anthropic 默认"
    );
}

#[test]
fn test_explicit_protocol_openai_is_chat_completions_not_codex() {
    // 拍板语义（2026-09-11）：显式 openai = chat/completions（业界通行），
    // 与旧前缀 openai/→Codex 的分歧是刻意的。
    let cfg = FactoryConfig {
        llm_ref: "glm-5.3-flash".to_string(),
        api_key: "GLM".to_string(),
        api_base: "http://127.0.0.1:15721".to_string(),
        protocol: "openai".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::HttpCompat);
    assert_eq!(sel.api_base, "http://127.0.0.1:15721", "显式 base 原样保留");
    assert_eq!(sel.model, "glm-5.3-flash", "模型名不被重映射");
}

#[test]
fn test_explicit_protocol_responses_is_codex() {
    let cfg = FactoryConfig {
        llm_ref: "openai/gpt-5.2".to_string(),
        api_key: "k".to_string(),
        protocol: "responses".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Codex);
    assert_eq!(
        sel.api_base, "https://chatgpt.com/backend-api/codex",
        "空 base 补 Responses 默认"
    );
}

#[test]
fn test_explicit_protocol_claude_alias_normalizes() {
    // claude 是 anthropic 的用户侧别名（下拉/CLI 都可能收到）。
    let cfg = FactoryConfig {
        llm_ref: "zhipu/glm-5.3-flash".to_string(),
        api_key: "k".to_string(),
        protocol: "claude".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
}

#[test]
fn test_explicit_protocol_unknown_errors_loud() {
    let cfg = FactoryConfig {
        llm_ref: "zhipu/glm-5.3-flash".to_string(),
        api_key: "k".to_string(),
        protocol: "grpc".to_string(),
        ..Default::default()
    };
    let err = resolve_provider_selection(&cfg).unwrap_err();
    assert!(err.contains("grpc"), "got: {err}");
    assert!(err.contains("anthropic | openai | responses"));
}

#[test]
fn test_explicit_protocol_does_not_override_cli_providers() {
    // CLI 型是本地进程不是 wire 协议——protocol 对它们无意义，前缀优先。
    let cfg = FactoryConfig {
        llm_ref: "claude-cli/claude-code".to_string(),
        protocol: "openai".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_explicit_protocol_case_insensitive() {
    let cfg = FactoryConfig {
        llm_ref: "x/y".to_string(),
        api_key: "k".to_string(),
        protocol: "ANTHROPIC".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
}

// --- P3A 超时对齐（2026-09-12）：per-model timeout 透传链 ---

#[test]
fn test_timeout_passthrough_to_selection() {
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "key".to_string(),
        timeout_secs: 300,
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.timeout_secs, 300);
}

#[test]
fn test_timeout_unset_is_zero() {
    // 0 = 未设置 → create_provider 落 lane 默认（effective_timeout）。
    let cfg = FactoryConfig {
        llm_ref: "anthropic/claude-sonnet".to_string(),
        api_key: "key".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.timeout_secs, 0);
}

#[test]
fn test_effective_timeout_lane_default_and_override() {
    assert_eq!(super::effective_timeout(0), 600);
    assert_eq!(super::effective_timeout(300), 300);
    assert_eq!(super::effective_timeout(1), 1);
}
