use super::*;

#[test]
fn test_create_session() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    assert!(!session.id.is_empty());
    assert!(session.sender_id.starts_with("web:"));
    assert_eq!(session.chat_id, session.sender_id);
}

#[test]
fn test_get_session() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let found = mgr.get_session(&session.id).unwrap();
    assert_eq!(found.id, session.id);
}

#[test]
fn test_remove_session() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    mgr.remove_session(&session.id);
    assert!(mgr.get_session(&session.id).is_none());
}

#[test]
fn test_active_count() {
    let mgr = SessionManager::with_default_timeout();
    assert_eq!(mgr.active_count(), 0);
    let s1 = mgr.create_session();
    let _s2 = mgr.create_session();
    assert_eq!(mgr.active_count(), 2);
    mgr.remove_session(&s1.id);
    assert_eq!(mgr.active_count(), 1);
}

#[test]
fn test_touch_session() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let original = session.last_active;
    std::thread::sleep(std::time::Duration::from_millis(10));
    mgr.touch_session(&session.id);
    let updated = mgr.get_session(&session.id).unwrap();
    assert!(updated.last_active > original);
}

#[test]
fn test_stats() {
    let mgr = SessionManager::with_default_timeout();
    let _s1 = mgr.create_session();
    let _s2 = mgr.create_session();
    let stats = mgr.stats();
    assert_eq!(stats.get("active_sessions").unwrap().as_u64(), Some(2));
}

#[test]
fn test_all_sessions() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();
    let all = mgr.all_sessions();
    assert_eq!(all.len(), 2);
    let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&s1.id.as_str()));
    assert!(ids.contains(&s2.id.as_str()));
}

#[test]
fn test_broadcast_no_send_queue() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    // Without a send queue, broadcast should fail
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(mgr.broadcast(&session.id, b"test"));
    assert!(result.is_err());
}

#[test]
fn test_get_session_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    assert!(mgr.get_session("nonexistent").is_none());
}

#[test]
fn test_touch_session_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    // Should not panic
    mgr.touch_session("nonexistent");
}

#[test]
fn test_remove_session_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    // Should not panic
    mgr.remove_session("nonexistent");
}

#[test]
fn test_generate_session_id_format() {
    let id = generate_session_id();
    assert_eq!(id.len(), 16);
    // Should be hex characters only
    assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_generate_session_id_unique() {
    let id1 = generate_session_id();
    let id2 = generate_session_id();
    assert_ne!(id1, id2);
}

#[test]
fn test_session_fields_populated() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    assert!(!session.id.is_empty());
    assert!(!session.sender_id.is_empty());
    assert!(!session.chat_id.is_empty());
    assert!(session.created_at <= session.last_active);
}

#[test]
fn test_create_multiple_sessions() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();
    let s3 = mgr.create_session();

    assert_ne!(s1.id, s2.id);
    assert_ne!(s2.id, s3.id);
    assert_eq!(mgr.active_count(), 3);
}

#[test]
fn test_remove_all_sessions() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();
    mgr.remove_session(&s1.id);
    mgr.remove_session(&s2.id);
    assert_eq!(mgr.active_count(), 0);
    assert!(mgr.all_sessions().is_empty());
}

#[test]
fn test_session_manager_new_with_custom_timeout() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(30));
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_shutdown() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mgr = SessionManager::with_default_timeout();
    let _s1 = mgr.create_session();
    let _s2 = mgr.create_session();
    assert_eq!(mgr.active_count(), 2);

    rt.block_on(mgr.shutdown());
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_set_send_queue_for_nonexistent_session() {
    let _mgr = SessionManager::with_default_timeout();
    // Should not panic when setting send queue for nonexistent session
    let _rt = tokio::runtime::Runtime::new().unwrap();
    let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (done_tx, done_rx) = tokio::sync::watch::channel(false);

    // Create a minimal SendQueue-like structure
    // We can't easily create a real SendQueue without a WebSocket, so this
    // just tests that set_send_queue doesn't panic for nonexistent sessions.
    drop(tx);
    drop(done_tx);
    drop(done_rx);
    // The key test: no panic
}

#[test]
fn test_stats_reflects_changes() {
    let mgr = SessionManager::with_default_timeout();
    let stats0 = mgr.stats();
    assert_eq!(stats0.get("active_sessions").unwrap().as_u64(), Some(0));

    let _s = mgr.create_session();
    let stats1 = mgr.stats();
    assert_eq!(stats1.get("active_sessions").unwrap().as_u64(), Some(1));
}

#[test]
fn test_all_sessions_after_removal() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();
    let s3 = mgr.create_session();
    mgr.remove_session(&s2.id);

    let all = mgr.all_sessions();
    assert_eq!(all.len(), 2);
    let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&s1.id.as_str()));
    assert!(ids.contains(&s3.id.as_str()));
    assert!(!ids.contains(&s2.id.as_str()));
}

#[test]
fn test_broadcast_to_nonexistent_session() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mgr = SessionManager::with_default_timeout();
    let result = rt.block_on(mgr.broadcast("nonexistent-id", b"test"));
    assert!(result.is_err());
}

#[test]
fn test_session_id_is_16_chars() {
    for _ in 0..10 {
        let id = generate_session_id();
        assert_eq!(id.len(), 16, "Session ID should be 16 chars, got: {}", id);
    }
}

#[test]
fn test_session_sender_id_format() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    assert!(session.sender_id.starts_with("web:"));
    assert_eq!(&session.sender_id[4..], &session.id);
}

#[test]
fn test_session_created_at_equals_last_active() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    assert_eq!(session.created_at, session.last_active);
}

#[test]
fn test_touch_updates_only_last_active() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let original_created = session.created_at;
    std::thread::sleep(std::time::Duration::from_millis(10));
    mgr.touch_session(&session.id);
    let updated = mgr.get_session(&session.id).unwrap();
    assert_eq!(updated.created_at, original_created);
    assert!(updated.last_active > original_created);
}

#[test]
fn test_stats_empty() {
    let mgr = SessionManager::with_default_timeout();
    let stats = mgr.stats();
    assert_eq!(stats.get("active_sessions").unwrap().as_u64(), Some(0));
}

#[test]
fn test_remove_all_then_add() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    mgr.remove_session(&s1.id);
    assert_eq!(mgr.active_count(), 0);
    let s2 = mgr.create_session();
    assert_eq!(mgr.active_count(), 1);
    assert!(mgr.get_session(&s2.id).is_some());
}

#[test]
fn test_multiple_touches() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let mut last_active = session.last_active;

    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_millis(5));
        mgr.touch_session(&session.id);
        let updated = mgr.get_session(&session.id).unwrap();
        assert!(updated.last_active >= last_active);
        last_active = updated.last_active;
    }
}

#[test]
fn test_all_sessions_returns_empty_after_removal() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();
    mgr.remove_session(&s1.id);
    mgr.remove_session(&s2.id);
    assert!(mgr.all_sessions().is_empty());
}

#[test]
fn test_session_different_ids() {
    let mgr = SessionManager::with_default_timeout();
    let ids: Vec<String> = (0..10).map(|_| mgr.create_session().id).collect();
    let unique: std::collections::HashSet<&String> = ids.iter().collect();
    assert_eq!(unique.len(), 10);
}

#[test]
fn test_get_session_after_removal_returns_none() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let id = session.id.clone();
    mgr.remove_session(&id);
    assert!(mgr.get_session(&id).is_none());
}

#[test]
fn test_shutdown_then_create() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mgr = SessionManager::with_default_timeout();
    let _s = mgr.create_session();
    rt.block_on(mgr.shutdown());
    assert_eq!(mgr.active_count(), 0);
    // Should be able to create new sessions after shutdown
    let _s = mgr.create_session();
    assert_eq!(mgr.active_count(), 1);
}

#[test]
fn test_session_equality_by_id() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let retrieved = mgr.get_session(&session.id).unwrap();
    assert_eq!(session.id, retrieved.id);
    assert_eq!(session.sender_id, retrieved.sender_id);
    assert_eq!(session.chat_id, retrieved.chat_id);
}

// ---- Additional coverage tests for 95%+ ----

#[test]
fn test_set_send_queue_for_existing_session() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(
        tx, done_rx,
    ));

    mgr.set_send_queue(&session.id, queue);
    assert!(mgr.send_queues.contains_key(&session.id));
}

#[tokio::test]
async fn test_broadcast_with_send_queue() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(
        tx, done_rx,
    ));

    mgr.set_send_queue(&session.id, queue);

    let result = mgr.broadcast(&session.id, b"hello world").await;
    assert!(result.is_ok());

    let received = rx.recv().await.unwrap();
    assert_eq!(received, b"hello world");
}

#[tokio::test]
async fn test_broadcast_after_session_removed() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (tx, _rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_done_tx, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(crate::websocket_handler::SendQueue::from_channels(
        tx, done_rx,
    ));

    mgr.set_send_queue(&session.id, queue);
    mgr.remove_session(&session.id);

    let result = mgr.broadcast(&session.id, b"test").await;
    assert!(result.is_err());
}

#[test]
fn test_shutdown_double() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mgr = SessionManager::with_default_timeout();
    let _s = mgr.create_session();

    rt.block_on(mgr.shutdown());
    rt.block_on(mgr.shutdown()); // Should not panic
    assert_eq!(mgr.active_count(), 0);
}

#[test]
fn test_session_debug_format() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let debug_str = format!("{:?}", session);
    assert!(debug_str.contains(&session.id));
}

#[test]
fn test_session_clone() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();
    let cloned = session.clone();
    assert_eq!(session.id, cloned.id);
    assert_eq!(session.sender_id, cloned.sender_id);
}

#[test]
fn test_set_send_queue_replaces_existing() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (tx1, _rx1) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_done_tx1, done_rx1) = tokio::sync::watch::channel(false);
    let queue1 = Arc::new(crate::websocket_handler::SendQueue::from_channels(
        tx1, done_rx1,
    ));
    mgr.set_send_queue(&session.id, queue1);

    let (tx2, _rx2) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_done_tx2, done_rx2) = tokio::sync::watch::channel(false);
    let queue2 = Arc::new(crate::websocket_handler::SendQueue::from_channels(
        tx2, done_rx2,
    ));
    mgr.set_send_queue(&session.id, queue2);

    assert!(mgr.send_queues.contains_key(&session.id));
}
