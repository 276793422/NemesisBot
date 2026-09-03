use super::*;

#[test]
fn test_normalize_model_strips_known_prefix() {
    assert_eq!(
        normalize_model("deepseek/chat", "https://api.deepseek.com"),
        "chat"
    );
    assert_eq!(
        normalize_model("groq/llama3", "https://api.groq.com"),
        "llama3"
    );
    assert_eq!(
        normalize_model("zhipu/glm-4", "https://open.bigmodel.cn"),
        "glm-4"
    );
    assert_eq!(
        normalize_model("ollama/llama3", "http://localhost:11434"),
        "llama3"
    );
}

#[test]
fn test_normalize_model_preserves_openrouter() {
    assert_eq!(
        normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
        "openai/gpt-4"
    );
}

#[test]
fn test_normalize_model_no_prefix() {
    assert_eq!(normalize_model("gpt-4", "https://api.openai.com"), "gpt-4");
}

#[test]
fn test_normalize_model_unknown_prefix() {
    assert_eq!(
        normalize_model("myprovider/model", "https://example.com"),
        "myprovider/model"
    );
}

#[test]
fn test_uses_completion_tokens() {
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4"));
    assert!(OpenAICompatProvider::uses_completion_tokens("o1-preview"));
    assert!(OpenAICompatProvider::uses_completion_tokens("gpt-5"));
    assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4"));
    assert!(!OpenAICompatProvider::uses_completion_tokens(
        "deepseek-chat"
    ));
}

#[test]
fn test_requires_fixed_temperature() {
    assert!(OpenAICompatProvider::requires_fixed_temperature("kimi-k2"));
    assert!(OpenAICompatProvider::requires_fixed_temperature("Kimi K2"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
}

#[test]
fn test_parse_response_simple() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "Hello!",
                "role": "assistant"
            },
            "finish_reason": "stop"
        }],
        "usage": {
            "prompt_tokens": 10,
            "completion_tokens": 5,
            "total_tokens": 15
        }
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "Hello!");
    assert_eq!(resp.finish_reason, "stop");
    assert_eq!(resp.usage.unwrap().total_tokens, 15);
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_with_tool_calls() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "call_123",
                    "type": "function",
                    "function": {
                        "name": "read_file",
                        "arguments": "{\"path\":\"/tmp/test\"}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    assert_eq!(resp.tool_calls[0].id, "call_123");
    assert_eq!(
        resp.tool_calls[0].function.as_ref().unwrap().name,
        "read_file"
    );
}

#[test]
fn test_parse_response_empty_choices() {
    let data = serde_json::json!({
        "choices": []
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert_eq!(resp.finish_reason, "stop");
}

#[test]
fn test_build_request_body_basic() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: "Hello".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    assert_eq!(body["model"], "gpt-4");
    assert_eq!(body["messages"][0]["role"], "user");
}

#[test]
fn test_build_request_body_with_tools() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let tools = vec![ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "Read a file".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }];

    let body = provider.build_request_body(
        &[],
        &tools,
        "gpt-4",
        &ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(1000),
            ..Default::default()
        },
    );
    assert!(body.get("tools").is_some());
    assert_eq!(body["tool_choice"], "auto");
    assert_eq!(body["temperature"], 0.7);
    assert_eq!(body["max_tokens"], 1000);
}

#[test]
fn test_config_default() {
    let config = OpenAICompatConfig::default();
    assert_eq!(config.name, "openai-compat");
    assert_eq!(config.timeout_secs, 600);
    assert!(config.base_url.is_empty());
    assert!(config.api_key.is_empty());
    assert!(config.default_model.is_empty());
    assert!(config.proxy.is_none());
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_normalize_model_with_base() {
    assert_eq!(
        normalize_model("nvidia/llama3", "https://api.nvidia.com"),
        "llama3"
    );
    assert_eq!(
        normalize_model("ollama/llama3", "http://localhost:11434"),
        "llama3"
    );
    assert_eq!(
        normalize_model("google/gemini", "https://generativelanguage.googleapis.com"),
        "gemini"
    );
    assert_eq!(
        normalize_model("moonshot/kimi", "https://api.moonshot.cn"),
        "kimi"
    );
}

#[test]
fn test_normalize_model_unknown_provider_prefix() {
    assert_eq!(
        normalize_model("myco/model", "https://example.com"),
        "myco/model"
    );
}

#[test]
fn test_normalize_model_openrouter_preserves() {
    assert_eq!(
        normalize_model("openai/gpt-4", "https://openrouter.ai/api/v1"),
        "openai/gpt-4"
    );
    assert_eq!(
        normalize_model("anthropic/claude-3", "https://OPENROUTER.AI/api/v1"),
        "anthropic/claude-3" // case-insensitive
    );
}

#[test]
fn test_parse_response_no_usage() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": "No usage info", "role": "assistant" },
            "finish_reason": "stop"
        }]
    });
    let resp = parse_response(&data);
    assert!(resp.usage.is_none());
    assert_eq!(resp.content, "No usage info");
}

#[test]
fn test_parse_response_null_content() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": null, "role": "assistant" },
            "finish_reason": "stop"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
}

#[test]
fn test_parse_response_null_finish_reason() {
    let data = serde_json::json!({
        "choices": [{
            "message": { "content": "test", "role": "assistant" },
            "finish_reason": null
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.finish_reason, "stop"); // defaults to "stop"
}

#[test]
fn test_parse_response_no_choices_field() {
    let data = serde_json::json!({});
    let resp = parse_response(&data);
    assert_eq!(resp.content, "");
    assert_eq!(resp.finish_reason, "stop");
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_parse_response_multiple_tool_calls() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [
                    {
                        "id": "call_1",
                        "type": "function",
                        "function": { "name": "read_file", "arguments": "{\"path\":\"/a\"}" }
                    },
                    {
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "write_file", "arguments": "{\"path\":\"/b\",\"content\":\"hello\"}" }
                    },
                    {
                        "id": "call_3",
                        "type": "function",
                        "function": { "name": "run_command", "arguments": "{\"cmd\":\"ls\"}" }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 3);
    assert_eq!(resp.tool_calls[0].id, "call_1");
    assert_eq!(resp.tool_calls[1].id, "call_2");
    assert_eq!(resp.tool_calls[2].id, "call_3");
}

#[test]
fn test_parse_response_tool_call_with_invalid_args() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "id": "call_bad",
                    "type": "function",
                    "function": { "name": "test", "arguments": "not valid json" }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });
    let resp = parse_response(&data);
    assert_eq!(resp.tool_calls.len(), 1);
    // Arguments should be parsed as raw fallback
    assert!(resp.tool_calls[0].arguments.is_some());
    assert!(
        resp.tool_calls[0]
            .arguments
            .as_ref()
            .unwrap()
            .contains_key("raw")
    );
}

#[test]
fn test_parse_response_tool_call_missing_id() {
    let data = serde_json::json!({
        "choices": [{
            "message": {
                "content": "",
                "tool_calls": [{
                    "type": "function",
                    "function": { "name": "test", "arguments": "{}" }
                }]
            }
        }]
    });
    let resp = parse_response(&data);
    // Missing id should be filtered out by filter_map
    assert!(resp.tool_calls.is_empty());
}

#[test]
fn test_uses_completion_tokens_additional_models() {
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-plus"));
    assert!(OpenAICompatProvider::uses_completion_tokens("glm-4-flash"));
    assert!(OpenAICompatProvider::uses_completion_tokens("o1"));
    assert!(!OpenAICompatProvider::uses_completion_tokens("gpt-4-turbo"));
    assert!(!OpenAICompatProvider::uses_completion_tokens(
        "claude-3-opus"
    ));
}

#[test]
fn test_requires_fixed_temperature_additional() {
    assert!(OpenAICompatProvider::requires_fixed_temperature(
        "kimi-k2-latest"
    ));
    assert!(OpenAICompatProvider::requires_fixed_temperature(
        "Kimi-K2-Pro"
    ));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("kimi-v1"));
    assert!(!OpenAICompatProvider::requires_fixed_temperature("gpt-4"));
}

#[test]
fn test_build_request_body_completion_tokens() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(
        &[],
        &[],
        "glm-4",
        &ChatOptions {
            max_tokens: Some(2048),
            ..Default::default()
        },
    );
    assert_eq!(body["max_completion_tokens"], 2048);
    assert!(body.get("max_tokens").is_none());
}

#[test]
fn test_build_request_body_kimi_fixed_temperature() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    // Kimi K2 should force temperature=1.0
    let body = provider.build_request_body(
        &[],
        &[],
        "kimi-k2",
        &ChatOptions {
            temperature: Some(0.5),
            ..Default::default()
        },
    );
    assert_eq!(body["temperature"], 1.0);
}

#[test]
fn test_build_request_body_no_tools_no_tool_choice() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(&[], &[], "gpt-4", &ChatOptions::default());
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
}

#[test]
fn test_build_request_body_with_stop_and_top_p() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let body = provider.build_request_body(
        &[],
        &[],
        "gpt-4",
        &ChatOptions {
            top_p: Some(0.95),
            stop: Some(vec!["END".to_string()]),
            ..Default::default()
        },
    );
    assert_eq!(body["top_p"], 0.95);
    assert!(body.get("stop").is_some());
}

#[test]
fn test_config_serialization_roundtrip() {
    let config = OpenAICompatConfig {
        name: "my-compat".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key: "sk-test".to_string(),
        default_model: "test-model".to_string(),
        timeout_secs: 120,
        proxy: Some("http://proxy:8080".to_string()),
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: OpenAICompatConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "my-compat");
    assert_eq!(deserialized.base_url, "https://api.test.com/v1");
    assert_eq!(deserialized.proxy, Some("http://proxy:8080".to_string()));
}

#[test]
fn test_default_model_and_name_accessors() {
    let config = OpenAICompatConfig {
        name: "my-provider".to_string(),
        base_url: "https://api.test.com/v1".to_string(),
        api_key: "test".to_string(),
        default_model: "default-model".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);
    assert_eq!(provider.default_model(), "default-model");
    assert_eq!(provider.name(), "my-provider");
}

// H4 (U16 half): reasoning_effort wire field
#[test]
fn test_effort_openai_request_includes_field() {
    let provider = OpenAICompatProvider::new(OpenAICompatConfig::default());
    let messages = vec![Message {
        role: "user".to_string(),
        content: "hi".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: std::collections::HashMap::new(),
    }];
    let mut opts = ChatOptions::default();
    opts.reasoning_effort = Some("high".to_string());
    let body = provider.build_request_body(&messages, &[], "gpt-4", &opts);
    assert_eq!(body["reasoning_effort"], serde_json::json!("high"));

    // None → field absent.
    let opts_none = ChatOptions::default();
    let body_none = provider.build_request_body(&messages, &[], "gpt-4", &opts_none);
    assert!(body_none.get("reasoning_effort").is_none());

    // Empty string → absent (defensive).
    let mut opts_empty = ChatOptions::default();
    opts_empty.reasoning_effort = Some(String::new());
    let body_empty = provider.build_request_body(&messages, &[], "gpt-4", &opts_empty);
    assert!(body_empty.get("reasoning_effort").is_none());
}

use serde_json::json;

// ===========================================================================
// W4c 补测（2026-08-25）：chat() HTTP 矩阵（wiremock）+ normalize 分支 + serde 默认
// ===========================================================================

use wiremock::matchers::{body_partial_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn oacc_config(base: &str) -> OpenAICompatConfig {
    OpenAICompatConfig {
        name: "oacc-test".to_string(),
        base_url: base.to_string(),
        api_key: "sk-test".to_string(),
        default_model: "default-m".to_string(),
        timeout_secs: 10,
        proxy: None,
    }
}

fn oacc_messages() -> Vec<Message> {
    vec![Message {
        role: "user".to_string(),
        content: "hi".into(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }]
}

#[tokio::test]
async fn test_w4c_oacc_chat_success_uses_default_model_and_bearer() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .and(header("Authorization", "Bearer sk-test"))
        .and(body_partial_json(json!({"model": "default-m"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "choices": [{
                "message": {"role": "assistant", "content": "hello-back"},
                "finish_reason": "stop"
            }],
            "usage": {"prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7}
        })))
        .mount(&server)
        .await;

    let provider = OpenAICompatProvider::new(oacc_config(&server.uri()));
    // model 为空 → 落到 default_model
    let resp = provider
        .chat(&oacc_messages(), &[], "", &ChatOptions::default())
        .await
        .unwrap();
    assert_eq!(resp.content, "hello-back");
    assert_eq!(resp.usage.unwrap().total_tokens, 7);
}

#[tokio::test]
async fn test_w4c_oacc_chat_401_maps_auth_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(401).set_body_string("unauthorized"))
        .mount(&server)
        .await;

    let provider = OpenAICompatProvider::new(oacc_config(&server.uri()));
    let err = provider
        .chat(&oacc_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::Auth { status: 401, .. }));
}

#[tokio::test]
async fn test_w4c_oacc_chat_invalid_json_maps_format_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json-at-all"))
        .mount(&server)
        .await;

    let provider = OpenAICompatProvider::new(oacc_config(&server.uri()));
    let err = provider
        .chat(&oacc_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Format { message, .. } => assert!(!message.is_empty()),
        other => panic!("expected Format, got {:?}", other),
    }
}

#[tokio::test]
async fn test_w4c_oacc_chat_dead_port_maps_timeout() {
    // 指向一个必然关闭的端口：send 失败 → Timeout
    let provider = OpenAICompatProvider::new(oacc_config("http://127.0.0.1:1"));
    let err = provider
        .chat(&oacc_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(matches!(err, FailoverError::Timeout { .. }));
}

#[tokio::test]
async fn test_w4c_oacc_chat_empty_base_url_rejected() {
    let provider = OpenAICompatProvider::new(oacc_config(""));
    let err = provider
        .chat(&oacc_messages(), &[], "m", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Format { message, .. } => {
            assert!(message.contains("API base URL not configured"))
        }
        other => panic!("expected Format, got {:?}", other),
    }
}

#[test]
fn test_w4c_oacc_config_serde_default_timeout() {
    let c: OpenAICompatConfig = serde_json::from_str(
        r#"{"name":"x","base_url":"http://b","api_key":"k","default_model":"m"}"#,
    )
    .unwrap();
    assert_eq!(c.timeout_secs, 600);
    assert!(c.proxy.is_none());
}

#[test]
fn test_w4c_oacc_provider_with_proxy_builds() {
    let mut cfg = oacc_config("http://127.0.0.1:9");
    cfg.proxy = Some("http://127.0.0.1:9".to_string());
    let _provider = OpenAICompatProvider::new(cfg); // 不 panic 即可
}

#[test]
fn test_w4c_oacc_normalize_model_with_base_variants() {
    let mut cfg = oacc_config("https://api.moonshot.cn");
    let provider = OpenAICompatProvider::new(cfg.clone());
    // moonshot 前缀剥除
    assert_eq!(provider.normalize_model_with_base("moonshot/kimi"), "kimi");
    // 无斜杠原样
    assert_eq!(provider.normalize_model_with_base("plain"), "plain");
    // 未知前缀保留
    assert_eq!(
        provider.normalize_model_with_base("weird/model"),
        "weird/model"
    );
    // openrouter.ai base → 全名保留
    cfg.base_url = "https://openrouter.ai/api/v1".to_string();
    let or_provider = OpenAICompatProvider::new(cfg);
    assert_eq!(
        or_provider.normalize_model_with_base("openai/gpt-4o"),
        "openai/gpt-4o"
    );
    // openrouter 前缀在普通 base 上也剥除
    assert_eq!(
        provider.normalize_model_with_base("openrouter/qwen"),
        "qwen"
    );
}

// ============================================================
// 字节快照（goal T1/T2 纪律 6）：content 类型多态化
// （String → MessageContent）前后，纯文本请求体必须逐字节一致
// ——这是 prompt cache 前缀契约（真相源 §1.5.2）。本测试先于
// 类型改动写好并锁死当前字节；类型改动后必须原样保持绿。
// 注：请求体经 serde_json::Value（无 preserve_order → BTreeMap），
// 键序为字母序——快照锁定的是"当前真实字节"而非声明序。
// ============================================================

#[test]
fn test_request_body_bytesnapshot_pure_text() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let messages = vec![
        Message {
            role: "system".to_string(),
            content: "You are helpful.".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "user".to_string(),
            content: "Hello".into(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "assistant".to_string(),
            content: "Let me check.".into(),
            tool_calls: vec![ToolCall {
                id: "call_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "read_file".to_string(),
                    arguments: "{}".to_string(),
                }),
                name: None,
                arguments: None,
            }],
            tool_call_id: None,
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
        Message {
            role: "tool".to_string(),
            content: "file data".into(),
            tool_calls: vec![],
            tool_call_id: Some("call_1".to_string()),
            timestamp: None,
            reasoning_content: None,
            extra: HashMap::new(),
        },
    ];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let serialized = serde_json::to_string(&body).unwrap();
    assert_eq!(
        serialized,
        r#"{"messages":[{"content":"You are helpful.","role":"system"},{"content":"Hello","role":"user"},{"content":"Let me check.","role":"assistant","tool_calls":[{"function":{"arguments":"{}","name":"read_file"},"id":"call_1","type":"function"}]},{"content":"file data","role":"tool","tool_call_id":"call_1"}],"model":"gpt-4"}"#
    );
}

// ============================================================
// T2：OpenAI vision 数组适配（goal T2）
// ============================================================

#[test]
fn test_request_body_image_parts_openai_format() {
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let messages = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "这是什么".to_string(),
            },
            // Base64 → data URI；带 detail
            ContentPart::Image {
                image: ImageSource::Base64 {
                    media_type: "image/png".to_string(),
                    data: "aGVsbG8=".to_string(),
                },
                detail: Some(ImageDetail::Low),
            },
            // Url 原样透传；detail None 不传
            ContentPart::Image {
                image: ImageSource::Url("https://example.com/b.jpg".to_string()),
                detail: None,
            },
        ]),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    let body = provider.build_request_body(&messages, &[], "gpt-4", &ChatOptions::default());
    let content = body["messages"][0]["content"]
        .as_array()
        .expect("content 数组");
    assert_eq!(content.len(), 3);

    // part 0：文本
    assert_eq!(content[0]["type"], "text");
    assert_eq!(content[0]["text"], "这是什么");

    // part 1：base64 → data URI + detail 透传
    assert_eq!(content[1]["type"], "image_url");
    assert_eq!(
        content[1]["image_url"]["url"],
        "data:image/png;base64,aGVsbG8="
    );
    assert_eq!(content[1]["image_url"]["detail"], "low");

    // part 2：URL 原样；无 detail 键
    assert_eq!(content[2]["type"], "image_url");
    assert_eq!(content[2]["image_url"]["url"], "https://example.com/b.jpg");
    assert!(content[2]["image_url"].get("detail").is_none());
}

#[test]
fn test_request_body_same_image_two_rounds_byte_identical() {
    // prompt cache 契约（真相源 §1.5.2 前提①②）：同一图片多轮序列化字节一致
    let config = OpenAICompatConfig {
        name: "test".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        api_key: "test-key".to_string(),
        default_model: "gpt-4".to_string(),
        timeout_secs: 30,
        proxy: None,
    };
    let provider = OpenAICompatProvider::new(config);

    let image_part = || ContentPart::Image {
        image: ImageSource::Base64 {
            media_type: "image/png".to_string(),
            data: "aGVsbG8=".to_string(),
        },
        detail: None,
    };

    // 第一轮：[user(+img)]
    let round1 = vec![Message {
        role: "user".to_string(),
        content: MessageContent::Parts(vec![
            ContentPart::Text {
                text: "看图".to_string(),
            },
            image_part(),
        ]),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
        reasoning_content: None,
        extra: HashMap::new(),
    }];

    // 第二轮：[user(+img), assistant, user] —— 历史前缀含同一张图
    let mut round2 = round1.clone();
    round2.push(Message::text("assistant", "图里是猫"));
    round2.push(Message::text("user", "它是什么品种？"));

    let body1 = provider.build_request_body(&round1, &[], "gpt-4", &ChatOptions::default());
    let body2_full = provider.build_request_body(&round2, &[], "gpt-4", &ChatOptions::default());

    let s1 = serde_json::to_string(&body1).unwrap();
    let s2_full = serde_json::to_string(&body2_full).unwrap();
    // 同一构建器对同一输入字节稳定
    let body1_again = provider.build_request_body(&round1, &[], "gpt-4", &ChatOptions::default());
    assert_eq!(s1, serde_json::to_string(&body1_again).unwrap());
    // 第二轮的 messages 前缀与第一轮逐字节一致（追加式历史不重写前缀）
    let prefix2 = serde_json::to_string(&body2_full["messages"][0]).unwrap();
    let msg1 = serde_json::to_string(&body1["messages"][0]).unwrap();
    assert_eq!(msg1, prefix2);
    let _ = s2_full; // 完整体仅用于构造前缀
}
