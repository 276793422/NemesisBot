use super::*;

fn make_server() -> McpServer {
    let mut server = McpServer::new("test-server", "1.0.0");

    // Register an echo tool.
    let echo_tool = McpTool {
        name: "echo".into(),
        description: Some("Echo back the input".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "message": { "type": "string" } },
            "required": ["message"]
        }),
    };
    let echo_handler: ToolHandler = Arc::new(|args| {
        let msg = args
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        ToolCallResult::ok(msg)
    });
    server.register_tool(echo_tool, echo_handler).unwrap();

    // Register an error tool.
    let fail_tool = McpTool {
        name: "always_fail".into(),
        description: Some("Always returns an error".into()),
        input_schema: serde_json::json!({ "type": "object", "properties": {} }),
    };
    let fail_handler: ToolHandler = Arc::new(|_| ToolCallResult::err("deliberate failure"));
    server.register_tool(fail_tool, fail_handler).unwrap();

    // Register a resource.
    server.register_resource(
        Resource {
            uri: "file:///test.txt".into(),
            name: "test.txt".into(),
            description: Some("A test resource".into()),
            mime_type: Some("text/plain".into()),
        },
        ResourceContent {
            uri: "file:///test.txt".into(),
            mime_type: Some("text/plain".into()),
            text: Some("hello world".into()),
        },
    );

    server
}

#[tokio::test]
async fn initialize_handshake() {
    let server = make_server();
    let req = JSONRPCRequest::new("initialize", None);
    let resp = server.handle_request(&req).await;

    assert!(!resp.is_error());
    let result = resp.result.unwrap();
    assert_eq!(result["protocolVersion"], PROTOCOL_VERSION);
    assert_eq!(result["serverInfo"]["name"], "test-server");
    assert_eq!(result["serverInfo"]["version"], "1.0.0");
}

#[tokio::test]
async fn list_and_call_tools() {
    let server = make_server();

    // List tools.
    let list_req = JSONRPCRequest::new("tools/list", None);
    let list_resp = server.handle_request(&list_req).await;
    assert!(!list_resp.is_error());

    let tools_binding = list_resp.result.unwrap();
    let tools = tools_binding["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);

    // Call echo tool.
    let call_req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "name": "echo",
            "arguments": { "message": "hi there" }
        })),
    );
    let call_resp = server.handle_request(&call_req).await;
    assert!(!call_resp.is_error());

    let result: ToolCallResult = serde_json::from_value(
        call_resp.result.unwrap(),
    )
    .unwrap();
    assert!(!result.is_error);
    assert_eq!(result.content[0].text.as_deref(), Some("hi there"));

    // Call the always-fail tool.
    let fail_req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "name": "always_fail",
            "arguments": {}
        })),
    );
    let fail_resp = server.handle_request(&fail_req).await;
    let fail_result: ToolCallResult =
        serde_json::from_value(fail_resp.result.unwrap()).unwrap();
    assert!(fail_result.is_error);
}

#[tokio::test]
async fn resources_list_and_read() {
    let server = make_server();

    // List resources.
    let list_req = JSONRPCRequest::new("resources/list", None);
    let list_resp = server.handle_request(&list_req).await;
    let res_binding = list_resp.result.unwrap();
    let resources = res_binding["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 1);
    assert_eq!(resources[0]["uri"], "file:///test.txt");

    // Read resource.
    let read_req = JSONRPCRequest::new(
        "resources/read",
        Some(serde_json::json!({ "uri": "file:///test.txt" })),
    );
    let read_resp = server.handle_request(&read_req).await;
    assert!(!read_resp.is_error());
    let cont_binding = read_resp.result.unwrap();
    let contents = cont_binding["contents"].as_array().unwrap();
    assert_eq!(contents[0]["text"], "hello world");
}

#[tokio::test]
async fn method_not_found_and_raw_handling() {
    let server = make_server();

    // Unknown method.
    let req = JSONRPCRequest::new("nonexistent/method", None);
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
    assert_eq!(resp.error.unwrap().code, JSONRPCError::METHOD_NOT_FOUND);

    // Raw JSON handling (malformed).
    let raw_resp = server.handle_raw("not valid json").await;
    let parsed: JSONRPCResponse = serde_json::from_str(&raw_resp).unwrap();
    assert!(parsed.is_error());
    assert_eq!(parsed.error.unwrap().code, JSONRPCError::PARSE_ERROR);

    // Raw JSON handling (valid request).
    let valid_raw = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "ping"
    })
    .to_string();
    let ping_resp_raw = server.handle_raw(&valid_raw).await;
    let ping_resp: JSONRPCResponse = serde_json::from_str(&ping_resp_raw).unwrap();
    assert!(!ping_resp.is_error());
}

#[tokio::test]
async fn call_nonexistent_tool() {
    let server = make_server();
    let req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "name": "nonexistent_tool",
            "arguments": {}
        })),
    );
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn call_tool_missing_name() {
    let server = make_server();
    let req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "arguments": {}
        })),
    );
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn read_nonexistent_resource() {
    let server = make_server();
    let req = JSONRPCRequest::new(
        "resources/read",
        Some(serde_json::json!({ "uri": "file:///nonexistent.txt" })),
    );
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn register_duplicate_tool() {
    let mut server = McpServer::new("test", "1.0");
    let tool = McpTool {
        name: "dup".into(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let handler: ToolHandler = Arc::new(|_| ToolCallResult::ok("ok"));
    server.register_tool(tool.clone(), handler.clone()).unwrap();
    let result = server.register_tool(tool, handler);
    assert!(result.is_err());
}

#[test]
fn server_info() {
    let server = McpServer::new("my-server", "2.0.0");
    assert_eq!(server.info().name, "my-server");
    assert_eq!(server.info().version, "2.0.0");
}

#[test]
fn tool_count_via_list() {
    let mut server = McpServer::new("test", "1.0");

    // Initially empty
    let tool = McpTool {
        name: "t1".into(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let handler: ToolHandler = Arc::new(|_| ToolCallResult::ok("ok"));
    server.register_tool(tool, handler).unwrap();

    // Verify via handle_request (tools/list)
    let rt = tokio::runtime::Runtime::new().unwrap();
    let req = JSONRPCRequest::new("tools/list", None);
    let resp = rt.block_on(server.handle_request(&req));
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
}

#[test]
fn resource_count_via_list() {
    let server = McpServer::new("test", "1.0");
    let rt = tokio::runtime::Runtime::new().unwrap();
    let req = JSONRPCRequest::new("resources/list", None);
    let resp = rt.block_on(server.handle_request(&req));
    let result = resp.result.unwrap();
    let resources = result["resources"].as_array().unwrap();
    assert!(resources.is_empty());
}

#[tokio::test]
async fn prompts_list_not_supported() {
    // Server without prompts capability returns method not found for prompts/list
    let server = McpServer::new("test", "1.0");
    let req = JSONRPCRequest::new("prompts/list", None);
    let resp = server.handle_request(&req).await;
    // prompts/list is not in the method dispatch table
    assert!(resp.is_error());
}

#[tokio::test]
async fn prompts_get_nonexistent() {
    let server = McpServer::new("test", "1.0");
    let req = JSONRPCRequest::new(
        "prompts/get",
        Some(serde_json::json!({ "name": "nonexistent" })),
    );
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn server_capabilities() {
    let server = make_server();
    let req = JSONRPCRequest::new("initialize", None);
    let resp = server.handle_request(&req).await;
    let result = resp.result.unwrap();
    assert!(result["capabilities"].is_object());
}

#[tokio::test]
async fn handle_raw_empty_body() {
    let server = make_server();
    let resp = server.handle_raw("").await;
    let parsed: JSONRPCResponse = serde_json::from_str(&resp).unwrap();
    assert!(parsed.is_error());
}

#[tokio::test]
async fn multiple_tools_registered() {
    let mut server = McpServer::new("test", "1.0");

    for i in 0..5 {
        let tool = McpTool {
            name: format!("tool_{}", i),
            description: Some(format!("Tool number {}", i)),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let handler: ToolHandler = Arc::new(move |_| ToolCallResult::ok(format!("result_{}", i)));
        server.register_tool(tool, handler).unwrap();
    }

    let req = JSONRPCRequest::new("tools/list", None);
    let resp = server.handle_request(&req).await;
    let result = resp.result.unwrap();
    let tools = result["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 5);
}

// ---- New tests ----

#[tokio::test]
async fn shared_server_clone_and_handle() {
    let shared = SharedMcpServer::new("shared-test", "1.0");
    let _shared2 = shared.clone();

    let req = JSONRPCRequest::new("ping", None);
    let resp = shared.handle_request(&req).await;
    assert!(!resp.is_error());
}

#[tokio::test]
async fn shared_server_register_tool() {
    let shared = SharedMcpServer::new("shared-test", "1.0");
    let tool = McpTool {
        name: "shared_tool".into(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let handler: ToolHandler = Arc::new(|_| ToolCallResult::ok("shared ok"));
    shared.register_tool(tool, handler).await.unwrap();

    let req = JSONRPCRequest::new("tools/list", None);
    let resp = shared.handle_request(&req).await;
    let binding = resp.result.unwrap();
    let tools = binding["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 1);
}

#[tokio::test]
async fn shared_server_handle_raw() {
    let shared = SharedMcpServer::new("raw-test", "1.0");
    let raw = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
    let resp_str = shared.handle_raw(raw).await;
    let resp: JSONRPCResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.is_error());
}

#[tokio::test]
async fn server_ping() {
    let server = McpServer::new("test", "1.0");
    let req = JSONRPCRequest::new("ping", None);
    let resp = server.handle_request(&req).await;
    assert!(!resp.is_error());
    assert!(resp.result.unwrap().as_object().unwrap().is_empty());
}

#[tokio::test]
async fn call_tool_without_arguments() {
    let server = make_server();
    let req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "name": "echo"
        })),
    );
    let resp = server.handle_request(&req).await;
    assert!(!resp.is_error());
}

#[tokio::test]
async fn call_tool_with_null_params() {
    let server = make_server();
    let req = JSONRPCRequest {
        jsonrpc: "2.0".into(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "tools/call".into(),
        params: None,
    };
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn read_resource_missing_uri() {
    let server = make_server();
    let req = JSONRPCRequest::new(
        "resources/read",
        Some(serde_json::json!({})),
    );
    let resp = server.handle_request(&req).await;
    assert!(resp.is_error());
}

#[tokio::test]
async fn server_capabilities_has_tools_and_resources() {
    let server = make_server();
    let caps = server.capabilities();
    assert!(caps.tools.is_some());
    assert!(caps.resources.is_some());
    assert!(caps.prompts.is_none());
}

#[tokio::test]
async fn register_multiple_resources() {
    let mut server = McpServer::new("test", "1.0");
    for i in 0..3 {
        server.register_resource(
            Resource {
                uri: format!("file:///{}.txt", i),
                name: format!("{}.txt", i),
                description: None,
                mime_type: None,
            },
            ResourceContent {
                uri: format!("file:///{}.txt", i),
                mime_type: None,
                text: Some(format!("content {}", i)),
            },
        );
    }
    let req = JSONRPCRequest::new("resources/list", None);
    let resp = server.handle_request(&req).await;
    let binding = resp.result.unwrap();
    let resources = binding["resources"].as_array().unwrap();
    assert_eq!(resources.len(), 3);

    // Read each
    for i in 0..3 {
        let read_req = JSONRPCRequest::new(
            "resources/read",
            Some(serde_json::json!({ "uri": format!("file:///{}.txt", i) })),
        );
        let read_resp = server.handle_request(&read_req).await;
        assert!(!read_resp.is_error());
    }
}

#[tokio::test]
async fn handle_raw_valid_tools_list() {
    let server = make_server();
    let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let resp_str = server.handle_raw(raw).await;
    let resp: JSONRPCResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.is_error());
}

#[tokio::test]
async fn handle_raw_notification_no_id() {
    let server = make_server();
    let raw = r#"{"jsonrpc":"2.0","method":"ping"}"#;
    let resp_str = server.handle_raw(raw).await;
    let resp: JSONRPCResponse = serde_json::from_str(&resp_str).unwrap();
    assert!(!resp.is_error());
    assert_eq!(resp.id, serde_json::Value::Null);
}

#[test]
fn server_error_display() {
    let e = ServerError::ToolNotFound("x".into());
    assert!(e.to_string().contains("x"));

    let e2 = ServerError::InvalidRequest("bad".into());
    assert!(e2.to_string().contains("bad"));
}

#[test]
fn tool_handler_arc_clone() {
    let handler: ToolHandler = Arc::new(|_| ToolCallResult::ok("ok"));
    let handler2 = handler.clone();
    let result = handler(serde_json::json!({}));
    assert!(!result.is_error);
    let result2 = handler2(serde_json::json!({}));
    assert!(!result2.is_error);
}

#[tokio::test]
async fn register_and_call_tool_with_complex_args() {
    let mut server = McpServer::new("test", "1.0");
    let tool = McpTool {
        name: "compute".into(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let handler: ToolHandler = Arc::new(|args| {
        let a = args.get("a").and_then(|v| v.as_i64()).unwrap_or(0);
        let b = args.get("b").and_then(|v| v.as_i64()).unwrap_or(0);
        ToolCallResult::ok(format!("{}", a + b))
    });
    server.register_tool(tool, handler).unwrap();

    let req = JSONRPCRequest::new(
        "tools/call",
        Some(serde_json::json!({
            "name": "compute",
            "arguments": { "a": 3, "b": 5 }
        })),
    );
    let resp = server.handle_request(&req).await;
    assert!(!resp.is_error());
    let result: ToolCallResult = serde_json::from_value(resp.result.unwrap()).unwrap();
    assert_eq!(result.content[0].text.as_deref(), Some("8"));
}

#[test]
fn server_info_method() {
    let server = McpServer::new("my-server", "3.0");
    assert_eq!(server.info().name, "my-server");
    assert_eq!(server.info().version, "3.0");
}

#[tokio::test]
async fn empty_server_tools_list() {
    let server = McpServer::new("empty", "0.1");
    let req = JSONRPCRequest::new("tools/list", None);
    let resp = server.handle_request(&req).await;
    let binding = resp.result.unwrap();
    let tools = binding["tools"].as_array().unwrap();
    assert!(tools.is_empty());
}

#[tokio::test]
async fn empty_server_resources_list() {
    let server = McpServer::new("empty", "0.1");
    let req = JSONRPCRequest::new("resources/list", None);
    let resp = server.handle_request(&req).await;
    let binding = resp.result.unwrap();
    let resources = binding["resources"].as_array().unwrap();
    assert!(resources.is_empty());
}
