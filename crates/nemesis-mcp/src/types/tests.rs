use super::*;

#[test]
fn serialize_and_deserialize_jsonrpc_request() {
    let req = JSONRPCRequest::new("tools/list", None);
    let json = serde_json::to_string(&req).unwrap();

    // Must contain the required fields
    assert!(json.contains("\"jsonrpc\":\"2.0\""));
    assert!(json.contains("\"method\":\"tools/list\""));
    assert!(json.contains("\"id\":"));

    let roundtrip: JSONRPCRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(roundtrip.jsonrpc, "2.0");
    assert_eq!(roundtrip.method, "tools/list");
    assert!(roundtrip.id.is_some());
}

#[test]
fn serialize_and_deserialize_jsonrpc_response() {
    let resp = JSONRPCResponse::success(
        serde_json::Value::String("abc".into()),
        serde_json::json!({"tools": []}),
    );
    let json = serde_json::to_string(&resp).unwrap();
    let roundtrip: JSONRPCResponse = serde_json::from_str(&json).unwrap();

    assert!(!roundtrip.is_error());
    assert_eq!(roundtrip.id, serde_json::Value::String("abc".into()));
    assert!(roundtrip.result.is_some());
    assert!(roundtrip.error.is_none());
}

#[test]
fn jsonrpc_error_display_and_std_error() {
    let err = JSONRPCError::new(-32601, "Method not found: foo");
    assert_eq!(
        format!("{err}"),
        "JSON-RPC error -32601: Method not found: foo"
    );
    // Verify it satisfies std::error::Error
    let _: &dyn std::error::Error = &err;
}

#[test]
fn mcp_tool_and_tool_call_result_roundtrip() {
    let tool = McpTool {
        name: "read_file".into(),
        description: Some("Read a file".into()),
        input_schema: serde_json::json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }),
    };
    let json = serde_json::to_string(&tool).unwrap();
    let rt: McpTool = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "read_file");

    let result = ToolCallResult::ok("hello");
    let rj = serde_json::to_string(&result).unwrap();
    let rr: ToolCallResult = serde_json::from_str(&rj).unwrap();
    assert!(!rr.is_error);
    assert_eq!(rr.content[0].text.as_deref(), Some("hello"));
}

#[test]
fn server_config_default_and_builder() {
    let cfg = ServerConfig::new("test", "node")
        .arg("server.js")
        .env("FOO=bar")
        .timeout(60);

    assert_eq!(cfg.name, "test");
    assert_eq!(cfg.command, "node");
    assert_eq!(cfg.args, vec!["server.js"]);
    assert_eq!(cfg.env.as_ref(), Some(&vec!["FOO=bar".to_string()]));
    assert_eq!(cfg.timeout_secs, 60);

    // Serialization round-trip
    let json = serde_json::to_string(&cfg).unwrap();
    let rt: ServerConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, cfg.name);
    assert_eq!(rt.timeout_secs, 60);
}

#[test]
fn prompt_types_roundtrip() {
    let prompt = Prompt {
        name: "greet".into(),
        description: Some("Greet a person".into()),
        arguments: vec![
            PromptArgument {
                name: "name".into(),
                description: Some("Person's name".into()),
                required: Some(true),
            },
        ],
    };
    let json = serde_json::to_string(&prompt).unwrap();
    let rt: Prompt = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "greet");
    assert_eq!(rt.arguments.len(), 1);
    assert_eq!(rt.arguments[0].name, "name");

    let result = PromptResult {
        messages: vec![PromptMessage {
            role: "user".into(),
            content: PromptMessageContent::text("Hello, {{name}}!"),
        }],
        description: Some("Greeting prompt".into()),
    };
    let rj = serde_json::to_string(&result).unwrap();
    let rr: PromptResult = serde_json::from_str(&rj).unwrap();
    assert_eq!(rr.messages.len(), 1);
    assert_eq!(rr.messages[0].role, "user");
    assert_eq!(
        rr.messages[0].content.text.as_deref(),
        Some("Hello, {{name}}!")
    );
}

#[test]
fn initialize_params_and_result_roundtrip() {
    let params = InitializeParams {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ClientCapabilities {
            tools: Some(serde_json::json!({})),
            resources: Some(serde_json::json!({})),
            prompts: Some(serde_json::json!({})),
        },
        client_info: ClientInfo {
            name: "test-client".into(),
            version: "1.0.0".into(),
        },
    };
    let json = serde_json::to_string(&params).unwrap();
    let rt: InitializeParams = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.protocol_version, PROTOCOL_VERSION);
    assert_eq!(rt.client_info.name, "test-client");

    let init_result = InitializeResult {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ServerCapabilities::default(),
        server_info: ServerInfo {
            name: "test-server".into(),
            version: "2.0.0".into(),
        },
    };
    let rj = serde_json::to_string(&init_result).unwrap();
    let rr: InitializeResult = serde_json::from_str(&rj).unwrap();
    assert_eq!(rr.server_info.name, "test-server");
}

#[test]
fn prompt_message_content_helpers() {
    let content = PromptMessageContent::text("hello world");
    assert_eq!(content.content_type, "text");
    assert_eq!(content.text.as_deref(), Some("hello world"));
    assert!(content.data.is_none());
}

#[test]
fn jsonrpc_request_with_params() {
    let params = serde_json::json!({"name": "test_tool"});
    let req = JSONRPCRequest::new("tools/call", Some(params.clone()));
    assert_eq!(req.method, "tools/call");
    assert_eq!(req.params, Some(params));
}

#[test]
fn jsonrpc_request_with_string_id() {
    let req = JSONRPCRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::String("test-id-123".to_string())),
        method: "tools/list".to_string(),
        params: None,
    };
    assert_eq!(req.id, Some(serde_json::Value::String("test-id-123".to_string())));
    assert_eq!(req.method, "tools/list");
}

#[test]
fn jsonrpc_response_error() {
    let resp = JSONRPCResponse::error(
        serde_json::Value::String("err-id".into()),
        JSONRPCError::new(-32600, "Invalid Request"),
    );
    assert!(resp.is_error());
    assert!(resp.error.is_some());
    assert!(resp.result.is_none());
    assert_eq!(resp.id, serde_json::Value::String("err-id".into()));
}

#[test]
fn jsonrpc_response_null_id() {
    let resp = JSONRPCResponse::error(
        serde_json::Value::Null,
        JSONRPCError::new(-32700, "Parse error"),
    );
    assert!(resp.is_error());
    assert_eq!(resp.id, serde_json::Value::Null);
}

#[test]
fn jsonrpc_error_codes() {
    let err = JSONRPCError::new(-32700, "Parse error");
    assert_eq!(err.code, -32700);

    let err = JSONRPCError::new(-32600, "Invalid Request");
    assert_eq!(err.code, -32600);

    let err = JSONRPCError::new(-32601, "Method not found");
    assert_eq!(err.code, -32601);

    let err = JSONRPCError::new(-32602, "Invalid params");
    assert_eq!(err.code, -32602);

    let err = JSONRPCError::new(-32603, "Internal error");
    assert_eq!(err.code, -32603);
}

#[test]
fn tool_call_result_error() {
    let result = ToolCallResult::err("file not found");
    assert!(result.is_error);
    assert_eq!(result.content[0].text.as_deref(), Some("file not found"));
}

#[test]
fn tool_call_result_serialization() {
    let result = ToolCallResult::ok("success output");
    let json = serde_json::to_string(&result).unwrap();
    assert!(json.contains("\"isError\":false"));
    assert!(json.contains("\"content\":"));

    let parsed: ToolCallResult = serde_json::from_str(&json).unwrap();
    assert!(!parsed.is_error);
}

#[test]
fn server_config_multiple_args() {
    let cfg = ServerConfig::new("multi", "python")
        .arg("server.py")
        .arg("--port")
        .arg("8080")
        .env("DEBUG=1")
        .env("LOG_LEVEL=info")
        .timeout(120);

    assert_eq!(cfg.args, vec!["server.py", "--port", "8080"]);
    assert_eq!(cfg.env.as_ref().map(|e| e.len()), Some(2));
    assert_eq!(cfg.timeout_secs, 120);
}

#[test]
fn server_config_default_timeout() {
    let cfg = ServerConfig::new("test", "node");
    assert_eq!(cfg.timeout_secs, 30);
}

#[test]
fn protocol_version_constant() {
    assert_eq!(PROTOCOL_VERSION, "2025-06-18");
}

#[test]
fn client_capabilities_default() {
    let caps = ClientCapabilities::default();
    assert!(caps.tools.is_none());
    assert!(caps.resources.is_none());
    assert!(caps.prompts.is_none());
}

#[test]
fn server_capabilities_default() {
    let caps = ServerCapabilities::default();
    assert!(caps.tools.is_none());
    assert!(caps.resources.is_none());
    assert!(caps.prompts.is_none());
}

#[test]
fn mcp_tool_without_description() {
    let tool = McpTool {
        name: "simple_tool".into(),
        description: None,
        input_schema: serde_json::json!({"type": "object"}),
    };
    let json = serde_json::to_string(&tool).unwrap();
    let rt: McpTool = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "simple_tool");
    assert!(rt.description.is_none());
}

#[test]
fn prompt_without_arguments() {
    let prompt = Prompt {
        name: "simple".into(),
        description: None,
        arguments: vec![],
    };
    let json = serde_json::to_string(&prompt).unwrap();
    let rt: Prompt = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.arguments.len(), 0);
}

#[test]
fn prompt_message_content_image() {
    let content = PromptMessageContent {
        content_type: "image".to_string(),
        text: None,
        data: Some("base64encodeddata".to_string()),
    };
    assert_eq!(content.content_type, "image");
    assert!(content.text.is_none());
    assert!(content.data.is_some());
}

#[test]
fn jsonrpc_request_deserialize_with_null_params() {
    let json = r#"{"jsonrpc":"2.0","method":"ping","id":1,"params":null}"#;
    let req: JSONRPCRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.method, "ping");
    assert!(req.params.is_none());
}

#[test]
fn initialize_params_serialization() {
    let params = InitializeParams {
        protocol_version: PROTOCOL_VERSION.to_string(),
        capabilities: ClientCapabilities::default(),
        client_info: ClientInfo {
            name: "test".into(),
            version: "1.0".into(),
        },
    };
    let json = serde_json::to_string(&params).unwrap();
    assert!(json.contains("protocolVersion"));
    assert!(json.contains("capabilities"));
    assert!(json.contains("clientInfo"));
}

#[test]
fn jsonrpc_request_notification_no_id() {
    let req = JSONRPCRequest::notification("some/event", Some(serde_json::json!({"data": 1})));
    assert!(req.id.is_none());
    assert_eq!(req.method, "some/event");
    assert!(req.params.is_some());
    let json = serde_json::to_string(&req).unwrap();
    assert!(!json.contains("\"id\":"));
}

#[test]
fn jsonrpc_error_convenience_methods() {
    let e1 = JSONRPCError::method_not_found("foo/bar");
    assert_eq!(e1.code, JSONRPCError::METHOD_NOT_FOUND);
    assert!(e1.message.contains("foo/bar"));

    let e2 = JSONRPCError::invalid_params("missing x");
    assert_eq!(e2.code, JSONRPCError::INVALID_PARAMS);

    let e3 = JSONRPCError::internal("boom");
    assert_eq!(e3.code, JSONRPCError::INTERNAL_ERROR);
}

#[test]
fn tool_content_text_helper() {
    let tc = ToolContent::text("hello");
    assert_eq!(tc.content_type, "text");
    assert_eq!(tc.text.as_deref(), Some("hello"));
}

#[test]
fn tool_call_result_default() {
    let tcr = ToolCallResult::default();
    assert!(tcr.content.is_empty());
    assert!(!tcr.is_error);
}

#[test]
fn resource_serialization_roundtrip() {
    let r = Resource {
        uri: "file:///x.txt".into(),
        name: "x".into(),
        description: Some("desc".into()),
        mime_type: Some("text/plain".into()),
    };
    let json = serde_json::to_string(&r).unwrap();
    let rt: Resource = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.uri, "file:///x.txt");
    assert_eq!(rt.name, "x");
    assert_eq!(rt.description.as_deref(), Some("desc"));
    assert_eq!(rt.mime_type.as_deref(), Some("text/plain"));
}

#[test]
fn resource_content_default() {
    let rc = ResourceContent::default();
    assert!(rc.uri.is_empty());
    assert!(rc.mime_type.is_none());
    assert!(rc.text.is_none());
}

#[test]
fn resource_content_roundtrip() {
    let rc = ResourceContent {
        uri: "mem://data".into(),
        mime_type: Some("application/json".into()),
        text: Some(r#"{"key":"val"}"#.into()),
    };
    let json = serde_json::to_string(&rc).unwrap();
    let rt: ResourceContent = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.uri, "mem://data");
    assert_eq!(rt.text.as_deref(), Some(r#"{"key":"val"}"#));
}

#[test]
fn server_info_serialization() {
    let si = ServerInfo { name: "srv".into(), version: "0.1".into() };
    let json = serde_json::to_string(&si).unwrap();
    let rt: ServerInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "srv");
    assert_eq!(rt.version, "0.1");
}

#[test]
fn client_info_serialization() {
    let ci = ClientInfo { name: "cli".into(), version: "2.0".into() };
    let json = serde_json::to_string(&ci).unwrap();
    let rt: ClientInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.name, "cli");
}

#[test]
fn initialize_result_roundtrip() {
    let ir = InitializeResult {
        protocol_version: "2025-06-18".into(),
        capabilities: ServerCapabilities {
            tools: Some(ToolCapabilities { list_changed: Some(true) }),
            resources: None,
            prompts: None,
        },
        server_info: ServerInfo { name: "s".into(), version: "1".into() },
    };
    let json = serde_json::to_string(&ir).unwrap();
    let rt: InitializeResult = serde_json::from_str(&json).unwrap();
    assert!(rt.capabilities.tools.is_some());
    assert!(rt.capabilities.resources.is_none());
}

#[test]
fn prompt_argument_optional_fields() {
    let pa = PromptArgument {
        name: "arg1".into(),
        description: None,
        required: None,
    };
    let json = serde_json::to_string(&pa).unwrap();
    let rt: PromptArgument = serde_json::from_str(&json).unwrap();
    assert!(rt.description.is_none());
    assert!(rt.required.is_none());
}

#[test]
fn prompt_message_content_resource() {
    let pmc = PromptMessageContent {
        content_type: "resource".into(),
        text: None,
        data: Some("base64data".into()),
    };
    let json = serde_json::to_string(&pmc).unwrap();
    let rt: PromptMessageContent = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.content_type, "resource");
    assert!(rt.text.is_none());
}

#[test]
fn prompt_result_default() {
    let pr = PromptResult::default();
    assert!(pr.messages.is_empty());
    assert!(pr.description.is_none());
}

#[test]
fn mcp_tool_input_schema_serialized_as_input_schema() {
    let tool = McpTool {
        name: "t".into(),
        description: None,
        input_schema: serde_json::json!({"type":"object"}),
    };
    let json = serde_json::to_string(&tool).unwrap();
    assert!(json.contains("\"inputSchema\""));
}

#[test]
fn server_config_deserialize_with_defaults() {
    let json = r#"{"name":"n","command":"c"}"#;
    let cfg: ServerConfig = serde_json::from_str(json).unwrap();
    assert_eq!(cfg.name, "n");
    assert_eq!(cfg.command, "c");
    assert!(cfg.args.is_empty());
    assert!(cfg.env.is_none());
    assert_eq!(cfg.timeout_secs, 30);
}

#[test]
fn jsonrpc_request_new_has_unique_ids() {
    let r1 = JSONRPCRequest::new("m1", None);
    let r2 = JSONRPCRequest::new("m2", None);
    assert_ne!(r1.id, r2.id);
}

#[test]
fn jsonrpc_response_is_error_false_on_success() {
    let resp = JSONRPCResponse::success(serde_json::Value::Null, serde_json::json!({}));
    assert!(!resp.is_error());
}

#[test]
fn jsonrpc_error_with_data() {
    let err = JSONRPCError {
        code: -32001,
        message: "custom".into(),
        data: Some(serde_json::json!({"detail":"info"})),
    };
    let json = serde_json::to_string(&err).unwrap();
    let rt: JSONRPCError = serde_json::from_str(&json).unwrap();
    assert!(rt.data.is_some());
    assert_eq!(rt.data.unwrap()["detail"], "info");
}

#[test]
fn tool_capabilities_default() {
    let tc = ToolCapabilities { list_changed: None };
    let json = serde_json::to_string(&tc).unwrap();
    assert!(!json.contains("listChanged"));
}

#[test]
fn resource_capabilities_roundtrip() {
    let rc = ResourceCapabilities {
        subscribe: Some(true),
        list_changed: Some(false),
    };
    let json = serde_json::to_string(&rc).unwrap();
    let rt: ResourceCapabilities = serde_json::from_str(&json).unwrap();
    assert_eq!(rt.subscribe, Some(true));
    assert_eq!(rt.list_changed, Some(false));
}

#[test]
fn prompt_capabilities_default() {
    let pc = PromptCapabilities { list_changed: None };
    let json = serde_json::to_string(&pc).unwrap();
    assert!(!json.contains("listChanged"));
}

#[test]
fn client_capabilities_with_all_fields() {
    let cc = ClientCapabilities {
        tools: Some(serde_json::json!({})),
        resources: Some(serde_json::json!({})),
        prompts: Some(serde_json::json!({})),
    };
    let json = serde_json::to_string(&cc).unwrap();
    let rt: ClientCapabilities = serde_json::from_str(&json).unwrap();
    assert!(rt.tools.is_some());
    assert!(rt.resources.is_some());
    assert!(rt.prompts.is_some());
}
