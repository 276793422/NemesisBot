// codex.rs 覆盖率补充测试（build_client 代理两臂 45-50、build_request_body
// 的 tools 段 219-225、parse_codex_response 的 message 输出文本臂 361-367、
// chat 的 token_source 失败 → Auth 464-467、200 + 非 JSON → Format
// 511-513）。

use super::*;
use crate::types::{ChatOptions, Message, ToolDefinition};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn base_config() -> CodexConfig {
    CodexConfig {
        api_key: "cov-key".to_string(),
        base_url: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    }
}

fn one_tool() -> ToolDefinition {
    ToolDefinition {
        tool_type: "function".to_string(),
        function: ToolFunctionDefinition {
            name: "read_file".to_string(),
            description: "读文件".to_string(),
            parameters: serde_json::json!({"type": "object"}),
        },
    }
}

/// build_client 的合法代理臂（45-48）与非法代理 warn 回落臂（49-50）——
/// 两态都只验构造不 panic。
#[test]
fn build_client_proxy_arms_survive_construction() {
    let valid = CodexConfig {
        proxy: Some("http://127.0.0.1:9".to_string()),
        ..base_config()
    };
    let _ = CodexProvider::new(valid);

    let invalid = CodexConfig {
        proxy: Some(":://not-a-proxy-url".to_string()),
        ..base_config()
    };
    let _ = CodexProvider::new(invalid);
}

/// build_request_body：带工具 + web_search → body["tools"] 落段（219-225）；
/// max_tokens/temperature 注入对照。
#[test]
fn build_request_body_embeds_tools_and_sampling() {
    let provider = CodexProvider::new(CodexConfig {
        enable_web_search: true,
        ..base_config()
    });
    let messages = vec![Message::text("user", "hello")];
    let mut options = ChatOptions::default();
    options.max_tokens = Some(512);
    options.temperature = Some(0.3);

    let body = provider.build_request_body(&messages, &[one_tool()], "gpt-5.2", &options);
    assert_eq!(body["model"], "gpt-5.2");
    assert_eq!(body["max_output_tokens"], 512);
    assert_eq!(body["temperature"], 0.3);
    let tools = body["tools"].as_array().expect("tools 段必须存在");
    assert_eq!(tools.last().unwrap()["type"], "web_search");
}

/// parse_codex_response：message 项的 output_text 文本（361-367）、
/// function_call 项、未知 type 项静默跳过。
#[test]
fn parse_codex_response_message_function_call_and_unknown() {
    let data = serde_json::json!({
        "output": [
            {"type": "message", "content": [
                {"type": "output_text", "text": "你好"},
                {"type": "output_text", "text": "，世界"}
            ]},
            {"type": "function_call", "call_id": "call-1", "name": "exec",
             "arguments": "{\"cmd\":\"ls\"}"},
            {"type": "reasoning", "summary": []},
            {"type": "message", "content": [
                {"type": "other_thing", "text": "不该进 content"}
            ]}
        ]
    });
    let resp = parse_codex_response(&data);
    assert_eq!(resp.content, "你好，世界");
    assert_eq!(resp.tool_calls.len(), 1, "{:?}", resp.tool_calls);
    let call = &resp.tool_calls[0];
    assert_eq!(call.id, "call-1");
    let f = call.function.as_ref().unwrap();
    assert_eq!(f.name, "exec");
    assert!(f.arguments.contains("ls"), "{}", f.arguments);
}

/// token_source 失败 → Auth（464-467），不发 HTTP。
#[tokio::test]
async fn chat_with_failing_token_source_returns_auth() {
    let provider = CodexProvider::with_token_source(
        base_config(),
        Box::new(|| Err("cov-token-expired".to_string())),
    );
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "gpt-5.2", &ChatOptions::default())
        .await
        .unwrap_err();
    match err {
        FailoverError::Auth {
            provider: p,
            status,
            ..
        } => {
            assert_eq!(p, "codex");
            assert_eq!(status, 0);
        }
        other => panic!("预期 Auth，得到 {other:?}"),
    }
}

/// 200 + 非 JSON body → Format（511-513 的 resp.json 失败臂）。
#[tokio::test]
async fn chat_with_non_json_200_body_returns_format_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;

    let provider = CodexProvider::new(CodexConfig {
        base_url: server.uri(),
        ..base_config()
    });
    let messages = vec![Message::text("user", "hi")];
    let err = provider
        .chat(&messages, &[], "gpt-5.2", &ChatOptions::default())
        .await
        .unwrap_err();
    assert!(
        matches!(err, FailoverError::Format { .. }),
        "预期 Format，得到 {err:?}"
    );
}

/// 无工具且关 web_search → body 不带 tools 段（225 的回落臂）。
#[test]
fn build_request_body_without_tools_omits_tools_section() {
    let provider = CodexProvider::new(CodexConfig {
        enable_web_search: false,
        ..base_config()
    });
    let messages = vec![Message::text("user", "hi")];
    let body = provider.build_request_body(&messages, &[], "gpt-5.2", &ChatOptions::default());
    assert!(body.get("tools").is_none(), "{body}");
}

/// message 项缺 content 数组 → 该项静默跳过（367 的回落臂）。
#[test]
fn parse_codex_response_message_without_content_array_is_skipped() {
    let data = serde_json::json!({"output": [
        {"type": "message"},
        {"type": "message", "content": "字符串形态不是数组"}
    ]});
    let resp = parse_codex_response(&data);
    assert!(resp.content.is_empty());
    assert!(resp.tool_calls.is_empty());
}

/// output 空数组 → 项循环零执行（409），空响应照常合成。
#[test]
fn parse_codex_response_empty_output_array() {
    let resp = parse_codex_response(&serde_json::json!({"output": []}));
    assert!(resp.content.is_empty());
    assert_eq!(resp.finish_reason, "stop");
}
