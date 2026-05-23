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
        info!("[BotService] ServiceManager created");
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
        info!("[BotService] ServiceManager created with custom config");
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

        info!("[BotService] Starting basic services...");

        // HTTP server for Web UI is started separately in CmdDesktop/Gateway
        // (same pattern as Go: web server lifecycle managed by gateway command)

        state.basic_services_started = true;
        state.wait_group.add();
        info!("[BotService] Basic services started");
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

        info!("[BotService] Starting bot service...");

        if let Err(e) = self.bot_service.start() {
            error!("[BotService] Failed to start bot service: {}", e);
            return Err(format!("{}", e));
        }

        // Track the bot service in the wait group
        {
            let mut state = self.state.lock();
            state.wait_group.add();
        }

        info!("[BotService] Bot service started");
        Ok(())
    }

    /// Stop the bot service.
    pub fn stop_bot(&self) -> Result<(), String> {
        info!("[BotService] Stopping bot service...");

        if let Err(e) = self.bot_service.stop() {
            error!("[BotService] Failed to stop bot service: {}", e);
            return Err(format!("{}", e));
        }

        // Decrement wait group
        {
            let mut state = self.state.lock();
            state.wait_group.done();
        }

        info!("[BotService] Bot service stopped");
        Ok(())
    }

    /// Restart the bot service.
    pub fn restart_bot(&self) -> Result<(), String> {
        info!("[BotService] Restarting bot service...");

        if let Err(e) = self.bot_service.restart() {
            error!("[BotService] Failed to restart bot service: {}", e);
            return Err(format!("{}", e));
        }

        info!("[BotService] Bot service restarted");
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
        info!("[BotService] Shutdown signal triggered");
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
        info!("[BotService] Shutting down service manager...");

        // Stop bot service if running
        if self.bot_service.get_state().can_stop() {
            if let Err(e) = self.bot_service.stop() {
                error!("[BotService] Error stopping bot during shutdown: {}", e);
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

        info!("[BotService] Service manager shutdown complete");
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
                info!("[BotService] Shutdown signal received (Ctrl+C)");
            }
            _ = rx.recv() => {
                info!("[BotService] Shutdown signal received");
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
                info!("[BotService] Shutdown signal received (Ctrl+C)");
            }
            _ = shutdown_rx.recv() => {
                info!("[BotService] Shutdown broadcast received");
            }
            result = desktop_closed.changed() => {
                match result {
                    Ok(()) => {
                        info!("[BotService] Desktop UI closed, initiating shutdown");
                    }
                    Err(_) => {
                        // The sender was dropped, meaning the desktop process
                        // has exited without sending a clean close signal.
                        info!("[BotService] Desktop UI channel closed, initiating shutdown");
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
                info!("[BotService] Shutdown signal received (Ctrl+C)");
                true
            }
            _ = rx.recv() => {
                info!("[BotService] Shutdown signal received");
                true
            }
            _ = tokio::time::sleep(timeout) => {
                warn!("[BotService] Shutdown wait timed out after {:?}", timeout);
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
                info!("[BotService] Shutdown signal received (Ctrl+C)");
            }
            _ = rx.recv() => {
                info!("[BotService] Shutdown broadcast received");
            }
            _ = &mut external => {
                info!("[BotService] External shutdown trigger received");
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
                warn!("[BotService] Timed out waiting for services to complete");
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
mod tests;
