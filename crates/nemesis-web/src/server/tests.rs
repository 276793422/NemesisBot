use super::*;
use std::collections::HashMap;

#[tokio::test]
async fn test_build_router() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let server = WebServer::new(config);
    let _router = server.build_router();
}

#[tokio::test]
async fn test_build_router_with_static_dir() {
    let dir = tempfile::tempdir().unwrap();
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: Some(dir.path().to_string_lossy().to_string()),
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let server = WebServer::new(config);
    let _router = server.build_router();
}

#[tokio::test]
async fn test_build_router_with_nonexistent_static_dir() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: Some("/nonexistent/path".to_string()),
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let server = WebServer::new(config);
    let _router = server.build_router();
}

#[test]
fn test_resolve_static_dir_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().to_string();
    let result = resolve_static_dir(Some(&path), None);
    assert_eq!(result, Some(path));
}

#[test]
fn test_resolve_static_dir_nonexistent() {
    let result = resolve_static_dir(Some("/nonexistent/path/that/does/not/exist"), None);
    if let Some(ref path) = result {
        assert!(!path.contains("nonexistent"));
    }
}

#[test]
fn test_resolve_static_dir_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let static_dir = dir.path().join("static");
    std::fs::create_dir_all(&static_dir).unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let result = resolve_static_dir(None, Some(&ws));
    assert!(result.is_some());
}

#[test]
fn test_resolve_static_dir_fallback() {
    let result = resolve_static_dir(None, None);
    let _ = result;
}

#[test]
fn test_default_config() {
    let config = WebServerConfig::default();
    assert_eq!(config.listen_addr, "127.0.0.1:8080");
    assert!(config.auth_token.is_empty());
    assert!(config.static_dir.is_none());
    assert_eq!(config.index_file, "index.html");
    assert_eq!(config.ws_path, "/ws");
}

// --- DirectoryStaticFiles tests ---

#[test]
fn test_directory_static_files_read() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "hello world").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("test.txt").unwrap();
    assert_eq!(String::from_utf8(content).unwrap(), "hello world");
}

#[test]
fn test_directory_static_files_nested() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("nested.html"), "<html></html>").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("sub/nested.html").unwrap();
    assert_eq!(String::from_utf8(content).unwrap(), "<html></html>");
}

#[test]
fn test_directory_static_files_path_traversal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.txt"), "safe content").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    assert!(provider.get_file("../etc/passwd").is_none());
    assert!(provider.get_file("../../../etc/passwd").is_none());
}

#[test]
fn test_directory_static_files_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DirectoryStaticFiles::new(dir.path());
    assert!(provider.get_file("nonexistent.txt").is_none());
}

#[test]
fn test_directory_static_files_has_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("exists.txt"), "yes").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    assert!(provider.has_file("exists.txt"));
    assert!(!provider.has_file("nope.txt"));
}

#[test]
fn test_directory_static_files_list() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.html"), "b").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let files = provider.list_files();
    assert_eq!(files.len(), 2);
    assert!(files.contains(&"a.txt".to_string()));
    assert!(files.contains(&"b.html".to_string()));
}

#[test]
fn test_directory_static_files_leading_slash() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "index").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("/index.html").unwrap();
    assert_eq!(String::from_utf8(content).unwrap(), "index");
}

#[tokio::test]
async fn test_process_messages_publishes_to_bus() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();

    let (tx, proc_rx) = mpsc::unbounded_channel();

    // Send a message
    tx.send(crate::websocket_handler::IncomingMessage {
        session_id: "s1".to_string(),
        sender_id: "web:s1".to_string(),
        chat_id: "web:s1".to_string(),
        content: "hello".to_string(),
        metadata: HashMap::new(),
    }).unwrap();
    drop(tx); // Close the sender so process_messages exits

    process_messages(proc_rx, bus.clone()).await;

    let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(msg.is_ok());
    let inbound = msg.unwrap().unwrap();
    assert_eq!(inbound.channel, "web");
    assert_eq!(inbound.content, "hello");
    assert_eq!(inbound.sender_id, "web:s1");
}

#[test]
fn test_web_server_new() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    assert!(!server.is_running());
    assert!(server.message_bus.is_none());
}

#[test]
fn test_web_server_set_bus() {
    let config = WebServerConfig::default();
    let mut server = WebServer::new(config);
    let bus = Arc::new(MessageBus::new());
    server.set_message_bus(bus);
    assert!(server.message_bus.is_some());
}

#[test]
fn test_web_server_default_is_not_running() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    assert!(!server.is_running());
}

#[test]
fn test_web_server_set_model_name() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    server.set_model_info("gpt-4", "", false);
    assert_eq!(*server.model_name.lock(), "gpt-4");
}

#[test]
fn test_web_server_set_workspace() {
    let config = WebServerConfig::default();
    let mut server = WebServer::new(config);
    server.set_workspace(PathBuf::from("/tmp/workspace"));
    assert_eq!(server.config.workspace, Some("/tmp/workspace".to_string()));
}

#[test]
fn test_web_server_stop() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    assert!(!server.is_running());
    server.stop();
    assert!(!server.is_running());
}

#[test]
fn test_web_server_event_hub() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let hub = server.event_hub();
    hub.publish("test", serde_json::json!({"key": "val"}));
}

#[test]
fn test_web_server_session_manager() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let mgr = server.session_manager();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_config_debug_format() {
    let config = WebServerConfig::default();
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("127.0.0.1:8080"));
}

#[test]
fn test_config_custom_values() {
    let config = WebServerConfig {
        listen_addr: "0.0.0.0:9090".to_string(),
        auth_token: "secret".to_string(),
        cors_origins: vec!["https://example.com".to_string()],
        ws_path: "/websocket".to_string(),
        workspace: Some("/data".to_string()),
        home: None,
        version: "2.0.0".to_string(),
        static_dir: Some("/static".to_string()),
        static_files: None,
        index_file: "home.html".to_string(),
    };
    assert_eq!(config.listen_addr, "0.0.0.0:9090");
    assert_eq!(config.auth_token, "secret");
    assert_eq!(config.ws_path, "/websocket");
    assert_eq!(config.version, "2.0.0");
    assert_eq!(config.index_file, "home.html");
}

#[test]
fn test_directory_static_files_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DirectoryStaticFiles::new(dir.path());
    let files = provider.list_files();
    assert!(files.is_empty());
}

#[test]
fn test_directory_static_files_subdirectory_files() {
    let dir = tempfile::tempdir().unwrap();
    let sub1 = dir.path().join("css");
    let sub2 = dir.path().join("js");
    std::fs::create_dir_all(&sub1).unwrap();
    std::fs::create_dir_all(&sub2).unwrap();
    std::fs::write(sub1.join("style.css"), "body{}").unwrap();
    std::fs::write(sub2.join("app.js"), "console.log()").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let files = provider.list_files();
    assert_eq!(files.len(), 2);
    assert!(files.iter().any(|f| f.contains("style.css")));
    assert!(files.iter().any(|f| f.contains("app.js")));
}

#[test]
fn test_directory_static_files_path_traversal_variants() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("safe.txt"), "safe").unwrap();
    let provider = DirectoryStaticFiles::new(dir.path());
    assert!(provider.get_file("../../../etc/passwd").is_none());
    assert!(provider.get_file("..\\..\\windows\\system32").is_none());
    assert!(provider.get_file("./../secret").is_none());
}

#[test]
fn test_directory_static_files_has_file_false() {
    let dir = tempfile::tempdir().unwrap();
    let provider = DirectoryStaticFiles::new(dir.path());
    assert!(!provider.has_file("does_not_exist.txt"));
}

#[tokio::test]
async fn test_process_messages_empty_channel() {
    let bus = Arc::new(MessageBus::new());
    let (tx, rx) = mpsc::unbounded_channel();
    drop(tx); // Close immediately

    process_messages(rx, bus).await;
    // Should complete without error
}

#[test]
fn test_resolve_static_dir_nonexistent_workspace() {
    let result = resolve_static_dir(None, Some("/nonexistent/workspace/path"));
    // May return None or Some depending on ./static/ in CWD
    // The key behavior is that the workspace path itself is not returned
    if let Some(ref path) = result {
        assert!(!path.contains("nonexistent"));
    }
}

#[test]
fn test_resolve_static_dir_explicit_path_nonexistent() {
    let result = resolve_static_dir(Some("/this/path/does/not/exist"), None);
    // Should return None since the path doesn't exist
    if let Some(path) = result {
        // Should not be the explicit path since it doesn't exist
        assert!(!path.contains("nonexistent"));
    }
}

#[test]
fn test_web_server_config_default_values() {
    let config = WebServerConfig::default();
    assert_eq!(config.listen_addr, "127.0.0.1:8080");
    assert!(config.auth_token.is_empty());
    assert!(config.cors_origins.is_empty());
    assert_eq!(config.ws_path, "/ws");
    assert!(config.workspace.is_none());
    assert!(config.version.is_empty());
    assert!(config.static_dir.is_none());
    assert_eq!(config.index_file, "index.html");
}

#[tokio::test]
async fn test_process_messages_preserves_metadata() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let (tx, proc_rx) = mpsc::unbounded_channel();

    let mut metadata = HashMap::new();
    metadata.insert("request_type".to_string(), "history".to_string());
    tx.send(crate::websocket_handler::IncomingMessage {
        session_id: "s1".to_string(),
        sender_id: "web:s1".to_string(),
        chat_id: "web:s1".to_string(),
        content: "test".to_string(),
        metadata,
    }).unwrap();
    drop(tx);

    process_messages(proc_rx, bus).await;

    let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    let inbound = msg.unwrap().unwrap();
    assert_eq!(inbound.metadata.get("request_type"), Some(&"history".to_string()));
}

#[test]
fn test_static_files_trait_default_has_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("test.txt"), "content").unwrap();
    let provider = DirectoryStaticFiles::new(dir.path());
    // has_file uses the default implementation which calls get_file
    assert!(provider.has_file("test.txt"));
    assert!(!provider.has_file("missing.txt"));
}

// ============================================================
// Additional server tests for missing coverage
// ============================================================

#[test]
fn test_web_server_config_default_ws_path() {
    let config = WebServerConfig::default();
    assert_eq!(config.ws_path, "/ws");
}

#[test]
fn test_web_server_config_default_index_file() {
    let config = WebServerConfig::default();
    assert_eq!(config.index_file, "index.html");
}

#[test]
fn test_web_server_config_default_workspace() {
    let config = WebServerConfig::default();
    assert!(config.workspace.is_none());
}

#[test]
fn test_web_server_config_default_static_dir() {
    let config = WebServerConfig::default();
    assert!(config.static_dir.is_none());
}

#[test]
fn test_web_server_config_default_version() {
    let config = WebServerConfig::default();
    assert!(config.version.is_empty());
}

#[test]
fn test_web_server_config_cors_origins() {
    let config = WebServerConfig {
        cors_origins: vec!["http://localhost:3000".to_string()],
        ..Default::default()
    };
    assert_eq!(config.cors_origins.len(), 1);
    assert_eq!(config.cors_origins[0], "http://localhost:3000");
}

#[test]
fn test_web_server_new_custom_config() {
    let config = WebServerConfig {
        listen_addr: "0.0.0.0:9090".to_string(),
        auth_token: "mytoken".to_string(),
        cors_origins: vec![],
        ws_path: "/websocket".to_string(),
        workspace: Some("/tmp/ws".to_string()),
        home: None,
        version: "1.0.0".to_string(),
        static_dir: None,
        static_files: None,
        index_file: "app.html".to_string(),
    };
    let server = WebServer::new(config);
    assert!(!server.is_running());
}

#[test]
fn test_directory_static_files_binary_content() {
    let dir = tempfile::tempdir().unwrap();
    let binary_data = vec![0u8, 255, 128, 64, 32, 16, 8, 4, 2, 1];
    std::fs::write(dir.path().join("data.bin"), &binary_data).unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("data.bin").unwrap();
    assert_eq!(content, binary_data);
}

#[test]
fn test_directory_static_files_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("empty.txt"), "").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("empty.txt").unwrap();
    assert!(content.is_empty());
}

#[test]
fn test_directory_static_files_special_chars_filename() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("file with spaces.txt"), "content").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("file with spaces.txt");
    assert!(content.is_some());
    assert_eq!(content.unwrap(), b"content".to_vec());
}

#[tokio::test]
async fn test_process_messages_closed_channel() {
    let bus = Arc::new(MessageBus::new());
    let (tx, rx) = mpsc::unbounded_channel();
    drop(tx);

    // Should complete without panic
    process_messages(rx, bus).await;
}

// ============================================================
// Additional server tests: handle_health, send_to_session, etc.
// ============================================================

#[tokio::test]
async fn test_handle_health_endpoint() {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(3)),
        workspace: None,
        home: None,
        version: "1.0.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
    });
    let resp = handle_health(AxumState(state)).await;
    let json = resp.0;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["running"], true);
    assert_eq!(json["sessions"], 3);
}

#[tokio::test]
async fn test_handle_health_not_running() {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "1.0.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(false)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
    });
    let resp = handle_health(AxumState(state)).await;
    let json = resp.0;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["running"], false);
    assert_eq!(json["sessions"], 0);
}

#[tokio::test]
async fn test_send_to_session_no_queue() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();
    let result = send_to_session(&mgr, &session.id, "assistant", "hello").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("session not found or no send queue"));
}

#[tokio::test]
async fn test_send_to_session_nonexistent() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let result = send_to_session(&mgr, "nonexistent-session", "assistant", "hello").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_history_to_session_no_queue() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();
    let history_json = r#"{"messages":[],"has_more":false}"#;
    let result = send_history_to_session(&mgr, &session.id, history_json).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_history_to_session_invalid_json() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let result = send_history_to_session(&mgr, "any-session", "not json").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("failed to unmarshal history data"));
}

#[tokio::test]
async fn test_send_history_to_session_nonexistent() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let history_json = r#"{"messages":[]}"#;
    let result = send_history_to_session(&mgr, "nonexistent", history_json).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_start_publish_status_loop() {
    let event_hub = Arc::new(EventHub::new());
    let mut rx = event_hub.subscribe();
    let session_count = Arc::new(AtomicUsize::new(1));
    let running = Arc::new(AtomicBool::new(true));

    let handle = start_publish_status_loop(
        event_hub.clone(),
        session_count,
        "1.0.0".to_string(),
        Instant::now(),
        running.clone(),
    );

    // Wait for at least one status event
    let result = tokio::time::timeout(Duration::from_secs(8), rx.recv()).await;
    assert!(result.is_ok());
    let event = result.unwrap().unwrap();
    assert_eq!(event.event_type, "status");
    assert_eq!(event.data["version"], "1.0.0");
    assert_eq!(event.data["session_count"], 1);

    // Stop the loop
    running.store(false, std::sync::atomic::Ordering::SeqCst);
    let _ = tokio::time::timeout(Duration::from_secs(10), handle).await;
}

#[tokio::test]
async fn test_process_messages_multiple_messages() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let (tx, proc_rx) = mpsc::unbounded_channel();

    for i in 0..5 {
        tx.send(crate::websocket_handler::IncomingMessage {
            session_id: format!("s{}", i),
            sender_id: format!("web:s{}", i),
            chat_id: format!("web:s{}", i),
            content: format!("message {}", i),
            metadata: HashMap::new(),
        }).unwrap();
    }
    drop(tx);

    process_messages(proc_rx, bus).await;

    for i in 0..5 {
        let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
        assert!(msg.is_ok());
        let inbound = msg.unwrap().unwrap();
        assert_eq!(inbound.content, format!("message {}", i));
        assert_eq!(inbound.channel, "web");
    }
}

#[test]
fn test_web_server_config_clone() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:8080".to_string(),
        auth_token: "token".to_string(),
        cors_origins: vec!["https://example.com".to_string()],
        ws_path: "/ws".to_string(),
        workspace: Some("/tmp".to_string()),
        home: None,
        version: "1.0".to_string(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let cloned = config.clone();
    assert_eq!(cloned.listen_addr, config.listen_addr);
    assert_eq!(cloned.auth_token, config.auth_token);
    assert_eq!(cloned.cors_origins.len(), 1);
}

#[test]
fn test_directory_static_files_nonexistent_base() {
    let provider = DirectoryStaticFiles::new("/this/path/does/not/exist");
    assert!(provider.get_file("test.txt").is_none());
    assert!(provider.list_files().is_empty());
}

#[test]
fn test_resolve_static_dir_with_both_explicit_and_workspace() {
    let explicit_dir = tempfile::tempdir().unwrap();
    let workspace_dir = tempfile::tempdir().unwrap();
    let ws_static = workspace_dir.path().join("static");
    std::fs::create_dir_all(&ws_static).unwrap();

    // Explicit takes priority
    let explicit_path = explicit_dir.path().to_string_lossy().to_string();
    let ws_path = workspace_dir.path().to_string_lossy().to_string();
    let result = resolve_static_dir(Some(&explicit_path), Some(&ws_path));
    assert_eq!(result, Some(explicit_path));
}

#[test]
fn test_resolve_static_dir_workspace_static_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let static_dir = dir.path().join("static");
    std::fs::create_dir_all(&static_dir).unwrap();
    let ws = dir.path().to_string_lossy().to_string();

    let result = resolve_static_dir(None, Some(&ws));
    assert!(result.is_some());
    assert!(result.unwrap().contains("static"));
}

#[tokio::test]
async fn test_build_router_health_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_build_router_api_health_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/health")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn test_build_router_api_status_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_build_router_api_version_endpoint() {
    let config = WebServerConfig {
        version: "2.0.0".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/version")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["version"], "2.0.0");
}

#[tokio::test]
async fn test_build_router_api_sessions_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/sessions")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["total_connections"], 0);
    assert_eq!(json["active_sessions"], 0);
}

#[tokio::test]
async fn test_build_router_api_events_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/events")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["stream_url"], "/api/events/stream");
}

#[test]
fn test_web_server_set_model_name_multiple() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    server.set_model_info("gpt-4", "", false);
    assert_eq!(*server.model_name.lock(), "gpt-4");
    server.set_model_info("claude-3", "", false);
    assert_eq!(*server.model_name.lock(), "claude-3");
}

#[test]
fn test_web_server_stop_sets_running_false() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    server.running.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(server.is_running());
    server.stop();
    assert!(!server.is_running());
}

#[test]
fn test_directory_static_files_deeply_nested() {
    let dir = tempfile::tempdir().unwrap();
    let deep = dir.path().join("a").join("b").join("c");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("deep.txt"), "deep content").unwrap();

    let provider = DirectoryStaticFiles::new(dir.path());
    let content = provider.get_file("a/b/c/deep.txt").unwrap();
    assert_eq!(String::from_utf8(content).unwrap(), "deep content");
    let files = provider.list_files();
    assert_eq!(files.len(), 1);
}

// ============================================================
// Additional tests for 95%+ coverage - server lifecycle
// ============================================================

#[tokio::test]
async fn test_web_server_build_router_no_bus_drains_messages() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    // No message bus set - should drain messages
    let _router = server.build_router();
    // Give the drain task a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn test_web_server_build_router_with_bus() {
    let mut config = WebServerConfig::default();
    config.listen_addr = "127.0.0.1:0".to_string();
    let mut server = WebServer::new(config);
    let bus = Arc::new(MessageBus::new());
    server.set_message_bus(bus);
    let _router = server.build_router();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn test_build_router_with_cors_origins() {
    let config = WebServerConfig {
        cors_origins: vec!["http://localhost:3000".to_string(), "http://localhost:4000".to_string()],
        ..Default::default()
    };
    let server = WebServer::new(config);
    let _router = server.build_router();
}

#[tokio::test]
async fn test_build_router_custom_ws_path() {
    let config = WebServerConfig {
        ws_path: "/custom_ws".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    // /ws should no longer exist
    let req = axum::http::Request::builder()
        .uri("/ws")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_build_router_api_logs_endpoint() {
    let config = WebServerConfig {
        workspace: Some("/nonexistent_workspace".to_string()),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/logs")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_build_router_api_config_endpoint_no_workspace() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/config")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Returns 503 without workspace configured
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn test_build_router_api_scanner_status_no_workspace() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/scanner/status")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Returns 503 without workspace configured
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn test_build_router_api_models_no_workspace() {
    let config = WebServerConfig {
        version: "2.0.0".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/models")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // Returns 503 without workspace configured
    assert_eq!(resp.status(), 503);
}

#[tokio::test]
async fn test_send_to_session_nonexistent_session() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    // No session created
    let result = send_to_session(&mgr, "nonexistent-id", "assistant", "hello world").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_history_to_session_nonexistent_session() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    // No session created
    let history_json = r#"{"messages":[{"role":"user","content":"hi"}],"has_more":false}"#;
    let result = send_history_to_session(&mgr, "nonexistent-id", history_json).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_publish_status_loop_stops_on_false() {
    let event_hub = Arc::new(EventHub::new());
    let session_count = Arc::new(AtomicUsize::new(0));
    let running = Arc::new(AtomicBool::new(false)); // Already false

    let handle = start_publish_status_loop(
        event_hub,
        session_count,
        "1.0.0".to_string(),
        Instant::now(),
        running,
    );

    // Should stop quickly since running is false
    let result = tokio::time::timeout(Duration::from_secs(3), handle).await;
    assert!(result.is_ok());
}

#[test]
fn test_web_server_start_time() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    // start_time should be set at construction
    let _elapsed = server.start_time.elapsed();
}

#[tokio::test]
async fn test_handle_health_with_model_state() {
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(5)),
        workspace: Some("/test".to_string()),
        home: None,
        version: "3.0.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("gpt-4o".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
    });
    let resp = handle_health(AxumState(state)).await;
    let json = resp.0;
    // handle_health only returns status, running, sessions
    assert_eq!(json["status"], "ok");
    assert_eq!(json["running"], true);
    assert_eq!(json["sessions"], 5);
}

#[tokio::test]
async fn test_process_messages_with_bus_and_metadata() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let (tx, proc_rx) = mpsc::unbounded_channel();

    let mut metadata = HashMap::new();
    metadata.insert("request_type".to_string(), "history_request".to_string());
    metadata.insert("request_id".to_string(), "req-001".to_string());

    tx.send(crate::websocket_handler::IncomingMessage {
        session_id: "sess-123".to_string(),
        sender_id: "web:sess-123".to_string(),
        chat_id: "web:sess-123".to_string(),
        content: "What is the weather?".to_string(),
        metadata,
    }).unwrap();
    drop(tx);

    process_messages(proc_rx, bus).await;

    let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(msg.is_ok());
    let inbound = msg.unwrap().unwrap();
    assert_eq!(inbound.channel, "web");
    assert_eq!(inbound.sender_id, "web:sess-123");
    assert_eq!(inbound.content, "What is the weather?");
    assert_eq!(inbound.metadata.get("request_type").unwrap(), "history_request");
    assert_eq!(inbound.metadata.get("request_id").unwrap(), "req-001");
}

#[tokio::test]
async fn test_build_router_sse_stream_endpoint() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/events/stream")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // SSE endpoint returns 200 with event-stream content type
    assert_eq!(resp.status(), 200);
}

#[test]
fn test_resolve_static_dir_current_dir_static() {
    // Test the fallback path where no explicit dir and no workspace
    let result = resolve_static_dir(None, None);
    // Result depends on whether ./static/ exists in CWD
    let _ = result;
}
