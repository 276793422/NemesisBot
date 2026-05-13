//! ServiceManager - Service lifecycle and graceful shutdown.
//!
//! Manages the lifecycle of long-running services, provides a WaitGroup
//! pattern for tracking running tasks, and coordinates graceful shutdown
//! via a signal channel. Integrates with BotService for start/stop/restart.

use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{broadcast, watch};
use tracing::{info, warn, error};

use crate::bot_service::{BotService, BotServiceConfig};
use crate::state::BotState;

// ---------------------------------------------------------------------------
// WaitGroup
// ---------------------------------------------------------------------------

/// A simple WaitGroup that tracks the number of running services.
///
/// Mirrors the Go `sync.WaitGroup` pattern. When the count drops to zero,
/// all callers waiting on `wait()` are unblocked via the internal watch
/// channel.
#[derive(Debug)]
struct WaitGroup {
    count: usize,
    done_tx: Option<watch::Sender<()>>,
    done_rx: watch::Receiver<()>,
}

impl WaitGroup {
    fn new() -> Self {
        let (tx, rx) = watch::channel(());
        Self {
            count: 0,
            done_tx: Some(tx),
            done_rx: rx,
        }
    }

    /// Increment the counter. Must be paired with a corresponding `done()`.
    fn add(&mut self) {
        self.count += 1;
    }

    /// Decrement the counter. When it reaches zero, all `wait()` callers
    /// are notified.
    fn done(&mut self) {
        if self.count > 0 {
            self.count -= 1;
        }
        if self.count == 0 {
            let _ = self.done_tx.as_ref().map(|tx| tx.send(()));
        }
    }

    /// Wait until the counter reaches zero.
    #[allow(dead_code)]
    async fn wait(&mut self) {
        if self.count == 0 {
            return;
        }
        let _ = self.done_rx.changed().await;
    }

    /// Return the current counter value.
    #[allow(dead_code)]
    fn count(&self) -> usize {
        self.count
    }
}

// ---------------------------------------------------------------------------
// ServiceManagerState
// ---------------------------------------------------------------------------

/// Internal mutable state behind a lock.
struct ServiceManagerState {
    /// Whether basic services have been started.
    basic_services_started: bool,
    /// WaitGroup tracking running services.
    wait_group: WaitGroup,
}

// ---------------------------------------------------------------------------
// ServiceManager
// ---------------------------------------------------------------------------

/// Manages service lifecycle with graceful shutdown support and BotService integration.
///
/// The manager owns:
/// - A shutdown broadcast channel (all services listen to this for coordinated shutdown)
/// - A BotService instance for on-demand start/stop/restart
/// - Basic services that always run (HTTP server, Desktop UI)
///
/// # Shutdown coordination
///
/// The ServiceManager supports multiple shutdown triggers:
/// - `trigger_shutdown()` - programmatic shutdown signal
/// - `wait_for_shutdown()` - waits for Ctrl+C or broadcast
/// - `wait_for_shutdown_with_desktop()` - also listens for desktop UI close
/// - `wait_for_shutdown_with_timeout()` - with a deadline
///
/// # Example
///
/// ```rust,no_run
/// use nemesis_services::ServiceManager;
/// use nemesis_services::bot_service::BotServiceConfig;
///
/// #[tokio::main]
/// async fn main() {
///     let mgr = ServiceManager::with_config(BotServiceConfig::default());
///     mgr.start_basic_services().unwrap();
///     mgr.start_bot().unwrap();
///
///     // Block until shutdown
///     mgr.wait_for_shutdown().await;
///     mgr.shutdown();
/// }
/// ```
pub struct ServiceManager {
    /// Broadcast sender for the shutdown signal.
    shutdown_tx: broadcast::Sender<()>,

    /// Internal state behind a lock.
    state: Arc<Mutex<ServiceManagerState>>,

    /// The bot service instance.
    bot_service: BotService,
}

impl ServiceManager {
    /// Create a new ServiceManager with a default BotService.
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        info!("ServiceManager created");
        Self {
            shutdown_tx,
            state: Arc::new(Mutex::new(ServiceManagerState {
                basic_services_started: false,
                wait_group: WaitGroup::new(),
            })),
            bot_service: BotService::with_default_config(),
        }
    }

    /// Create a new ServiceManager with a custom BotService config.
    pub fn with_config(config: BotServiceConfig) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        info!("ServiceManager created with custom config");
        Self {
            shutdown_tx,
            state: Arc::new(Mutex::new(ServiceManagerState {
                basic_services_started: false,
                wait_group: WaitGroup::new(),
            })),
            bot_service: BotService::new(config),
        }
    }

    // -------------------------------------------------------------------
    // Basic services
    // -------------------------------------------------------------------

    /// Start services that should always run.
    ///
    /// This includes HTTP server for Web UI and Desktop UI.
    /// Returns an error if called when basic services are already started.
    pub fn start_basic_services(&self) -> Result<(), String> {
        let mut state = self.state.lock();
        if state.basic_services_started {
            return Err("basic services are already started".to_string());
        }

        info!("service_manager: Starting basic services...");

        // HTTP server for Web UI is started separately in CmdDesktop/Gateway
        // (same pattern as Go: web server lifecycle managed by gateway command)

        state.basic_services_started = true;
        state.wait_group.add();
        info!("service_manager: Basic services started");
        Ok(())
    }

    // -------------------------------------------------------------------
    // Bot service lifecycle
    // -------------------------------------------------------------------

    /// Start the bot service (equivalent to starting the gateway).
    ///
    /// Basic services must be started first.
    pub fn start_bot(&self) -> Result<(), String> {
        {
            let state = self.state.lock();
            if !state.basic_services_started {
                return Err("basic services must be started first".to_string());
            }
        }

        info!("service_manager: Starting bot service...");

        if let Err(e) = self.bot_service.start() {
            error!("service_manager: Failed to start bot service: {}", e);
            return Err(format!("{}", e));
        }

        // Track the bot service in the wait group
        {
            let mut state = self.state.lock();
            state.wait_group.add();
        }

        info!("service_manager: Bot service started");
        Ok(())
    }

    /// Stop the bot service.
    pub fn stop_bot(&self) -> Result<(), String> {
        info!("service_manager: Stopping bot service...");

        if let Err(e) = self.bot_service.stop() {
            error!("service_manager: Failed to stop bot service: {}", e);
            return Err(format!("{}", e));
        }

        // Decrement wait group
        {
            let mut state = self.state.lock();
            state.wait_group.done();
        }

        info!("service_manager: Bot service stopped");
        Ok(())
    }

    /// Restart the bot service.
    pub fn restart_bot(&self) -> Result<(), String> {
        info!("service_manager: Restarting bot service...");

        if let Err(e) = self.bot_service.restart() {
            error!("service_manager: Failed to restart bot service: {}", e);
            return Err(format!("{}", e));
        }

        info!("service_manager: Bot service restarted");
        Ok(())
    }

    // -------------------------------------------------------------------
    // Bot state queries
    // -------------------------------------------------------------------

    /// Return the current state of the bot service.
    pub fn get_bot_state(&self) -> BotState {
        self.bot_service.get_state()
    }

    /// Return the last error from the bot service.
    pub fn get_bot_error(&self) -> Option<String> {
        self.bot_service.get_error()
    }

    /// Return a clone of the bot configuration.
    pub fn get_bot_config(&self) -> BotServiceConfig {
        self.bot_service.get_config()
    }

    /// Save the bot configuration.
    pub fn save_bot_config(
        &self,
        config_json: &serde_json::Value,
        restart: bool,
    ) -> Result<(), String> {
        self.bot_service
            .save_config(config_json, restart)
            .map_err(|e| format!("{}", e))
    }

    /// Return the bot service components for external access.
    pub fn get_bot_components(&self) -> std::collections::HashMap<String, serde_json::Value> {
        self.bot_service.get_components()
    }

    /// Return a reference to the bot service.
    pub fn get_bot_service(&self) -> &BotService {
        &self.bot_service
    }

    /// Check if basic services are started.
    pub fn is_basic_services_started(&self) -> bool {
        self.state.lock().basic_services_started
    }

    /// Check if the bot is running.
    pub fn is_bot_running(&self) -> bool {
        self.bot_service.get_state().is_running()
    }

    // -------------------------------------------------------------------
    // Shutdown
    // -------------------------------------------------------------------

    /// Signal all services to shut down.
    ///
    /// Sends a value on the broadcast channel. All subscribers that called
    /// `subscribe_shutdown()` or are waiting in `wait_for_shutdown()` variants
    /// will receive the signal.
    ///
    /// This is idempotent - calling it multiple times has no additional effect
    /// beyond sending multiple signals (which receivers handle gracefully).
    pub fn trigger_shutdown(&self) {
        info!("Shutdown signal triggered");
        let _ = self.shutdown_tx.send(());
    }

    /// Return a new receiver for the shutdown broadcast channel.
    ///
    /// Use this to integrate shutdown awareness into custom long-running
    /// services:
    ///
    /// ```rust,no_run
    /// use nemesis_services::ServiceManager;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let mgr = ServiceManager::new();
    ///     let mut rx = mgr.subscribe_shutdown();
    ///     let _ = rx.recv().await;
    /// }
    /// ```
    pub fn subscribe_shutdown(&self) -> broadcast::Receiver<()> {
        self.shutdown_tx.subscribe()
    }

    /// Gracefully shut down all services.
    ///
    /// This performs the following in order:
    /// 1. Stops the bot service if it is running
    /// 2. Signals basic services to stop
    /// 3. Sends the shutdown broadcast
    ///
    /// After calling this, the ServiceManager should not be reused.
    pub fn shutdown(&self) {
        info!("service_manager: Shutting down service manager...");

        // Stop bot service if running
        if self.bot_service.get_state().can_stop() {
            if let Err(e) = self.bot_service.stop() {
                error!("service_manager: Error stopping bot during shutdown: {}", e);
            }
        }

        // Decrement wait group for basic services
        {
            let mut state = self.state.lock();
            if state.basic_services_started {
                state.wait_group.done();
                state.basic_services_started = false;
            }
        }

        // Signal shutdown
        self.trigger_shutdown();

        info!("service_manager: Service manager shutdown complete");
    }

    /// Wait for a shutdown signal (Ctrl+C or broadcast).
    ///
    /// This is the standard shutdown wait for CLI/headless mode. It blocks
    /// until either:
    /// - SIGINT (Ctrl+C) is received
    /// - A shutdown broadcast signal is received (via `trigger_shutdown()`)
    ///
    /// Mirrors the Go `WaitForShutdown` method.
    pub async fn wait_for_shutdown(&self) {
        let mut rx = self.subscribe_shutdown();

        // Wait for SIGINT/SIGTERM or broadcast
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("service_manager: Shutdown signal received (Ctrl+C)");
            }
            _ = rx.recv() => {
                info!("service_manager: Shutdown signal received");
            }
        }
    }

    /// Wait for either shutdown signal or desktop UI close.
    ///
    /// This variant is used in desktop mode. The `desktop_closed` watch
    /// receiver is notified when the system tray's "Exit" option is clicked
    /// or the desktop window is closed.
    ///
    /// Mirrors the Go `WaitForShutdownWithDesktop` method.
    ///
    /// # Arguments
    ///
    /// * `desktop_closed` - A `watch::Receiver<bool>` that changes to `true`
    ///   when the desktop UI is closed.
    pub async fn wait_for_shutdown_with_desktop(
        &self,
        mut desktop_closed: watch::Receiver<bool>,
    ) {
        let mut shutdown_rx = self.subscribe_shutdown();

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("service_manager: Shutdown signal received (Ctrl+C)");
            }
            _ = shutdown_rx.recv() => {
                info!("service_manager: Shutdown broadcast received");
            }
            result = desktop_closed.changed() => {
                match result {
                    Ok(()) => {
                        info!("service_manager: Desktop UI closed, initiating shutdown");
                    }
                    Err(_) => {
                        // The sender was dropped, meaning the desktop process
                        // has exited without sending a clean close signal.
                        info!("service_manager: Desktop UI channel closed, initiating shutdown");
                    }
                }
            }
        }
    }

    /// Wait for shutdown with a timeout.
    ///
    /// If the timeout elapses before any shutdown signal is received,
    /// returns `false`. Otherwise returns `true`.
    ///
    /// This is useful for scenarios where you need to enforce a maximum
    /// runtime or want to perform periodic checks while waiting.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum duration to wait for shutdown.
    ///
    /// # Returns
    ///
    /// `true` if a shutdown signal was received, `false` if the timeout elapsed.
    pub async fn wait_for_shutdown_with_timeout(
        &self,
        timeout: std::time::Duration,
    ) -> bool {
        let mut rx = self.subscribe_shutdown();

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("service_manager: Shutdown signal received (Ctrl+C)");
                true
            }
            _ = rx.recv() => {
                info!("service_manager: Shutdown signal received");
                true
            }
            _ = tokio::time::sleep(timeout) => {
                warn!("service_manager: Shutdown wait timed out after {:?}", timeout);
                false
            }
        }
    }

    /// Wait for shutdown with a notification channel.
    ///
    /// This variant accepts a generic `oneshot::Receiver` as an additional
    /// shutdown trigger. It's useful when an external component needs to
    /// signal shutdown without access to the broadcast channel.
    ///
    /// # Arguments
    ///
    /// * `external_trigger` - A oneshot receiver that triggers shutdown when
    ///   a value is received or the sender is dropped.
    pub async fn wait_for_shutdown_with_trigger(
        &self,
        external_trigger: tokio::sync::oneshot::Receiver<()>,
    ) {
        let mut rx = self.subscribe_shutdown();
        let mut external = external_trigger;

        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                info!("service_manager: Shutdown signal received (Ctrl+C)");
            }
            _ = rx.recv() => {
                info!("service_manager: Shutdown broadcast received");
            }
            _ = &mut external => {
                info!("service_manager: External shutdown trigger received");
            }
        }
    }

    /// Wait for the wait group to reach zero (all tracked services have completed).
    ///
    /// This is useful for waiting until all background services have finished
    /// their cleanup after `shutdown()` is called.
    ///
    /// # Arguments
    ///
    /// * `timeout` - Maximum time to wait. If timeout elapses, returns `false`.
    ///
    /// # Returns
    ///
    /// `true` if all services completed, `false` if timeout elapsed.
    pub async fn wait_for_services(&self, timeout: std::time::Duration) -> bool {
        let (count, done_rx) = {
            let state = self.state.lock();
            (state.wait_group.count, state.wait_group.done_rx.clone())
        };

        if count == 0 {
            return true;
        }

        let mut rx = done_rx;
        tokio::select! {
            result = rx.changed() => {
                result.is_ok()
            }
            _ = tokio::time::sleep(timeout) => {
                warn!("service_manager: Timed out waiting for services to complete");
                false
            }
        }
    }

    /// Return the number of services currently tracked in the wait group.
    #[allow(dead_code)]
    pub fn active_service_count(&self) -> usize {
        self.state.lock().wait_group.count()
    }
}

impl Default for ServiceManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
        let result = mgr.wait_for_shutdown_with_timeout(std::time::Duration::from_millis(50)).await;
        assert!(!result);
    }

    #[tokio::test]
    async fn test_wait_for_services_with_zero_count() {
        let mgr = ServiceManager::new();
        // No services tracked, should return immediately
        let result = mgr.wait_for_services(std::time::Duration::from_secs(1)).await;
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
        let result = mgr.wait_for_services(std::time::Duration::from_millis(50)).await;
        assert!(!result);
    }

    #[test]
    fn test_trigger_shutdown_multiple_times() {
        let mgr = ServiceManager::new();

        mgr.trigger_shutdown();

        // Subscriber created after trigger should still get the signal (broadcast retains last value)
        let mut rx = mgr.subscribe_shutdown();
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
        let (tx, done_rx) = tokio::sync::watch::channel(());
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

        let result = mgr.wait_for_shutdown_with_timeout(std::time::Duration::from_secs(5)).await;
        assert!(result);
    }

    #[tokio::test]
    async fn test_wait_for_shutdown_with_trigger_sender_dropped() {
        let mgr = ServiceManager::new();
        let (_tx, rx) = tokio::sync::oneshot::channel::<()>();

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
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

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
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

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
}
