//! S10b (quality-hardening goal 冲刺, web 批次 2): SessionManager surface the
//! existing tests skip — stats/all_sessions/shutdown + the broadcast no-queue
//! error path (queue-present arm needs a real WS sink; covered by the live
//! websocket_handler tests).

use super::*;

#[tokio::test]
async fn broadcast_without_queue_errs_and_stats_track_sessions() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.create_session();
    let s2 = mgr.create_session();

    assert_eq!(mgr.active_count(), 2);
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], serde_json::json!(2));

    let all = mgr.all_sessions();
    assert_eq!(all.len(), 2);
    let ids: Vec<&str> = all.iter().map(|s| s.id.as_str()).collect();
    assert!(ids.contains(&s1.id.as_str()));
    assert!(ids.contains(&s2.id.as_str()));

    // No send queue registered → error mentioning the session id.
    let err = mgr
        .broadcast(&s1.id, br#"{"type":"system"}"#)
        .await
        .unwrap_err();
    assert!(err.contains(&s1.id), "error names the session: {}", err);
}

#[tokio::test]
async fn shutdown_clears_sessions_and_queues() {
    let mgr = SessionManager::with_default_timeout();
    mgr.create_session();
    mgr.create_session();
    assert_eq!(mgr.active_count(), 2);

    mgr.shutdown().await;
    assert_eq!(mgr.active_count(), 0, "sessions cleared");

    // Broadcast after shutdown still errors cleanly (queue map empty).
    let err = mgr.broadcast("any", b"x").await.unwrap_err();
    assert!(err.contains("no send queue"));
}
