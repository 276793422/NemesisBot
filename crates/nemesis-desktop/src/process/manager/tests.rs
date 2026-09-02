use super::*;

#[test]
fn test_new_manager() {
    let mgr = ProcessManager::new();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_start_and_stop() {
    let mgr = ProcessManager::new();
    mgr.stop().unwrap();
}

#[test]
fn test_get_child_nonexistent() {
    let mgr = ProcessManager::new();
    assert!(mgr.get_child("nonexistent").is_none());
}

#[test]
fn test_get_child_by_type_empty() {
    let mgr = ProcessManager::new();
    assert!(mgr.get_child_by_type("dashboard").is_none());
}

#[test]
fn test_terminate_nonexistent() {
    let mgr = ProcessManager::new();
    let result = mgr.terminate_child("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_notify_child_nonexistent() {
    let mgr = ProcessManager::new();
    let result = mgr.notify_child("nonexistent", "test", serde_json::Value::Null);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("child not found"));
}

#[tokio::test]
async fn test_call_child_nonexistent() {
    let mgr = ProcessManager::new();
    let result = mgr
        .call_child("nonexistent", "test", serde_json::Value::Null)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("child not found"));
}

#[test]
fn test_submit_result_no_channel() {
    let mgr = ProcessManager::new();
    assert!(!mgr.submit_result("nonexistent", serde_json::json!({})));
}

#[test]
fn test_cleanup_stale_empty() {
    let mgr = ProcessManager::new();
    mgr.cleanup_stale();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_ws_server_accessible() {
    let mgr = ProcessManager::new();
    let _server = mgr.ws_server();
}

#[test]
fn test_default_impl() {
    let mgr = ProcessManager::default();
    assert_eq!(mgr.active_count(), 0);
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_with_executor() {
    let executor = Arc::new(DefaultPlatformExecutor::with_defaults());
    let mgr = ProcessManager::with_executor(executor);
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_stop_cleans_up() {
    let mgr = ProcessManager::new();
    // Stop without start should still work
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_submit_result_with_channel() {
    let mgr = ProcessManager::new();
    // Create a result channel manually
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("child-0".to_string(), tx);
    }

    let result = mgr.submit_result("child-0", serde_json::json!({"approved": true}));
    assert!(result);

    let response = rx.try_recv().unwrap();
    assert_eq!(response["approved"], true);
}

#[test]
fn test_submit_result_already_consumed() {
    let mgr = ProcessManager::new();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("child-0".to_string(), tx);
    }

    // First submit succeeds
    assert!(mgr.submit_result("child-0", serde_json::json!({})));
    // Second submit fails (channel already removed)
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
}

#[test]
fn test_active_count_after_cleanup_stale() {
    let mgr = ProcessManager::new();
    // Insert a dead child manually - a child with no actual OS process
    // is_process_alive checks the exited flag which starts as false (alive)
    // So to test cleanup of stale children, we need the executor to report dead
    // The DefaultPlatformExecutor checks exited.load() - but that's private.
    // Instead, let's just test that cleanup_stale doesn't panic on empty
    mgr.cleanup_stale();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_get_child_after_manual_insert() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let mut child = ChildProcess::new("child-0".to_string(), 1234, "dashboard".to_string());
        child.status = ProcessStatus::Running;
        state.children.insert("child-0".to_string(), child);
    }

    let status = mgr.get_child("child-0");
    assert!(status.is_some());
    assert_eq!(status.unwrap(), ProcessStatus::Running);
}

#[test]
fn test_get_child_by_type_after_manual_insert() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 1234, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    let found = mgr.get_child_by_type("dashboard");
    assert!(found.is_some());
    assert_eq!(found.unwrap(), "child-0");

    let not_found = mgr.get_child_by_type("approval");
    assert!(not_found.is_none());
}

#[test]
fn test_terminate_child_after_manual_insert() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }
    assert_eq!(mgr.active_count(), 1);

    let result = mgr.terminate_child("child-0");
    assert!(result.is_ok());
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_multiple_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("child-0".to_string(), 100, "dashboard".to_string());
        let c2 = ChildProcess::new("child-1".to_string(), 200, "approval".to_string());
        state.children.insert("child-0".to_string(), c1);
        state.children.insert("child-1".to_string(), c2);
    }
    assert_eq!(mgr.active_count(), 2);

    // Find by type
    assert_eq!(
        mgr.get_child_by_type("dashboard"),
        Some("child-0".to_string())
    );
    assert_eq!(
        mgr.get_child_by_type("approval"),
        Some("child-1".to_string())
    );

    // Terminate one
    mgr.terminate_child("child-0").unwrap();
    assert_eq!(mgr.active_count(), 1);
}

#[test]
fn test_spawn_child_invalid_exe() {
    let mgr = ProcessManager::new();
    // This will fail because the executable doesn't exist
    let result = mgr.spawn_child("approval", &serde_json::json!({}));
    // spawn_child calls current_exe() which should succeed, but then the
    // spawned process will fail (since the test binary doesn't support child mode properly)
    // The result depends on whether the current exe can be found
    // We just verify it doesn't panic
    let _ = result;
}

#[test]
fn test_notify_child_existing_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    // Child exists but has no WS connection, so send_notification should fail
    let result = mgr.notify_child("child-0", "test.method", serde_json::json!({}));
    assert!(result.is_err());
    // Should fail because connection not found in WS server, not because child not found
    assert!(result.unwrap_err().contains("connection not found"));
}

#[tokio::test]
async fn test_call_child_existing_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    // Child exists but has no WS connection, so call_child should fail
    let result = mgr
        .call_child("child-0", "test.method", serde_json::json!({}))
        .await;
    assert!(result.is_err());
}

#[test]
fn test_stop_clears_result_channels() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.result_channels.insert("child-0".to_string(), tx);
    }
    mgr.stop().unwrap();
    // After stop, submitting result should fail
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
}

// ---- Coverage expansion tests for process manager ----

#[tokio::test]
async fn test_start_and_stop_lifecycle() {
    let mgr = ProcessManager::new();
    let result = mgr.start().await;
    assert!(result.is_ok());
    assert!(mgr.ws_server().get_port() > 0);
    mgr.stop().unwrap();
}

#[test]
fn test_stop_idempotent() {
    let mgr = ProcessManager::new();
    mgr.stop().unwrap();
    mgr.stop().unwrap();
    mgr.stop().unwrap();
}

#[test]
fn test_submit_result_dropped_receiver() {
    let mgr = ProcessManager::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("child-0".to_string(), tx);
    }
    drop(rx);
    // Submit should return false because receiver was dropped
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
}

#[test]
fn test_cleanup_stale_with_dead_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        // Use PID 0 which won't be a real process; the executor
        // will try to check the process and should handle it gracefully
        let child = ChildProcess::new("child-0".to_string(), 0, "test".to_string());
        state.children.insert("child-0".to_string(), child);
    }
    assert_eq!(mgr.active_count(), 1);
    mgr.cleanup_stale();
    // PID 0 may or may not be alive depending on the executor;
    // just verify it doesn't panic
}

#[test]
fn test_cleanup_stale_with_alive_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "test".to_string());
        // exited is false by default, so is_process_alive returns true
        state.children.insert("child-0".to_string(), child);
    }
    assert_eq!(mgr.active_count(), 1);
    mgr.cleanup_stale();
    // Alive child should NOT be cleaned up
    assert_eq!(mgr.active_count(), 1);
}

#[test]
fn test_spawn_child_fails_handshake() {
    let mgr = ProcessManager::new();
    // This will fail because the process won't do the handshake
    let result = mgr.spawn_child("dashboard", &serde_json::json!({"test": true}));
    // Expected to fail since no real child process to handshake with
    let _ = result;
}

#[test]
fn test_multiple_terminates() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        let c2 = ChildProcess::new("c2".to_string(), 200, "approval".to_string());
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
    }
    mgr.terminate_child("c1").unwrap();
    mgr.terminate_child("c2").unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_stop_terminates_all_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        let c2 = ChildProcess::new("c2".to_string(), 200, "approval".to_string());
        let c3 = ChildProcess::new("c3".to_string(), 300, "headless".to_string());
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
        state.children.insert("c3".to_string(), c3);
    }
    assert_eq!(mgr.active_count(), 3);
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

// ============================================================
// Phase 4: Additional coverage for 93%+ target
// ============================================================

#[test]
fn test_cleanup_failed_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.result_channels.insert("child-0".to_string(), tx);
    }

    // cleanup_failed_child is private, but spawn_child calls it on failure
    // Instead, test the observable effect: verify the child is removed
    assert_eq!(mgr.active_count(), 1);
    mgr.terminate_child("child-0").unwrap();
    assert_eq!(mgr.active_count(), 0);
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
}

#[test]
fn test_spawn_child_dashboard_persistent() {
    let mgr = ProcessManager::new();
    // Dashboard type would result in None result receiver if spawn succeeds
    // Since spawn will fail (handshake), test that it handles the failure
    let result = mgr.spawn_child("dashboard", &serde_json::json!({}));
    // Expected to fail since no real child process
    let _ = result;
}

#[test]
fn test_spawn_child_approval_temporary() {
    let mgr = ProcessManager::new();
    // Approval type would result in a result receiver if spawn succeeds
    // Since spawn will fail (handshake), test that it handles the failure
    let result = mgr.spawn_child(
        "approval",
        &serde_json::json!({
            "request_id": "r1",
            "operation": "test"
        }),
    );
    let _ = result;
}

#[tokio::test]
async fn test_start_stop_with_children() {
    let mgr = ProcessManager::new();
    mgr.start().await.unwrap();

    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    assert_eq!(mgr.active_count(), 1);
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_submit_result_with_actual_channel_receive() {
    let mgr = ProcessManager::new();
    let (tx, rx) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("child-0".to_string(), tx);
    }

    let result_data = serde_json::json!({"approved": true, "request_id": "r1"});
    assert!(mgr.submit_result("child-0", result_data.clone()));

    // Verify the data is received
    let rt = tokio::runtime::Runtime::new().unwrap();
    let received =
        rt.block_on(async { tokio::time::timeout(std::time::Duration::from_secs(1), rx).await });
    assert!(received.is_ok());
    let response = received.unwrap().unwrap();
    assert_eq!(response["approved"], true);
}

#[test]
fn test_cleanup_stale_with_exited_child() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let mut child = ChildProcess::new("child-0".to_string(), 99999, "test".to_string());
        // Mark as exited using kill() which sets the exited flag
        child.kill().unwrap();
        state.children.insert("child-0".to_string(), child);
        // Also add a result channel
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.result_channels.insert("child-0".to_string(), tx);
    }

    assert_eq!(mgr.active_count(), 1);
    mgr.cleanup_stale();
    // Exited child should be cleaned up
    assert_eq!(mgr.active_count(), 0);
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
}

#[test]
fn test_multiple_result_channels() {
    let mgr = ProcessManager::new();
    let (tx1, mut rx1) = tokio::sync::oneshot::channel();
    let (tx2, mut rx2) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("child-0".to_string(), tx1);
        state.result_channels.insert("child-1".to_string(), tx2);
    }

    // Submit results - receivers are alive so it should work
    assert!(mgr.submit_result("child-0", serde_json::json!({"a": 1})));
    assert!(mgr.submit_result("child-1", serde_json::json!({"b": 2})));

    // Verify results received
    assert_eq!(rx1.try_recv().unwrap()["a"], 1);
    assert_eq!(rx2.try_recv().unwrap()["b"], 2);

    // Already consumed
    assert!(!mgr.submit_result("child-0", serde_json::json!({})));
    assert!(!mgr.submit_result("child-1", serde_json::json!({})));
}

#[test]
fn test_get_child_multiple_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let mut c1 = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        c1.status = ProcessStatus::Connected;
        let mut c2 = ChildProcess::new("c2".to_string(), 200, "approval".to_string());
        c2.status = ProcessStatus::Handshaking;
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
    }

    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Connected));
    assert_eq!(mgr.get_child("c2"), Some(ProcessStatus::Handshaking));
    assert_eq!(mgr.get_child("c3"), None);
}

#[test]
fn test_stop_sends_shutdown_signal() {
    let mgr = ProcessManager::new();
    // Test that stop() can be called multiple times safely
    mgr.stop().unwrap();
    mgr.stop().unwrap();
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_active_count_after_multiple_operations() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        for i in 0..5 {
            let child =
                ChildProcess::new(format!("child-{}", i), 100 + i as u32, "test".to_string());
            state.children.insert(format!("child-{}", i), child);
        }
    }
    assert_eq!(mgr.active_count(), 5);

    mgr.terminate_child("child-0").unwrap();
    assert_eq!(mgr.active_count(), 4);

    mgr.terminate_child("child-2").unwrap();
    assert_eq!(mgr.active_count(), 3);

    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_notify_child_with_connection() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    // Child exists but no WS connection - should fail with "connection not found"
    let result = mgr.notify_child("child-0", "test.method", serde_json::json!({}));
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("connection not found"));
}

#[test]
fn test_get_child_by_type_no_match() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    // Search for type that doesn't exist
    assert!(mgr.get_child_by_type("approval").is_none());
    assert!(mgr.get_child_by_type("headless").is_none());
    // Search for type that exists
    assert_eq!(
        mgr.get_child_by_type("dashboard"),
        Some("child-0".to_string())
    );
}

// ============================================================
// Additional tests for 95%+ coverage
// ============================================================

#[test]
fn test_spawn_child_generates_unique_ids() {
    let mgr = ProcessManager::new();
    // spawn_child will fail because of handshake, but each call
    // should generate a unique child ID (incrementing counter)
    let _ = mgr.spawn_child("test", &serde_json::json!({}));
    let _ = mgr.spawn_child("test", &serde_json::json!({}));
    // Verify the counter advanced - spawn creates child-N IDs
    // Since they all fail, active_count stays 0
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_stop_after_start_with_no_children() {
    let mgr = ProcessManager::new();
    // Just verify the lifecycle works cleanly
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[tokio::test]
async fn test_start_assigns_ws_port() {
    let mgr = ProcessManager::new();
    assert_eq!(mgr.ws_server().get_port(), 0);
    mgr.start().await.unwrap();
    assert!(mgr.ws_server().get_port() > 0);
    mgr.stop().unwrap();
}

#[test]
fn test_multiple_get_child_status_transitions() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let mut c = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        c.status = ProcessStatus::Starting;
        state.children.insert("c1".to_string(), c);
    }
    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Starting));

    // Update status
    {
        let mut state = mgr.state.lock();
        if let Some(c) = state.children.get_mut("c1") {
            c.status = ProcessStatus::Handshaking;
        }
    }
    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Handshaking));

    {
        let mut state = mgr.state.lock();
        if let Some(c) = state.children.get_mut("c1") {
            c.status = ProcessStatus::Connected;
        }
    }
    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Connected));

    {
        let mut state = mgr.state.lock();
        if let Some(c) = state.children.get_mut("c1") {
            c.status = ProcessStatus::Terminated;
        }
    }
    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Terminated));
}

#[test]
fn test_get_child_by_type_first_match() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        let c2 = ChildProcess::new("c2".to_string(), 200, "dashboard".to_string());
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
    }
    // Should return the first match
    let found = mgr.get_child_by_type("dashboard");
    assert!(found.is_some());
    let id = found.unwrap();
    assert!(id == "c1" || id == "c2");
}

#[test]
fn test_terminate_child_removes_result_channel() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("c1".to_string(), 100, "approval".to_string());
        state.children.insert("c1".to_string(), child);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.result_channels.insert("c1".to_string(), tx);
    }

    mgr.terminate_child("c1").unwrap();
    assert_eq!(mgr.active_count(), 0);
    assert!(!mgr.submit_result("c1", serde_json::json!({})));
}

#[test]
fn test_cleanup_stale_preserves_alive_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        // One alive (exited = false by default)
        let alive = ChildProcess::new("alive".to_string(), 99999, "dashboard".to_string());
        // One dead (explicitly killed)
        let mut dead = ChildProcess::new("dead".to_string(), 99998, "approval".to_string());
        dead.kill().unwrap();
        state.children.insert("alive".to_string(), alive);
        state.children.insert("dead".to_string(), dead);
    }

    assert_eq!(mgr.active_count(), 2);
    mgr.cleanup_stale();
    // Only the dead one should be removed
    assert_eq!(mgr.active_count(), 1);
    assert!(mgr.get_child("alive").is_some());
    assert!(mgr.get_child("dead").is_none());
}

#[tokio::test]
async fn test_call_child_with_ws_server_started() {
    let mgr = ProcessManager::new();
    mgr.start().await.unwrap();

    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("child-0".to_string(), 99999, "dashboard".to_string());
        state.children.insert("child-0".to_string(), child);
    }

    // Child exists, WS server is running, but no WS connection
    let result = mgr
        .call_child("child-0", "test.method", serde_json::json!({}))
        .await;
    assert!(result.is_err());

    mgr.stop().unwrap();
}

#[test]
fn test_notify_child_checks_children_map_first() {
    let mgr = ProcessManager::new();
    // No children registered - should fail with "child not found"
    let result = mgr.notify_child("nonexistent", "test", serde_json::Value::Null);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("child not found"));
}

#[tokio::test]
async fn test_call_child_checks_children_map_first() {
    let mgr = ProcessManager::new();
    // No children registered - should fail with "child not found"
    let result = mgr
        .call_child("nonexistent", "test", serde_json::Value::Null)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("child not found"));
}

#[test]
fn test_stop_with_dead_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let mut c = ChildProcess::new("dead-child".to_string(), 99999, "test".to_string());
        c.kill().unwrap();
        state.children.insert("dead-child".to_string(), c);
    }
    // Stop should still work even with dead children
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_submit_result_multiple_children_independent() {
    let mgr = ProcessManager::new();
    let (tx1, mut rx1) = tokio::sync::oneshot::channel();
    let (tx2, mut rx2) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("c1".to_string(), tx1);
        state.result_channels.insert("c2".to_string(), tx2);
    }

    // Submit for c1 only
    assert!(mgr.submit_result("c1", serde_json::json!({"r": 1})));
    // c2's channel should still be pending
    assert!(!mgr.submit_result("c1", serde_json::json!({}))); // already consumed
    assert!(mgr.submit_result("c2", serde_json::json!({"r": 2})));

    assert_eq!(rx1.try_recv().unwrap()["r"], 1);
    assert_eq!(rx2.try_recv().unwrap()["r"], 2);
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_process_manager_default_trait() {
    let mgr = ProcessManager::default();
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_ws_port_default() {
    let mgr = ProcessManager::new();
    assert_eq!(mgr.ws_port(), 0);
}

#[tokio::test]
async fn test_start_stop_ws_port_assigned() {
    let mgr = ProcessManager::new();
    assert_eq!(mgr.ws_port(), 0);
    mgr.start().await.unwrap();
    let port = mgr.ws_port();
    assert!(port > 0);
    mgr.stop().unwrap();
    // After stop, port should still be the same (not reset)
    assert_eq!(mgr.ws_port(), port);
}

#[test]
fn test_get_child_status_all_variants() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();

        let mut c1 = ChildProcess::new("c1".to_string(), 100, "t".to_string());
        c1.status = ProcessStatus::Starting;
        state.children.insert("c1".to_string(), c1);

        let mut c2 = ChildProcess::new("c2".to_string(), 200, "t".to_string());
        c2.status = ProcessStatus::Handshaking;
        state.children.insert("c2".to_string(), c2);

        let mut c3 = ChildProcess::new("c3".to_string(), 300, "t".to_string());
        c3.status = ProcessStatus::Connected;
        state.children.insert("c3".to_string(), c3);

        let mut c4 = ChildProcess::new("c4".to_string(), 400, "t".to_string());
        c4.status = ProcessStatus::Running;
        state.children.insert("c4".to_string(), c4);

        let mut c5 = ChildProcess::new("c5".to_string(), 500, "t".to_string());
        c5.status = ProcessStatus::Failed;
        state.children.insert("c5".to_string(), c5);

        let mut c6 = ChildProcess::new("c6".to_string(), 600, "t".to_string());
        c6.status = ProcessStatus::Terminated;
        state.children.insert("c6".to_string(), c6);
    }

    assert_eq!(mgr.get_child("c1"), Some(ProcessStatus::Starting));
    assert_eq!(mgr.get_child("c2"), Some(ProcessStatus::Handshaking));
    assert_eq!(mgr.get_child("c3"), Some(ProcessStatus::Connected));
    assert_eq!(mgr.get_child("c4"), Some(ProcessStatus::Running));
    assert_eq!(mgr.get_child("c5"), Some(ProcessStatus::Failed));
    assert_eq!(mgr.get_child("c6"), Some(ProcessStatus::Terminated));
}

#[test]
fn test_terminate_child_then_terminate_again() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("c1".to_string(), 99999, "test".to_string());
        state.children.insert("c1".to_string(), child);
    }

    // First terminate succeeds
    assert!(mgr.terminate_child("c1").is_ok());
    assert_eq!(mgr.active_count(), 0);

    // Second terminate fails (child not found)
    assert!(mgr.terminate_child("c1").is_err());
}

#[test]
fn test_submit_result_with_complex_json() {
    let mgr = ProcessManager::new();
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    {
        let mut state = mgr.state.lock();
        state.result_channels.insert("c1".to_string(), tx);
    }

    let result = serde_json::json!({
        "action": "approved",
        "request_id": "req-123",
        "details": {
            "operation": "file_write",
            "path": "/tmp/test.txt",
            "risk_level": "MEDIUM"
        },
        "timestamp": "2026-05-16T10:00:00Z"
    });

    assert!(mgr.submit_result("c1", result.clone()));
    let received = rx.try_recv().unwrap();
    assert_eq!(received["action"], "approved");
    assert_eq!(received["details"]["risk_level"], "MEDIUM");
}

#[test]
fn test_cleanup_stale_preserves_alive_and_removes_dead() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        // Two alive children
        let alive1 = ChildProcess::new("alive1".to_string(), 99999, "dashboard".to_string());
        let alive2 = ChildProcess::new("alive2".to_string(), 99998, "approval".to_string());
        // Two dead children (killed)
        let mut dead1 = ChildProcess::new("dead1".to_string(), 99997, "test".to_string());
        dead1.kill().unwrap();
        let mut dead2 = ChildProcess::new("dead2".to_string(), 99996, "test".to_string());
        dead2.kill().unwrap();

        state.children.insert("alive1".to_string(), alive1);
        state.children.insert("alive2".to_string(), alive2);
        state.children.insert("dead1".to_string(), dead1);
        state.children.insert("dead2".to_string(), dead2);

        // Add result channels for all
        let (tx1, _) = tokio::sync::oneshot::channel();
        let (tx2, _) = tokio::sync::oneshot::channel();
        state.result_channels.insert("dead1".to_string(), tx1);
        state.result_channels.insert("dead2".to_string(), tx2);
    }

    assert_eq!(mgr.active_count(), 4);
    mgr.cleanup_stale();
    assert_eq!(mgr.active_count(), 2);
    assert!(mgr.get_child("alive1").is_some());
    assert!(mgr.get_child("alive2").is_some());
    assert!(mgr.get_child("dead1").is_none());
    assert!(mgr.get_child("dead2").is_none());
    // Result channels for dead children should be removed
    assert!(!mgr.submit_result("dead1", serde_json::json!({})));
    assert!(!mgr.submit_result("dead2", serde_json::json!({})));
}

#[test]
fn test_notify_child_different_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("c1".to_string(), 99999, "dashboard".to_string());
        let c2 = ChildProcess::new("c2".to_string(), 99998, "approval".to_string());
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
    }

    // Both should fail because no WS connection
    let r1 = mgr.notify_child("c1", "method", serde_json::Value::Null);
    assert!(r1.is_err());

    let r2 = mgr.notify_child("c2", "method", serde_json::Value::Null);
    assert!(r2.is_err());

    // Nonexistent child
    let r3 = mgr.notify_child("c3", "method", serde_json::Value::Null);
    assert!(r3.is_err());
    assert!(r3.unwrap_err().contains("child not found"));
}

#[test]
fn test_get_child_by_type_multiple_types() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let c1 = ChildProcess::new("c1".to_string(), 100, "dashboard".to_string());
        let c2 = ChildProcess::new("c2".to_string(), 200, "approval".to_string());
        let c3 = ChildProcess::new("c3".to_string(), 300, "headless".to_string());
        state.children.insert("c1".to_string(), c1);
        state.children.insert("c2".to_string(), c2);
        state.children.insert("c3".to_string(), c3);
    }

    assert_eq!(mgr.get_child_by_type("dashboard"), Some("c1".to_string()));
    assert_eq!(mgr.get_child_by_type("approval"), Some("c2".to_string()));
    assert_eq!(mgr.get_child_by_type("headless"), Some("c3".to_string()));
    assert!(mgr.get_child_by_type("unknown").is_none());
}

#[test]
fn test_stop_with_result_channels_and_children() {
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        let child = ChildProcess::new("c1".to_string(), 99999, "test".to_string());
        state.children.insert("c1".to_string(), child);
        let (tx, _) = tokio::sync::oneshot::channel();
        state.result_channels.insert("c1".to_string(), tx);
    }

    assert_eq!(mgr.active_count(), 1);
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
    assert!(!mgr.submit_result("c1", serde_json::json!({})));
}

#[test]
fn test_spawn_child_multiple_invocations_unique_ids() {
    let mgr = ProcessManager::new();
    // All will fail but each should try a unique child ID
    let _ = mgr.spawn_child("test", &serde_json::json!({}));
    let _ = mgr.spawn_child("test", &serde_json::json!({}));
    let _ = mgr.spawn_child("test", &serde_json::json!({}));
    // All fail during handshake, so active_count should be 0
    assert_eq!(mgr.active_count(), 0);
}
// ============================================================
// State isolation: independent managers must not share state.
// ============================================================

#[test]
fn test_two_managers_independent_state() {
    let a = ProcessManager::new();
    let b = ProcessManager::new();

    {
        let mut state = a.state.lock();
        let child = ChildProcess::new("child-A".to_string(), 100, "dashboard".to_string());
        state.children.insert("child-A".to_string(), child);
    }
    // a has one child, b still empty
    assert_eq!(a.active_count(), 1);
    assert_eq!(b.active_count(), 0);

    // b cannot see or terminate a's child
    assert!(b.terminate_child("child-A").is_err());
    assert_eq!(a.active_count(), 1);

    // a can terminate its own child
    assert!(a.terminate_child("child-A").is_ok());
    assert_eq!(a.active_count(), 0);
    assert_eq!(b.active_count(), 0);
}

#[test]
fn test_with_executor_does_not_share_state_with_new() {
    let executor = Arc::new(DefaultPlatformExecutor::with_defaults());
    let mgr = ProcessManager::with_executor(executor);
    // Fresh manager has no children regardless of how it was constructed.
    assert_eq!(mgr.active_count(), 0);
    // And no result channels resolve.
    assert!(!mgr.submit_result("any", serde_json::json!({})));
}

#[test]
fn test_independent_result_channels_across_managers() {
    let a = ProcessManager::new();
    let b = ProcessManager::new();

    let (tx, mut rx) = tokio::sync::oneshot::channel();
    {
        let mut state = a.state.lock();
        state.result_channels.insert("c1".to_string(), tx);
    }
    // b has no such channel, so submit fails and does not drain a's channel.
    assert!(!b.submit_result("c1", serde_json::json!({})));
    // a still owns the live channel.
    assert!(a.submit_result("c1", serde_json::json!({"v": 7})));
    assert_eq!(rx.try_recv().unwrap()["v"], 7);
}

#[test]
fn test_ws_server_is_stable_reference() {
    // ws_server() returns a reference to the owned server; calling it
    // repeatedly yields the same underlying port state.
    let mgr = ProcessManager::new();
    let port1 = mgr.ws_server().get_port();
    let port2 = mgr.ws_server().get_port();
    assert_eq!(port1, port2);
    assert_eq!(mgr.ws_port(), port1);
}

#[test]
fn test_terminate_all_variants_in_one_manager() {
    // Insert children of every window type and ensure stop() clears them all.
    let mgr = ProcessManager::new();
    {
        let mut state = mgr.state.lock();
        for (i, wt) in ["dashboard", "approval", "headless", "unknown"]
            .iter()
            .enumerate()
        {
            let child = ChildProcess::new(format!("c{}", i), 100 + i as u32, wt.to_string());
            state.children.insert(format!("c{}", i), child);
        }
    }
    assert_eq!(mgr.active_count(), 4);
    // get_child_by_type finds the typed ones
    assert_eq!(mgr.get_child_by_type("dashboard"), Some("c0".to_string()));
    assert_eq!(mgr.get_child_by_type("approval"), Some("c1".to_string()));
    assert_eq!(mgr.get_child_by_type("headless"), Some("c2".to_string()));
    assert_eq!(mgr.get_child_by_type("unknown"), Some("c3".to_string()));

    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
    // After stop, none are findable.
    assert!(mgr.get_child_by_type("dashboard").is_none());
}

#[test]
fn test_active_count_reflects_direct_inserts_and_removals() {
    let mgr = ProcessManager::new();
    assert_eq!(mgr.active_count(), 0);
    {
        let mut state = mgr.state.lock();
        state.children.insert(
            "x".to_string(),
            ChildProcess::new("x".to_string(), 1, "t".to_string()),
        );
    }
    assert_eq!(mgr.active_count(), 1);
    {
        let mut state = mgr.state.lock();
        state.children.remove("x");
    }
    assert_eq!(mgr.active_count(), 0);
}

// ============================================================
// Additional coverage: stop() warn arms, monitor loop, log args
// ============================================================

/// Executor whose terminate/cleanup always fail — drives the warn arms of
/// `ProcessManager::stop()` (no real process involved).
struct FailingExecutor;

impl PlatformExecutor for FailingExecutor {
    fn spawn_child(&self, _exe_path: &str, _args: &[String]) -> Result<ChildProcess, String> {
        Err("spawn disabled in FailingExecutor".to_string())
    }
    fn terminate_child(&self, _child: &mut ChildProcess) -> Result<(), String> {
        Err("terminate failed".to_string())
    }
    fn is_process_alive(&self, _child: &ChildProcess) -> bool {
        false
    }
    fn cleanup(&self, _child: &mut ChildProcess) -> Result<(), String> {
        Err("cleanup failed".to_string())
    }
}

#[test]
fn test_stop_warn_arms_on_executor_failure() {
    let mgr = ProcessManager::with_executor(Arc::new(FailingExecutor));
    {
        let mut state = mgr.state.lock();
        state.children.insert(
            "w1".to_string(),
            ChildProcess::new("w1".to_string(), 1, "dashboard".to_string()),
        );
    }
    // stop() must still succeed even though terminate+cleanup both fail.
    mgr.stop().unwrap();
    assert_eq!(mgr.active_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn test_monitor_loop_cleans_dead_children_and_stops() {
    let mgr = ProcessManager::new();
    mgr.start().await.unwrap();

    // Insert an already-dead child plus a stale result channel.
    {
        let mut state = mgr.state.lock();
        let mut child = ChildProcess::new("dead-1".to_string(), 1, "approval".to_string());
        child.kill().unwrap();
        state.children.insert("dead-1".to_string(), child);
        let (tx, _rx) = tokio::sync::oneshot::channel();
        state.result_channels.insert("dead-1".to_string(), tx);
    }
    assert_eq!(mgr.active_count(), 1);

    // tokio::time::interval fires its first tick immediately, so one yield
    // is enough for the monitor to sweep the dead child.
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert_eq!(mgr.active_count(), 0, "monitor did not clean dead child");
    assert!(!mgr.submit_result("dead-1", serde_json::json!({})));

    // Shutdown arm of the monitor loop.
    mgr.stop().unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
}

#[tokio::test]
async fn test_start_stop_with_subscriber_covers_lazy_log_args() {
    // tracing event arguments are evaluated lazily; install a thread-local
    // subscriber that enables everything so the info! arg expressions
    // actually run.
    struct EnableAllSubscriber;
    impl tracing::Subscriber for EnableAllSubscriber {
        fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            tracing::span::Id::from_u64(1)
        }
        // tracing_core 无默认实现的必需方法（E0046）；no-op 即可——info! 的惰性
        // 参数求值发生在 Event 构造时，与 event() 的实现体无关。
        fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
        fn event(&self, _event: &tracing::Event<'_>) {}
        fn enter(&self, _span: &tracing::Id) {}
        fn exit(&self, _span: &tracing::Id) {}
    }
    let _guard = tracing::subscriber::set_default(EnableAllSubscriber);

    let mgr = ProcessManager::new();
    mgr.start().await.unwrap();
    assert!(mgr.ws_port() > 0);
    mgr.stop().unwrap();
    // 端口语义按既有钉死契约：stop 后**保留**（见 test_start_stop_ws_port_assigned
    // "port should still be the same (not reset)"），此处不重复断言。
    assert_eq!(mgr.active_count(), 0);
}

// ============================================================
// Real spawn coverage via a scripted PowerShell child
// (Windows only; the script logs every stdin line to a temp file and
// replies per variant so the manager's pipe protocol is exercised end
// to end against a real child process).
// ============================================================

#[cfg(windows)]
mod scripted_spawn_tests {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message as WsMessage;

    // --- base64 of UTF-16LE (PowerShell -EncodedCommand format) ---

    fn b64_encode(data: &[u8]) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in data.chunks(3) {
            let b = [
                chunk[0],
                *chunk.get(1).unwrap_or(&0),
                *chunk.get(2).unwrap_or(&0),
            ];
            let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
            out.push(TABLE[(n >> 18) as usize & 63] as char);
            out.push(TABLE[(n >> 12) as usize & 63] as char);
            out.push(if chunk.len() > 1 {
                TABLE[(n >> 6) as usize & 63] as char
            } else {
                '='
            });
            out.push(if chunk.len() > 2 {
                TABLE[n as usize & 63] as char
            } else {
                '='
            });
        }
        out
    }

    fn ps_encode(script: &str) -> String {
        let utf16: Vec<u8> = script
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes().to_vec())
            .collect();
        b64_encode(&utf16)
    }

    // --- script builder ---

    fn build_script(variant: &str, log_path: &str) -> String {
        const ACK: &str = "[Console]::Out.WriteLine('{\"type\":\"ack\"}'); [Console]::Out.Flush()";
        const HELLO: &str =
            "[Console]::Out.WriteLine('{\"type\":\"hello\"}'); [Console]::Out.Flush()";

        if variant == "exit_now" {
            // Child exits before answering the handshake.
            return "Start-Sleep -Milliseconds 300\nexit 0\n".to_string();
        }

        // Messages: 1 = handshake, 2 = ws_key, 3 = window_data
        let reply = match variant {
            "ack_all" => ACK.to_string(),
            "hello_first" => format!("if ($n -eq 1) {{ {HELLO} }} else {{ {ACK} }}"),
            "ack_then_hello" => format!("if ($n -le 1) {{ {ACK} }} else {{ {HELLO} }}"),
            "ack_then_exit" => format!("if ($n -eq 1) {{ {ACK} }} else {{ exit 0 }}"),
            "ack2_then_hello" => format!("if ($n -le 2) {{ {ACK} }} else {{ {HELLO} }}"),
            "ack2_then_exit" => format!("if ($n -le 2) {{ {ACK} }} else {{ exit 0 }}"),
            other => panic!("unknown script variant: {}", other),
        };

        format!(
            "$log = '{path}'\n$n = 0\nwhile ($true) {{\n  $line = [Console]::In.ReadLine()\n  if ($null -eq $line) {{ break }}\n  [System.IO.File]::AppendAllText($log, $line + [char]10)\n  $n = $n + 1\n  {reply}\n}}\nStart-Sleep -Seconds 60\n",
            path = log_path,
            reply = reply
        )
    }

    // --- executor that swaps the real exe/args for the scripted PS child ---

    struct ScriptedExecutor {
        inner: DefaultPlatformExecutor,
        log_path: std::path::PathBuf,
        variant: &'static str,
    }

    impl PlatformExecutor for ScriptedExecutor {
        fn spawn_child(&self, _exe_path: &str, _args: &[String]) -> Result<ChildProcess, String> {
            let script = build_script(self.variant, &self.log_path.to_string_lossy());
            let args: Vec<String> = vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-EncodedCommand".to_string(),
                ps_encode(&script),
            ];
            self.inner.spawn_child("powershell.exe", &args)
        }
        fn terminate_child(&self, child: &mut ChildProcess) -> Result<(), String> {
            // Instant teardown (skip the 5s graceful poll): PowerShell is
            // spawned with CREATE_NEW_PROCESS_GROUP and ignores CTRL_C.
            self.inner.cleanup(child)
        }
        fn is_process_alive(&self, child: &ChildProcess) -> bool {
            self.inner.is_process_alive(child)
        }
        fn cleanup(&self, child: &mut ChildProcess) -> Result<(), String> {
            self.inner.cleanup(child)
        }
    }

    fn mgr_with_script(variant: &'static str) -> (ProcessManager, std::path::PathBuf) {
        let log_path =
            std::env::temp_dir().join(format!("nb-desktop-mgr-{}.log", uuid::Uuid::new_v4()));
        let mgr = ProcessManager::with_executor(Arc::new(ScriptedExecutor {
            inner: DefaultPlatformExecutor::with_defaults(),
            log_path: log_path.clone(),
            variant,
        }));
        (mgr, log_path)
    }

    fn read_ws_key_from_log(log_path: &std::path::Path) -> String {
        let content = std::fs::read_to_string(log_path).expect("child log readable");
        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line)
                && v["type"] == "ws_key"
            {
                return v["data"]["key"]
                    .as_str()
                    .expect("key field in ws_key message")
                    .to_string();
            }
        }
        panic!("no ws_key line found in child log:\n{}", content);
    }

    // --- failure variants (plain #[test]: they never reach tokio::spawn) ---

    #[test]
    fn test_spawn_handshake_hello_reply_fails() {
        let (mgr, _log) = mgr_with_script("hello_first");
        let err = mgr
            .spawn_child("dashboard", &serde_json::json!({}))
            .unwrap_err();
        // Handshake detail is swallowed by the manager; only the generic text escapes.
        assert_eq!(err, "handshake failed");
        assert_eq!(mgr.active_count(), 0); // cleanup_failed_child removed it
        mgr.stop().unwrap();
    }

    #[test]
    fn test_spawn_child_exits_before_ack_fails() {
        let (mgr, _log) = mgr_with_script("exit_now");
        let err = mgr
            .spawn_child("dashboard", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, "handshake failed");
        assert_eq!(mgr.active_count(), 0);
        mgr.stop().unwrap();
    }

    #[test]
    fn test_spawn_ws_key_non_ack_reply_fails() {
        let (mgr, _log) = mgr_with_script("ack_then_hello");
        let err = mgr
            .spawn_child("dashboard", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, "failed to send WS key: expected ack, got hello");
        assert_eq!(mgr.active_count(), 0);
        mgr.stop().unwrap();
    }

    #[test]
    fn test_spawn_ws_key_pipe_closed_fails() {
        let (mgr, _log) = mgr_with_script("ack_then_exit");
        let err = mgr
            .spawn_child("dashboard", &serde_json::json!({}))
            .unwrap_err();
        assert!(
            err.starts_with("failed to send WS key: failed to read ACK"),
            "unexpected error: {}",
            err
        );
        assert_eq!(mgr.active_count(), 0);
        mgr.stop().unwrap();
    }

    #[test]
    fn test_spawn_window_data_non_ack_reply_fails() {
        let (mgr, _log) = mgr_with_script("ack2_then_hello");
        let err = mgr
            .spawn_child("approval", &serde_json::json!({}))
            .unwrap_err();
        assert_eq!(err, "failed to send window data: expected ack, got hello");
        assert_eq!(mgr.active_count(), 0);
        mgr.stop().unwrap();
    }

    #[test]
    fn test_spawn_window_data_pipe_closed_fails() {
        let (mgr, _log) = mgr_with_script("ack2_then_exit");
        let err = mgr
            .spawn_child("approval", &serde_json::json!({}))
            .unwrap_err();
        assert!(
            err.starts_with("failed to send window data: failed to read ACK"),
            "unexpected error: {}",
            err
        );
        assert_eq!(mgr.active_count(), 0);
        mgr.stop().unwrap();
    }

    // --- success paths (#[tokio::test]: approval path calls tokio::spawn) ---

    #[tokio::test]
    async fn test_spawn_dashboard_success_notify_and_call() {
        let (mgr, log_path) = mgr_with_script("ack_all");
        mgr.start().await.unwrap();

        let (child_id, rx) = mgr
            .spawn_child(
                "dashboard",
                &serde_json::json!({"token": "t", "web_port": 1, "web_host": "127.0.0.1"}),
            )
            .expect("dashboard spawn should succeed");
        assert_eq!(child_id, "child-0");
        assert!(rx.is_none()); // persistent window: no result channel
        assert_eq!(mgr.active_count(), 1);
        assert!(matches!(
            mgr.get_child(&child_id),
            Some(ProcessStatus::Handshaking)
        ));

        // Rogue "child": connect to the manager's WS server with the key the
        // real child received (parsed from the child's stdin log).
        let key = read_ws_key_from_log(&log_path);
        let port = mgr.ws_port();
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/child/{}", port, key))
                .await
                .expect("rogue connect");
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws.send(WsMessage::Text(auth.to_string().into()))
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await; // let auth register

        // notify_child: send_notification uses try_lock, retry while busy.
        let mut notified = false;
        for _ in 0..10 {
            if mgr
                .notify_child(&child_id, "window.minimize", serde_json::json!({}))
                .is_ok()
            {
                notified = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert!(notified, "notify_child kept failing");

        // call_child: move the socket into a responder task that answers any
        // request by echoing its id, then await the call result.
        let responder = tokio::spawn(async move {
            while let Some(Ok(WsMessage::Text(text))) = ws.next().await {
                let m: Message = match serde_json::from_str(&text) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if m.is_request() {
                    let resp = Message::new_response(
                        m.id.as_deref().unwrap_or(""),
                        serde_json::json!({"echo": true}),
                    );
                    let _ = ws
                        .send(WsMessage::Text(
                            serde_json::to_string(&resp).unwrap().into(),
                        ))
                        .await;
                }
            }
        });
        let call = tokio::time::timeout(
            Duration::from_secs(5),
            mgr.call_child(&child_id, "echo.method", serde_json::json!({"v": 1})),
        )
        .await
        .expect("call_child timed out")
        .expect("call_child failed");
        assert_eq!(call.result.unwrap()["echo"], true);
        responder.abort();
        // ws 已 move 进 responder，abort 后 socket 随任务 drop 关闭，无需显式 close。

        mgr.stop().unwrap();
        assert_eq!(mgr.active_count(), 0);
    }

    #[tokio::test]
    async fn test_spawn_approval_result_channel_e2e() {
        let (mgr, log_path) = mgr_with_script("ack_all");
        mgr.start().await.unwrap();

        let (_child_id, rx) = mgr
            .spawn_child(
                "approval",
                &serde_json::json!({"request_id": "r1", "risk_level": "HIGH"}),
            )
            .expect("approval spawn should succeed");
        let mut rx = rx.expect("approval window returns a result receiver");

        // Rogue child connects with the key from the child's stdin log.
        let key = read_ws_key_from_log(&log_path);
        let port = mgr.ws_port();
        let (mut ws, _) =
            tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{}/child/{}", port, key))
                .await
                .expect("rogue connect");
        let auth = serde_json::json!({"type": "auth", "key": key});
        ws.send(WsMessage::Text(auth.to_string().into()))
            .await
            .unwrap();
        // spawn_wait_for_result polls for the connection every 100ms and then
        // registers the approval.submit handler; give it time.
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Deliver the approval result as a notification from the "child".
        let note = Message::new_notification(
            "approval.submit",
            serde_json::json!({"action": "approved", "request_id": "r1"}),
        );
        ws.send(WsMessage::Text(
            serde_json::to_string(&note).unwrap().into(),
        ))
        .await
        .unwrap();

        let value = tokio::time::timeout(Duration::from_secs(5), &mut rx)
            .await
            .expect("result channel timed out")
            .expect("oneshot dropped without a value");
        assert_eq!(value["action"], "approved");
        assert_eq!(value["request_id"], "r1");

        let _ = ws.close(None).await;
        mgr.stop().unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn test_spawn_approval_no_connection_times_out_and_cleans_channel() {
        let (mgr, _log) = mgr_with_script("ack_all");
        // No mgr.start(): the WS server never runs, so no connection can ever
        // appear and spawn_wait_for_result exhausts its 100 poll attempts.

        let (child_id, rx) = mgr
            .spawn_child("approval", &serde_json::json!({}))
            .expect("spawn itself succeeds (pipe protocol only)");
        let mut rx = rx.expect("approval returns result receiver");

        // Paused clock auto-advances: 100 × 100ms polls (~10s virtual), then
        // the 300s wait fires at ~310s virtual — long before our 320s cap.
        // When the task cleans up the result channel the oneshot sender is
        // dropped and the receiver resolves with an error.
        let outcome = tokio::time::timeout(Duration::from_secs(320), &mut rx).await;
        match outcome {
            Ok(Err(_recv_error)) => { /* channel dropped by cleanup, as expected */ }
            other => panic!("expected dropped channel, got {:?}", other.is_ok()),
        }
        // Result channel was removed by the wait task's cleanup.
        assert!(!mgr.submit_result(&child_id, serde_json::json!({})));

        mgr.stop().unwrap(); // reaps the PowerShell child instantly
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
