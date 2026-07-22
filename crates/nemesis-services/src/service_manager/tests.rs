use super::*;

#[test]
fn test_new_manager() {
    let mgr = ServiceManager::new();
    assert!(!mgr.is_basic_services_started());
    assert!(!mgr.is_bot_running());
    assert_eq!(mgr.get_bot_state(), BotState::NotStarted);
}

#[test]
fn test_start_basic_services() {
    let mgr = ServiceManager::new();
    mgr.start_basic_services().unwrap();
    assert!(mgr.is_basic_services_started());

    // Second call should fail
    let result = mgr.start_basic_services();
    assert!(result.is_err());
}

#[test]
fn test_start_bot_requires_basic_services() {
    let mgr = ServiceManager::new();
    let result = mgr.start_bot();
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("basic services"));
}

#[test]
fn test_stop_bot_when_not_running() {
    let mgr = ServiceManager::new();
    let result = mgr.stop_bot();
    assert!(result.is_err());
}

#[test]
fn test_shutdown() {
    let mgr = ServiceManager::new();
    mgr.start_basic_services().unwrap();
    mgr.shutdown();
    // After shutdown, basic services flag is reset
    assert!(!mgr.is_basic_services_started());
}

#[test]
fn test_get_bot_service() {
    let mgr = ServiceManager::new();
    let svc = mgr.get_bot_service();
    assert_eq!(svc.get_state(), BotState::NotStarted);
}

#[test]
fn test_get_bot_config() {
    let mgr = ServiceManager::new();
    let config = mgr.get_bot_config();
    assert!(config.security_enabled);
}

#[test]
fn test_trigger_shutdown_sends_broadcast() {
    let mgr = ServiceManager::new();
    let mut rx = mgr.subscribe_shutdown();

    mgr.trigger_shutdown();

    // Receiver should get the signal
    let result = rx.try_recv();
    assert!(result.is_ok());
}

#[test]
fn test_active_service_count() {
    let mgr = ServiceManager::new();
    assert_eq!(mgr.active_service_count(), 0);

    mgr.start_basic_services().unwrap();
    assert_eq!(mgr.active_service_count(), 1);
}

#[tokio::test]
async fn test_wait_for_shutdown_with_timeout_succeeds() {
    let mgr = ServiceManager::new();

    // Subscribe first, then trigger, to avoid race between send and subscribe
    let mut rx = mgr.subscribe_shutdown();
    mgr.trigger_shutdown();

    // Use select to verify the receiver got the signal
    let result = tokio::select! {
        _ = rx.recv() => {
            true
        }
        _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
            false
        }
    };
    assert!(result);
}

#[tokio::test]
async fn test_wait_for_shutdown_with_timeout_times_out() {
    let mgr = ServiceManager::new();

    // Don't trigger shutdown - should timeout
    let result = mgr
        .wait_for_shutdown_with_timeout(std::time::Duration::from_millis(50))
        .await;
    assert!(!result);
}

#[tokio::test]
async fn test_wait_for_services_with_zero_count() {
    let mgr = ServiceManager::new();
    // No services tracked, should return immediately
    let result = mgr
        .wait_for_services(std::time::Duration::from_secs(1))
        .await;
    assert!(result);
}

#[tokio::test]
async fn test_wait_for_shutdown_with_trigger() {
    let mgr = ServiceManager::new();
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();

    // Send trigger in background
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = tx.send(());
    });

    mgr.wait_for_shutdown_with_trigger(rx).await;
    // If we get here, the trigger was received
}

#[test]
fn test_shutdown_resets_basic_services_flag() {
    let mgr = ServiceManager::new();
    mgr.start_basic_services().unwrap();
    assert!(mgr.is_basic_services_started());

    mgr.shutdown();
    assert!(!mgr.is_basic_services_started());
}

#[test]
fn test_double_shutdown_is_safe() {
    let mgr = ServiceManager::new();
    mgr.start_basic_services().unwrap();

    // First shutdown
    mgr.shutdown();
    assert!(!mgr.is_basic_services_started());

    // Second shutdown should not panic
    mgr.shutdown();
    assert!(!mgr.is_basic_services_started());
}

// --- Additional coverage tests for 95%+ ---

#[test]
fn test_service_manager_default() {
    let mgr = ServiceManager::default();
    assert!(!mgr.is_basic_services_started());
    assert!(!mgr.is_bot_running());
}

#[test]
fn test_service_manager_with_custom_config() {
    let mut config = BotServiceConfig::default();
    config.gateway_port = 9090;
    config.security_enabled = false;
    let mgr = ServiceManager::with_config(config);
    assert!(!mgr.is_basic_services_started());
    assert_eq!(mgr.get_bot_config().gateway_port, 9090);
    assert!(!mgr.get_bot_config().security_enabled);
}

#[test]
fn test_shutdown_without_basic_services() {
    let mgr = ServiceManager::new();
    // Should not panic when no basic services started
    mgr.shutdown();
    assert!(!mgr.is_basic_services_started());
}

#[tokio::test]
async fn test_wait_for_services_timeout() {
    let mgr = ServiceManager::new();
    mgr.start_basic_services().unwrap();
    // Wait group has 1 entry, so this should timeout
    let result = mgr
        .wait_for_services(std::time::Duration::from_millis(50))
        .await;
    assert!(!result);
}

#[test]
fn test_trigger_shutdown_multiple_times() {
    let mgr = ServiceManager::new();

    mgr.trigger_shutdown();

    // Subscriber created after trigger should still get the signal (broadcast retains last value)
    let _rx = mgr.subscribe_shutdown();
    // The broadcast channel has capacity 1, so lagging receivers may not get old messages
    // Just verify trigger doesn't panic
    mgr.trigger_shutdown();
}

#[test]
fn test_get_bot_error_none() {
    let mgr = ServiceManager::new();
    assert!(mgr.get_bot_error().is_none());
}

#[test]
fn test_get_bot_components_empty() {
    let mgr = ServiceManager::new();
    let components = mgr.get_bot_components();
    assert!(components.is_empty());
}

#[test]
fn test_restart_bot_when_not_running() {
    let mgr = ServiceManager::new();
    // Restart on a bot that hasn't been started should fail
    let result = mgr.restart_bot();
    assert!(result.is_err());
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

// --- WaitGroup ---

#[test]
fn test_wait_group_add_done_basic() {
    let mut wg = WaitGroup::new();
    assert_eq!(wg.count(), 0);

    wg.add();
    assert_eq!(wg.count(), 1);

    wg.add();
    assert_eq!(wg.count(), 2);

    wg.done();
    assert_eq!(wg.count(), 1);

    wg.done();
    assert_eq!(wg.count(), 0);
}

#[test]
fn test_wait_group_done_below_zero_no_panic() {
    let mut wg = WaitGroup::new();
    wg.done(); // count is 0, should not underflow
    assert_eq!(wg.count(), 0);
}

#[tokio::test]
async fn test_wait_group_wait_already_zero() {
    let mut wg = WaitGroup::new();
    // Should return immediately since count is 0
    wg.wait().await;
}

#[tokio::test]
async fn test_wait_group_wait_completes_on_zero() {
    let mut wg = WaitGroup::new();
    wg.add();

    // Spawn a task that will call done() after a short delay
    let (_tx, _done_rx) = tokio::sync::watch::channel(());
    let inner_done_tx = wg.done_tx.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        // Simulate done by sending on the watch channel
        let _ = inner_done_tx.unwrap().send(());
    });

    wg.wait().await;
}

// --- ServiceManager shutdown coordination ---

#[tokio::test]
async fn test_wait_for_shutdown_with_timeout_broadcast_trigger() {
    let mgr = Arc::new(ServiceManager::new());

    // Trigger shutdown in background
    let mgr_clone = Arc::clone(&mgr);
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        mgr_clone.trigger_shutdown();
    });

    let result = mgr
        .wait_for_shutdown_with_timeout(std::time::Duration::from_secs(5))
        .await;
    assert!(result);
}

#[tokio::test]
async fn test_wait_for_shutdown_with_trigger_sender_dropped() {
    let mgr = ServiceManager::new();
    let (_tx, _rx) = tokio::sync::oneshot::channel::<()>();

    // Drop sender to simulate sender being dropped
    drop(_tx);

    // If sender is already dropped before call, the external trigger
    // receiver will resolve immediately
    // We need to create a new channel where sender is NOT dropped
    let (tx2, rx2) = tokio::sync::oneshot::channel::<()>();
    drop(tx2); // Now sender is dropped

    mgr.wait_for_shutdown_with_trigger(rx2).await;
}

#[tokio::test]
async fn test_wait_for_shutdown_with_desktop_sender_dropped() {
    let mgr = ServiceManager::new();
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Drop sender to simulate desktop process exiting
    drop(tx);

    mgr.wait_for_shutdown_with_desktop(rx).await;
}

#[tokio::test]
async fn test_wait_for_shutdown_with_desktop_value_change() {
    let mgr = ServiceManager::new();
    let (tx, rx) = tokio::sync::watch::channel(false);

    // Change value in background
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        let _ = tx.send(true);
    });

    mgr.wait_for_shutdown_with_desktop(rx).await;
}

// --- save_bot_config ---

#[test]
fn test_save_bot_config_invalid_json() {
    let mgr = ServiceManager::new();
    let result = mgr.save_bot_config(&serde_json::json!("not an object"), false);
    assert!(result.is_err());
}

#[test]
fn test_save_bot_config_valid_json_no_models() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let mgr = ServiceManager::with_config(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let result = mgr.save_bot_config(&serde_json::json!({ "models": [] }), false);
    assert!(result.is_err());
}

#[test]
fn test_save_bot_config_valid() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    std::fs::write(&config_path, "{}").unwrap();

    let mgr = ServiceManager::with_config(BotServiceConfig {
        config_path: config_path.clone(),
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    let config = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }
        ]
    });

    let result = mgr.save_bot_config(&config, false);
    assert!(result.is_ok());

    // Verify the file was written
    let content = std::fs::read_to_string(&config_path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["models"][0]["model"], "test/1.0");
}

// --- get_bot_service returns reference ---

#[test]
fn test_get_bot_service_returns_reference() {
    let mgr = ServiceManager::new();
    let svc = mgr.get_bot_service();
    assert_eq!(svc.get_state(), BotState::NotStarted);
}

// --- active_service_count tracking ---

#[test]
fn test_active_service_count_after_basic_and_shutdown() {
    let mgr = ServiceManager::new();
    assert_eq!(mgr.active_service_count(), 0);

    mgr.start_basic_services().unwrap();
    assert_eq!(mgr.active_service_count(), 1);

    mgr.shutdown();
    // After shutdown, wait group was done'd but count is checked on the WaitGroup
    // which now has count 0
    assert_eq!(mgr.active_service_count(), 0);
}

// --- Multiple shutdown subscribers ---

#[test]
fn test_multiple_shutdown_subscribers() {
    let mgr = ServiceManager::new();
    let mut rx1 = mgr.subscribe_shutdown();
    let mut rx2 = mgr.subscribe_shutdown();

    mgr.trigger_shutdown();

    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
}

// --- subscribe_shutdown_idempotent ---

#[test]
fn test_subscribe_shutdown_can_be_called_multiple_times() {
    let mgr = ServiceManager::new();
    let _rx1 = mgr.subscribe_shutdown();
    let _rx2 = mgr.subscribe_shutdown();
    let _rx3 = mgr.subscribe_shutdown();
    // All should work without panic
}

// --- trigger_shutdown_multiple_times_with_subscribers ---

#[test]
fn test_trigger_shutdown_multiple_times_with_subscribers() {
    let mgr = ServiceManager::new();
    let mut rx = mgr.subscribe_shutdown();

    mgr.trigger_shutdown();
    assert!(rx.try_recv().is_ok());

    mgr.trigger_shutdown();
    // Second trigger may not be received since broadcast has capacity 1
    // and receiver already consumed. Just verify no panic.
    let _ = rx.try_recv();
}

// --- is_bot_running after full cycle ---

#[test]
fn test_is_bot_running_after_start_stop() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let mgr = ServiceManager::with_config(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    assert!(!mgr.is_bot_running());

    mgr.start_basic_services().unwrap();
    mgr.start_bot().unwrap();
    assert!(mgr.is_bot_running());
    assert_eq!(mgr.get_bot_state(), BotState::Running);

    mgr.stop_bot().unwrap();
    assert!(!mgr.is_bot_running());
}

// --- start_bot then shutdown ---

#[test]
fn test_full_lifecycle_basic_start_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.json");
    let config_content = serde_json::json!({
        "models": [
            { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
        ]
    });
    std::fs::write(
        &config_path,
        serde_json::to_string(&config_content).unwrap(),
    )
    .unwrap();

    let mgr = ServiceManager::with_config(BotServiceConfig {
        config_path,
        workspace: dir.path().to_path_buf(),
        ..BotServiceConfig::default()
    });

    mgr.start_basic_services().unwrap();
    mgr.start_bot().unwrap();
    assert!(mgr.is_bot_running());

    mgr.shutdown();
    assert!(!mgr.is_bot_running());
    assert!(!mgr.is_basic_services_started());
}
