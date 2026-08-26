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
        "task-1",
        "success",
        "response",
        "",
    )
    .await;

    let _ = _rx;
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
///   - chat_id        ← payload._source.chat_id (fallback "default")
///   - session_key    ← "cluster_rpc:{source_node_id}/{chat_id}"
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
// W3b coverage batch: setter bodies (set_llm_channel /
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
        self.results
            .lock()
            .unwrap()
            .push((task_id.into(), status.into(), response.into(), error.into()));
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
    let ok = send_callback(Some(&client), "origin-node", "w3b-cb-1", "success", "done-work", "").await;
    assert!(ok, "callback should succeed against the live server");
    {
        let cap = captured.lock().unwrap();
        assert_eq!(cap.len(), 1);
        assert_eq!(cap[0]["task_id"], "w3b-cb-1");
        assert_eq!(cap[0]["status"], "success");
        assert!(cap[0].get("error").is_none(), "error field omitted when empty");
    }

    // Non-empty error → payload carries it
    let ok2 =
        send_callback(Some(&client), "origin-node", "w3b-cb-2", "error", "", "boom-detail").await;
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
    let ok = send_callback(Some(&client), "ghost-node", "w3b-t-ex", "success", "r", "").await;
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
        "w3b-ds-1",
        "success",
        "answer",
        "",
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
// S4 coverage: empty-content ack, persist_result skip arm with a
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
        "s4-del-1",
        "success",
        "answer",
        "",
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
        "s4-persist-1",
        "failed",
        "",
        "boom",
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
    let ok = send_callback(Some(&client), "ghost-node", "s4-retry-1", "success", "r", "").await;
    assert!(!ok, "all callback retries should be exhausted");
}
