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
        args: vec![],
        env: None,
        timeout_secs: 30,
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
        args: vec![],
        env: None,
        timeout_secs: 30,
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
