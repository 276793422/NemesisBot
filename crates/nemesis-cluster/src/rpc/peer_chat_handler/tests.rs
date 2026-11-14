use super::*;

fn make_request() -> PeerChatRequest {
    PeerChatRequest {
        request_type: "chat".into(),
        content: "What is Rust?".into(),
        context: serde_json::json!({
            "chat_id": "chat-123",
            "sender_id": "node-a",
        }),
    }
}

#[test]
fn test_default_timeout() {
    let handler = PeerChatHandler::new("node-b".into());
    assert_eq!(handler.timeout_secs(), 7200);
}

// 集群完备性加固：llm_timeout 单一真相源（0=不限，文档口径）。
#[test]
fn llm_timeout_from_config_zero_is_unlimited() {
    assert_eq!(
        llm_timeout_from_config_secs(0),
        Duration::MAX,
        "0 = 不限（tokio timeout 永不触发）"
    );
}

#[test]
fn llm_timeout_from_config_passthrough() {
    assert_eq!(llm_timeout_from_config_secs(7200).as_secs(), 7200);
    assert_eq!(llm_timeout_from_config_secs(1).as_secs(), 1);
}

#[test]
fn set_timeout_accepts_unlimited_sentinel() {
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_timeout(llm_timeout_from_config_secs(0));
    assert_eq!(handler.timeout_secs(), u64::MAX);
}

#[test]
fn test_validate_valid_request() {
    let handler = PeerChatHandler::new("node-b".into());
    assert!(handler.validate(&make_request()).is_ok());
}

#[test]
fn test_validate_empty_content() {
    let handler = PeerChatHandler::new("node-b".into());
    let mut req = make_request();
    req.content = String::new();
    assert!(handler.validate(&req).is_err());
}

#[tokio::test]
async fn test_handle_returns_ack() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "type": "chat",
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");
    assert!(!ack.task_id.is_empty());
}

#[test]
fn test_handle_missing_content() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "type": "chat",
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "error");
}

#[tokio::test]
async fn test_handle_extracts_task_id() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "custom-task-123",
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.task_id, "custom-task-123");
}

#[test]
fn test_request_type_default() {
    let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
        "content": "test"
    }))
    .unwrap();
    assert_eq!(req.request_type, "request");
}

// -- Mock LLM channel for integration-style tests --

struct MockLlmChannel {
    response: String,
    should_fail: bool,
}

impl LlmChannel for MockLlmChannel {
    fn submit(
        &self,
        _session_key: &str,
        _content: &str,
        _correlation_id: &str,
    ) -> Result<oneshot::Receiver<String>, String> {
        if self.should_fail {
            return Err("channel not available".into());
        }
        let (tx, rx) = oneshot::channel();
        let response = self.response.clone();
        tokio::spawn(async move {
            let _ = tx.send(response);
        });
        Ok(rx)
    }
}

#[tokio::test]
async fn test_async_processing_success() {
    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }

    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            response: &str,
            error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            if let Some(tx) = self.tx.lock().unwrap().take() {
                let _ = tx.send(PeerChatResult {
                    task_id: task_id.into(),
                    status: status.into(),
                    response: response.into(),
                    error: if error.is_empty() {
                        None
                    } else {
                        Some(error.into())
                    },
                });
            }
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let llm = Arc::new(MockLlmChannel {
        response: "Rust is a systems programming language.".into(),
        should_fail: false,
    });

    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });

    let source_info = Some(serde_json::json!({"node_id": "node-a"}));
    let req = make_request();

    // Run the async processing directly
    tokio::spawn(async move {
        process_async(
            "test-task",
            &req,
            "node-a",
            "node-a",
            &source_info,
            Some(llm.as_ref()),
            None, // no rpc_client -> will fall back to persist
            Some(persister.as_ref()),
            Duration::from_secs(10),
            "node-b",
        )
        .await;
    });

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let result = result.unwrap().unwrap();
    assert_eq!(result.status, "success");
    assert_eq!(result.response, "Rust is a systems programming language.");
    let _ = tx; // suppress unused warning
}

#[tokio::test]
async fn test_async_processing_no_llm_channel() {
    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }

    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            _response: &str,
            error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            if let Some(tx) = self.tx.lock().unwrap().take() {
                let _ = tx.send(PeerChatResult {
                    task_id: task_id.into(),
                    status: status.into(),
                    response: String::new(),
                    error: if error.is_empty() {
                        None
                    } else {
                        Some(error.into())
                    },
                });
            }
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });
    let source_info = Some(serde_json::json!({"node_id": "node-a"}));
    let req = make_request();

    tokio::spawn(async move {
        process_async(
            "test-task-2",
            &req,
            "node-a",
            "node-a",
            &source_info,
            None, // no LLM channel
            None,
            Some(persister.as_ref()),
            Duration::from_secs(10),
            "node-b",
        )
        .await;
    });

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let result = result.unwrap().unwrap();
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().contains("rpc channel not available"));
    let _ = tx;
}

// -- Additional coverage tests --

#[test]
fn test_peer_chat_handler_with_timeout() {
    let handler = PeerChatHandler::with_timeout("node-c".into(), Duration::from_secs(120));
    assert_eq!(handler.timeout_secs(), 120);
    assert_eq!(handler.node_id(), "node-c");
}

#[test]
fn test_peer_chat_handler_node_id() {
    let handler = PeerChatHandler::new("my-node".into());
    assert_eq!(handler.node_id(), "my-node");
}

#[test]
fn test_peer_chat_request_deserialization() {
    let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
        "type": "chat",
        "content": "Hello",
        "context": {"chat_id": "c1", "sender_id": "s1"}
    }))
    .unwrap();
    assert_eq!(req.request_type, "chat");
    assert_eq!(req.content, "Hello");
    assert_eq!(req.context["chat_id"], "c1");
}

#[test]
fn test_peer_chat_ack_fields() {
    let ack = PeerChatAck {
        status: "accepted".into(),
        task_id: "task-123".into(),
        error: None,
    };
    assert_eq!(ack.status, "accepted");
    assert_eq!(ack.task_id, "task-123");
}

#[test]
fn test_peer_chat_result_fields() {
    let result = PeerChatResult {
        task_id: "t-1".into(),
        status: "success".into(),
        response: "hello".into(),
        error: None,
    };
    assert_eq!(result.task_id, "t-1");
    assert!(result.error.is_none());
}

#[test]
fn test_rpc_meta_fields() {
    let meta = RpcMeta {
        from: Some("node-a".into()),
    };
    assert_eq!(meta.from.as_deref(), Some("node-a"));

    let meta_none = RpcMeta { from: None };
    assert!(meta_none.from.is_none());
}

#[tokio::test]
async fn test_handle_invalid_payload() {
    let handler = PeerChatHandler::new("node-b".into());
    // Pass a non-object value that can't be deserialized to PeerChatRequest
    let payload = serde_json::json!(42);
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "error");
    assert!(ack.task_id.is_empty());
}

#[tokio::test]
async fn test_handle_with_rpc_meta() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello from meta",
    });
    let meta = RpcMeta {
        from: Some("source-node".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");
}

#[tokio::test]
async fn test_persist_result_no_persister() {
    let handler = PeerChatHandler::new("node-b".into());
    // No persister set -> should not panic
    handler.persist_result("task-1", "success", "response", "", "node-a");
}

#[tokio::test]
async fn test_persist_result_empty_source() {
    let handler = PeerChatHandler::new("node-b".into());
    // Empty source_node_id -> should not persist
    handler.persist_result("task-1", "success", "response", "", "");
}

#[tokio::test]
async fn test_delete_result_no_persister() {
    let handler = PeerChatHandler::new("node-b".into());
    // No persister set -> should not panic
    handler.delete_result("task-1");
}

#[tokio::test]
async fn test_wait_for_tasks_empty() {
    let handler = PeerChatHandler::new("node-b".into());
    // No active tasks -> should return immediately
    handler.wait_for_tasks().await;
}

#[tokio::test]
async fn test_handle_auto_task_id_generation() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        // no task_id -> should auto-generate
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");
    assert!(!ack.task_id.is_empty());
}

#[test]
fn test_peer_chat_request_serialization_roundtrip() {
    let req = PeerChatRequest {
        request_type: "task".into(),
        content: "Do something".into(),
        context: serde_json::json!({"key": "value"}),
    };
    let json = serde_json::to_string(&req).unwrap();
    let parsed: PeerChatRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.request_type, "task");
    assert_eq!(parsed.content, "Do something");
    assert_eq!(parsed.context["key"], "value");
}

#[test]
fn test_peer_chat_ack_serialization_roundtrip() {
    let ack = PeerChatAck {
        status: "accepted".into(),
        task_id: "t-123".into(),
        error: None,
    };
    let json = serde_json::to_string(&ack).unwrap();
    let parsed: PeerChatAck = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.status, "accepted");
    assert_eq!(parsed.task_id, "t-123");
}

#[tokio::test]
async fn test_async_processing_llm_submit_fails() {
    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }

    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            _response: &str,
            error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            if let Some(tx) = self.tx.lock().unwrap().take() {
                let _ = tx.send(PeerChatResult {
                    task_id: task_id.into(),
                    status: status.into(),
                    response: String::new(),
                    error: if error.is_empty() {
                        None
                    } else {
                        Some(error.into())
                    },
                });
            }
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let llm = Arc::new(MockLlmChannel {
        response: String::new(),
        should_fail: true,
    });

    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });
    let source_info = Some(serde_json::json!({"node_id": "node-a"}));
    let req = make_request();

    tokio::spawn(async move {
        process_async(
            "test-task-fail",
            &req,
            "node-a",
            "node-a",
            &source_info,
            Some(llm.as_ref()),
            None,
            Some(persister.as_ref()),
            Duration::from_secs(10),
            "node-b",
        )
        .await;
    });

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let result = result.unwrap().unwrap();
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().contains("failed to process"));
    let _ = tx;
}

// ============================================================
// Coverage improvement: more async processing edge cases
// ============================================================

#[tokio::test]
async fn test_async_processing_llm_channel_closed() {
    // LLM channel returns a receiver that gets dropped immediately
    struct DroppingLlmChannel;
    impl LlmChannel for DroppingLlmChannel {
        fn submit(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<oneshot::Receiver<String>, String> {
            // Create a channel but drop the sender immediately
            let (tx, rx) = oneshot::channel();
            drop(tx); // Drop sender so receiver gets Err
            Ok(rx)
        }
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            _response: &str,
            error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            if let Some(tx) = self.tx.lock().unwrap().take() {
                let _ = tx.send(PeerChatResult {
                    task_id: task_id.into(),
                    status: status.into(),
                    response: String::new(),
                    error: if error.is_empty() {
                        None
                    } else {
                        Some(error.into())
                    },
                });
            }
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let llm = Arc::new(DroppingLlmChannel);
    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });
    let source_info = Some(serde_json::json!({"node_id": "node-a"}));
    let req = make_request();

    tokio::spawn(async move {
        process_async(
            "test-task-drop",
            &req,
            "node-a",
            "node-a",
            &source_info,
            Some(llm.as_ref()),
            None,
            Some(persister.as_ref()),
            Duration::from_secs(10),
            "node-b",
        )
        .await;
    });

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let result = result.unwrap().unwrap();
    assert_eq!(result.status, "error");
    assert!(result.error.unwrap().contains("response channel closed"));
    let _ = tx;
}

#[tokio::test]
async fn test_async_processing_no_source_node() {
    // When source_node_id is empty, callback should fail and result should be persisted
    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        #[allow(dead_code)]
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            _task_id: &str,
            _status: &str,
            _response: &str,
            _error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            Ok(()) // Don't send since source is empty
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let llm = Arc::new(MockLlmChannel {
        response: "Response".into(),
        should_fail: false,
    });

    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });
    let source_info = None; // No source info
    let req = make_request();

    tokio::spawn(async move {
        process_async(
            "test-no-source",
            &req,
            "node-a",
            "", // empty source_node_id
            &source_info,
            Some(llm.as_ref()),
            None,
            Some(persister.as_ref()),
            Duration::from_secs(10),
            "node-b",
        )
        .await;
    });

    // This should complete without hanging
    let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let _ = tx;
}

#[tokio::test]
async fn test_handle_extracts_source_from_rpc_meta() {
    // After the session_key refactor, source_node_id is taken from rpc_meta.from
    // (injected by the RPC server from the wire `from` field), NOT from
    // payload._source.node_id. The `_source` field is preserved for chat_id and
    // other downstream consumers.
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "_source": {"node_id": "should-be-ignored"},
    });
    let meta = RpcMeta {
        from: Some("source-node-1".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");
}

#[tokio::test]
async fn test_handle_no_rpc_meta_uses_empty_source() {
    // When rpc_meta is None (e.g., lightweight non-gateway nodes), source_node_id
    // is empty. session_key falls back to chat_id alone (or "default" if chat_id
    // is also missing). The handler should still ACK normally.
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "_source": {"chat_id": "web:abc"},
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");
}

#[test]
fn test_persist_result_with_persister() {
    let (tx, rx) = std::sync::mpsc::channel::<(String, String, String, String, String)>();

    struct MockPersister {
        tx: std::sync::Mutex<std::sync::mpsc::Sender<(String, String, String, String, String)>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            response: &str,
            error: &str,
            source_node: &str,
        ) -> Result<(), String> {
            let _ = self.tx.lock().unwrap().send((
                task_id.into(),
                status.into(),
                response.into(),
                error.into(),
                source_node.into(),
            ));
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_result_persister(Arc::new(MockPersister {
        tx: std::sync::Mutex::new(tx),
    }));

    handler.persist_result("task-1", "success", "response text", "", "node-a");

    let result = rx.recv_timeout(std::time::Duration::from_secs(1));
    assert!(result.is_ok());
    let (task_id, status, response, error, source) = result.unwrap();
    assert_eq!(task_id, "task-1");
    assert_eq!(status, "success");
    assert_eq!(response, "response text");
    assert_eq!(error, "");
    assert_eq!(source, "node-a");
}

#[test]
fn test_delete_result_with_persister() {
    struct MockPersister {
        deleted: std::sync::Mutex<Option<String>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            _task_id: &str,
            _status: &str,
            _response: &str,
            _error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, task_id: &str) -> Result<(), String> {
            *self.deleted.lock().unwrap() = Some(task_id.into());
            Ok(())
        }
    }

    let persister = Arc::new(MockPersister {
        deleted: std::sync::Mutex::new(None),
    });
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_result_persister(persister.clone());

    handler.delete_result("task-to-delete");

    let deleted = persister.deleted.lock().unwrap();
    assert_eq!(deleted.as_deref(), Some("task-to-delete"));
}

#[test]
fn test_peer_chat_request_type_field_serialization() {
    let json = r#"{"type":"task","content":"do something","context":{}}"#;
    let req: PeerChatRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.request_type, "task");

    let serialized = serde_json::to_string(&req).unwrap();
    assert!(serialized.contains(r#""type":"task""#));
}

#[test]
fn test_peer_chat_request_context_default() {
    let req: PeerChatRequest = serde_json::from_value(serde_json::json!({
        "content": "test"
    }))
    .unwrap();
    assert!(req.context.is_null());
}

#[tokio::test]
async fn test_wait_for_tasks_completes() {
    let handler = PeerChatHandler::new("node-b".into());
    // Submit a task
    let payload = serde_json::json!({"content": "hello"});
    let _ack = handler.handle(payload, None);
    // Wait for tasks to complete
    handler.wait_for_tasks().await;
}

#[test]
fn test_peer_chat_ack_serialization() {
    let ack = PeerChatAck {
        status: "accepted".into(),
        task_id: "t-123".into(),
        error: None,
    };
    let json = serde_json::to_string(&ack).unwrap();
    assert!(json.contains("accepted"));
    assert!(json.contains("t-123"));
}

// ============================================================
// Coverage improvement: more edge cases for peer chat
// ============================================================

#[tokio::test]
async fn test_handle_with_source_info_no_node_id() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "_source": {"other_field": "value"},
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");
}

#[tokio::test]
async fn test_handle_with_rpc_meta_none_from() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
    });
    let meta = RpcMeta { from: None };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");
}

#[tokio::test]
async fn test_handle_with_rpc_meta_with_from() {
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
    });
    let meta = RpcMeta {
        from: Some("source-node".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");
}

#[tokio::test]
async fn test_handle_context_sender_id_no_longer_used() {
    // After the session_key refactor, context.sender_id is no longer consulted
    // for session_key derivation. session_key is built from rpc_meta.from +
    // _source.chat_id only. The handler should still ACK normally.
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({
        "content": "Hello",
        "context": {"sender_id": "context-sender"},
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");
}

#[test]
fn test_persist_result_with_persister_fails() {
    struct FailingPersister;
    impl TaskResultPersister for FailingPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            _task_id: &str,
            _status: &str,
            _response: &str,
            _error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            Err("disk full".to_string())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_result_persister(Arc::new(FailingPersister));
    // Should not panic even when persister fails
    handler.persist_result("task-1", "success", "response", "", "node-a");
}

#[test]
fn test_delete_result_with_persister_fails() {
    struct FailingPersister;
    impl TaskResultPersister for FailingPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            _task_id: &str,
            _status: &str,
            _response: &str,
            _error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Err("not found".to_string())
        }
    }

    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_result_persister(Arc::new(FailingPersister));
    // Should not panic even when delete fails
    handler.delete_result("task-1");
}

#[tokio::test]
async fn test_wait_for_tasks_after_handle() {
    let handler = PeerChatHandler::new("node-b".into());
    // Handle a request to spawn a task
    let payload = serde_json::json!({"content": "Hello"});
    let _ack = handler.handle(payload, None);
    // Wait for the task to complete
    handler.wait_for_tasks().await;
    // Should not panic or hang
}

#[test]
fn test_peer_chat_request_default_type_serialization() {
    // Verify default type is "request" when not specified
    let json = r#"{"content": "test"}"#;
    let req: PeerChatRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.request_type, "request");
}

#[test]
fn test_peer_chat_request_all_types() {
    for request_type in &["chat", "request", "task", "query"] {
        let json = format!(r#"{{"type": "{}", "content": "test"}}"#, request_type);
        let req: PeerChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.request_type, *request_type);
    }
}

#[tokio::test]
async fn test_handle_with_persister_set_running_called() {
    let (tx, rx) = std::sync::mpsc::channel::<(String, String)>();

    struct MockPersister {
        tx: std::sync::Mutex<std::sync::mpsc::Sender<(String, String)>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, task_id: &str, source_node: &str) {
            let _ = self
                .tx
                .lock()
                .unwrap()
                .send((task_id.into(), source_node.into()));
        }
        fn set_result(&self, _: &str, _: &str, _: &str, _: &str, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_result_persister(Arc::new(MockPersister {
        tx: std::sync::Mutex::new(tx),
    }));

    // Source node ID is now taken from rpc_meta.from (not from payload._source.node_id).
    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "task-with-source",
        "_source": {"node_id": "should-be-ignored", "chat_id": "web:abc"},
    });
    let meta = RpcMeta {
        from: Some("source-node-1".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");

    let (task_id, source) = rx.recv_timeout(std::time::Duration::from_secs(1)).unwrap();
    assert_eq!(task_id, "task-with-source");
    assert_eq!(source, "source-node-1");
}

#[tokio::test]
async fn test_async_processing_llm_timeout() {
    // LLM channel returns a receiver that never sends (timeout)
    struct SlowLlmChannel;
    impl LlmChannel for SlowLlmChannel {
        fn submit(
            &self,
            _session_key: &str,
            _content: &str,
            _correlation_id: &str,
        ) -> Result<oneshot::Receiver<String>, String> {
            let (_tx, rx) = oneshot::channel();
            // Don't send anything, just let it timeout
            Ok(rx)
        }
    }

    let (tx, rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _task_id: &str, _source_node: &str) {}
        fn set_result(
            &self,
            task_id: &str,
            status: &str,
            _response: &str,
            error: &str,
            _source_node: &str,
        ) -> Result<(), String> {
            if let Some(tx) = self.tx.lock().unwrap().take() {
                let _ = tx.send(PeerChatResult {
                    task_id: task_id.into(),
                    status: status.into(),
                    response: String::new(),
                    error: if error.is_empty() {
                        None
                    } else {
                        Some(error.into())
                    },
                });
            }
            Ok(())
        }
        fn delete(&self, _task_id: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let llm = Arc::new(SlowLlmChannel);
    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });
    let source_info = Some(serde_json::json!({"node_id": "node-a"}));
    let req = make_request();

    tokio::spawn(async move {
        process_async(
            "test-task-timeout",
            &req,
            "node-a",
            "node-a",
            &source_info,
            Some(llm.as_ref()),
            None,
            Some(persister.as_ref()),
            Duration::from_millis(100), // Very short timeout
            "node-b",
        )
        .await;
    });

    let result = tokio::time::timeout(Duration::from_secs(5), rx).await;
    let result = result.unwrap().unwrap();
    assert_eq!(result.status, "error");
    // The error could be either "response channel closed" (if oneshot sender is dropped)
    // or "LLM processing timeout" (if the timeout fires first)
    let err = result.error.unwrap();
    assert!(
        err.contains("timeout") || err.contains("response channel closed") || err.contains("LLM"),
        "unexpected error: {}",
        err
    );
    let _ = tx;
}

#[tokio::test]
async fn test_send_callback_or_persist_no_source() {
    // When source_node_id is empty, should not succeed
    let (tx, _rx) = tokio::sync::oneshot::channel::<PeerChatResult>();

    struct MockPersister {
        #[allow(dead_code)]
        tx: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<PeerChatResult>>>,
    }
    impl TaskResultPersister for MockPersister {
        fn set_running(&self, _: &str, _: &str) {}
        fn set_result(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: &str,
            source_node: &str,
        ) -> Result<(), String> {
            // When source is empty, set_result should not be called
            assert!(
                !source_node.is_empty(),
                "set_result should not be called with empty source"
            );
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
    }

    let persister = Arc::new(MockPersister {
        tx: std::sync::Mutex::new(Some(tx)),
    });

    send_callback_or_persist(
        None,
        Some(persister.as_ref()),
        &None,
        "", // empty source_node_id
        "node-self",
        "task-1",
        "success",
        "response",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
}

#[test]
fn test_peer_chat_handler_setters() {
    let handler = PeerChatHandler::new("node-b".into());

    // Verify initial state
    assert!(handler.llm_channel.is_none());
    assert!(handler.rpc_client.is_none());
    assert!(handler.result_persister.is_none());

    // We can't easily create real instances, but we can test that the
    // new/with_timeout constructors work properly
    let handler2 = PeerChatHandler::with_timeout("node-c".into(), Duration::from_secs(300));
    assert_eq!(handler2.node_id(), "node-c");
    assert_eq!(handler2.timeout_secs(), 300);
}

// ============================================================
// Composite session_key tests (cluster_rpc:{node_id}/{chat_id})
// ============================================================

/// Verify the full composite session_key path: rpc_meta.from + _source.chat_id.
///
/// After the session_key isolation refactor:
///   - source_node_id ← rpc_meta.from (NOT payload._source.node_id)
///   - chat_id ← payload._source.chat_id (fallback "default")
///   - session_key ← "cluster_rpc:{source_node_id}/{chat_id}"
#[tokio::test]
async fn test_session_key_composite_with_node_id_and_chat_id() {
    let task_list = Arc::new(ClusterTaskList::new(std::env::temp_dir()));
    let work_queue = Arc::new(ClusterWorkQueue::new(64));
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_cluster_queue(task_list.clone(), work_queue.clone());

    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "test-composite-1",
        "_source": {
            "node_id": "should-be-ignored",
            "chat_id": "web:abc123",
        },
    });
    let meta = RpcMeta {
        from: Some("node-A".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");

    let task = task_list
        .get_task("test-composite-1")
        .expect("task should be enqueued");
    assert_eq!(task.source.node_id, "node-A");
    assert_eq!(task.source.session_key, "cluster_rpc:node-A/web:abc123");
}

/// Verify chat_id fallback to "default" when _source.chat_id is missing.
#[tokio::test]
async fn test_session_key_default_chat_id_when_missing() {
    let task_list = Arc::new(ClusterTaskList::new(std::env::temp_dir()));
    let work_queue = Arc::new(ClusterWorkQueue::new(64));
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_cluster_queue(task_list.clone(), work_queue.clone());

    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "test-composite-2",
        "_source": {
            "node_id": "should-be-ignored",
            // chat_id intentionally missing
        },
    });
    let meta = RpcMeta {
        from: Some("node-A".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");

    let task = task_list
        .get_task("test-composite-2")
        .expect("task should be enqueued");
    assert_eq!(task.source.node_id, "node-A");
    assert_eq!(task.source.session_key, "cluster_rpc:node-A/default");
}

/// Verify source_node_id is taken from rpc_meta.from and payload._source.node_id
/// is ignored. This guards against accidental regression to the old behavior.
#[tokio::test]
async fn test_source_node_id_from_rpc_meta_ignores_payload_source() {
    let task_list = Arc::new(ClusterTaskList::new(std::env::temp_dir()));
    let work_queue = Arc::new(ClusterWorkQueue::new(64));
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_cluster_queue(task_list.clone(), work_queue.clone());

    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "test-composite-3",
        "_source": {
            "node_id": "payload-source-id",  // must be ignored
            "chat_id": "web:xyz",
        },
    });
    let meta = RpcMeta {
        from: Some("real-node-id".into()),
    };
    let ack = handler.handle(payload, Some(meta));
    assert_eq!(ack.status, "accepted");

    let task = task_list
        .get_task("test-composite-3")
        .expect("task should be enqueued");
    assert_eq!(
        task.source.node_id, "real-node-id",
        "source_node_id must come from rpc_meta.from, not payload._source.node_id"
    );
    assert_eq!(task.source.session_key, "cluster_rpc:real-node-id/web:xyz");
}

/// Verify session_key when rpc_meta.from is None (degraded path).
///
/// When rpc_meta.from is absent, source_node_id is empty. session_key falls back
/// to chat_id alone (no node_id prefix). This is the degradation path for
/// lightweight non-gateway nodes that don't inject rpc_meta.
#[tokio::test]
async fn test_session_key_no_rpc_meta_falls_back_to_chat_id() {
    let task_list = Arc::new(ClusterTaskList::new(std::env::temp_dir()));
    let work_queue = Arc::new(ClusterWorkQueue::new(64));
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_cluster_queue(task_list.clone(), work_queue.clone());

    let payload = serde_json::json!({
        "content": "Hello",
        "task_id": "test-composite-4",
        "_source": {
            "chat_id": "web:degraded",
        },
    });
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "accepted");

    let task = task_list
        .get_task("test-composite-4")
        .expect("task should be enqueued");
    assert_eq!(task.source.node_id, "");
    assert_eq!(task.source.session_key, "cluster_rpc:web:degraded");
}

// ============================================================
// coverage batch: setter bodies (set_llm_channel /
// set_rpc_client / set_timeout), cluster-queue enqueue +
// queue-full Ack error, the REAL LLM timeout arm (existing
// SlowLlmChannel drops tx → "closed" arm, never the timeout
// arm), send_callback happy path via real server (payload +
// conditional error field), retry exhaustion, delete-on-success
// after successful callback.
// ============================================================

struct StaticResolverW3b {
    port: u16,
}
impl crate::rpc::client::PeerResolver for StaticResolverW3b {
    fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        Some((vec!["127.0.0.1".into()], self.port, true))
    }
    fn get_local_interfaces(&self) -> Vec<crate::rpc::client::LocalNetworkInterface> {
        Vec::new()
    }
    fn get_node_id(&self) -> String {
        "w3b-pch-client".into()
    }
}

/// LLM channel whose receiver NEVER resolves (sender leaked) — forces the
/// real `Err(_)` timeout arm in process_async, unlike SlowLlmChannel whose
/// tx is dropped at submit-return (which deterministically hits the
/// `Ok(Err(_))` channel-closed arm instead).
struct NeverRespondChannelW3b;
impl LlmChannel for NeverRespondChannelW3b {
    fn submit(
        &self,
        _session_key: &str,
        _content: &str,
        _correlation_id: &str,
    ) -> Result<oneshot::Receiver<String>, String> {
        let (tx, rx) = oneshot::channel();
        std::mem::forget(tx); // keep tx alive forever → rx never resolves
        Ok(rx)
    }
}

/// Persister that records every call for assertions.
struct RecordingPersisterW3b {
    running: std::sync::Mutex<Vec<String>>,
    results: std::sync::Mutex<Vec<(String, String, String, String)>>,
    deleted: std::sync::Mutex<Vec<String>>,
}
impl TaskResultPersister for RecordingPersisterW3b {
    fn set_running(&self, task_id: &str, _source_node: &str) {
        self.running.lock().unwrap().push(task_id.into());
    }
    fn set_result(
        &self,
        task_id: &str,
        status: &str,
        response: &str,
        error: &str,
        _source_node: &str,
    ) -> Result<(), String> {
        self.results.lock().unwrap().push((
            task_id.into(),
            status.into(),
            response.into(),
            error.into(),
        ));
        Ok(())
    }
    fn delete(&self, task_id: &str) -> Result<(), String> {
        self.deleted.lock().unwrap().push(task_id.into());
        Ok(())
    }
}

fn recording_persister_w3b() -> RecordingPersisterW3b {
    RecordingPersisterW3b {
        running: std::sync::Mutex::new(Vec::new()),
        results: std::sync::Mutex::new(Vec::new()),
        deleted: std::sync::Mutex::new(Vec::new()),
    }
}

#[test]
fn test_w3b_setter_bodies_llm_channel_rpc_client_timeout() {
    // Existing tests only cover set_result_persister / set_cluster_queue;
    // set_llm_channel / set_rpc_client / set_timeout bodies are exercised here.
    let mut handler = PeerChatHandler::with_timeout("node-b".into(), Duration::from_secs(9));
    assert_eq!(handler.timeout_secs(), 9);
    assert_eq!(handler.node_id(), "node-b");

    handler.set_timeout(Duration::from_secs(4));
    assert_eq!(handler.timeout_secs(), 4);

    handler.set_llm_channel(Arc::new(MockLlmChannel {
        response: "r".into(),
        should_fail: false,
    }));
    assert!(handler.llm_channel.is_some());

    handler.set_rpc_client(Arc::new(RpcClient::new()));
    assert!(handler.rpc_client.is_some());

    let tl = Arc::new(ClusterTaskList::new(std::env::temp_dir()));
    let wq = Arc::new(ClusterWorkQueue::new(2));
    handler.set_cluster_queue(tl, wq);
    assert!(handler.cluster_task_list.is_some());
    assert!(handler.cluster_work_queue.is_some());

    handler.set_result_persister(Arc::new(recording_persister_w3b()));
    assert!(handler.result_persister.is_some());
}

#[tokio::test]
async fn test_w3b_handle_cluster_queue_enqueues_to_work_queue() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let wq = Arc::new(ClusterWorkQueue::new(8));
    handler.set_cluster_queue(tl.clone(), wq.clone());

    let persister = Arc::new(recording_persister_w3b());
    handler.set_result_persister(persister.clone());

    let ack = handler.handle(
        serde_json::json!({"content": "queued work", "task_id": "w3b-ct-1"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack.status, "accepted");
    assert_eq!(ack.task_id, "w3b-ct-1");

    // set_running fired because rpc_meta.from was present
    assert_eq!(persister.running.lock().unwrap().as_slice(), ["w3b-ct-1"]);

    // Task created AND actually handed to the work-queue consumer
    let task = tl.get_task("w3b-ct-1").unwrap();
    assert_eq!(task.content, "queued work");
    assert_eq!(task.status, TaskStatus::Pending);
    let next = tokio::time::timeout(Duration::from_secs(2), wq.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, "w3b-ct-1");
}

// 压测②（三设备）：带取证 marker 的任务路由进优先通道——
// handler 级验证（队列级插队语义见 cluster_task/tests.rs；marker 常量
// 与 master 侧 build_selfcheck_prompt 同源）。
#[tokio::test]
async fn test_selfcheck_marker_routes_to_priority_lane() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let wq = Arc::new(ClusterWorkQueue::new(8));
    handler.set_cluster_queue(tl.clone(), wq.clone());
    handler.set_result_persister(Arc::new(recording_persister_w3b()));

    // 正式任务先到，取证任务后到 → 取证下一轮取号绝对插队。
    let ack_normal = handler.handle(
        serde_json::json!({"content": "long running work", "task_id": "sc-normal-1"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack_normal.status, "accepted");

    let marker_content = format!(
        "{}NB-9] 请取证并如实回报证据内容",
        crate::cluster_task::SELFCHECK_PROMPT_MARKER
    );
    let ack_sc = handler.handle(
        serde_json::json!({"content": marker_content, "task_id": "sc-prio-1"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack_sc.status, "accepted");

    let next = tokio::time::timeout(Duration::from_secs(2), wq.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, "sc-prio-1", "取证任务必须先于先到的正式任务出队");
    let next2 = tokio::time::timeout(Duration::from_secs(2), wq.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next2, "sc-normal-1");
}

#[tokio::test]
async fn test_w3b_handle_cluster_queue_full_returns_error_ack() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    // Capacity-1 queue pre-filled with an occupant emulates a full queue
    // (tokio mpsc panics on capacity 0).
    let wq = Arc::new(ClusterWorkQueue::new(1));
    wq.submit("occupant".to_string()).unwrap();
    handler.set_cluster_queue(tl, wq);

    let ack = handler.handle(
        serde_json::json!({"content": "no room", "task_id": "w3b-ct-2"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack.status, "error");
    assert!(ack.task_id.is_empty());
}

#[tokio::test]
async fn test_w3b_process_async_real_timeout_arm_persists_error() {
    // The leaked-sender channel guarantees the tokio timeout branch (not the
    // channel-closed branch) fires, and wait_for_tasks joins the spawned task,
    // so the persister call is finished when the await returns.
    let mut handler = PeerChatHandler::new("node-b".into());
    handler.set_timeout(Duration::from_millis(60));
    handler.set_llm_channel(Arc::new(NeverRespondChannelW3b));
    let persister = Arc::new(recording_persister_w3b());
    handler.set_result_persister(persister.clone());

    let ack = handler.handle(
        serde_json::json!({"content": "slow request", "task_id": "w3b-slow-1"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack.status, "accepted");
    handler.wait_for_tasks().await;

    let results = persister.results.lock().unwrap();
    assert_eq!(results.len(), 1, "timeout should persist one error result");
    assert_eq!(results[0].0, "w3b-slow-1");
    assert_eq!(results[0].1, "error");
    assert_eq!(results[0].3, "LLM processing timeout");
}

#[tokio::test]
async fn test_w3b_send_callback_real_server_success_and_error_field() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let cap2 = captured.clone();
    server.register_handler(
        "peer_chat_callback",
        Box::new(move |payload| {
            cap2.lock().unwrap().push(payload.clone());
            Ok(serde_json::json!({"status": "accepted"}))
        }),
    );
    server.start().await.unwrap();
    let port = server.port();

    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port }));

    // Success without error → payload omits the error field entirely
    let ok = send_callback(
        Some(&client),
        "origin-node",
        "node-self-w3b",
        "w3b-cb-1",
        "success",
        "done-work",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(ok, "callback should succeed against the live server");
    {
        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0]["task_id"], "w3b-cb-1");
        assert_eq!(cap[0]["status"], "success");
        assert!(
            cap[0].get("error").is_none(),
            "error field omitted when empty"
        );
        // 回归（真机 bug）：发送方身份必须随 wire 帧 `from` 走并在
        // server 注入的 `_rpc.from` 落到 handler——留空会让 master 记账键
        // `cluster_rpc:{worker}/{task_id}` 的 worker 段变空。
        assert_eq!(
            cap[0]["_rpc"]["from"], "node-self-w3b",
            "wire from 必须携带发送方节点 id（server 注入 _rpc.from）"
        );
    }

    // Non-empty error → payload carries it
    let ok2 = send_callback(
        Some(&client),
        "origin-node",
        "node-self-w3b",
        "w3b-cb-2",
        "error",
        "",
        "boom-detail",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(ok2);
    {
        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 2);
        assert_eq!(cap[1]["error"], "boom-detail");
    }

    server.stop().unwrap();
}

#[tokio::test(start_paused = true)]
async fn test_w3b_send_callback_retries_exhausted_when_peer_unreachable() {
    // Port 1 refuses connections; the paused clock makes the 5s/10s inter-attempt
    // backoffs instant. Three failures → false.
    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port: 1 }));
    let ok = send_callback(
        Some(&client),
        "ghost-node",
        "node-self-w3b",
        "w3b-t-ex",
        "success",
        "r",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(!ok, "all callback retries should be exhausted");
}

#[tokio::test]
async fn test_w3b_send_callback_or_persist_deletes_after_successful_callback() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    server.register_handler(
        "peer_chat_callback",
        Box::new(|_p| Ok(serde_json::json!({"status": "accepted"}))),
    );
    server.start().await.unwrap();
    let port = server.port();

    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port }));
    let persister = recording_persister_w3b();

    send_callback_or_persist(
        Some(&client),
        Some(&persister),
        &None,
        "origin-node",
        "node-self-w3b",
        "w3b-ds-1",
        "success",
        "answer",
        "",
        None,
        None,
        None,
        None,
    )
    .await;

    assert!(
        persister.results.lock().unwrap().is_empty(),
        "no persist on callback success"
    );
    assert_eq!(persister.deleted.lock().unwrap().as_slice(), ["w3b-ds-1"]);
    server.stop().unwrap();
}

// ============================================================
// coverage: empty-content ack, persist_result skip arm with a
// persister attached, callback delete-Err / set_result-Err arms,
// and the retry-warn field line under a subscriber.
// ============================================================

struct S4AllEventsSubscriber;
impl tracing::Subscriber for S4AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn s4_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4AllEventsSubscriber);
    });
}

/// Persister whose set_result/delete can be told to fail, counting calls.
struct S4FailingPersister {
    fail_set_result: bool,
    fail_delete: bool,
    set_result_calls: std::sync::atomic::AtomicUsize,
    delete_calls: std::sync::atomic::AtomicUsize,
}

impl TaskResultPersister for S4FailingPersister {
    fn set_running(&self, _task_id: &str, _source_node: &str) {}

    fn set_result(
        &self,
        _task_id: &str,
        _status: &str,
        _response: &str,
        _error: &str,
        _source_node: &str,
    ) -> Result<(), String> {
        self.set_result_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_set_result {
            Err("s4 set_result boom".into())
        } else {
            Ok(())
        }
    }

    fn delete(&self, _task_id: &str) -> Result<(), String> {
        self.delete_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.fail_delete {
            Err("s4 delete boom".into())
        } else {
            Ok(())
        }
    }
}

/// A payload with content present but EMPTY fails validation after successful
/// deserialization (peer_chat_handler.rs 216-222).
#[test]
fn test_s4_handle_empty_content_string() {
    s4_tracing_subscriber();
    let handler = PeerChatHandler::new("node-b".into());
    let payload = serde_json::json!({"content": "", "type": "chat"});
    let ack = handler.handle(payload, None);
    assert_eq!(ack.status, "error");
    assert!(ack.task_id.is_empty());
}

/// persist_result with a persister attached but an empty source node skips
/// set_result entirely (peer_chat_handler.rs 373-383).
#[tokio::test]
async fn test_s4_persist_result_empty_source_with_persister() {
    s4_tracing_subscriber();
    let mut handler = PeerChatHandler::new("node-b".into());
    let persister = Arc::new(S4FailingPersister {
        fail_set_result: false,
        fail_delete: false,
        set_result_calls: std::sync::atomic::AtomicUsize::new(0),
        delete_calls: std::sync::atomic::AtomicUsize::new(0),
    });
    handler.set_result_persister(persister.clone());

    handler.persist_result("s4-t", "success", "resp", "", "");

    assert_eq!(
        persister
            .set_result_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "empty source node must skip persistence"
    );
}

/// A successful callback followed by a failing persister.delete hits the
/// delete-error warn with field expressions (peer_chat_handler.rs 563-569).
#[tokio::test]
async fn test_s4_send_callback_or_persist_delete_failure_logs() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    s4_tracing_subscriber();
    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    server.register_handler(
        "peer_chat_callback",
        Box::new(|_p| Ok(serde_json::json!({"status": "accepted"}))),
    );
    server.start().await.unwrap();
    let port = server.port();

    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port }));
    let persister = S4FailingPersister {
        fail_set_result: false,
        fail_delete: true,
        set_result_calls: std::sync::atomic::AtomicUsize::new(0),
        delete_calls: std::sync::atomic::AtomicUsize::new(0),
    };

    send_callback_or_persist(
        Some(&client),
        Some(&persister),
        &None,
        "origin-node",
        "node-self-s4",
        "s4-del-1",
        "success",
        "answer",
        "",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(
        persister
            .delete_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "delete attempted after successful callback"
    );
    assert_eq!(
        persister
            .set_result_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "no persist when callback succeeded"
    );
    server.stop().unwrap();
}

/// A failed callback with a non-empty source node and a failing set_result
/// hits the persist-error warn (peer_chat_handler.rs 570-581).
#[tokio::test]
async fn test_s4_send_callback_or_persist_set_result_failure_logs() {
    s4_tracing_subscriber();
    let persister = S4FailingPersister {
        fail_set_result: true,
        fail_delete: false,
        set_result_calls: std::sync::atomic::AtomicUsize::new(0),
        delete_calls: std::sync::atomic::AtomicUsize::new(0),
    };

    // No rpc_client → send_callback returns false immediately.
    send_callback_or_persist(
        None,
        Some(&persister),
        &None,
        "origin-node-2",
        "node-self-s4",
        "s4-persist-1",
        "failed",
        "",
        "boom",
        None,
        None,
        None,
        None,
    )
    .await;

    assert_eq!(
        persister
            .set_result_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1,
        "set_result attempted after failed callback"
    );
}

/// The per-attempt warn field line only evaluates under an enabled subscriber;
/// the paused clock makes the 5s/10s backoffs instant (peer_chat_handler.rs
/// 633-644).
#[tokio::test(start_paused = true)]
async fn test_s4_send_callback_retry_warn_field_line() {
    s4_tracing_subscriber();
    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port: 1 }));
    let ok = send_callback(
        Some(&client),
        "ghost-node",
        "node-self-s4",
        "s4-retry-1",
        "success",
        "r",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(!ok, "all callback retries should be exhausted");
}

/// P2A：fail_class wire 契约——error 回调携带结构化
/// 分类字段；`None` 时字段整个不落（旧 master 忽略未知字段，兼容面不变）。
#[tokio::test]
async fn test_p2a_fail_class_field_wire_contract() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let cap2 = captured.clone();
    server.register_handler(
        "peer_chat_callback",
        Box::new(move |payload| {
            cap2.lock().unwrap().push(payload.clone());
            Ok(serde_json::json!({"status": "accepted"}))
        }),
    );
    server.start().await.unwrap();
    let port = server.port();

    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port }));

    // Error + fail_class → payload 携带结构化分类。
    let ok = send_callback(
        Some(&client),
        "origin-node",
        "node-self-p2a",
        "p2a-cb-1",
        "error",
        "",
        "工具参数校验连续失败 1 次，已停止重试。最近工具：'exec'。",
        None,
        Some("validation_budget"),
        None,
        None,
    )
    .await;
    assert!(ok);

    // Success + fail_class=None → 字段不落。
    let ok2 = send_callback(
        Some(&client),
        "origin-node",
        "node-self-p2a",
        "p2a-cb-2",
        "success",
        "done",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(ok2);

    {
        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 2);
        assert_eq!(
            cap[0]["fail_class"], "validation_budget",
            "error 回调必须携带 fail_class"
        );
        assert!(
            cap[1].get("fail_class").is_none(),
            "fail_class=None 时字段必须整个省略（wire 兼容）"
        );
    }

    server.stop().unwrap();
}

// --：delivery 字段 wire 契约 --------------------------------------

/// delivery 字段 wire 契约——`Some` 时整个 JSON
/// 原样落 payload["delivery"]；`None` 时字段整个不落（旧 master 忽略未知
/// 字段，兼容面与 usage/fail_class 同款）。
#[tokio::test]
async fn test_p4_delivery_field_wire_contract() {
    use crate::rpc::server::{RpcServer, RpcServerConfig};

    let server = Arc::new(RpcServer::new(RpcServerConfig {
        bind_address: "127.0.0.1:0".into(),
        ..Default::default()
    }));
    let captured = Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new()));
    let cap2 = captured.clone();
    server.register_handler(
        "peer_chat_callback",
        Box::new(move |payload| {
            cap2.lock().unwrap().push(payload.clone());
            Ok(serde_json::json!({"status": "accepted"}))
        }),
    );
    server.start().await.unwrap();
    let port = server.port();

    let client = RpcClient::with_resolver(Arc::new(StaticResolverW3b { port }));

    // Some(delivery) → payload 携带结构化交付证据。
    let ok = send_callback(
        Some(&client),
        "origin-node",
        "node-self-p4",
        "p4-cb-1",
        "success",
        "delivered",
        "",
        None,
        None,
        Some(serde_json::json!({
            "branch": "run-p4-cb-1",
            "ref_name": "refs/heads/nemesis/run-p4-cb-1",
            "commit": "abc123",
            "base_tree": "def456",
            "files_changed": 2,
            "patch_path": "logs/deliveries/p4-cb-1.patch",
            "patch_truncated": false
        })),
        None,
    )
    .await;
    assert!(ok);

    // None → 字段不落。
    let ok2 = send_callback(
        Some(&client),
        "origin-node",
        "node-self-p4",
        "p4-cb-2",
        "success",
        "done",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(ok2);

    {
        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 2);
        assert_eq!(
            cap[0]["delivery"]["branch"], "run-p4-cb-1",
            "delivery JSON 必须原样落 payload"
        );
        assert_eq!(cap[0]["delivery"]["files_changed"], 2);
        assert!(
            cap[1].get("delivery").is_none(),
            "delivery=None 时字段必须整个省略（wire 兼容）"
        );
    }

    server.stop().unwrap();
}

// -- required_profession_from_payload（集群专业职能框架）---------------

#[test]
fn required_profession_from_payload_arms() {
    use super::required_profession_from_payload;
    // 顶层 slug → 归一小写。
    assert_eq!(
        required_profession_from_payload(&serde_json::json!({"required_profession": "Dev:CPP"})),
        Some("dev:cpp".to_string())
    );
    // 缺键 / 空白 / 非字符串 → None（旧 A 端优雅降级）。
    assert_eq!(
        required_profession_from_payload(&serde_json::json!({})),
        None
    );
    assert_eq!(
        required_profession_from_payload(&serde_json::json!({"required_profession": "   "})),
        None
    );
    assert_eq!(
        required_profession_from_payload(&serde_json::json!({"required_profession": 42})),
        None
    );
}

// ---------------------------------------------------------------------------
// T44C 真机注入回归锁：回调必须走拨号穿透（must-reach）。
//
// registry 判 Offline 但对端实际可达（真实 TCP echo 在听）时，回调仍须
// 拨号送达。门禁瞬断的反面教材（T44C iptables run2 实证）：heal 滞后窗
// 内 3 次重试 15s 全被「peer is offline」拒发、零拨号，已完成任务结果
// 丢失。修复 = send_callback 走 require_online=false 通路；本测试钉死
// 「offline registry 不拦回调拨号」语义。
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_send_callback_dials_through_offline_registry() {
    use crate::rpc::client::{LocalNetworkInterface, PeerResolver, RpcClient};
    use crate::transport::conn::WireMessage;

    // 真 TCP echo：读一帧请求，按请求 id 回成功响应帧（client/tests.rs
    // spawn_arb_echo 同款最小形态）。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let Ok((mut sock, _)) = listener.accept().await else {
            return;
        };
        let mut len_buf = [0u8; 4];
        if sock.read_exact(&mut len_buf).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut buf = vec![0u8; len];
        if sock.read_exact(&mut buf).await.is_err() {
            return;
        }
        let Ok(req) = serde_json::from_slice::<WireMessage>(&buf) else {
            return;
        };
        let resp = WireMessage {
            version: "1.0".into(),
            id: req.id.clone(),
            msg_type: "response".into(),
            from: "echo-peer".into(),
            to: req.from.clone(),
            action: req.action.clone(),
            payload: serde_json::json!({"ok": true}),
            timestamp: chrono::Local::now().timestamp(),
            error: String::new(),
        };
        let json = serde_json::to_vec(&resp).unwrap();
        let _ = sock.write_all(&(json.len() as u32).to_be_bytes()).await;
        let _ = sock.write_all(&json).await;
        let _ = sock.flush().await;
    });

    // registry 报 Offline（heal 滞后窗形态），地址指向真实 echo。
    struct OfflineResolver(String);
    impl PeerResolver for OfflineResolver {
        fn get_peer_info(&self, _peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
            Some((vec![self.0.clone()], 0, false))
        }
        fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
            Vec::new()
        }
        fn get_node_id(&self) -> String {
            "test-worker".into()
        }
    }
    let client = RpcClient::with_resolver(Arc::new(OfflineResolver(addr)));

    // 修复前：门禁瞬断 → 立即 false（3 次重试零拨号，~15s 空转）。
    // 修复后：拨号穿透 → 首试即送达，true。
    let delivered = send_callback(
        Some(&client),
        "master-node",
        "test-worker",
        "task-t44c",
        "success",
        "echo-back",
        "",
        None,
        None,
        None,
        None,
    )
    .await;
    assert!(
        delivered,
        "回调必须穿透 offline registry 拨号送达（must-reach 语义）"
    );
}

// ---------------------------------------------------------------------------
// peer_chat_ack_rejected（BUG-1）：A 端拒单判定单一真相源
// ---------------------------------------------------------------------------

// B 端拒单 ACK（工作队列满）→ true。
#[test]
fn test_ack_rejected_detects_error_status() {
    let ack = serde_json::json!({"status": "error", "task_id": ""});
    assert!(peer_chat_ack_rejected(Some(&ack)));
}

// 正常受理 ACK → false（三个 A 端调用点的主路径零扰动）。
#[test]
fn test_ack_rejected_accepts_ok_status() {
    let ack = serde_json::json!({"status": "ok", "task_id": "t-1"});
    assert!(!peer_chat_ack_rejected(Some(&ack)));
}

// result 缺失 / 非对象 / 无 status 键：保守判不拒单（不把畸形载荷误当
// 拒单走失败收尾——传输层成功且无 error 字段时，悬挂由既有超时面兜）。
#[test]
fn test_ack_rejected_conservative_on_malformed_payload() {
    assert!(!peer_chat_ack_rejected(None));
    assert!(!peer_chat_ack_rejected(Some(&serde_json::json!("ok"))));
    assert!(!peer_chat_ack_rejected(Some(
        &serde_json::json!({"task_id": "t"})
    )));
    assert!(!peer_chat_ack_rejected(Some(
        &serde_json::json!({"status": 42})
    )));
}

// ---------------------------------------------------------------------------
// BUG-1B 端拒单回滚：队列满 → error ACK + 两处登记回滚
// ---------------------------------------------------------------------------

// 队列满拒单时：ACK status=error；set_running 占位被 delete 回滚（A 端
// query_task_result 不再被吊住）；task_list ghost Pending 被移除
// （complete_task 是唯一移除点）。
#[tokio::test]
async fn test_queue_full_rejection_rolls_back_running_and_task_list() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let wq = Arc::new(ClusterWorkQueue::new(1));
    handler.set_cluster_queue(tl.clone(), wq.clone());
    let persister = Arc::new(recording_persister_w3b());
    handler.set_result_persister(persister.clone());

    // 占满容量 1 的普通通道。
    wq.submit("pre-existing".into()).unwrap();

    let ack = handler.handle(
        serde_json::json!({"content": "will be rejected", "task_id": "rej-1"}),
        Some(RpcMeta {
            from: Some("origin-node".into()),
        }),
    );
    assert_eq!(ack.status, "error", "队列满必须同步拒单");
    assert_eq!(ack.task_id, "");

    // 回滚 ①：running 占位被 delete（而非留存吊住恢复轮询）。
    assert_eq!(
        persister.running.lock().unwrap().as_slice(),
        ["rej-1"],
        "set_running 先行发生（被测现状不变）"
    );
    assert_eq!(
        persister.deleted.lock().unwrap().as_slice(),
        ["rej-1"],
        "占位必须被 delete 回滚"
    );

    // 回滚 ②：ghost Pending 不驻留 task_list。
    assert!(
        tl.get_task("rej-1").is_none(),
        "拒单任务必须从 task_list 移除"
    );

    // 队列里只有占位的 pre-existing，被拒任务没有入队。
    let next = tokio::time::timeout(Duration::from_millis(500), wq.next())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(next, "pre-existing");
}

// 空 source_node_id（rpc_meta.from 缺失）拒单：不触碰 persister（
// set_running 本就没发生），task_list 照常回滚。
#[tokio::test]
async fn test_queue_full_rejection_without_source_node_skips_persister() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let wq = Arc::new(ClusterWorkQueue::new(1));
    handler.set_cluster_queue(tl.clone(), wq.clone());
    let persister = Arc::new(recording_persister_w3b());
    handler.set_result_persister(persister.clone());

    wq.submit("pre-existing".into()).unwrap();
    let ack = handler.handle(
        serde_json::json!({"content": "no source", "task_id": "rej-2"}),
        None,
    );
    assert_eq!(ack.status, "error");
    assert!(
        persister.deleted.lock().unwrap().is_empty(),
        "无 source 节点时 set_running 未发生，delete 也不该发生"
    );
    assert!(tl.get_task("rej-2").is_none(), "ghost 照常移除");
}

// ============================================================
// b mandate 前置闸（四道校验 × posture 矩阵 + enforcement 投影）
// ============================================================

use crate::roaming::mandate::{
    MandateKeyFile, MandatePolicy, build_default_mandate, now_unix, sign_mandate,
    signing_key_from_hex,
};
use crate::roaming::trust::mandate_pin_forced;

/// 生成一对 mandate 密钥（KeyFile 形态）。
fn make_gate_key(tag: &str) -> MandateKeyFile {
    use crate::roaming::mandate::public_key_bytes;
    let sk = p256::ecdsa::SigningKey::random(&mut rand::rngs::OsRng);
    MandateKeyFile {
        private_key: hex::encode(sk.to_bytes().as_slice()),
        public_key: hex::encode(public_key_bytes(sk.verifying_key())),
        created_at: format!("test-{tag}"),
    }
}

/// 签好 mandate 的 peer_chat 载荷（allowlist=None 不限；窗口从 signed_at 起
/// 3600s——默认命中 happy path）。
fn signed_payload(kf: &MandateKeyFile, task_id: &str, signed_at: u64) -> serde_json::Value {
    let sk = signing_key_from_hex(&kf.private_key).unwrap();
    let body = build_default_mandate(task_id, None, 3600, 100_000, 50, signed_at);
    let env = sign_mandate(&body, &sk).unwrap();
    let mut payload = serde_json::json!({
        "content": "do work",
        "task_id": task_id,
        "_source": {"chat_id": "web:g1"},
    });
    payload["mandate"] = serde_json::to_value(&env).unwrap();
    payload
}

fn verify_policy(ws: &std::path::Path) -> MandatePolicy {
    MandatePolicy {
        workspace: ws.to_path_buf(),
        mode: crate::config_loader::MandateMode::Verify,
    }
}

fn gate_meta() -> Option<RpcMeta> {
    Some(RpcMeta {
        from: Some("node-A".into()),
    })
}

#[tokio::test]
async fn mandate_valid_accepted_with_enforcement_projection() {
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-ok");
    mandate_pin_forced(tmp.path(), "node-A", &kf.public_key).unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    let payload = signed_payload(&kf, "gate-ok-1", now_unix());
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "accepted", "合法 mandate 必须放行");

    let task = tl.get_task("gate-ok-1").expect("放行任务必须入队");
    let enf = task.mandate.expect("enforcement 必须随任务投影");
    assert_eq!(enf.task_id, "gate-ok-1");
    assert_eq!(enf.tool_allowlist, None, "None = 不限语义投影");
    assert_eq!(enf.max_tokens, 100_000);
    assert_eq!(enf.max_tool_calls, 50);
    assert!(enf.expires_at > now_unix(), "窗口终界在未来");
}

#[tokio::test]
async fn mandate_tampered_rejected_and_not_enqueued() {
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-tamper");
    mandate_pin_forced(tmp.path(), "node-A", &kf.public_key).unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    let mut payload = signed_payload(&kf, "gate-tamper-1", now_unix());
    // 篡改 budget（扩权攻击形态）——canonical 摘要不匹配
    payload["mandate"]["budget"]["max_tokens"] = serde_json::json!(999_999_999);
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "error", "篡改 mandate 必须 error ACK");
    assert_eq!(ack.task_id, "gate-tamper-1");
    assert!(
        ack.error
            .as_deref()
            .unwrap_or("")
            .starts_with("mandate 拒绝:"),
        "拒绝文案必须带 mandate 拒绝: 前缀，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-tamper-1").is_none(), "拒单任务不得入队");
}

#[tokio::test]
async fn mandate_missing_verify_passes_old_behavior() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    // 无 mandate（旧 A 端形态）→ verify 放行 = 旧行为
    let ack = handler.handle(
        serde_json::json!({"content": "old peer", "task_id": "gate-miss-1"}),
        gate_meta(),
    );
    assert_eq!(ack.status, "accepted", "verify 模式缺失 mandate 放行");
    let task = tl.get_task("gate-miss-1").expect("放行任务必须入队");
    assert!(task.mandate.is_none(), "无 mandate = enforcement None");
    // mandate: null 同缺失语义
    let ack = handler.handle(
        serde_json::json!({"content": "old peer", "task_id": "gate-miss-2", "mandate": null}),
        gate_meta(),
    );
    assert_eq!(ack.status, "accepted", "null mandate 同缺失语义放行");
}

#[tokio::test]
async fn mandate_missing_require_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(MandatePolicy {
        workspace: tmp.path().to_path_buf(),
        mode: crate::config_loader::MandateMode::Require,
    });

    // require = 防剥离攻击：缺失也拒
    let ack = handler.handle(
        serde_json::json!({"content": "stripped?", "task_id": "gate-req-1"}),
        gate_meta(),
    );
    assert_eq!(ack.status, "error", "require 模式缺失 mandate 必须拒");
    assert!(
        ack.error.as_deref().unwrap_or("").contains("缺失"),
        "拒绝文案应说明缺失，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-req-1").is_none(), "拒单任务不得入队");
}

#[tokio::test]
async fn mandate_mode_off_ignores_even_garbage() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(MandatePolicy {
        workspace: tmp.path().to_path_buf(),
        mode: crate::config_loader::MandateMode::Off,
    });

    // off = 回滚开关：garbage mandate 也零触碰放行
    let ack = handler.handle(
        serde_json::json!({"content": "x", "task_id": "gate-off-1", "mandate": "garbage"}),
        gate_meta(),
    );
    assert_eq!(ack.status, "accepted", "off 模式对 mandate 内容零校验");
    let task = tl.get_task("gate-off-1").expect("放行任务必须入队");
    assert!(task.mandate.is_none(), "off 模式不投影 enforcement");
}

#[tokio::test]
async fn mandate_policy_absent_passes_with_garbage() {
    // 旧装配兼容：策略未装配（None）= 全放行——带 garbage mandate 的载荷
    // 不验（验收面没有 workspace 钉存钥就无从验起，宁放行不误伤）
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));

    let ack = handler.handle(
        serde_json::json!({"content": "x", "task_id": "gate-none-1", "mandate": "garbage"}),
        gate_meta(),
    );
    assert_eq!(ack.status, "accepted", "未装配策略 = 旧行为全放行");
    assert!(tl.get_task("gate-none-1").is_some());
}

#[tokio::test]
async fn mandate_expired_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-exp");
    mandate_pin_forced(tmp.path(), "node-A", &kf.public_key).unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    // 重放过期授权：signed_at = now-7200 → expires_at = now-3600（已过期）
    let now = now_unix();
    let payload = signed_payload(&kf, "gate-exp-1", now.saturating_sub(7200));
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "error", "过期 mandate 必须拒");
    assert!(
        ack.error.as_deref().unwrap_or("").contains("已过期"),
        "拒绝文案应说明过期，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-exp-1").is_none(), "拒单任务不得入队");
}

#[tokio::test]
async fn mandate_task_id_mismatch_rejected() {
    // 移植攻击：mandate 签给任务 1，贴到任务 2 的载荷
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-bind");
    mandate_pin_forced(tmp.path(), "node-A", &kf.public_key).unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    let mut payload = signed_payload(&kf, "gate-bind-1", now_unix());
    payload["task_id"] = serde_json::json!("gate-bind-2");
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "error", "移植 mandate 必须拒");
    assert!(
        ack.error.as_deref().unwrap_or("").contains("绑定不匹配"),
        "拒绝文案应说明绑宝不匹配，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-bind-2").is_none());
    assert!(tl.get_task("gate-bind-1").is_none());
}

#[tokio::test]
async fn mandate_no_pinned_key_verify_degrades_to_accept() {
    // CI UAT T-MRG verify 模式 + 簿未激活（
    // BookAbsent 纪律——announce 不建簿，激活唯一路径 = 管理面）= regime
    // 休眠 → 放行旧行为（默认形态承诺）。旧断言「必须拒」在
    // 默认形态下构成派发死锁：全新 4 节点集群 A 必签（sign_enabled 缺省
    // true）+ B 必验 + 钉存永不激活 = T-MRG-1/2/3/4/7 五测连红全滞留
    // backlog。管理面激活（mandate-pin-forced/trust-add）+ announce 重钉
    // 后，下一单起自动恢复全链验签。
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-nopin");
    // 故意不 pin（簿不存在 = 全新集群缺省形态）
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(verify_policy(tmp.path()));

    let payload = signed_payload(&kf, "gate-nopin-1", now_unix());
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(
        ack.status, "accepted",
        "verify + 簿未激活 = 放行（regime 休眠降级）"
    );
    assert!(tl.get_task("gate-nopin-1").is_some(), "放行即正常入队");
}

#[tokio::test]
async fn mandate_no_pinned_key_require_rejected() {
    // require = 显式 opt-in 严格态：没得验也拒（降级臂只属于 verify）。
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-req-nopin");
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(MandatePolicy {
        workspace: tmp.path().to_path_buf(),
        mode: crate::config_loader::MandateMode::Require,
    });

    let payload = signed_payload(&kf, "gate-req-nopin-1", now_unix());
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "error", "require + 无钉存钥必须拒");
    assert!(
        ack.error.as_deref().unwrap_or("").contains("无钉存公钥"),
        "拒绝文案应说明无钉存钥，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-req-nopin-1").is_none(), "拒单不入队");
}

#[tokio::test]
async fn mandate_corrupt_book_fail_closed() {
    // trust.toml 在场但损坏 = regime 已激活而验签基础
    // 设施失效 → verify 模式也不降级（拒，ACK 文案指明簿损坏）——降级只
    // 给簿缺席（regime 休眠）；坏簿静默放行 = 已激活验签面被无声降级。
    let tmp = tempfile::tempdir().unwrap();
    let kf = make_gate_key("gate-corrupt-book");
    let book = crate::roaming::trust::trust_book_path(tmp.path());
    std::fs::create_dir_all(book.parent().unwrap()).unwrap();
    std::fs::write(&book, "{{ not toml").unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    handler.set_mandate_policy(MandatePolicy {
        workspace: tmp.path().to_path_buf(),
        mode: crate::config_loader::MandateMode::Verify,
    });

    let payload = signed_payload(&kf, "gate-corrupt-book-1", now_unix());
    let ack = handler.handle(payload, gate_meta());
    assert_eq!(ack.status, "error", "verify + 坏簿 = 拒（fail-closed）");
    assert!(
        ack.error.as_deref().unwrap_or("").contains("trust.toml"),
        "拒绝文案应指明信任簿损坏，got: {:?}",
        ack.error
    );
    assert!(tl.get_task("gate-corrupt-book-1").is_none(), "拒单不入队");
}

// ---------------------------------------------------------------------------
// BUG-20261114-024：B 端入口重复投递闸（防重复执行防线）
// ---------------------------------------------------------------------------

fn dup_payload() -> serde_json::Value {
    serde_json::json!({
        "content": "retry copy",
        "type": "chat",
        "task_id": "dup-1",
    })
}

fn inflight_task() -> ClusterTask {
    ClusterTask {
        task_id: "dup-1".into(),
        source: TaskSource {
            node_id: "node-a".into(),
            rpc_address: String::new(),
            session_key: "cluster_rpc:node-a/chat-1".into(),
        },
        status: TaskStatus::Running,
        content: "original".into(),
        conversation: None,
        waiting_for_task_id: None,
        waiting_tool_call_id: None,
        callback_result: None,
        required_profession: None,
        async_usage_stash: None,
        mandate: None,
        dispatch_generation: None,
    }
}

#[tokio::test]
async fn duplicate_inflight_task_id_rejected_without_side_effects() {
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    let wq = Arc::new(ClusterWorkQueue::new(64));
    handler.set_cluster_queue(tl.clone(), wq.clone());
    tl.create_task(inflight_task());

    let ack = handler.handle(dup_payload(), None);
    assert_eq!(ack.status, "error", "同 task_id 在途 = 拒绝");
    assert!(
        ack.error
            .as_deref()
            .unwrap_or("")
            .contains("duplicate task_id"),
        "拒绝文案应点明重复 task_id，got: {:?}",
        ack.error
    );
    // 零副作用 1：在途条目未被静默覆盖。
    let existing = tl.get_task("dup-1").expect("在途条目仍在");
    assert_eq!(existing.content, "original", "条目未被重发内容覆盖");
    assert_eq!(existing.status, TaskStatus::Running);
    // 零副作用 2：队列未二次入队。
    let drained = tokio::time::timeout(Duration::from_millis(100), wq.next()).await;
    assert!(drained.is_err(), "拒绝路径不得向工作队列入队");
}

#[tokio::test]
async fn duplicate_task_id_after_completion_accepted_fresh() {
    // 终态条目已随 complete_task 移除 → 同 id 重发按新任务受理
    //（闸只拦在途三态，宁可放行不可误伤合法重发）。
    let tmp = tempfile::tempdir().unwrap();
    let mut handler = PeerChatHandler::new("node-b".into());
    let tl = Arc::new(ClusterTaskList::new(tmp.path().join("tasks")));
    handler.set_cluster_queue(tl.clone(), Arc::new(ClusterWorkQueue::new(64)));
    tl.create_task(inflight_task());
    tl.complete_task("dup-1");
    assert!(tl.get_task("dup-1").is_none(), "终态条目已移除");

    let ack = handler.handle(dup_payload(), None);
    assert_eq!(ack.status, "accepted", "完成后同 id = 新任务，放行");
    let fresh = tl.get_task("dup-1").expect("新任务入册");
    assert_eq!(fresh.content, "retry copy");
    assert_eq!(fresh.status, TaskStatus::Pending);
}
