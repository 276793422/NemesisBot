//! Process Manager - Manages child process lifecycle.
//!
//! Handles spawning, monitoring, and terminating child processes for
//! desktop mode (dashboard windows, approval popups, etc.).
//!
//! Provides:
//! - `notify_child()` — send a JSON-RPC notification to a child via WebSocket
//! - `call_child()` — send a JSON-RPC request to a child and await the response
//! - `monitor_loop()` — periodic background task to clean up dead children
//! - `spawn_child()` — create a child process, perform pipe handshake, send WS key

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::broadcast;
use tracing::{debug, info, warn};

use super::executor::{ChildProcess, DefaultPlatformExecutor, PlatformExecutor, ProcessStatus};
use super::handshake::{ACK_TIMEOUT, PipeMessage};
use crate::websocket::protocol::Message;
use crate::websocket::server::WebSocketServer;

/// Internal state that can be shared between tasks via Arc<Mutex>.
struct ManagerState {
    /// Active child processes.
    children: HashMap<String, ChildProcess>,
    /// Result channels for temporary windows (e.g., approval).
    result_channels: HashMap<String, tokio::sync::oneshot::Sender<serde_json::Value>>,
}

/// Manages child process lifecycle.
///
/// The ProcessManager coordinates child process creation, pipe-based handshake,
/// WebSocket key distribution, and background monitoring. It owns the WebSocket
/// server and routes notifications/calls to child processes.
pub struct ProcessManager {
    /// Shared mutable state.
    state: Arc<Mutex<ManagerState>>,
    /// Platform executor.
    executor: Arc<dyn PlatformExecutor>,
    /// Auto-incrementing ID counter.
    next_id: AtomicI64,
    /// Shutdown signal.
    shutdown_tx: broadcast::Sender<()>,
    /// WebSocket server for child communication.
    ws_server: Arc<WebSocketServer>,
}

impl ProcessManager {
    /// Create a new ProcessManager with default executor and WebSocket server.
    pub fn new() -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let ws_server = Arc::new(WebSocketServer::new(
            crate::websocket::server::KeyGenerator::new().into(),
        ));
        Self {
            state: Arc::new(Mutex::new(ManagerState {
                children: HashMap::new(),
                result_channels: HashMap::new(),
            })),
            executor: Arc::new(DefaultPlatformExecutor::with_defaults()),
            next_id: AtomicI64::new(0),
            shutdown_tx,
            ws_server,
        }
    }

    /// Create a ProcessManager with a custom executor.
    pub fn with_executor(executor: Arc<dyn PlatformExecutor>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let ws_server = Arc::new(WebSocketServer::new(
            crate::websocket::server::KeyGenerator::new().into(),
        ));
        Self {
            state: Arc::new(Mutex::new(ManagerState {
                children: HashMap::new(),
                result_channels: HashMap::new(),
            })),
            executor,
            next_id: AtomicI64::new(0),
            shutdown_tx,
            ws_server,
        }
    }

    /// Start the process manager.
    ///
    /// Starts the WebSocket server and spawns the background monitor loop task.
    pub async fn start(&self) -> Result<(), String> {
        info!("[ProcessManager] Starting...");

        // Start the WebSocket server
        let port = self.ws_server.start().await?;
        info!("[ProcessManager] WebSocket server started on port {}", port);

        // Start monitor loop in background
        self.spawn_monitor_loop();

        info!("[ProcessManager] Started");
        Ok(())
    }

    /// Stop the process manager and terminate all children.
    pub fn stop(&self) -> Result<(), String> {
        info!("[ProcessManager] Stopping...");

        // Signal shutdown
        let _ = self.shutdown_tx.send(());

        // Terminate all children
        let mut state = self.state.lock();
        for (id, child) in state.children.iter_mut() {
            info!("[ProcessManager] Terminating child: {}", id);
            if let Err(e) = self.executor.terminate_child(child) {
                warn!("[ProcessManager] Failed to terminate child {}: {}", id, e);
            }
            if let Err(e) = self.executor.cleanup(child) {
                warn!("[ProcessManager] Failed to cleanup child {}: {}", id, e);
            }
        }
        state.children.clear();

        // Clear result channels
        state.result_channels.clear();

        // Stop WebSocket server
        self.ws_server.stop();

        info!("[ProcessManager] Stopped");
        Ok(())
    }

    /// Spawn a child process for a specific window type.
    ///
    /// This performs the full lifecycle:
    /// 1. Create the child process with piped stdio
    /// 2. Perform pipe handshake (send handshake, wait for ACK)
    /// 3. Generate and send WebSocket key
    /// 4. Send window data
    /// 5. For non-persistent windows, create a result channel
    ///
    /// Returns (child_id, optional result receiver for temporary windows).
    pub fn spawn_child(
        &self,
        window_type: &str,
        data: &serde_json::Value,
    ) -> Result<
        (
            String,
            Option<tokio::sync::oneshot::Receiver<serde_json::Value>>,
        ),
        String,
    > {
        let child_id = format!("child-{}", self.next_id.fetch_add(1, Ordering::SeqCst));
        info!(
            "[ProcessManager] Spawning child {} (type: {})",
            child_id, window_type
        );

        // Get current executable path
        let exe_path =
            std::env::current_exe().map_err(|e| format!("failed to get executable path: {}", e))?;

        let args = vec![
            "--multiple".to_string(),
            "--child-id".to_string(),
            child_id.clone(),
            "--window-type".to_string(),
            window_type.to_string(),
        ];

        let mut child = self
            .executor
            .spawn_child(exe_path.to_string_lossy().as_ref(), &args)?;

        child.id = child_id.clone();
        child.window_type = window_type.to_string();

        let pid = child.pid;

        // Store child before any async operations
        {
            let mut state = self.state.lock();
            state.children.insert(child_id.clone(), child);
        }

        info!("[ProcessManager] Child {} created (PID: {})", child_id, pid);

        // Perform handshake via pipes
        let handshake_result = self.perform_handshake(&child_id)?;
        if !handshake_result.success {
            self.cleanup_failed_child(&child_id);
            return Err("handshake failed".to_string());
        }

        info!(
            "[ProcessManager] Handshake completed with child {}",
            child_id
        );

        // Generate WebSocket key
        let ws_port = self.ws_server.get_port();
        let ws_key = self.ws_server.key_generator().generate(&child_id, pid);

        info!(
            "[ProcessManager] WS key generated for child {}: port={}",
            child_id, ws_port
        );

        // Send WS key via pipe
        if let Err(e) = self.send_ws_key(&child_id, &ws_key, ws_port) {
            self.cleanup_failed_child(&child_id);
            return Err(format!("failed to send WS key: {}", e));
        }

        info!("[ProcessManager] WS key sent to child {}", child_id);

        // Send window data via pipe
        if let Err(e) = self.send_window_data(&child_id, data) {
            self.cleanup_failed_child(&child_id);
            return Err(format!("failed to send window data: {}", e));
        }

        info!("[ProcessManager] Window data sent to child {}", child_id);

        // Persistent windows (dashboard) don't wait for results
        let is_persistent = window_type == "dashboard";
        if is_persistent {
            info!(
                "[ProcessManager] Child {} is a persistent window (no result waiting)",
                child_id
            );
            return Ok((child_id, None));
        }

        // Temporary windows (e.g., approval): create result channel
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut state = self.state.lock();
            state.result_channels.insert(child_id.clone(), tx);
        }

        // Spawn a task to wait for child result
        self.spawn_wait_for_result(child_id.clone());

        Ok((child_id, Some(rx)))
    }

    /// Perform the parent-side handshake with a child process.
    ///
    /// Sends a handshake message via stdin pipe and waits for ACK on stdout pipe.
    fn perform_handshake(
        &self,
        child_id: &str,
    ) -> Result<super::handshake::HandshakeResult, String> {
        let mut state = self.state.lock();
        let child = state
            .children
            .get_mut(child_id)
            .ok_or_else(|| format!("child not found: {}", child_id))?;

        // Send handshake message
        let handshake_msg = PipeMessage::handshake();
        child.send_message(&handshake_msg)?;

        // Wait for ACK with timeout
        let start = std::time::Instant::now();
        while start.elapsed() < ACK_TIMEOUT {
            match child.read_message::<PipeMessage>() {
                Ok(ack) => {
                    if ack.is_ack() {
                        child.status = ProcessStatus::Handshaking;
                        return Ok(super::handshake::HandshakeResult {
                            success: true,
                            window_id: None,
                            error: None,
                        });
                    } else {
                        return Ok(super::handshake::HandshakeResult {
                            success: false,
                            window_id: None,
                            error: Some(format!("expected ack, got {}", ack.msg_type)),
                        });
                    }
                }
                Err(e) => {
                    return Ok(super::handshake::HandshakeResult {
                        success: false,
                        window_id: None,
                        error: Some(format!("failed to read ACK: {}", e)),
                    });
                }
            }
        }

        Ok(super::handshake::HandshakeResult {
            success: false,
            window_id: None,
            error: Some("ACK timeout".to_string()),
        })
    }

    /// Send the WebSocket key to a child process via stdin pipe.
    fn send_ws_key(&self, child_id: &str, key: &str, port: u16) -> Result<(), String> {
        let mut state = self.state.lock();
        let child = state
            .children
            .get_mut(child_id)
            .ok_or_else(|| format!("child not found: {}", child_id))?;

        // Send WS key message
        let ws_key_msg = PipeMessage::ws_key(key, port, &format!("/child/{}", key));
        child.send_message(&ws_key_msg)?;

        // Wait for ACK
        let start = std::time::Instant::now();
        while start.elapsed() < ACK_TIMEOUT {
            match child.read_message::<PipeMessage>() {
                Ok(ack) => {
                    if ack.is_ack() {
                        return Ok(());
                    } else {
                        return Err(format!("expected ack, got {}", ack.msg_type));
                    }
                }
                Err(e) => return Err(format!("failed to read ACK: {}", e)),
            }
        }
        Err("ACK timeout for WS key".to_string())
    }

    /// Send window data to a child process via stdin pipe.
    fn send_window_data(&self, child_id: &str, data: &serde_json::Value) -> Result<(), String> {
        let mut state = self.state.lock();
        let child = state
            .children
            .get_mut(child_id)
            .ok_or_else(|| format!("child not found: {}", child_id))?;

        // Send window data message
        let window_data_msg = PipeMessage::window_data(data);
        child.send_message(&window_data_msg)?;

        // Wait for ACK
        let start = std::time::Instant::now();
        while start.elapsed() < ACK_TIMEOUT {
            match child.read_message::<PipeMessage>() {
                Ok(ack) => {
                    if ack.is_ack() {
                        return Ok(());
                    } else {
                        return Err(format!("expected ack, got {}", ack.msg_type));
                    }
                }
                Err(e) => return Err(format!("failed to read ACK: {}", e)),
            }
        }
        Err("ACK timeout for window data".to_string())
    }

    /// Clean up a child that failed during spawn.
    fn cleanup_failed_child(&self, child_id: &str) {
        let mut state = self.state.lock();
        if let Some(mut child) = state.children.remove(child_id) {
            let _ = self.executor.terminate_child(&mut child);
            let _ = self.executor.cleanup(&mut child);
        }
        state.result_channels.remove(child_id);
    }

    /// Spawn a background task to wait for child result.
    ///
    /// This polls the WebSocket server for a connection from the child,
    /// then registers an `approval.submit` handler on the connection's dispatcher
    /// that delivers results to the result channel via `submit_result()`.
    fn spawn_wait_for_result(&self, child_id: String) {
        let mut shutdown_rx = self.shutdown_tx.subscribe();
        let ws_server = self.ws_server.clone();
        let state = self.state.clone();

        let handler_child_id = child_id.clone();
        let handler_state = self.state.clone();

        tokio::spawn(async move {
            // Wait for WebSocket connection (up to 10 seconds)
            let mut conn_attempts = 0;
            loop {
                if ws_server.get_connection(&child_id).is_some() {
                    break;
                }
                conn_attempts += 1;
                if conn_attempts > 100 {
                    // 10 seconds total (100 * 100ms)
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }

            if conn_attempts > 100 {
                eprintln!(
                    "[PM] WARNING: WS connection for {} not found after 10s",
                    child_id
                );
            }

            // Register approval.submit handler on the connection's dispatcher
            if let Some(conn) = ws_server.get_connection(&child_id) {
                eprintln!(
                    "[PM] WS connection found for {}, registering approval.submit handler",
                    child_id
                );

                let guard = conn.lock().await;
                let cid = handler_child_id.clone();
                let hs = handler_state.clone();
                guard
                    .dispatcher
                    .register_notification("approval.submit", move |msg| {
                        let action = msg
                            .params
                            .as_ref()
                            .and_then(|p| p.get("action"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("rejected")
                            .to_string();
                        let request_id = msg
                            .params
                            .as_ref()
                            .and_then(|p| p.get("request_id"))
                            .and_then(|a| a.as_str())
                            .unwrap_or("")
                            .to_string();

                        let result = serde_json::json!({
                            "action": action,
                            "request_id": request_id,
                        });

                        eprintln!(
                            "[PM] approval.submit handler fired: action={}, request_id={}",
                            action, request_id
                        );

                        // Submit via result channel
                        let mut s = hs.lock();
                        if let Some(tx) = s.result_channels.remove(&cid) {
                            eprintln!("[PM] Sending result to test channel");
                            let _ = tx.send(result);
                        } else {
                            eprintln!("[PM] WARNING: no result channel for {}", cid);
                        }
                    });
                drop(guard);
            }

            // Wait up to 5 minutes for result (or shutdown)
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(300)) => {
                    debug!("[ProcessManager] Timeout waiting for result from child {}", child_id);
                }
                _ = shutdown_rx.recv() => {
                    debug!("[ProcessManager] Shutdown while waiting for child {}", child_id);
                }
            }

            // Clean up result channel
            {
                let mut s = state.lock();
                s.result_channels.remove(&child_id);
            }
        });
    }

    /// Send a JSON-RPC notification to a child process via WebSocket.
    ///
    /// Notifications are fire-and-forget -- no response is expected.
    /// Returns an error if the child is not found or not connected.
    pub fn notify_child(
        &self,
        child_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<(), String> {
        info!(
            "[ProcessManager] Notifying child {} with method {}",
            child_id, method
        );

        // Verify the child exists
        {
            let state = self.state.lock();
            if !state.children.contains_key(child_id) {
                return Err(format!("child not found: {}", child_id));
            }
        }

        // Send notification via WebSocket server
        self.ws_server.send_notification(child_id, method, params)?;

        debug!(
            "[ProcessManager] Notification sent to child {}: {}",
            child_id, method
        );
        Ok(())
    }

    /// Send a JSON-RPC request to a child process and await the response.
    ///
    /// Uses the WebSocket server's call_child mechanism with a 30-second timeout.
    /// Returns the response message or an error.
    pub async fn call_child(
        &self,
        child_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Message, String> {
        info!(
            "[ProcessManager] Calling child {} with method {}",
            child_id, method
        );

        // Verify the child exists
        {
            let state = self.state.lock();
            if !state.children.contains_key(child_id) {
                return Err(format!("child not found: {}", child_id));
            }
        }

        // Send request via WebSocket server
        let result = self
            .ws_server
            .call_child(child_id, method, params)
            .await
            .map_err(|e| format!("call_child failed: {}", e))?;

        debug!(
            "[ProcessManager] Response received from child {}: {}",
            child_id, method
        );
        Ok(result)
    }

    /// Spawn the background monitor loop task.
    ///
    /// Runs every 30 seconds to check for dead child processes and clean them up.
    fn spawn_monitor_loop(&self) {
        let shutdown_rx = self.shutdown_tx.subscribe();
        let state = self.state.clone();
        let executor = self.executor.clone();
        let ws_server = self.ws_server.clone();

        tokio::spawn(async move {
            let mut shutdown_rx = shutdown_rx;
            let mut interval = tokio::time::interval(Duration::from_secs(30));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        // Collect dead child IDs
                        let dead_ids: Vec<String> = {
                            let s = state.lock();
                            s.children
                                .iter()
                                .filter(|(_, child)| !executor.is_process_alive(child))
                                .map(|(id, _)| id.clone())
                                .collect()
                        };

                        // Clean up dead children
                        for id in dead_ids {
                            info!("[ProcessManager] Child {} is dead, cleaning up", id);

                            let mut s = state.lock();
                            if let Some(mut child) = s.children.remove(&id) {
                                let _ = executor.cleanup(&mut child);
                            }
                            s.result_channels.remove(&id);
                            drop(s);

                            ws_server.remove_connection(&id);
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("[ProcessManager] Monitor loop shutting down");
                        return;
                    }
                }
            }
        });
    }

    /// Terminate a specific child process.
    pub fn terminate_child(&self, child_id: &str) -> Result<(), String> {
        let mut state = self.state.lock();

        let child = state
            .children
            .get_mut(child_id)
            .ok_or_else(|| format!("child not found: {}", child_id))?;

        info!("[ProcessManager] Terminating child: {}", child_id);

        self.executor.terminate_child(child)?;
        self.executor.cleanup(child)?;

        state.children.remove(child_id);
        state.result_channels.remove(child_id);

        // Remove WebSocket connection
        self.ws_server.remove_connection(child_id);

        Ok(())
    }

    /// Get a child process status by ID.
    pub fn get_child(&self, child_id: &str) -> Option<ProcessStatus> {
        let state = self.state.lock();
        state.children.get(child_id).map(|c| c.status)
    }

    /// Find a child by window type (returns the first match).
    pub fn get_child_by_type(&self, window_type: &str) -> Option<String> {
        let state = self.state.lock();
        for (id, child) in state.children.iter() {
            if child.window_type == window_type {
                return Some(id.clone());
            }
        }
        None
    }

    /// Submit a result for a child (used by approval windows).
    pub fn submit_result(&self, child_id: &str, result: serde_json::Value) -> bool {
        let mut state = self.state.lock();
        if let Some(tx) = state.result_channels.remove(child_id) {
            tx.send(result).is_ok()
        } else {
            false
        }
    }

    /// Clean up stale (dead) children synchronously.
    pub fn cleanup_stale(&self) {
        let dead_ids: Vec<String> = {
            let state = self.state.lock();
            state
                .children
                .iter()
                .filter(|(_, child)| !self.executor.is_process_alive(child))
                .map(|(id, _)| id.clone())
                .collect()
        };

        for id in dead_ids {
            info!("[ProcessManager] Cleaning up dead child: {}", id);
            let mut state = self.state.lock();
            if let Some(mut child) = state.children.remove(&id) {
                let _ = self.executor.cleanup(&mut child);
            }
            state.result_channels.remove(&id);
            drop(state);
            self.ws_server.remove_connection(&id);
        }
    }

    /// Return the number of active children.
    pub fn active_count(&self) -> usize {
        self.state.lock().children.len()
    }

    /// Get a reference to the WebSocket server.
    pub fn ws_server(&self) -> &Arc<WebSocketServer> {
        &self.ws_server
    }

    /// Get the WebSocket server port.
    pub fn ws_port(&self) -> u16 {
        self.ws_server.get_port()
    }
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
