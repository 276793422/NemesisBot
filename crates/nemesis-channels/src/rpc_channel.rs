//! RPC channel for cluster communication.
//!
//! `RPCChannel` is a special channel that handles request-response patterns
//! via correlation IDs. It maintains a map of pending requests and routes
//! responses back by matching the `[rpc:<correlation_id>]` prefix in outbound
//! message content.
//!
//! Key features:
//! - `input()`: Creates a pending request with a correlation ID and returns
//!   a oneshot receiver for the response.
//! - `send()`: Receives outbound messages, parses the correlation ID prefix,
//!   and delivers the response content to the matching pending request.
//! - Periodic cleanup of expired pending requests.
//! - Graceful shutdown with wait group for background tasks.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use rand::Rng;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tracing::{debug, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Configuration for an `RPCChannel`.
#[derive(Debug, Clone)]
pub struct RPCChannelConfig {
    /// Timeout for waiting on an RPC response.
    pub request_timeout: Duration,
    /// Interval between cleanup sweeps for expired pending requests.
    pub cleanup_interval: Duration,
}

impl Default for RPCChannelConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(30),
        }
    }
}

/// A pending RPC request awaiting a response.
struct PendingRequest {
    /// Timestamp when the request was created.
    created_at: tokio::time::Instant,
    /// Custom timeout for this specific request (if set via metadata).
    timeout: Duration,
    /// One-shot sender for delivering the response content.
    tx: oneshot::Sender<String>,
    /// Whether the response has been successfully delivered.
    delivered: bool,
}

/// Special channel for RPC / cluster communication.
///
/// Supports the `input` method for creating pending requests and routes
/// outbound responses by correlation ID prefix `[rpc:<id>]`.
pub struct RPCChannel {
    base: BaseChannel,
    config: RPCChannelConfig,
    pending: RwLock<HashMap<String, PendingRequest>>,
    /// Background task handles for cleanup and listeners.
    tasks: RwLock<Vec<JoinHandle<()>>>,
}

impl RPCChannel {
    /// Creates a new `RPCChannel` with the given configuration.
    pub fn new(config: RPCChannelConfig) -> Self {
        Self {
            base: BaseChannel::new("rpc"),
            config,
            pending: RwLock::new(HashMap::new()),
            tasks: RwLock::new(Vec::new()),
        }
    }

    /// Submits an inbound RPC message and returns a channel that will receive
    /// the response content when it arrives via `send`.
    ///
    /// The caller awaits the returned `oneshot::Receiver` to get the response.
    /// If a correlation ID is provided, it will be used; otherwise a unique
    /// one will be generated.
    pub fn input(&self, correlation_id: &str) -> Result<oneshot::Receiver<String>> {
        let (tx, rx) = oneshot::channel();

        let cid = if correlation_id.is_empty() {
            Self::generate_correlation_id()
        } else {
            correlation_id.to_string()
        };

        let mut map = self.pending.write();
        if map.contains_key(&cid) {
            return Err(NemesisError::Channel(format!(
                "correlation_id '{}' already has a pending request",
                cid
            )));
        }

        self.base.record_received();
        debug!(correlation_id = %cid, "[RPCChannel] registered pending RPC request");

        map.insert(
            cid,
            PendingRequest {
                created_at: tokio::time::Instant::now(),
                timeout: self.config.request_timeout,
                tx,
                delivered: false,
            },
        );

        Ok(rx)
    }

    /// Submits an inbound RPC message with a custom per-request timeout.
    ///
    /// This allows callers to override the default request timeout on a
    /// per-request basis (e.g., for long-running LLM operations).
    pub fn input_with_timeout(
        &self,
        correlation_id: &str,
        timeout: Duration,
    ) -> Result<oneshot::Receiver<String>> {
        let (tx, rx) = oneshot::channel();

        let cid = if correlation_id.is_empty() {
            Self::generate_correlation_id()
        } else {
            correlation_id.to_string()
        };

        let mut map = self.pending.write();
        if map.contains_key(&cid) {
            return Err(NemesisError::Channel(format!(
                "correlation_id '{}' already has a pending request",
                cid
            )));
        }

        self.base.record_received();
        debug!(
            correlation_id = %cid,
            timeout_secs = timeout.as_secs(),
            "[RPCChannel] registered pending RPC request with custom timeout"
        );

        map.insert(
            cid,
            PendingRequest {
                created_at: tokio::time::Instant::now(),
                timeout,
                tx,
                delivered: false,
            },
        );

        Ok(rx)
    }

    /// Returns the number of currently pending requests.
    pub fn pending_count(&self) -> usize {
        self.pending.read().len()
    }

    /// Starts a periodic cleanup task that removes expired pending requests.
    ///
    /// Two cleanup strategies:
    /// - **Undelivered + expired**: The request failed or was abandoned. The
    ///   oneshot sender is dropped, signaling `Err(RecvError)` to the waiter.
    /// - **Delivered + expired**: The response was successfully sent and the
    ///   record can be safely removed without closing the channel (the data
    ///   is already in the buffered oneshot).
    pub fn start_cleanup_task(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        let interval = self.config.cleanup_interval;

        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                if let Some(channel) = weak.upgrade() {
                    channel.cleanup_expired();
                } else {
                    break;
                }
            }
        });

        self.tasks.write().push(handle);
        debug!("[RPCChannel] cleanup task started");
    }

    /// Removes expired pending requests.
    ///
    /// For undelivered expired requests, the oneshot sender is dropped (signaling
    /// an error to the waiter). For delivered expired requests, the record is
    /// simply removed since the response is already buffered.
    fn cleanup_expired(&self) {
        let mut map = self.pending.write();
        let now = tokio::time::Instant::now();

        let expired: Vec<String> = map
            .iter()
            .filter(|(_, v)| !v.delivered && now.duration_since(v.created_at) > v.timeout)
            .map(|(k, _)| k.clone())
            .collect();

        let delivered: Vec<String> = map
            .iter()
            .filter(|(_, v)| v.delivered && now.duration_since(v.created_at) > v.timeout)
            .map(|(k, _)| k.clone())
            .collect();

        for id in &expired {
            if let Some(pending) = map.remove(id) {
                // Dropping the sender signals RecvError to the waiter
                debug!(correlation_id = %id, "[RPCChannel] cleaned up expired undelivered request");
                let _ = pending.tx; // explicitly drop
            }
        }

        for id in &delivered {
            if map.remove(id).is_some() {
                debug!(correlation_id = %id, "[RPCChannel] cleaned up delivered request");
            }
        }

        if !expired.is_empty() || !delivered.is_empty() {
            debug!(
                expired = expired.len(),
                delivered = delivered.len(),
                "[RPCChannel] cleanup sweep completed"
            );
        }
    }

    /// Parses the correlation ID from a `[rpc:<id>]` prefix in content.
    /// Returns `Some((correlation_id, rest_of_content))` if the prefix is found.
    pub fn parse_rpc_prefix(content: &str) -> Option<(String, String)> {
        let rest = content.strip_prefix("[rpc:")?;
        let end = rest.find(']')?;
        if end == 0 {
            return None;
        }
        let correlation_id = rest[..end].to_string();
        let remaining = rest[end + 1..].trim().to_string();
        Some((correlation_id, remaining))
    }

    /// Extracts just the correlation ID from content with `[rpc:<id>]` prefix.
    pub fn extract_correlation_id(content: &str) -> Option<String> {
        Self::parse_rpc_prefix(content).map(|(id, _)| id)
    }

    /// Strips the `[rpc:<id>]` prefix and returns the actual response content.
    pub fn strip_rpc_prefix(content: &str) -> String {
        match Self::parse_rpc_prefix(content) {
            Some((_, rest)) => rest,
            None => content.to_string(),
        }
    }

    /// Generates a unique correlation ID.
    ///
    /// Format: `rpc-<nanosecond_timestamp>-<4-digit-random>` to prevent
    /// collisions on systems with microsecond clocks.
    fn generate_correlation_id() -> String {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let rand_part = rand::thread_rng().gen_range(0..10000);
        format!("rpc-{}-{:04}", ts, rand_part)
    }

    /// Aborts all background tasks (cleanup loop, listeners).
    fn abort_tasks(&self) {
        let mut tasks = self.tasks.write();
        for handle in tasks.drain(..) {
            handle.abort();
        }
    }
}

#[async_trait]
impl Channel for RPCChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        debug!("[RPCChannel] starting");
        self.base.set_enabled(true);
        info!("[RPCChannel] started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        debug!("[RPCChannel] stopping");
        self.base.set_enabled(false);

        // Abort background tasks
        self.abort_tasks();

        // Clean up all pending requests - drop senders to signal waiters
        let mut map = self.pending.write();
        let count = map.len();
        for (id, pending) in map.drain() {
            debug!(correlation_id = %id, "[RPCChannel] cleared pending request on stop");
            let _ = pending.tx; // explicitly drop
        }

        info!(pending_cleared = count, "[RPCChannel] stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.base.record_sent();

        // Only process messages targeted to this channel
        if msg.channel != self.name() {
            warn!(
                msg_channel = %msg.channel,
                ch_name = %self.name(),
                "[RPCChannel] channel mismatch - message not for this channel"
            );
            return Ok(());
        }

        // Try to extract [rpc:<correlation_id>] prefix from the content.
        if let Some((correlation_id, response_content)) = Self::parse_rpc_prefix(&msg.content) {
            let mut map = self.pending.write();
            if let Some(pending) = map.get_mut(&correlation_id) {
                debug!(
                    correlation_id = %correlation_id,
                    content_len = response_content.len(),
                    "[RPCChannel] routing response to pending request"
                );

                // Send the actual content (without prefix) to the waiter
                let tx = {
                    // Take the sender out to send without holding the mutable ref
                    // We need to replace it with a dummy since we can't take from a mutable ref
                    // Actually, we can use std::mem::take approach
                    let tx = std::mem::replace(&mut pending.tx, {
                        // Create a dummy sender/receiver pair to replace the taken sender
                        let (dummy_tx, _) = oneshot::channel();
                        dummy_tx
                    });
                    tx
                };

                match tx.send(response_content.clone()) {
                    Ok(()) => {
                        debug!(
                            correlation_id = %correlation_id,
                            "[RPCChannel] response delivered successfully"
                        );
                        pending.delivered = true;
                    }
                    Err(_) => {
                        warn!(
                            correlation_id = %correlation_id,
                            "[RPCChannel] failed to deliver response (receiver dropped)"
                        );
                    }
                }
            } else {
                warn!(
                    correlation_id = %correlation_id,
                    pending_count = map.len(),
                    "[RPCChannel] no pending request found for RPC response"
                );
                // Log all pending IDs for debugging
                let ids: Vec<&String> = map.keys().collect();
                if !ids.is_empty() {
                    debug!(pending_ids = ?ids, "[RPCChannel] pending correlation IDs");
                }
            }
        } else {
            // No correlation ID prefix -- nothing to route.
            debug!("[RPCChannel] received outbound message without correlation ID prefix");
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
