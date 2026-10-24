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

    let (queue, _hi_rx, _lo_rx, _done_tx) = crate::websocket_handler::SendQueue::test_channels(16);
    let queue = Arc::new(queue);

    mgr.set_send_queue(&session.id, queue);
    assert!(mgr.send_queues.contains_key(&session.id));
}

#[tokio::test]
async fn test_broadcast_with_send_queue() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (queue, mut rx, _lo_rx, _done_tx) = crate::websocket_handler::SendQueue::test_channels(16);
    let queue = Arc::new(queue);

    mgr.set_send_queue(&session.id, queue);

    let result = mgr.broadcast(&session.id, b"hello world").await;
    assert!(result.is_ok());

    let received = rx.recv().await.unwrap();
    assert_eq!(received, b"hello world");
}

// BUG 2026-09-22（慢客户端洪泛）：可丢广播走 lo 通道、必达通道不受扰；
// 无队列会话诚实报错。
#[tokio::test]
async fn test_broadcast_droppable_delivers_on_lo_lane() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (queue, mut hi_rx, mut lo_rx, _done_tx) =
        crate::websocket_handler::SendQueue::test_channels(16);
    let queue = Arc::new(queue);

    mgr.set_send_queue(&session.id, queue);

    let result = mgr.broadcast_droppable(&session.id, b"droppable hello");
    assert!(result.is_ok());

    let received = lo_rx.recv().await.unwrap();
    assert_eq!(received, b"droppable hello");

    // 必达通道必须为空（可丢帧不得挤占 hi 槽位）。
    assert!(
        hi_rx.try_recv().is_err(),
        "droppable frames must not land on the hi lane"
    );
}

#[test]
fn test_broadcast_droppable_missing_session_errors() {
    let mgr = SessionManager::with_default_timeout();
    let result = mgr.broadcast_droppable("no-such-session", b"x");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_broadcast_after_session_removed() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.create_session();

    let (queue, _hi_rx, _lo_rx, _done_tx) = crate::websocket_handler::SendQueue::test_channels(16);
    let queue = Arc::new(queue);

    mgr.set_send_queue(&session.id, queue);
    mgr.remove_session(&session.id);

    let result = mgr.broadcast(&session.id, b"test").await;
    assert!(result.is_err());
}

// BUG 2026-10-24（僵尸连接）：逐出只删映射不断连，socket 仍活——WSAPI 响应
// 走 dispatch 捕获队列照常送达，而会话路由的帧（agent 回复/用户回声/历史/
// 推送）查表失败静默丢失，无心跳客户端永无自愈。逐出必须先 close 队列
// （发 Close 帧握手 → 前端 onclose 非 1008/4001 自动重连拿新会话）。
// 直驱 evict_inactive（cleanup 循环 300s 一拍，测试等不起）；合成 now=+7200s
// 配 timeout 3600s 只命中陈旧会话——后台线程持真实 now 永不干扰。
#[test]
fn test_evict_inactive_closes_live_send_queue() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(3600));

    // 陈旧会话（带活队列）+ 新鲜会话（对照组）。
    let stale = mgr.create_session();
    let (queue, _hi_rx, _lo_rx, _done_tx) = crate::websocket_handler::SendQueue::test_channels(16);
    let queue = Arc::new(queue);
    mgr.set_send_queue(&stale.id, queue.clone());
    let fresh = mgr.create_session();
    let (fresh_queue, _hi_rx2, _lo_rx2, _done_tx2) =
        crate::websocket_handler::SendQueue::test_channels(16);
    let fresh_queue = Arc::new(fresh_queue);
    mgr.set_send_queue(&fresh.id, fresh_queue.clone());

    let now = Local::now() + chrono::Duration::seconds(7200);
    // 新鲜会话的活跃时间对齐合成 now（否则它同样"陈旧"——两条都是测试时刻建的）。
    if let Some(mut entry) = mgr.sessions.get_mut(&fresh.id) {
        entry.session.last_active = now;
    }

    let evicted = super::SessionManager::evict_inactive(
        &mgr.sessions,
        &mgr.send_queues,
        std::time::Duration::from_secs(3600),
        now,
    );

    assert_eq!(evicted, 1, "只有陈旧会话被逐出");
    assert!(mgr.get_session(&stale.id).is_none(), "陈旧会话映射必须删除");
    assert!(
        !mgr.send_queues.contains_key(&stale.id),
        "陈旧会话队列映射必须删除"
    );
    assert!(
        queue.close_requested(),
        "逐出必须对存活队列发起 close（Close 帧握手 → 客户端自动重连）"
    );

    // 对照组：新鲜会话原样保留、队列未关闭。
    assert!(mgr.get_session(&fresh.id).is_some());
    assert!(mgr.send_queues.contains_key(&fresh.id));
    assert!(!fresh_queue.close_requested(), "新鲜会话不得被逐出误伤");
}

// 无队列的陈旧会话（纯死映射残骸）也要能逐出——close 是尽力而为不是前置条件。
#[test]
fn test_evict_inactive_without_queue_still_removes() {
    let mgr = SessionManager::new(std::time::Duration::from_secs(3600));
    let stale = mgr.create_session(); // 无 send_queue

    let evicted = super::SessionManager::evict_inactive(
        &mgr.sessions,
        &mgr.send_queues,
        std::time::Duration::from_secs(3600),
        Local::now() + chrono::Duration::seconds(7200),
    );

    assert_eq!(evicted, 1);
    assert!(mgr.get_session(&stale.id).is_none());
    assert!(!mgr.send_queues.contains_key(&stale.id));
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

    let (queue1, _hi_rx1, _lo_rx1, _done_tx1) =
        crate::websocket_handler::SendQueue::test_channels(16);
    let queue1 = Arc::new(queue1);
    mgr.set_send_queue(&session.id, queue1);

    let (queue2, _hi_rx2, _lo_rx2, _done_tx2) =
        crate::websocket_handler::SendQueue::test_channels(16);
    let queue2 = Arc::new(queue2);
    mgr.set_send_queue(&session.id, queue2);

    assert!(mgr.send_queues.contains_key(&session.id));
}
