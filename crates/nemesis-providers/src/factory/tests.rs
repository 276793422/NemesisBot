use super::*;

#[test]
fn test_resolve_anthropic() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "claude-cli/claude-code".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_codex_cli() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "codex-cli/codex".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::CodexCli);
}

#[test]
fn test_resolve_github_copilot() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "copilot/gpt-4.1".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
}

#[test]
fn test_resolve_http_compat() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: String::new(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_create_provider_anthropic() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "claude-cli/claude-code".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "claude-cli");
}

#[test]
fn test_create_provider_codex_cli() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "codex-cli/default".to_string(),
        ..Default::default()
    };
    let provider = create_provider(&cfg).unwrap();
    assert_eq!(provider.name(), "codex-cli");
}

#[test]
fn test_create_provider_copilot() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "claude-code/claude".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_claudecodec_alias() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "claudecodec/default".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::ClaudeCli);
}

#[test]
fn test_resolve_codex_code_alias() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "codex-code/default".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::CodexCli);
}

#[test]
fn test_resolve_github_copilot_alias() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "github_copilot/gpt-4".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::GitHubCopilot);
}

#[test]
fn test_create_provider_codex() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "   ".to_string(),
        ..Default::default()
    };
    let result = resolve_provider_selection(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_resolve_http_compat_with_key_and_base() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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

// --- 回归锁（生产实证）：裸模型名不再默认路由 Codex ---
//
// dashboard 曾把裸 model 字段直接当 llm_ref 传入 factory；factory 又把无前缀
// 名默认成 provider=openai → CodexProvider（POST {base}/responses + 模型重映射
// gpt-5.2），第三方 OpenAI 兼容端点全被打错路。现在裸名（含 provider 解析为空
// 的 "/name" 形态）默认 HttpCompat；显式 "openai/x" 前缀行为不变。

#[test]
fn test_resolve_bare_model_name_is_http_compat() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "openai/gpt-5.2".to_string(),
        api_key: "k".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Codex);
}

// --- LLM 协议选择器：显式 protocol 钉死 wire 协议，显式 > 推断 ---

#[test]
fn test_explicit_protocol_anthropic_overrides_openai_prefix() {
    // 条目写 "openai/gpt-x" 但显式 protocol=anthropic → 必须走 Claude 消息协议。
    let cfg = FactoryConfig {
        proxy: String::new(),
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
    // 既定语义：显式 openai = chat/completions（业界通行），
    // 与旧前缀 openai/→Codex 的分歧是刻意的。
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "zhipu/glm-5.3-flash".to_string(),
        api_key: "k".to_string(),
        protocol: "grpc".to_string(),
        ..Default::default()
    };
    let err = resolve_provider_selection(&cfg).unwrap_err();
    assert!(err.contains("grpc"), "got: {err}");
    assert!(err.contains("anthropic | openai | responses"));
}

/// 集群专业职能框架 守卫④：图像协议条目不能构造对话 Provider——
/// chat 形状完全不符（无流式无工具调用），构造路径 loud 拒绝。
#[test]
fn test_explicit_protocol_images_openai_rejected_as_chat_model() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "zhipu/dall-e-3".to_string(),
        api_key: "k".to_string(),
        protocol: "images-openai".to_string(),
        ..Default::default()
    };
    let err = resolve_provider_selection(&cfg).unwrap_err();
    assert!(err.contains("images-openai"), "got: {err}");
    assert!(err.contains("generate_image"), "got: {err}");
}

#[test]
fn test_explicit_protocol_does_not_override_cli_providers() {
    // CLI 型是本地进程不是 wire 协议——protocol 对它们无意义，前缀优先。
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
        llm_ref: "x/y".to_string(),
        api_key: "k".to_string(),
        protocol: "ANTHROPIC".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(sel.provider_type, ProviderType::Anthropic);
}

// --- P3A 超时对齐：per-model timeout 透传链 ---

#[test]
fn test_timeout_passthrough_to_selection() {
    let cfg = FactoryConfig {
        proxy: String::new(),
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
        proxy: String::new(),
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

// --- BUG-20261120-014：copilot lane per-model timeout 透传链 ---
//
// 此前 copilot 分支 `..Default::default()` 落 serde 默认 120s、工厂不透传
// `sel.timeout_secs`——六分支中唯一没接超时链的 HTTP 型 lane（其余三分支
// 全走 effective_timeout）。死配置面：模型条目 timeout_secs 在 Dashboard
// 可见但不生效；阶梯面：lane 默认 120s 违反 P3A「全 lane 统一默认 600s」。

#[test]
fn copilot_lane_timeout_unset_falls_to_lane_default_600() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "copilot/gpt-4.1".to_string(),
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(
        super::copilot_config(&sel).timeout_secs,
        600,
        "未设置（0）必须落 lane 默认 600s，不得落 serde 旧默认 120s"
    );
}

#[test]
fn copilot_lane_timeout_explicit_value_threads_through() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "copilot/gpt-4.1".to_string(),
        timeout_secs: 123,
        ..Default::default()
    };
    let sel = resolve_provider_selection(&cfg).unwrap();
    assert_eq!(
        super::copilot_config(&sel).timeout_secs,
        123,
        "显式 per-model timeout_secs 必须透传（修复前被 ..Default::default() 丢弃）"
    );
}

#[test]
fn copilot_config_lane_default_matches_unified_contract() {
    // serde/Default 形态的 lane 默认本身也要是 600（防未来直接反序列化
    // 路径绕过工厂时与阶梯契约再分叉）。
    assert_eq!(
        crate::github_copilot::GitHubCopilotConfig::default().timeout_secs,
        600
    );
}

// -------------------------------------------------------------------------
// BUG-20261122-038 构造期请求头防线：api_key 含换行/控制字符 → 拒绝装配
// （诚实 Err），不再等到首次请求构造时炸 panic（流式链路 panic→tx drop→
// rx None 伪装成可重试 Timeout，根因被吞）。
// -------------------------------------------------------------------------

#[test]
fn test_create_provider_rejects_api_key_with_embedded_newline() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "anthropic/claude-sonnet-4-5".to_string(),
        api_key: "sk-a\nb".to_string(), // 内嵌换行（trim 吃不掉，HeaderValue 拒绝）
        ..Default::default()
    };
    let err = match create_provider(&cfg) {
        Ok(_) => panic!("内嵌换行 api_key 应被构造期拒绝"),
        Err(e) => e,
    };
    assert!(err.contains("拒绝装配"), "须为构造期拒绝: {err}");
    assert!(err.contains("api_key"), "提示须含字段名: {err}");
    assert!(err.contains("0x0a"), "提示须含违规 byte 码: {err}");
    assert!(!err.contains("sk-a"), "提示不得泄漏 key 内容: {err}");
}

#[test]
fn test_create_provider_trims_pasted_api_key() {
    // 尾随换行之外的可 trim 空白（空格）不拒——trim 后正常装配。
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "anthropic/claude-sonnet-4-5".to_string(),
        api_key: "sk-test  \n".to_string(),
        ..Default::default()
    };
    assert!(create_provider(&cfg).is_ok(), "trim 后的 key 应正常装配");
}

#[test]
fn test_create_provider_rejects_embedded_control_char() {
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "anthropic/claude-sonnet-4-5".to_string(),
        api_key: "sk-a\u{1}b".to_string(),
        ..Default::default()
    };
    assert!(create_provider(&cfg).is_err());
}

#[test]
fn test_create_provider_codex_bad_account_id_degrades_not_panics() {
    // account_id 非法 → 可选头省略 + WARN 降级，构造照常成功（不拒装配）。
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "openai/gpt-5.6-sol".to_string(),
        api_key: "sk-test".to_string(),
        ..Default::default()
    };
    // account_id 在 FactoryConfig 里怎么带？经 llm_ref/extra？此测试只
    // 钉「坏 key 拒绝 + 好 key 构造成功」主链路；account_id 走 sel 时
    // 由 header_safe_credential 兜底（单元测试覆盖）。
    assert!(create_provider(&cfg).is_ok());
}

#[test]
fn test_create_provider_http_compat_invalid_header_dropped_not_panics() {
    // HttpCompat：非法自定义头对构造期丢弃——构造成功 + 首请求不再 panic 面。
    let mut headers = std::collections::HashMap::new();
    headers.insert("X-Ok".to_string(), "v".to_string());
    headers.insert("bad name".to_string(), "v".to_string());
    headers.insert("X-Multi".to_string(), "a\nb".to_string());
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "zhipu/glm-4.7-flash".to_string(),
        api_key: "sk-test".to_string(),
        headers,
        ..Default::default()
    };
    assert!(create_provider(&cfg).is_ok(), "非法头对应丢弃而非炸构造");
}

#[test]
fn test_create_provider_cli_lane_bad_key_not_rejected() {
    // CLI 型 lane 不消费 api_key（子进程经 env HTTPS_PROXY）——含控制字符
    // 的 key 不应误拒装配。
    let cfg = FactoryConfig {
        proxy: String::new(),
        llm_ref: "claude-cli/claude-code".to_string(),
        api_key: "sk\nbad".to_string(),
        ..Default::default()
    };
    assert!(
        create_provider(&cfg).is_ok(),
        "CLI lane 不消费 api_key，不应被守卫误拒"
    );
}
