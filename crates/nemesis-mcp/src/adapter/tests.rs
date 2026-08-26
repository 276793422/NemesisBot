use super::*;
use crate::client::ClientResult;

#[test]
fn sanitize_name_basic() {
    assert_eq!(sanitize_name("hello-world"), "hello_world");
    assert_eq!(sanitize_name("hello_world"), "hello_world");
    assert_eq!(sanitize_name("hello world"), "hello_world");
    assert_eq!(sanitize_name("hello.world"), "hello_world");
    assert_eq!(sanitize_name("hello@world!"), "hello_world_");
    assert_eq!(sanitize_name("abc123"), "abc123");
    assert_eq!(sanitize_name(""), "");
    assert_eq!(sanitize_name("Hello-World"), "hello_world");
}

#[test]
fn tool_result_helpers() {
    let ok = ToolResult::ok("success message");
    assert_eq!(ok.content, "success message");
    assert!(!ok.is_error);

    let err = ToolResult::err("failure message");
    assert_eq!(err.content, "failure message");
    assert!(err.is_error);
}

#[test]
fn tool_definition_serialization() {
    let def = ToolDefinition {
        name: "mcp_test_echo".into(),
        description: "[MCP:test] Echo tool".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "message": { "type": "string" }
            },
            "additionalProperties": false,
        }),
    };

    let json = serde_json::to_string(&def).unwrap();
    let rt: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "mcp_test_echo");
    assert_eq!(rt.description, "[MCP:test] Echo tool");
}

#[test]
fn sanitize_name_special_chars() {
    assert_eq!(sanitize_name("foo/bar"), "foo_bar");
    assert_eq!(sanitize_name("foo\\bar"), "foo_bar");
    assert_eq!(sanitize_name("foo:bar"), "foo_bar");
    assert_eq!(sanitize_name("foo;bar"), "foo_bar");
    assert_eq!(sanitize_name("foo|bar"), "foo_bar");
    assert_eq!(sanitize_name("foo<bar>"), "foo_bar_");
}

#[test]
fn sanitize_name_unicode() {
    let result = sanitize_name("hello-world");
    assert!(result.contains('-') || result.contains('_'));
}

#[test]
fn sanitize_name_numbers_only() {
    assert_eq!(sanitize_name("12345"), "12345");
}

#[test]
fn tool_result_ok_is_not_error() {
    let ok = ToolResult::ok("data");
    assert!(!ok.is_error);
    assert_eq!(ok.content, "data");
}

#[test]
fn tool_result_err_is_error() {
    let err = ToolResult::err("oops");
    assert!(err.is_error);
    assert_eq!(err.content, "oops");
}

#[test]
fn tool_result_empty_ok() {
    let ok = ToolResult::ok("");
    assert!(!ok.is_error);
    assert_eq!(ok.content, "");
}

#[test]
fn tool_result_empty_err() {
    let err = ToolResult::err("");
    assert!(err.is_error);
    assert_eq!(err.content, "");
}

#[test]
fn tool_definition_complex_params() {
    let def = ToolDefinition {
        name: "mcp_complex".into(),
        description: "Complex tool".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "options": {
                    "type": "object",
                    "properties": {
                        "recursive": { "type": "boolean" },
                        "depth": { "type": "integer" }
                    }
                }
            },
            "required": ["path"]
        }),
    };

    let json = serde_json::to_string(&def).unwrap();
    let rt: ToolDefinition = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "mcp_complex");
    let params = rt.parameters.as_object().unwrap();
    assert!(params.contains_key("properties"));
    assert!(params.contains_key("required"));
}

#[test]
fn sanitize_name_long_string() {
    let long = "a".repeat(200);
    let result = sanitize_name(&long);
    assert_eq!(result.len(), 200);
}

#[test]
fn sanitize_name_mixed_chars() {
    let result = sanitize_name("My Tool Name!@#$%");
    assert!(
        result
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    );
    assert_eq!(result, "my_tool_name_____");
}

// ---- New tests ----

#[test]
fn sanitize_name_lowercases_and_replaces() {
    assert_eq!(sanitize_name("hello-world-test"), "hello_world_test");
    assert_eq!(sanitize_name("Hello-World"), "hello_world");
}

#[test]
fn sanitize_name_preserves_underscores() {
    assert_eq!(sanitize_name("hello_world_test"), "hello_world_test");
}

#[test]
fn sanitize_name_all_special() {
    let result = sanitize_name("!@#$%^&*()");
    assert!(result.chars().all(|c| c == '_'));
}

#[test]
fn sanitize_name_with_spaces() {
    assert_eq!(sanitize_name("hello world"), "hello_world");
    assert_eq!(sanitize_name("a b c"), "a_b_c");
}

#[test]
fn tool_result_serialization() {
    let ok = ToolResult::ok("message");
    let json = serde_json::to_string(&ok).unwrap();
    let rt: ToolResult = serde_json::from_str(&json).unwrap();
    assert!(!rt.is_error);
    assert_eq!(rt.content, "message");

    let err = ToolResult::err("error msg");
    let json = serde_json::to_string(&err).unwrap();
    let rt: ToolResult = serde_json::from_str(&json).unwrap();
    assert!(rt.is_error);
}

#[test]
fn tool_definition_default_name_pattern() {
    // Verify that when creating McpAdapter, the name follows mcp_{server}_{tool}
    let def = ToolDefinition {
        name: "mcp_test_server_echo".into(),
        description: "[MCP:test_server] Echo tool".into(),
        parameters: serde_json::json!({"type": "object"}),
    };
    assert!(def.name.starts_with("mcp_"));
    assert!(def.description.starts_with("[MCP:"));
}

#[test]
fn sanitize_name_tabs_and_newlines() {
    assert_eq!(sanitize_name("hello\tworld"), "hello_world");
    assert_eq!(sanitize_name("hello\nworld"), "hello_world");
}

#[test]
fn tool_result_ok_with_multiline() {
    let ok = ToolResult::ok("line1\nline2\nline3");
    assert_eq!(ok.content, "line1\nline2\nline3");
    assert!(!ok.is_error);
}

#[test]
fn tool_result_err_with_multiline() {
    let err = ToolResult::err("error1\nerror2");
    assert_eq!(err.content, "error1\nerror2");
    assert!(err.is_error);
}

#[test]
fn tool_definition_name_uniqueness() {
    let def1 = ToolDefinition {
        name: "mcp_server1_tool1".into(),
        description: "Tool 1 from server 1".into(),
        parameters: serde_json::json!({"type": "object"}),
    };
    let def2 = ToolDefinition {
        name: "mcp_server2_tool1".into(),
        description: "Tool 1 from server 2".into(),
        parameters: serde_json::json!({"type": "object"}),
    };
    assert_ne!(def1.name, def2.name);
}

// ============================================================
// Tests using mock client for McpAdapter coverage
// ============================================================

use std::sync::atomic::{AtomicBool, Ordering};

/// Mock client for testing McpAdapter without a real server.
struct MockClient {
    server_info: Option<ServerInfo>,
    tools: Vec<McpTool>,
    call_results: std::sync::Mutex<std::collections::VecDeque<ToolCallResult>>,
    initialized: AtomicBool,
}

impl MockClient {
    fn new(server_name: &str, tools: Vec<McpTool>) -> Self {
        Self {
            server_info: Some(ServerInfo {
                name: server_name.to_string(),
                version: "1.0.0".to_string(),
            }),
            tools,
            call_results: std::sync::Mutex::new(std::collections::VecDeque::new()),
            initialized: AtomicBool::new(false),
        }
    }

    fn with_call_result(&self, result: ToolCallResult) {
        self.call_results.lock().unwrap().push_back(result);
    }
}

#[async_trait]
impl Client for MockClient {
    async fn initialize(&mut self) -> ClientResult<InitializeResult> {
        self.initialized.store(true, Ordering::SeqCst);
        Ok(InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: self.server_info.clone().unwrap(),
        })
    }

    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
        Ok(self.tools.clone())
    }

    async fn call_tool(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult> {
        let mut results = self.call_results.lock().unwrap();
        if let Some(result) = results.pop_front() {
            Ok(result)
        } else {
            Ok(ToolCallResult::ok("default mock result"))
        }
    }

    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
        Ok(vec![])
    }

    async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> {
        Ok(ResourceContent::default())
    }

    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
        Ok(vec![])
    }

    async fn get_prompt(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<PromptResult> {
        Ok(PromptResult::default())
    }

    async fn close(&mut self) -> ClientResult<()> {
        Ok(())
    }

    fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    fn is_connected(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }
}

#[test]
fn test_mcp_adapter_new_with_description() {
    let mock = MockClient::new("test_server", vec![]);
    let mcp_tool = McpTool {
        name: "echo".to_string(),
        description: Some("Echo the input".to_string()),
        input_schema: serde_json::json!({"type": "object", "properties": {"message": {"type": "string"}}}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool.clone());

    let def = adapter.definition();
    assert_eq!(def.name, "mcp_test_server_echo");
    assert!(def.description.contains("[MCP:test_server]"));
    assert!(def.description.contains("Echo the input"));

    assert_eq!(adapter.mcp_tool().name, "echo");
}

#[test]
fn test_mcp_adapter_new_without_description() {
    let mock = MockClient::new("my_server", vec![]);
    let mcp_tool = McpTool {
        name: "read".to_string(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let def = adapter.definition();
    assert!(def.description.contains("[MCP:my_server]"));
    assert!(def.description.contains("MCP tool: read"));
}

#[test]
fn test_mcp_adapter_name_sanitization() {
    let mock = MockClient::new("my server!", vec![]);
    let mcp_tool = McpTool {
        name: "my tool@1".to_string(),
        description: Some("desc".to_string()),
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let def = adapter.definition();
    // sanitize_name lowercases and replaces special chars with underscores
    assert!(def.name.contains("my_server_"));
    assert!(def.name.contains("my_tool_1"));
}

#[test]
fn test_mcp_adapter_with_timeout() {
    let mock = MockClient::new("test", vec![]);
    let mcp_tool = McpTool {
        name: "tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool).with_timeout(Duration::from_secs(60));
    let def = adapter.definition();
    assert_eq!(def.name, "mcp_test_tool");
}

#[test]
fn test_mcp_adapter_parameters_structure() {
    let mock = MockClient::new("srv", vec![]);
    let schema = serde_json::json!({"type": "object", "properties": {"x": {"type": "number"}}});
    let mcp_tool = McpTool {
        name: "compute".to_string(),
        description: Some("Compute".to_string()),
        input_schema: schema.clone(),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let params = adapter.definition().parameters.as_object().unwrap();
    assert_eq!(params["type"], "object");
    assert_eq!(params["additionalProperties"], false);
    // The input_schema is nested under "properties"
    assert!(params["properties"].is_object());
}

#[tokio::test]
async fn test_mcp_adapter_execute_text_result() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult::ok("Hello from tool!"));

    let mcp_tool = McpTool {
        name: "echo".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({"message": "hi"})).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "Hello from tool!");
}

#[tokio::test]
async fn test_mcp_adapter_execute_error_result() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult::err("Something went wrong"));

    let mcp_tool = McpTool {
        name: "fail_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("fail_tool"));
    assert!(result.content.contains("Something went wrong"));
}

#[tokio::test]
async fn test_mcp_adapter_execute_error_result_no_text() {
    let mock = MockClient::new("test", vec![]);
    // Error result with image content (no text) — should return "unknown error"
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent {
            content_type: "image".to_string(),
            text: None,
        }],
        is_error: true,
    });

    let mcp_tool = McpTool {
        name: "img_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("unknown error"));
}

#[tokio::test]
async fn test_mcp_adapter_execute_image_content() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent {
            content_type: "image".to_string(),
            text: Some("base64data".to_string()),
        }],
        is_error: false,
    });

    let mcp_tool = McpTool {
        name: "img_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    assert!(result.content.contains("[Image:"));
    assert!(result.content.contains("base64data"));
}

#[tokio::test]
async fn test_mcp_adapter_execute_resource_content() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent {
            content_type: "resource".to_string(),
            text: Some("resource_data".to_string()),
        }],
        is_error: false,
    });

    let mcp_tool = McpTool {
        name: "res_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    assert!(result.content.contains("[Resource:"));
    assert!(result.content.contains("resource_data"));
}

#[tokio::test]
async fn test_mcp_adapter_execute_unknown_content_type() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent {
            content_type: "custom_type".to_string(),
            text: Some("custom_data".to_string()),
        }],
        is_error: false,
    });

    let mcp_tool = McpTool {
        name: "custom_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "custom_data");
}

#[tokio::test]
async fn test_mcp_adapter_execute_multiple_content() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent::text("part1"), ToolContent::text("part2")],
        is_error: false,
    });

    let mcp_tool = McpTool {
        name: "multi".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    assert!(result.content.contains("part1"));
    assert!(result.content.contains("part2"));
}

#[tokio::test]
async fn test_mcp_adapter_execute_non_object_args() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult::ok("ok"));

    let mcp_tool = McpTool {
        name: "flex".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    // Pass a string instead of object
    let result = adapter.execute(serde_json::json!("not an object")).await;
    assert!(!result.is_error);

    // Pass null
    let result = adapter.execute(serde_json::json!(null)).await;
    assert!(!result.is_error);

    // Pass array
    let result = adapter.execute(serde_json::json!([1, 2, 3])).await;
    assert!(!result.is_error);
}

#[tokio::test]
async fn test_create_tools_from_client() {
    let tools = vec![
        McpTool {
            name: "tool_a".to_string(),
            description: Some("Tool A".to_string()),
            input_schema: serde_json::json!({"type": "object"}),
        },
        McpTool {
            name: "tool_b".to_string(),
            description: None,
            input_schema: serde_json::json!({"type": "object"}),
        },
    ];
    let mock = MockClient::new("my_server", tools);

    let adapters = create_tools_from_client(Box::new(mock)).await.unwrap();
    assert_eq!(adapters.len(), 2);

    let def0 = adapters[0].definition();
    assert_eq!(def0.name, "mcp_my_server_tool_a");
    assert!(def0.description.contains("[MCP:my_server]"));
    assert!(def0.description.contains("Tool A"));

    let def1 = adapters[1].definition();
    assert_eq!(def1.name, "mcp_my_server_tool_b");
    assert!(def1.description.contains("MCP tool: tool_b"));
}

#[tokio::test]
async fn test_create_tools_from_client_empty() {
    let mock = MockClient::new("empty_server", vec![]);
    let adapters = create_tools_from_client(Box::new(mock)).await.unwrap();
    assert!(adapters.is_empty());
}

#[test]
fn test_mcp_adapter_no_server_info() {
    // Create a mock with no server info
    struct NoInfoMock;

    #[async_trait]
    impl Client for NoInfoMock {
        async fn initialize(&mut self) -> ClientResult<InitializeResult> {
            Ok(InitializeResult {
                protocol_version: PROTOCOL_VERSION.to_string(),
                capabilities: ServerCapabilities::default(),
                server_info: ServerInfo {
                    name: "n".into(),
                    version: "1".into(),
                },
            })
        }
        async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
            Ok(vec![])
        }
        async fn call_tool(
            &mut self,
            _name: &str,
            _args: serde_json::Value,
        ) -> ClientResult<ToolCallResult> {
            Ok(ToolCallResult::ok(""))
        }
        async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
            Ok(vec![])
        }
        async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> {
            Ok(ResourceContent::default())
        }
        async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
            Ok(vec![])
        }
        async fn get_prompt(
            &mut self,
            _name: &str,
            _args: serde_json::Value,
        ) -> ClientResult<PromptResult> {
            Ok(PromptResult::default())
        }
        async fn close(&mut self) -> ClientResult<()> {
            Ok(())
        }
        fn server_info(&self) -> Option<&ServerInfo> {
            None
        }
        fn is_connected(&self) -> bool {
            false
        }
    }

    let mcp_tool = McpTool {
        name: "test".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(NoInfoMock), mcp_tool);
    let def = adapter.definition();
    // When no server info, should use "unknown"
    assert!(def.name.contains("unknown"));
}

// ===========================================================================
// W4c 补测（2026-08-25）：create_tools_from_client_named + ArcClientAdapter
// execute 分支（error/client-error/timeout/image/resource/非对象 args）+
// sanitize_schema 边界（非对象输入 / 嵌套 properties type 数组展平）
// ===========================================================================

fn w4c_tool(name: &str, desc: Option<&str>, schema: serde_json::Value) -> McpTool {
    McpTool {
        name: name.to_string(),
        description: desc.map(|s| s.to_string()),
        input_schema: schema,
    }
}

#[tokio::test]
async fn test_w4c_named_uses_config_name_over_self_reported() {
    // server_info 自报名 "self name"，配置名 "Cfg Name!" → 前缀取配置名（核心契约）
    let mock = MockClient::new(
        "self name",
        vec![
            w4c_tool("tool_a", Some("Tool A"), serde_json::json!({"type": "object"})),
            w4c_tool("tool_b", None, serde_json::json!({"type": "object"})),
        ],
    );
    let adapters = create_tools_from_client_named(Box::new(mock), "Cfg Name!", 0).await.unwrap();
    assert_eq!(adapters.len(), 2);
    let d0 = adapters[0].definition();
    // "Cfg Name!" → "cfg_name_"（'!' 也被换成 '_'，故双下划线）
    assert_eq!(d0.name, "mcp_cfg_name__tool_a");
    // 描述里用未 sanitize 的原始配置名
    assert!(d0.description.contains("[MCP:Cfg Name!]"));
    assert!(d0.description.contains("Tool A"));
    let d1 = adapters[1].definition();
    assert_eq!(d1.name, "mcp_cfg_name__tool_b");
    assert!(d1.description.contains("MCP tool: tool_b"));
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_text_result() {
    let mock = MockClient::new("srv", vec![w4c_tool("echo", None, serde_json::json!({}))]);
    mock.with_call_result(ToolCallResult::ok("echoed"));
    let adapters = create_tools_from_client_named(Box::new(mock), "srv", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({"text": "hi"})).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "echoed");
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_non_object_args_still_calls() {
    let mock = MockClient::new("srv", vec![w4c_tool("flex", None, serde_json::json!({}))]);
    mock.with_call_result(ToolCallResult::ok("ok"));
    let adapters = create_tools_from_client_named(Box::new(mock), "srv", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!("not-an-object")).await;
    assert!(!result.is_error);
    assert_eq!(result.content, "ok");
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_is_error_result_maps_err() {
    let mock = MockClient::new("srv", vec![w4c_tool("boom", None, serde_json::json!({}))]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent::text("kaboom")],
        is_error: true,
    });
    let adapters = create_tools_from_client_named(Box::new(mock), "srv", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("returned error"));
    assert!(result.content.contains("kaboom"));
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_is_error_no_text_unknown() {
    let mock = MockClient::new("srv", vec![w4c_tool("boom2", None, serde_json::json!({}))]);
    mock.with_call_result(ToolCallResult {
        content: vec![],
        is_error: true,
    });
    let adapters = create_tools_from_client_named(Box::new(mock), "srv", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("unknown error"));
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_image_and_resource_content() {
    let mock = MockClient::new(
        "srv",
        vec![w4c_tool("rich", None, serde_json::json!({}))],
    );
    mock.with_call_result(ToolCallResult {
        content: vec![
            ToolContent {
                content_type: "image".to_string(),
                text: Some("b64data".to_string()),
            },
            ToolContent {
                content_type: "resource".to_string(),
                text: Some("res-data".to_string()),
            },
        ],
        is_error: false,
    });
    let adapters = create_tools_from_client_named(Box::new(mock), "srv", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    assert!(result.content.contains("[Image: b64data]"));
    assert!(result.content.contains("[Resource: res-data]"));
}

/// call_tool 直接返回 Err（JSON-RPC error / 传输错误）的 mock client。
struct W4cFailingClient {
    server_info: Option<ServerInfo>,
}

#[async_trait]
impl Client for W4cFailingClient {
    async fn initialize(&mut self) -> ClientResult<InitializeResult> {
        Ok(InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: self.server_info.clone().unwrap(),
        })
    }
    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
        Ok(vec![w4c_tool("failing", None, serde_json::json!({}))])
    }
    async fn call_tool(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult> {
        Err(crate::client::ClientError::NotConnected)
    }
    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
        Ok(vec![])
    }
    async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> {
        Ok(ResourceContent::default())
    }
    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
        Ok(vec![])
    }
    async fn get_prompt(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<PromptResult> {
        Ok(PromptResult::default())
    }
    async fn close(&mut self) -> ClientResult<()> {
        Ok(())
    }
    fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }
    fn is_connected(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_client_error_maps_err() {
    let client = W4cFailingClient {
        server_info: Some(ServerInfo {
            name: "f".to_string(),
            version: "1".to_string(),
        }),
    };
    let adapters = create_tools_from_client_named(Box::new(client), "f", 0)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("MCP tool 'failing' error"));
    assert!(result.content.contains("not connected"));
}

/// call_tool 永远阻塞的 mock client（驱动 adapter 超时臂）。
struct W4cSlowClient {
    server_info: Option<ServerInfo>,
}

#[async_trait]
impl Client for W4cSlowClient {
    async fn initialize(&mut self) -> ClientResult<InitializeResult> {
        Ok(InitializeResult {
            protocol_version: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
            server_info: self.server_info.clone().unwrap(),
        })
    }
    async fn list_tools(&mut self) -> ClientResult<Vec<McpTool>> {
        Ok(vec![w4c_tool("slow", None, serde_json::json!({}))])
    }
    async fn call_tool(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<ToolCallResult> {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        Ok(ToolCallResult::ok("too late"))
    }
    async fn list_resources(&mut self) -> ClientResult<Vec<Resource>> {
        Ok(vec![])
    }
    async fn read_resource(&mut self, _uri: &str) -> ClientResult<ResourceContent> {
        Ok(ResourceContent::default())
    }
    async fn list_prompts(&mut self) -> ClientResult<Vec<Prompt>> {
        Ok(vec![])
    }
    async fn get_prompt(
        &mut self,
        _name: &str,
        _arguments: serde_json::Value,
    ) -> ClientResult<PromptResult> {
        Ok(PromptResult::default())
    }
    async fn close(&mut self) -> ClientResult<()> {
        Ok(())
    }
    fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }
    fn is_connected(&self) -> bool {
        true
    }
}

#[tokio::test]
async fn test_w4c_named_adapter_execute_timeout_maps_err() {
    let client = W4cSlowClient {
        server_info: Some(ServerInfo {
            name: "s".to_string(),
            version: "1".to_string(),
        }),
    };
    // timeout_secs=1 → adapter 1s 超时（client 睡 30s）
    let adapters = create_tools_from_client_named(Box::new(client), "s", 1)
        .await
        .unwrap();
    let result = adapters[0].execute(serde_json::json!({})).await;
    assert!(result.is_error);
    assert!(result.content.contains("timed out after"));
}

#[test]
fn test_w4c_sanitize_schema_non_object_becomes_default_object() {
    let mock = MockClient::new("srv", vec![]);
    let adapter = McpAdapter::new(
        Box::new(mock),
        w4c_tool("t", None, serde_json::json!("not-a-schema")),
    );
    let p = &adapter.definition().parameters;
    assert_eq!(p["type"], "object");
    assert_eq!(p["additionalProperties"], false);
    assert!(p["properties"].is_object());
}

#[test]
fn test_w4c_sanitize_schema_flattens_nested_type_arrays() {
    let mock = MockClient::new("srv", vec![]);
    let schema = serde_json::json!({
        "type": "object",
        "properties": {
            "text": {"type": ["string", "null"]},
            "nested": {
                "type": "object",
                "properties": {
                    "inner": {"type": ["integer", "null"]}
                }
            }
        }
    });
    let adapter = McpAdapter::new(Box::new(mock), w4c_tool("t", None, schema));
    let p = &adapter.definition().parameters;
    // 顶层强制 object
    assert_eq!(p["type"], "object");
    // 缺省时补 additionalProperties
    assert_eq!(p["additionalProperties"], false);
    // 一级属性 type 数组展平为首个
    assert_eq!(p["properties"]["text"]["type"], "string");
    // 嵌套对象的属性 type 数组也展平
    assert_eq!(p["properties"]["nested"]["properties"]["inner"]["type"], "integer");
}

#[test]
fn test_w4c_sanitize_schema_keeps_existing_additional_properties() {
    let mock = MockClient::new("srv", vec![]);
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": true,
        "properties": {}
    });
    let adapter = McpAdapter::new(Box::new(mock), w4c_tool("t", None, schema));
    let p = &adapter.definition().parameters;
    assert_eq!(p["additionalProperties"], true);
}

// ===========================================================================
// S1 补测（2026-08-26）：execute() 内容类型 `_` 未知回退臂
// ===========================================================================

#[tokio::test]
async fn test_s1_adapter_execute_unknown_content_type_falls_back_to_raw_text() {
    let mock = MockClient::new("test", vec![]);
    mock.with_call_result(ToolCallResult {
        content: vec![ToolContent {
            content_type: "audio".to_string(),
            text: Some("raw-audio-marker".to_string()),
        }],
        is_error: false,
    });

    let mcp_tool = McpTool {
        name: "aud_tool".to_string(),
        description: None,
        input_schema: serde_json::json!({}),
    };
    let adapter = McpAdapter::new(Box::new(mock), mcp_tool);

    let result = adapter.execute(serde_json::json!({})).await;
    assert!(!result.is_error);
    // Unknown content types fall through with their raw text, no decoration.
    assert_eq!(result.content, "raw-audio-marker");
}
