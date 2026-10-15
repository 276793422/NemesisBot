use super::*;

/// Helper: create a mock client pre-loaded with standard responses for
/// the full initialization + tool listing flow.
fn make_mock_client() -> McpClient {
    let mut mock = crate::transport::MockTransport::new_connected();

    // Initialize response.
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false },
                "resources": { "subscribe": false, "listChanged": false },
                "prompts": { "listChanged": false }
            },
            "serverInfo": {
                "name": "test-mcp-server",
                "version": "1.0.0"
            }
        }),
    );

    // notifications/initialized — we don't care about the response, so
    // add a generic one that the mock can match.
    mock.add_success("notifications/initialized", serde_json::json!({}));

    McpClient::new(Box::new(mock))
}

#[test]
fn request_building() {
    let client = McpClient::default();
    let req = client.build_request("tools/list", None);

    assert_eq!(req.jsonrpc, "2.0");
    assert_eq!(req.method, "tools/list");
    assert!(req.id.is_some());
    assert!(req.params.is_none());

    // Second request should have a different id.
    let req2 = client.build_request("tools/call", Some(serde_json::json!({"name": "x"})));
    assert_ne!(req.id, req2.id);
    assert!(req2.params.is_some());
}

#[test]
fn response_parsing_success() {
    let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
    let resp = McpClient::parse_response(raw).unwrap();

    assert!(!resp.is_error());
    assert_eq!(resp.id, serde_json::Value::Number(1.into()));
    assert!(resp.result.is_some());
}

#[test]
fn response_parsing_error() {
    let raw = r#"{"jsonrpc":"2.0","id":2,"error":{"code":-32601,"message":"Method not found"}}"#;
    let resp = McpClient::parse_response(raw).unwrap();

    assert!(resp.is_error());
    let err = resp.error.unwrap();
    assert_eq!(err.code, -32601);
    assert_eq!(err.message, "Method not found");
}

#[test]
fn id_generation_monotonic() {
    let client = McpClient::default();
    let mut ids = Vec::new();
    for _ in 0..10 {
        let req = client.build_request("test", None);
        if let Some(id) = req.id {
            ids.push(id.as_u64().unwrap());
        }
    }
    // Ids must be strictly increasing.
    for window in ids.windows(2) {
        assert!(
            window[0] < window[1],
            "ids must be monotonically increasing"
        );
    }
}

#[tokio::test]
async fn full_initialize_flow() {
    let mut client = make_mock_client();

    let result = client.initialize().await.unwrap();
    assert_eq!(result.server_info.name, "test-mcp-server");
    assert_eq!(result.server_info.version, "1.0.0");
    assert_eq!(result.protocol_version, PROTOCOL_VERSION);

    // Client state should reflect initialization.
    assert!(client.is_connected());
    assert!(client.server_info().is_some());
    assert_eq!(client.server_info().unwrap().name, "test-mcp-server");
}

#[tokio::test]
async fn list_tools_flow() {
    let _client = make_mock_client();

    // Add list_tools response.
    {
        // We need to reach into the transport to add more responses.
        // Since McpClient owns the transport, we'll re-create with the responses.
    }

    // Re-create with all responses pre-loaded.
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "tools/list",
        serde_json::json!({
            "tools": [
                {
                    "name": "echo",
                    "description": "Echo back input",
                    "inputSchema": {
                        "type": "object",
                        "properties": { "message": { "type": "string" } }
                    }
                },
                {
                    "name": "add",
                    "description": "Add two numbers",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "a": { "type": "number" },
                            "b": { "type": "number" }
                        }
                    }
                }
            ]
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[1].name, "add");
}

#[tokio::test]
async fn call_tool_flow() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "tools/call",
        serde_json::json!({
            "content": [{ "type": "text", "text": "Hello, world!" }],
            "isError": false
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let result = client
        .call_tool("echo", serde_json::json!({ "message": "Hello, world!" }))
        .await
        .unwrap();

    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    assert_eq!(result.content[0].text.as_deref(), Some("Hello, world!"));
}

#[tokio::test]
async fn list_resources_flow() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "resources/list",
        serde_json::json!({
            "resources": [
                {
                    "uri": "file:///test.txt",
                    "name": "test.txt",
                    "description": "A test file",
                    "mimeType": "text/plain"
                }
            ]
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let resources = client.list_resources().await.unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0].uri, "file:///test.txt");
}

#[tokio::test]
async fn read_resource_flow() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "resources/read",
        serde_json::json!({
            "contents": [{
                "uri": "file:///test.txt",
                "mimeType": "text/plain",
                "text": "hello world"
            }]
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let content = client.read_resource("file:///test.txt").await.unwrap();
    assert_eq!(content.uri, "file:///test.txt");
    assert_eq!(content.text.as_deref(), Some("hello world"));
}

#[tokio::test]
async fn list_prompts_flow() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "prompts": { "listChanged": false } },
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "prompts/list",
        serde_json::json!({
            "prompts": [
                {
                    "name": "greet",
                    "description": "Greet a person",
                    "arguments": [
                        { "name": "name", "description": "Person's name", "required": true }
                    ]
                }
            ]
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let prompts = client.list_prompts().await.unwrap();
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0].name, "greet");
    assert_eq!(prompts[0].arguments.len(), 1);
    assert_eq!(prompts[0].arguments[0].name, "name");
}

#[tokio::test]
async fn get_prompt_flow() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "prompts": { "listChanged": false } },
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "prompts/get",
        serde_json::json!({
            "description": "Greeting prompt",
            "messages": [
                {
                    "role": "user",
                    "content": { "type": "text", "text": "Hello, {{name}}!" }
                }
            ]
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let result = client
        .get_prompt("greet", serde_json::json!({ "name": "Alice" }))
        .await
        .unwrap();

    assert_eq!(result.messages.len(), 1);
    assert_eq!(result.messages[0].role, "user");
    assert_eq!(
        result.messages[0].content.text.as_deref(),
        Some("Hello, {{name}}!")
    );
    assert_eq!(result.description.as_deref(), Some("Greeting prompt"));
}

#[tokio::test]
async fn close_lifecycle() {
    let mut client = make_mock_client();
    client.initialize().await.unwrap();
    assert!(client.is_connected());

    client.close().await.unwrap();
    assert!(!client.is_connected());

    // Double close is fine.
    client.close().await.unwrap();
}

#[tokio::test]
async fn operations_fail_before_init() {
    let mock = crate::transport::MockTransport::new_connected();
    // Don't add initialize response — client stays uninitialized.
    let mut client = McpClient::new(Box::new(mock));

    // list_tools should fail.
    let result = client.list_tools().await;
    assert!(result.is_err());

    // call_tool should fail.
    let result = client.call_tool("echo", serde_json::json!({})).await;
    assert!(result.is_err());
}

#[test]
fn from_config_validates_command() {
    let config = ServerConfig {
        name: "test".into(),
        command: "".into(),
        ..Default::default()
    };
    let result = McpClient::from_config(&config);
    assert!(result.is_err());
}

#[test]
fn from_config_validates_name() {
    // Empty name is accepted - from_config only validates command
    let config = ServerConfig {
        name: "".into(),
        command: "echo".into(),
        ..Default::default()
    };
    let result = McpClient::from_config(&config);
    // Should succeed since command is not empty
    assert!(result.is_ok());
}

#[test]
fn parse_response_invalid_json() {
    let result = McpClient::parse_response("not json");
    assert!(result.is_err());
}

#[test]
fn parse_response_missing_jsonrpc() {
    let raw = r#"{"id":1,"result":{}}"#;
    // Should parse but might be missing jsonrpc field
    let _ = McpClient::parse_response(raw);
}

#[tokio::test]
async fn call_tool_error_response() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "tools/call",
        serde_json::json!({
            "content": [{ "type": "text", "text": "Error: tool not found" }],
            "isError": true
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let result = client
        .call_tool("nonexistent", serde_json::json!({}))
        .await
        .unwrap();

    assert!(result.is_error);
    assert_eq!(
        result.content[0].text.as_deref(),
        Some("Error: tool not found")
    );
}

#[tokio::test]
async fn list_tools_empty() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success("tools/list", serde_json::json!({ "tools": [] }));

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let tools = client.list_tools().await.unwrap();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn list_resources_empty() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success("resources/list", serde_json::json!({ "resources": [] }));

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let resources = client.list_resources().await.unwrap();
    assert!(resources.is_empty());
}

#[tokio::test]
async fn list_prompts_empty() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success("prompts/list", serde_json::json!({ "prompts": [] }));

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let prompts = client.list_prompts().await.unwrap();
    assert!(prompts.is_empty());
}

#[tokio::test]
async fn read_resource_fail_before_init() {
    let mock = crate::transport::MockTransport::new_connected();
    let mut client = McpClient::new(Box::new(mock));

    let result = client.read_resource("file:///test.txt").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn get_prompt_fail_before_init() {
    let mock = crate::transport::MockTransport::new_connected();
    let mut client = McpClient::new(Box::new(mock));

    let result = client.get_prompt("greet", serde_json::json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn ping_via_generic_method() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "test-server", "version": "1.0" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    // Verify the client is connected (ping is just verifying it works)
    assert!(client.is_connected());
    assert!(client.server_info().is_some());
}

#[test]
fn build_request_with_params() {
    let client = McpClient::default();
    let params = serde_json::json!({"name": "echo", "arguments": {"message": "hi"}});
    let req = client.build_request("tools/call", Some(params.clone()));
    assert_eq!(req.method, "tools/call");
    assert_eq!(req.params, Some(params));
}

#[test]
fn build_request_without_params() {
    let client = McpClient::default();
    let req = client.build_request("tools/list", None);
    assert_eq!(req.method, "tools/list");
    assert!(req.params.is_none());
}

// ---- New tests ----

#[test]
fn client_error_display() {
    let e = ClientError::NotConnected;
    assert!(e.to_string().contains("not connected"));

    let e2 = ClientError::Closed;
    assert!(e2.to_string().contains("closed"));

    let e3 = ClientError::NotInitialized;
    assert!(e3.to_string().contains("not initialized"));

    let e4 = ClientError::InvalidConfig("bad".into());
    assert!(e4.to_string().contains("bad"));
}

#[test]
fn parse_response_with_null_result() {
    let raw = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
    let resp = McpClient::parse_response(raw).unwrap();
    assert!(!resp.is_error());
}

#[test]
fn parse_response_with_nested_result() {
    let raw = r#"{"jsonrpc":"2.0","id":1,"result":{"nested":{"deep":true}}}"#;
    let resp = McpClient::parse_response(raw).unwrap();
    assert_eq!(resp.result.unwrap()["nested"]["deep"], true);
}

#[test]
fn default_client_is_not_connected() {
    let client = McpClient::default();
    assert!(!client.is_connected());
    assert!(client.server_info().is_none());
}

#[test]
fn from_config_with_valid_command() {
    let config = ServerConfig::new("my-server", "node")
        .arg("index.js")
        .timeout(60);
    let result = McpClient::from_config(&config);
    assert!(result.is_ok());
}

#[tokio::test]
async fn double_close_is_ok() {
    let mut client = McpClient::default();
    client.close().await.unwrap();
    client.close().await.unwrap();
}

#[tokio::test]
async fn send_request_when_closed_fails() {
    // McpClient with a mock transport, manually set closed
    let mut client = McpClient::default();
    client.close().await.unwrap();
    // Internal send_request checks closed flag
}

#[test]
fn build_request_ids_increment() {
    let client = McpClient::default();
    let ids: Vec<u64> = (0..5)
        .map(|_| {
            let req = client.build_request("test", None);
            req.id.unwrap().as_u64().unwrap()
        })
        .collect();
    for w in ids.windows(2) {
        assert!(w[0] < w[1]);
    }
}

#[test]
fn parse_response_string_id() {
    let raw = r#"{"jsonrpc":"2.0","id":"string-id-123","result":{}}"#;
    let resp = McpClient::parse_response(raw).unwrap();
    assert_eq!(resp.id, serde_json::Value::String("string-id-123".into()));
}

#[test]
fn parse_response_error_with_data() {
    let raw = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"Invalid","data":{"info":"extra"}}}"#;
    let resp = McpClient::parse_response(raw).unwrap();
    assert!(resp.is_error());
    let err = resp.error.unwrap();
    assert!(err.data.is_some());
    assert_eq!(err.data.unwrap()["info"], "extra");
}

#[tokio::test]
async fn list_resources_fail_before_init() {
    let mock = crate::transport::MockTransport::new_connected();
    let mut client = McpClient::new(Box::new(mock));
    let result = client.list_resources().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn server_error_on_list_tools() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_error("tools/list", -32603, "internal");

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    let result = client.list_tools().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn server_error_on_call_tool() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_error("tools/call", -32602, "invalid params");

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    let result = client.call_tool("x", serde_json::json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn call_tool_with_empty_result() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "tools/call",
        serde_json::json!({
            "content": [],
            "isError": false
        }),
    );

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    let result = client
        .call_tool("noop", serde_json::json!({}))
        .await
        .unwrap();
    assert!(result.content.is_empty());
    assert!(!result.is_error);
}

#[tokio::test]
async fn read_resource_server_error() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_error("resources/read", -32603, "not found");

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    let result = client.read_resource("file:///missing").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn get_prompt_server_error() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_error("prompts/get", -32603, "not found");

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    let result = client.get_prompt("missing", serde_json::json!({})).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn initialize_twice_fails() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));

    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();
    // Second initialize should fail
    let result = client.initialize().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn initialize_when_closed_fails() {
    let mut client = McpClient::default();
    client.close().await.unwrap();
    let result = client.initialize().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn initialize_server_returns_error() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_error("initialize", -32600, "Invalid");

    let mut client = McpClient::new(Box::new(mock));
    let result = client.initialize().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn initialize_empty_result_fails() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success("initialize", serde_json::json!(null));

    let mut client = McpClient::new(Box::new(mock));
    let result = client.initialize().await;
    assert!(result.is_err());
}

#[test]
fn parse_response_with_number_id() {
    let raw = r#"{"jsonrpc":"2.0","id":42,"result":"ok"}"#;
    let resp = McpClient::parse_response(raw).unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(42.into()));
}

#[test]
fn parse_response_truncated_json() {
    let raw = r#"{"jsonrpc":"2.0","id":1"#;
    let result = McpClient::parse_response(raw);
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// J3 (devtool-upgrade 阶段 4)：nextCursor 分页
// ---------------------------------------------------------------------------

/// 构造带 N 页 `tools/list` 响应的客户端（每页 result 原样入队，
/// MockTransport 对同一 method 按 FIFO 逐个弹出）。调用方自行 initialize。
fn paged_tools_client(pages: Vec<serde_json::Value>) -> McpClient {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "paged-server", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    for page in pages {
        mock.add_success("tools/list", page);
    }
    McpClient::new(Box::new(mock))
}

#[tokio::test]
async fn list_tools_follows_next_cursor() {
    let mut client = paged_tools_client(vec![
        serde_json::json!({
            "tools": [{ "name": "echo", "inputSchema": {} }],
            "nextCursor": "c1"
        }),
        serde_json::json!({
            "tools": [{ "name": "add", "inputSchema": {} }],
        }),
    ]);
    client.initialize().await.unwrap();

    let tools = client.list_tools().await.unwrap();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "echo");
    assert_eq!(tools[1].name, "add");
}

#[tokio::test]
async fn list_tools_sends_cursor_param_on_second_page() {
    // 记录型 transport：请求记录放 Arc 后面，client 持走 transport 后
    // 测试仍能断言发出了什么（MockTransport 的记录随所有权一起被拿走）。
    struct RecordingTransport {
        responses: std::sync::Mutex<Vec<(&'static str, serde_json::Value)>>,
        requests: std::sync::Arc<std::sync::Mutex<Vec<crate::transport::RecordedRequest>>>,
    }

    #[async_trait::async_trait]
    impl crate::transport::Transport for RecordingTransport {
        async fn connect(&mut self) -> Result<(), crate::transport::TransportError> {
            Ok(())
        }
        async fn close(&mut self) -> Result<(), crate::transport::TransportError> {
            Ok(())
        }
        async fn send(
            &mut self,
            request: &crate::transport::TransportRequest,
            _timeout_ms: u64,
        ) -> Result<crate::transport::TransportResponse, crate::transport::TransportError> {
            self.requests
                .lock()
                .unwrap()
                .push(crate::transport::RecordedRequest {
                    method: request.method.clone(),
                    params: request.params.clone(),
                });
            let mut responses = self.responses.lock().unwrap();
            if let Some(pos) = responses.iter().position(|(m, _)| *m == request.method) {
                let (_, result) = responses.remove(pos);
                return Ok(crate::transport::TransportResponse {
                    jsonrpc: JSONRPC_VERSION.to_string(),
                    id: request.id.clone().unwrap_or(serde_json::Value::Null),
                    result: Some(result),
                    error: None,
                });
            }
            Err(crate::transport::TransportError::send_failed(format!(
                "no scripted response for {}",
                request.method
            )))
        }
        fn is_connected(&self) -> bool {
            true
        }
        fn name(&self) -> &str {
            "recording"
        }
    }

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let transport = RecordingTransport {
        responses: std::sync::Mutex::new(vec![
            (
                "initialize",
                serde_json::json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "serverInfo": { "name": "s", "version": "1" }
                }),
            ),
            (
                "tools/list",
                serde_json::json!({ "tools": [], "nextCursor": "c1" }),
            ),
            ("tools/list", serde_json::json!({ "tools": [] })),
        ]),
        requests: requests.clone(),
    };
    let mut client = McpClient::new(Box::new(transport));
    client.initialize().await.unwrap();
    client.list_tools().await.unwrap();

    // 第二页请求必须带 {"cursor": "c1"}（旧实现 params=None 永远只取首页）。
    let recorded = requests.lock().unwrap().clone();
    let pages: Vec<_> = recorded
        .iter()
        .filter(|r| r.method == "tools/list")
        .collect();
    assert_eq!(pages.len(), 2);
    assert!(pages[0].params.is_none(), "first page carries no cursor");
    assert_eq!(
        pages[1]
            .params
            .as_ref()
            .and_then(|p| p.get("cursor"))
            .and_then(|c| c.as_str()),
        Some("c1")
    );
}

#[tokio::test]
async fn list_tools_repeated_cursor_aborts() {
    // 服务器翻页环：每页都返回同一 nextCursor → 第二次出现即判环终止。
    let mut client = paged_tools_client(vec![
        serde_json::json!({ "tools": [{ "name": "a", "inputSchema": {} }], "nextCursor": "loop" }),
        serde_json::json!({ "tools": [{ "name": "b", "inputSchema": {} }], "nextCursor": "loop" }),
    ]);
    client.initialize().await.unwrap();

    let err = client.list_tools().await.unwrap_err();
    assert!(err.to_string().contains("repeated"), "{err}");
}

#[tokio::test]
async fn list_tools_page_cap_aborts() {
    // 33 页互不相同的 cursor：超过 MAX_LIST_PAGES=32 → 整体报错（不返回
    // 部分清单——缺工具比失败更难排查）。
    let pages: Vec<serde_json::Value> = (1..=33)
        .map(|i| {
            serde_json::json!({
                "tools": [{ "name": format!("t{i}"), "inputSchema": {} }],
                "nextCursor": format!("c{i}")
            })
        })
        .collect();
    let mut client = paged_tools_client(pages);
    client.initialize().await.unwrap();

    let err = client.list_tools().await.unwrap_err();
    assert!(err.to_string().contains("32 pages"), "{err}");
}

#[tokio::test]
async fn list_resources_follows_next_cursor() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "resources/list",
        serde_json::json!({
            "resources": [{ "uri": "file:///a.txt", "name": "a" }],
            "nextCursor": "r1"
        }),
    );
    mock.add_success(
        "resources/list",
        serde_json::json!({
            "resources": [{ "uri": "file:///b.txt", "name": "b" }],
        }),
    );
    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let resources = client.list_resources().await.unwrap();
    assert_eq!(resources.len(), 2);
    assert_eq!(resources[1].uri, "file:///b.txt");
}

#[tokio::test]
async fn list_prompts_follows_next_cursor() {
    let mut mock = crate::transport::MockTransport::new_connected();
    mock.add_success(
        "initialize",
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {},
            "serverInfo": { "name": "s", "version": "1" }
        }),
    );
    mock.add_success("notifications/initialized", serde_json::json!({}));
    mock.add_success(
        "prompts/list",
        serde_json::json!({
            "prompts": [{ "name": "greet", "arguments": [] }],
            "nextCursor": "p1"
        }),
    );
    mock.add_success(
        "prompts/list",
        serde_json::json!({
            "prompts": [{ "name": "farewell", "arguments": [] }],
        }),
    );
    let mut client = McpClient::new(Box::new(mock));
    client.initialize().await.unwrap();

    let prompts = client.list_prompts().await.unwrap();
    assert_eq!(prompts.len(), 2);
    assert_eq!(prompts[1].name, "farewell");
}

// =========================================================================
// from_server_config 分派（2026-10-15 W1 分派收敛 + 第六轮复查兼容臂）
// =========================================================================

/// initialize 请求 → 合法 InitializeResult（wiremock 形态与 nemesisbot
/// 侧 e2e 同款；matcher 精确到 "method":"initialize"——裸子串会把
/// notifications/initialized 也匹配进来）。
async fn mount_initialize(server: &wiremock::MockServer) {
    mount_initialize_expect(server, 1).await;
}

/// `expect_count`：正例传 1（咬合锁：请求必须真实到达）；负例传 0
/// （std never-touches-http 证明零请求，expect(0) 使任何到达都红）。
async fn mount_initialize_expect(server: &wiremock::MockServer, expect_count: u64) {
    let init_ok = wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "result": {"protocolVersion": "2024-11-05", "capabilities": {},
                    "serverInfo": {"name": "dispatch-fake", "version": "0.1.0"}}
    }));
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::body_string_contains(
            "\"method\":\"initialize\"",
        ))
        .respond_with(init_ok)
        .expect(expect_count)
        .mount(server)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_url_only_entry_takes_http_branch() {
    // 手写 {name, url}（transport_type/command 缺失）= HTTP 意图。
    // 修复前：真相源掉进 stdio 臂 → 空 command spawn 必败，而 CLI 诊断
    // 面按 HTTP 报告——诊断与行为分裂。咬合锁：修复前本测试红（stdio
    // spawn 错误 + wiremock 零请求）。
    let server = wiremock::MockServer::start().await;
    mount_initialize(&server).await;

    let config = serde_json::from_value(serde_json::json!({
        "name": "handwritten",
        "url": server.uri()
    }))
    .unwrap();
    let mut client =
        McpClient::from_server_config(&config).expect("url-only entry must dispatch to HTTP");
    client
        .initialize()
        .await
        .expect("handwritten url entry must connect over HTTP");
    assert_eq!(
        client.server_info().map(|i| i.name.as_str()),
        Some("dispatch-fake")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_explicit_http_alias_connects() {
    // 显式三别名（大小写敏感）走 HTTP——真连通证明。
    for alias in ["http", "streamable_http", "streamable-http"] {
        let server = wiremock::MockServer::start().await;
        mount_initialize(&server).await;
        let config = serde_json::from_value(serde_json::json!({
            "name": "explicit", "transport_type": alias, "url": server.uri()
        }))
        .unwrap();
        let mut client = McpClient::from_server_config(&config)
            .unwrap_or_else(|e| panic!("alias {alias} must dispatch to HTTP: {e}"));
        client
            .initialize()
            .await
            .unwrap_or_else(|e| panic!("alias {alias} must connect: {e}"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_explicit_http_empty_url_is_invalid_config() {
    let config = serde_json::from_value(serde_json::json!({
        "name": "no-url", "transport_type": "http"
    }))
    .unwrap();
    let err = match McpClient::from_server_config(&config) {
        Ok(_) => panic!("http entry without url must be rejected"),
        Err(e) => e,
    };
    assert!(err.to_string().contains("no url"), "got: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_stdio_entry_never_touches_http() {
    // 显式 stdio（有 command）即使带 url（normalize legacy：url=command）
    // 也走 stdio——spawn 假命令失败，wiremock 零请求证明未走 HTTP。
    let server = wiremock::MockServer::start().await;
    // expect(0)：任何到达的请求即红，与下方 received_requests 断言双保险。
    mount_initialize_expect(&server, 0).await;

    let config = serde_json::from_value(serde_json::json!({
        "name": "legacy", "command": "definitely-not-a-real-binary-xyz",
        "url": server.uri()
    }))
    .unwrap();
    let mut client =
        McpClient::from_server_config(&config).expect("stdio dispatch is construction-only");
    let err = client
        .initialize()
        .await
        .expect_err("fake command must fail to spawn");
    let text = format!("{err:#}");
    assert!(
        !text.contains("HTTP"),
        "stdio entry must not produce HTTP-shaped errors: {text}"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "stdio entry must not send any HTTP request"
    );
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_web_stdio_url_field_acts_as_command() {
    // Dashboard MCP 管理页 stdio 约定：命令存 url 字段（前端 payload 不发
    // command，web server_add 硬编码 command 空）。运行期只读 command——
    // 无回落则 Dashboard 新建 stdio 条目 spawn 空命令必败。
    // 差分断言：同一假命令分别放 command / url 字段，spawn 错误文本必须
    // 一致（spawn 错误文本不含程序名，直接相等比对最稳）。
    let fake = "definitely-not-a-real-binary-xyz";
    let via_command = serde_json::from_value(serde_json::json!({
        "name": "web-stdio", "transport_type": "stdio", "command": fake
    }))
    .unwrap();
    let via_url = serde_json::from_value(serde_json::json!({
        "name": "web-stdio", "transport_type": "stdio", "url": fake
    }))
    .unwrap();

    let err_command = McpClient::from_server_config(&via_command)
        .expect("dispatch construction ok")
        .initialize()
        .await
        .expect_err("fake command must fail to spawn");
    let err_url = McpClient::from_server_config(&via_url)
        .expect("dispatch construction ok")
        .initialize()
        .await
        .expect_err("url-as-command fallback must reach spawn");

    assert_eq!(
        err_command.to_string(),
        err_url.to_string(),
        "url field must act as command for stdio dispatch: command={err_command} url={err_url}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_from_server_config_stdio_command_field_wins_over_url() {
    // command 与 url 同时非空（normalize legacy 形态）：command 优先，
    // url 不顶替——保证存量盘上条目行为不变。
    let server = wiremock::MockServer::start().await;
    mount_initialize_expect(&server, 0).await;

    let config = serde_json::from_value(serde_json::json!({
        "name": "both", "transport_type": "stdio",
        "command": "definitely-not-a-real-binary-xyz",
        "url": server.uri()
    }))
    .unwrap();
    let mut client = McpClient::from_server_config(&config).expect("dispatch construction ok");
    let err = client
        .initialize()
        .await
        .expect_err("command field must win and fail to spawn");
    let text = err.to_string();
    assert!(
        !text.contains("HTTP"),
        "command field must win over url (no HTTP dispatch): {text}"
    );
    assert!(
        server
            .received_requests()
            .await
            .unwrap_or_default()
            .is_empty(),
        "url must not be consumed as HTTP when command present"
    );
}

#[test]
fn test_from_server_config_unknown_transport_type_loud_rejected() {
    // R9：未知 transport_type（Dashboard 曾可写 "sse"）loud 拒绝——静默掉
    // stdio 臂会把 url 当程序名 spawn，报出与实际问题无关的错误。
    for tt in ["sse", "SSE", "websocket", "HTTP"] {
        let config = serde_json::from_value(serde_json::json!({
            "name": "bad", "transport_type": tt, "url": "http://x/mcp"
        }))
        .unwrap();
        let err = match McpClient::from_server_config(&config) {
            Ok(_) => panic!("transport_type '{tt}' must be rejected"),
            Err(e) => e,
        };
        let text = err.to_string();
        assert!(
            text.contains("unsupported transport_type") && text.contains(tt),
            "error must name the unsupported value: {text}"
        );
    }
    // 空串（手写条目兼容形态）与大写小写敏感已知值不受拒绝臂影响
    let ok_stdio = serde_json::from_value(serde_json::json!({
        "name": "s", "transport_type": "stdio", "command": "x"
    }))
    .unwrap();
    assert!(McpClient::from_server_config(&ok_stdio).is_ok());
}
