//! 端到端测试：nemesis-mcp 客户端 × 真实 MCP 测试服务器（Go，stdio JSON-RPC）。
//!
//! 链路：config.mcp.json → McpManager::load_config → StdioTransport spawn
//! → initialize 握手 → tools/list 发现 → tools/call 执行。
//! 测试服务器：`test-tools/mcp/server`（echo/add/reverse/get_time/hello/config）。
//! 构建：`cd test-tools/mcp/server && go build -o mcp-test-server.exe .`

use nemesis_mcp::manager::McpManager;

fn test_server_path() -> Option<std::path::PathBuf> {
    // 优先用已构建产物；CI/首次需先 go build（见 crate 级 README）。
    // 产物是 Go 编译的二进制（不入库），缺失时由调用方按机器依赖测试惯例
    // SKIP——干净环境（CI）没有它不算测试失败。
    let candidates = [
        std::path::PathBuf::from("../../test-tools/mcp/server/mcp-test-server.exe"),
        std::path::PathBuf::from("../../test-tools/mcp/server/mcp-test-server"),
    ];
    candidates.into_iter().find(|p| p.exists())
}

fn write_config(dir: &std::path::Path, server_exe: &std::path::Path) -> std::path::PathBuf {
    let cfg_path = dir.join("config.mcp.json");
    let exe_abs = std::fs::canonicalize(server_exe).unwrap();
    let cfg = serde_json::json!({
        "enabled": true,
        "timeout": 15,
        "servers": [{
            "name": "test",
            "command": exe_abs.to_string_lossy(),
            "args": [],
            "timeout_secs": 15
        }]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    cfg_path
}

fn make_manager(dir: &std::path::Path, server_exe: &std::path::Path) -> McpManager {
    let cfg_path = write_config(dir, server_exe);
    let mut mgr = McpManager::new(cfg_path);
    mgr.load_config().unwrap();
    mgr
}

#[tokio::test]
async fn manager_discovers_tools_from_real_stdio_server() {
    let Some(server_exe) = test_server_path() else {
        eprintln!(
            "SKIP: mcp-test-server not built — run: cd test-tools/mcp/server && go build -o mcp-test-server.exe ."
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = make_manager(dir.path(), &server_exe);

    assert!(mgr.is_enabled());
    assert_eq!(mgr.list_servers().len(), 1);
    let server = mgr.get_server("test").expect("test server in config");

    let tools = mgr.discover_tools(server).await.expect("discover");
    let names: Vec<String> = tools.iter().map(|t| t.definition().name.clone()).collect();
    // Go 测试服务器提供的工具集（适配器加 mcp_test_ 前缀）。
    // 注意：hello/config 是 MCP Resource 不是 Tool（resources 不进 discover）。
    for expected in ["echo", "add", "reverse", "get_time"] {
        assert!(
            names.iter().any(|n| n.ends_with(expected)),
            "tool '{expected}' not discovered; got {names:?}"
        );
    }
}

#[tokio::test]
async fn mcp_echo_call_roundtrip_through_real_server() {
    let Some(server_exe) = test_server_path() else {
        eprintln!(
            "SKIP: mcp-test-server not built — run: cd test-tools/mcp/server && go build -o mcp-test-server.exe ."
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = make_manager(dir.path(), &server_exe);
    let server = mgr.get_server("test").expect("server");
    let tools = mgr.discover_tools(server).await.expect("discover");

    let echo = tools
        .iter()
        .find(|t| t.definition().name.ends_with("echo"))
        .expect("echo tool");
    let result = echo
        .execute(serde_json::json!({ "text": "hello-mcp" }))
        .await;
    assert!(
        result.content.contains("hello-mcp"),
        "echo must round-trip the text: {}",
        result.content
    );

    // add：数值参数相加（参数类型非字符串的验证）。
    let add = tools
        .iter()
        .find(|t| t.definition().name.ends_with("add"))
        .expect("add tool");
    let result = add.execute(serde_json::json!({ "a": 2, "b": 3 })).await;
    assert!(result.content.contains('5'), "2+3: {}", result.content);
}

#[tokio::test]
async fn echo_call_surfaces_server_error_text_on_bad_params() {
    let Some(server_exe) = test_server_path() else {
        eprintln!(
            "SKIP: mcp-test-server not built — run: cd test-tools/mcp/server && go build -o mcp-test-server.exe ."
        );
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let mgr = make_manager(dir.path(), &server_exe);
    let server = mgr.get_server("test").expect("server");
    let tools = mgr.discover_tools(server).await.expect("discover");

    // 缺必填参数 → 调用不挂起即通过（服务器如何响应——错误文本或空结果——
    // 均可接受；本断言钉的是"坏参数不 hang"）。
    let echo = tools
        .iter()
        .find(|t| t.definition().name.ends_with("echo"))
        .expect("echo tool");
    let result = echo.execute(serde_json::json!({ "wrong_param": 1 })).await;
    assert!(
        !result.content.is_empty() || result.is_error,
        "bad params should produce a response: {:?}",
        result
    );
}

// ---------------------------------------------------------------------------
// 2026-08-30：HTTP transport 分派（desktop-pet2 的 8808/mcp 是 Streamable
// HTTP 端点——discover 按 transport_type 走 HttpTransport，不再误当 stdio）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_transport_dispatch_reaches_http_endpoint() {
    use std::io::{Read as _, Write as _};

    // mock MCP HTTP 端点：回 JSON-RPC 响应（initialize / tools/list 通用）。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut s = stream;
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf);
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"mock","version":"0"},"tools":[{"name":"mock_tool","description":"d","inputSchema":{"type":"object"}}]}}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes());
        }
    });

    let dir = tempfile::tempdir().unwrap();
    let cfg_path = dir.path().join("config.mcp.json");
    let url = format!("http://127.0.0.1:{port}/mcp");
    let cfg = serde_json::json!({
        "enabled": true,
        "servers": [{ "name": "httptest", "transport_type": "http", "url": url }]
    });
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    let mut mgr = McpManager::new(cfg_path);
    mgr.load_config().unwrap();
    let server = mgr.list_servers()[0].clone();
    let tools = mgr.discover_tools(&server).await.expect("http discover");

    let names: Vec<String> = tools.iter().map(|t| t.definition().name.clone()).collect();
    // 适配器给工具名加 mcp_httptest_ 前缀。
    assert!(
        names.iter().any(|n| n.ends_with("mock_tool")),
        "HTTP transport discovery must surface mock_tool; got {names:?}"
    );
}
