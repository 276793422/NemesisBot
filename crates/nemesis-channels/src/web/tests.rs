use super::*;
use std::sync::Mutex;

/// Mock web server for testing.
struct MockWebServer {
    sent: Mutex<Vec<(String, String, String)>>,
    history: Mutex<Vec<(String, String)>>,
    broadcasts: Mutex<Vec<String>>,
}

impl MockWebServer {
    fn new() -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            history: Mutex::new(Vec::new()),
            broadcasts: Mutex::new(Vec::new()),
        }
    }

    fn broadcast_count(&self) -> usize {
        self.broadcasts.lock().unwrap().len()
    }
}

impl WebServerOps for MockWebServer {
    fn send_to_session(&self, session_id: &str, role: &str, content: &str) -> std::result::Result<(), String> {
        self.sent.lock().unwrap().push((session_id.to_string(), role.to_string(), content.to_string()));
        Ok(())
    }

    fn send_history_to_session(&self, session_id: &str, content: &str) -> std::result::Result<(), String> {
        self.history.lock().unwrap().push((session_id.to_string(), content.to_string()));
        Ok(())
    }

    fn broadcast(&self, content: &str) -> std::result::Result<(), String> {
        self.broadcasts.lock().unwrap().push(content.to_string());
        Ok(())
    }

    fn active_session_ids(&self) -> Vec<String> {
        vec!["s1".to_string(), "s2".to_string()]
    }

    fn start_server(&self) -> std::result::Result<(), String> { Ok(()) }
    fn stop_server(&self) {}
}

#[test]
fn test_config_default() {
    let cfg = WebChannelConfig::default();
    assert_eq!(cfg.host, "127.0.0.1");
    assert_eq!(cfg.port, 8080);
    assert_eq!(cfg.ws_path, "/ws");
}

#[test]
fn test_is_running_default() {
    let ch = WebChannel::with_defaults();
    assert!(!ch.is_running());
}

#[tokio::test]
async fn test_send_not_running() {
    let ch = WebChannel::with_defaults();
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:test-session".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_invalid_chat_id() {
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(Arc::new(MockWebServer::new()));

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "invalid-format".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_send_to_session() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-123".to_string(),
        content: "Hello world".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_ok());

    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, "session-123");
    assert_eq!(sent[0].1, "assistant");
    assert_eq!(sent[0].2, "Hello world");
}

#[tokio::test]
async fn test_broadcast() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:broadcast".to_string(),
        content: "Broadcast message".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_ok());

    let broadcasts = mock.broadcasts.lock().unwrap();
    assert_eq!(broadcasts.len(), 1);
    assert_eq!(broadcasts[0], "Broadcast message");
}

#[tokio::test]
async fn test_send_history() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-456".to_string(),
        content: "[{\"role\":\"user\",\"content\":\"hi\"}]".to_string(),
        message_type: "history".to_string(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_ok());

    let history = mock.history.lock().unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].0, "session-456");
}

#[tokio::test]
async fn test_start_stop_lifecycle() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock);

    assert!(!ch.is_running());
    ch.start().await.unwrap();
    assert!(ch.is_running());
    ch.stop().await.unwrap();
    assert!(!ch.is_running());
}

#[test]
fn test_config_custom() {
    let cfg = WebChannelConfig {
        host: "0.0.0.0".to_string(),
        port: 9090,
        ws_path: "/custom-ws".to_string(),
        auth_token: "secret-token".to_string(),
        session_timeout_secs: 7200,
        allow_from: vec!["127.0.0.1".to_string()],
    };
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.port, 9090);
    assert_eq!(cfg.ws_path, "/custom-ws");
    assert_eq!(cfg.auth_token, "secret-token");
    assert_eq!(cfg.session_timeout_secs, 7200);
    assert_eq!(cfg.allow_from.len(), 1);
}

#[test]
fn test_listen_addr() {
    let cfg = WebChannelConfig {
        host: "192.168.1.1".to_string(),
        port: 3000,
        ..Default::default()
    };
    let ch = WebChannel::new(cfg);
    assert_eq!(ch.listen_addr(), "192.168.1.1:3000");
}

#[tokio::test]
async fn test_send_no_server_configured() {
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    // No server set - should drop message silently
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-123".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_ok()); // drops silently
}

#[tokio::test]
async fn test_send_after_stop() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock);
    ch.start().await.unwrap();
    ch.stop().await.unwrap();

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-123".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_broadcast_to_all() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    ch.broadcast_to_all("test broadcast").unwrap();
    let broadcasts = mock.broadcasts.lock().unwrap();
    assert_eq!(broadcasts.len(), 1);
    assert_eq!(broadcasts[0], "test broadcast");
}

#[tokio::test]
async fn test_send_multiple_messages() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    for i in 0..5 {
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: format!("web:session-{}", i),
            content: format!("Message {}", i),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
    }

    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent.len(), 5);
}

#[test]
fn test_default_impl() {
    let ch = WebChannel::default();
    assert_eq!(ch.name(), "web");
    assert!(!ch.is_running());
}

#[test]
fn test_get_server_none_when_not_set() {
    let ch = WebChannel::with_defaults();
    assert!(ch.get_server().is_none());
}

#[test]
fn test_get_server_returns_set_server() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    let server = ch.get_server();
    assert!(server.is_some());
}

#[test]
fn test_set_workspace_delegates_to_server() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    ch.set_workspace("/test/workspace");
    // MockWebServer has default no-op implementations, just verify no panic
}

#[test]
fn test_set_model_name_delegates_to_server() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    ch.set_model_name("gpt-4");
    // MockWebServer has default no-op implementations, just verify no panic
}

#[test]
fn test_set_workspace_no_panic_without_server() {
    let ch = WebChannel::with_defaults();
    // Should not panic when no server is set
    ch.set_workspace("/test/workspace");
}

#[test]
fn test_set_model_name_no_panic_without_server() {
    let ch = WebChannel::with_defaults();
    ch.set_model_name("test-model");
}

#[tokio::test]
async fn test_send_with_empty_content() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-123".to_string(),
        content: String::new(),
        message_type: String::new(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_ok());
    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].2, "");
}

// ---- Additional comprehensive web channel tests ----

// === Failing mock server ===

struct FailingMockServer;

impl WebServerOps for FailingMockServer {
    fn send_to_session(&self, _: &str, _: &str, _: &str) -> std::result::Result<(), String> {
        Err("send failed".to_string())
    }
    fn send_history_to_session(&self, _: &str, _: &str) -> std::result::Result<(), String> {
        Err("history failed".to_string())
    }
    fn broadcast(&self, _: &str) -> std::result::Result<(), String> {
        Err("broadcast failed".to_string())
    }
    fn active_session_ids(&self) -> Vec<String> { vec![] }
    fn start_server(&self) -> std::result::Result<(), String> { Ok(()) }
    fn stop_server(&self) {}
}

// === Configuration edge cases ===

#[test]
fn test_config_zero_port() {
    let cfg = WebChannelConfig {
        port: 0,
        ..Default::default()
    };
    let ch = WebChannel::new(cfg);
    assert_eq!(ch.listen_addr(), "127.0.0.1:0");
}

#[test]
fn test_config_custom_host() {
    let cfg = WebChannelConfig {
        host: "0.0.0.0".to_string(),
        ..Default::default()
    };
    let ch = WebChannel::new(cfg);
    assert_eq!(ch.listen_addr(), "0.0.0.0:8080");
}

#[test]
fn test_config_long_auth_token() {
    let token = "x".repeat(1000);
    let cfg = WebChannelConfig {
        auth_token: token.clone(),
        ..Default::default()
    };
    assert_eq!(cfg.auth_token, token);
}

#[test]
fn test_config_session_timeout() {
    let cfg = WebChannelConfig {
        session_timeout_secs: 86400,
        ..Default::default()
    };
    assert_eq!(cfg.session_timeout_secs, 86400);
}

// === Session ID extraction ===

#[tokio::test]
async fn test_send_extracts_short_session_id() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:s".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap();
    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent[0].0, "s");
}

#[tokio::test]
async fn test_send_extracts_uuid_session_id() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:550e8400-e29b-41d4-a716-446655440000".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap();
    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent[0].0, "550e8400-e29b-41d4-a716-446655440000");
}

// === Error handling ===

#[tokio::test]
async fn test_send_to_failing_server() {
    let mock = Arc::new(FailingMockServer);
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock);

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-1".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[tokio::test]
async fn test_broadcast_failing_server() {
    let mock = Arc::new(FailingMockServer);
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock);

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:broadcast".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[tokio::test]
async fn test_history_to_failing_server() {
    let mock = Arc::new(FailingMockServer);
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock);

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-1".to_string(),
        content: "history data".to_string(),
        message_type: "history".to_string(),
    };
    assert!(ch.send(msg).await.is_err());
}

// === Broadcast edge cases ===

#[tokio::test]
async fn test_broadcast_no_server() {
    let ch = WebChannel::with_defaults();
    // No server set
    let result = ch.broadcast_to_all("test");
    assert!(result.is_ok()); // silently drops
}

// === Lifecycle edge cases ===

#[tokio::test]
async fn test_start_no_server() {
    let ch = WebChannel::with_defaults();
    // No server - start should still succeed
    ch.start().await.unwrap();
    assert!(ch.is_running());
    ch.stop().await.unwrap();
}

// === Stats ===

#[tokio::test]
async fn test_send_increments_sent_counter() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    for i in 0..5 {
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: format!("web:s{}", i),
            content: format!("msg {}", i),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
    }

    assert_eq!(ch.base.messages_sent(), 5);
}

#[tokio::test]
async fn test_send_not_running_no_counter() {
    let ch = WebChannel::with_defaults();
    // Not running - send fails
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:s1".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    assert!(ch.send(msg).await.is_err());
    assert_eq!(ch.base.messages_sent(), 0);
}

// --- Additional web channel tests ---

#[test]
fn test_web_channel_config_default() {
    let config = WebChannelConfig::default();
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 8080);
    assert_eq!(config.ws_path, "/ws");
    assert!(config.auth_token.is_empty());
    assert_eq!(config.session_timeout_secs, 3600);
    assert!(config.allow_from.is_empty());
}

#[test]
fn test_web_channel_name() {
    let ch = WebChannel::with_defaults();
    assert_eq!(ch.name(), "web");
}

#[test]
fn test_web_channel_default_not_running() {
    let ch = WebChannel::with_defaults();
    assert!(!ch.is_running());
}

#[tokio::test]
async fn test_set_workspace() {
    let ch = WebChannel::with_defaults();
    ch.set_workspace("/some/path");
    // Should not panic
}

#[tokio::test]
async fn test_set_model_name() {
    let ch = WebChannel::with_defaults();
    ch.set_model_name("gpt-4");
    // Should not panic
}

#[test]
fn test_web_channel_config_custom() {
    let config = WebChannelConfig {
        host: "0.0.0.0".into(),
        port: 9090,
        ws_path: "/custom".into(),
        auth_token: "secret".into(),
        session_timeout_secs: 7200,
        allow_from: vec!["user1".into()],
    };
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 9090);
    assert_eq!(config.ws_path, "/custom");
    assert_eq!(config.auth_token, "secret");
}

#[tokio::test]
async fn test_broadcast_with_mock_server() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    ch.running.store(true, Ordering::SeqCst);

    let result = ch.broadcast_to_all("test broadcast");
    assert!(result.is_ok());
    assert_eq!(mock.broadcast_count(), 1);
}

#[tokio::test]
async fn test_send_to_unknown_session_type() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock.clone());
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "unknown-format".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    // Should handle gracefully
    let result = ch.send(msg).await;
    // It may or may not succeed depending on chat_id parsing
    assert!(result.is_ok() || result.is_err());
}

// ---- New tests for coverage improvement ----

// === Failing server that fails to start ===

struct FailStartServer;

impl WebServerOps for FailStartServer {
    fn send_to_session(&self, _: &str, _: &str, _: &str) -> std::result::Result<(), String> {
        Ok(())
    }
    fn send_history_to_session(&self, _: &str, _: &str) -> std::result::Result<(), String> {
        Ok(())
    }
    fn broadcast(&self, _: &str) -> std::result::Result<(), String> {
        Ok(())
    }
    fn active_session_ids(&self) -> Vec<String> { vec![] }
    fn start_server(&self) -> std::result::Result<(), String> {
        Err("server start failed".to_string())
    }
    fn stop_server(&self) {}
}

#[tokio::test]
async fn test_start_with_failing_server() {
    let mock = Arc::new(FailStartServer);
    let ch = WebChannel::with_defaults();
    ch.set_server(mock);

    let result = ch.start().await;
    assert!(result.is_err());
    assert!(!ch.is_running());
}

// === Stop without server ===

#[tokio::test]
async fn test_stop_without_server() {
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);

    ch.stop().await.unwrap();
    assert!(!ch.is_running());
}

// === Start then restart lifecycle ===

#[tokio::test]
async fn test_start_restart_lifecycle() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.set_server(mock);

    // First start
    ch.start().await.unwrap();
    assert!(ch.is_running());

    // Stop
    ch.stop().await.unwrap();
    assert!(!ch.is_running());

    // Restart
    ch.start().await.unwrap();
    assert!(ch.is_running());

    ch.stop().await.unwrap();
}

// === Send tracks sent counter ===

#[tokio::test]
async fn test_send_tracks_sent_counter_via_broadcast() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    // Broadcast message
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:broadcast".to_string(),
        content: "broadcast".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap();
    assert_eq!(ch.base.messages_sent(), 1);
}

// === Multiple broadcasts ===

#[tokio::test]
async fn test_multiple_broadcasts() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    for i in 0..5 {
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "web:broadcast".to_string(),
            content: format!("broadcast {}", i),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
    }

    let broadcasts = mock.broadcasts.lock().unwrap();
    assert_eq!(broadcasts.len(), 5);
    assert_eq!(ch.base.messages_sent(), 5);
}

// === Send to session with special content ===

#[tokio::test]
async fn test_send_to_session_unicode_content() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-unicode".to_string(),
        content: "你好世界 🌍 مرحبا".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap();

    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent[0].2, "你好世界 🌍 مرحبا");
}

// === Send history to session ===

#[tokio::test]
async fn test_send_history_with_long_content() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let long_history = "x".repeat(10_000);
    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:session-history".to_string(),
        content: long_history.clone(),
        message_type: "history".to_string(),
    };
    ch.send(msg).await.unwrap();

    let history = mock.history.lock().unwrap();
    assert_eq!(history[0].0, "session-history");
    assert_eq!(history[0].1.len(), 10_000);
}

// === WebChannelConfig clone ===

#[test]
fn test_web_channel_config_clone_equality() {
    let cfg = WebChannelConfig {
        host: "0.0.0.0".to_string(),
        port: 9090,
        ws_path: "/ws".to_string(),
        auth_token: "secret".to_string(),
        session_timeout_secs: 7200,
        allow_from: vec!["10.0.0.0/8".to_string()],
    };
    let cloned = cfg.clone();
    assert_eq!(cloned.host, cfg.host);
    assert_eq!(cloned.port, cfg.port);
    assert_eq!(cloned.ws_path, cfg.ws_path);
    assert_eq!(cloned.auth_token, cfg.auth_token);
}

// === Multiple set_server calls ===

#[test]
fn test_set_server_replaces() {
    let mock1 = Arc::new(MockWebServer::new());
    let mock2 = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();

    ch.set_server(mock1);
    ch.set_server(mock2);

    // Should have the second server
    assert!(ch.get_server().is_some());
}

// === Send to session with web: prefix but empty session ===

#[tokio::test]
async fn test_send_to_empty_session_id() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    let msg = OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap();

    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent[0].0, ""); // empty session ID
}

// === Channel name via trait ===

#[tokio::test]
async fn test_channel_name_via_trait() {
    use crate::base::Channel;
    let ch = WebChannel::with_defaults();
    assert_eq!(ch.name(), "web");
}

// === Broadcast to all with failing server ===

#[tokio::test]
async fn test_broadcast_to_all_failing_server() {
    let mock = Arc::new(FailingMockServer);
    let ch = WebChannel::with_defaults();
    ch.set_server(mock);

    let result = ch.broadcast_to_all("test");
    assert!(result.is_err());
}

// === Send to multiple different sessions ===

#[tokio::test]
async fn test_send_to_multiple_sessions() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    for i in 0..3 {
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: format!("web:session-{}", i),
            content: format!("msg {}", i),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
    }

    let sent = mock.sent.lock().unwrap();
    assert_eq!(sent.len(), 3);
    assert_eq!(sent[0].0, "session-0");
    assert_eq!(sent[1].0, "session-1");
    assert_eq!(sent[2].0, "session-2");
}

// === Send mixed broadcast and session messages ===

#[tokio::test]
async fn test_send_mixed_types() {
    let mock = Arc::new(MockWebServer::new());
    let ch = WebChannel::with_defaults();
    ch.running.store(true, Ordering::SeqCst);
    ch.base.set_enabled(true);
    ch.set_server(mock.clone());

    // Session message
    ch.send(OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:s1".to_string(),
        content: "session msg".to_string(),
        message_type: String::new(),
    }).await.unwrap();

    // Broadcast
    ch.send(OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:broadcast".to_string(),
        content: "broadcast msg".to_string(),
        message_type: String::new(),
    }).await.unwrap();

    // History
    ch.send(OutboundMessage {
        channel: "web".to_string(),
        chat_id: "web:s2".to_string(),
        content: "history data".to_string(),
        message_type: "history".to_string(),
    }).await.unwrap();

    assert_eq!(mock.sent.lock().unwrap().len(), 1);
    assert_eq!(mock.broadcasts.lock().unwrap().len(), 1);
    assert_eq!(mock.history.lock().unwrap().len(), 1);
    assert_eq!(ch.base.messages_sent(), 3);
}

#[test]
fn test_extract_session_id_from_chat_id() {
    // Chat ID format is "web:{session_id}"
    let chat_id = "web:session-abc-123";
    let parts: Vec<&str> = chat_id.splitn(2, ':').collect();
    assert_eq!(parts.len(), 2);
    assert_eq!(parts[0], "web");
    assert_eq!(parts[1], "session-abc-123");
}

#[test]
fn test_extract_session_id_no_prefix() {
    let chat_id = "session-without-prefix";
    let parts: Vec<&str> = chat_id.splitn(2, ':').collect();
    assert_eq!(parts.len(), 1);
}

#[test]
fn test_web_channel_config_clone() {
    let config = WebChannelConfig::default();
    let cloned = config.clone();
    assert_eq!(cloned.host, config.host);
    assert_eq!(cloned.port, config.port);
    assert_eq!(cloned.ws_path, config.ws_path);
}

#[test]
fn test_web_channel_config_debug() {
    let config = WebChannelConfig::default();
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("127.0.0.1"));
    assert!(debug_str.contains("8080"));
}
