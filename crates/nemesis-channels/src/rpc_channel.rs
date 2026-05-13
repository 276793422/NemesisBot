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
        debug!(correlation_id = %cid, "registered pending RPC request");

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
            "registered pending RPC request with custom timeout"
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
        debug!("RPC channel cleanup task started");
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
                debug!(correlation_id = %id, "cleaned up expired undelivered request");
                let _ = pending.tx; // explicitly drop
            }
        }

        for id in &delivered {
            if map.remove(id).is_some() {
                debug!(correlation_id = %id, "cleaned up delivered request");
            }
        }

        if !expired.is_empty() || !delivered.is_empty() {
            debug!(
                expired = expired.len(),
                delivered = delivered.len(),
                "RPC cleanup sweep completed"
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

    async fn start(&self) -> Result<()> {
        debug!("starting RPC channel");
        self.base.set_enabled(true);
        info!("RPC channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        debug!("stopping RPC channel");
        self.base.set_enabled(false);

        // Abort background tasks
        self.abort_tasks();

        // Clean up all pending requests - drop senders to signal waiters
        let mut map = self.pending.write();
        let count = map.len();
        for (id, pending) in map.drain() {
            debug!(correlation_id = %id, "cleared pending request on stop");
            let _ = pending.tx; // explicitly drop
        }

        info!(pending_cleared = count, "RPC channel stopped");
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        self.base.record_sent();

        // Only process messages targeted to this channel
        if msg.channel != self.name() {
            warn!(
                msg_channel = %msg.channel,
                ch_name = %self.name(),
                "channel mismatch - message not for this channel"
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
                    "routing RPC response to pending request"
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
                            "response delivered successfully via Send"
                        );
                        pending.delivered = true;
                    }
                    Err(_) => {
                        warn!(
                            correlation_id = %correlation_id,
                            "failed to deliver response (receiver dropped)"
                        );
                    }
                }
            } else {
                warn!(
                    correlation_id = %correlation_id,
                    pending_count = map.len(),
                    "no pending request found for RPC response"
                );
                // Log all pending IDs for debugging
                let ids: Vec<&String> = map.keys().collect();
                if !ids.is_empty() {
                    debug!(pending_ids = ?ids, "pending correlation IDs");
                }
            }
        } else {
            // No correlation ID prefix -- nothing to route.
            debug!("RPC channel received outbound message without correlation ID prefix");
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rpc_channel_start_stop() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        assert_eq!(ch.name(), "rpc");

        ch.start().await.unwrap();
        assert!(ch.base.is_enabled());

        ch.stop().await.unwrap();
        assert!(!ch.base.is_enabled());
    }

    #[tokio::test]
    async fn test_rpc_input_and_send() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("corr-123").unwrap();

        // Simulate an outbound response with correlation ID prefix.
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:corr-123] Hello from RPC".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        // The oneshot receiver should get the response (without prefix).
        let response = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert!(response.is_ok());
        let content = response.unwrap().unwrap();
        assert_eq!(content, "Hello from RPC");
    }

    #[tokio::test]
    async fn test_rpc_input_with_timeout() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch
            .input_with_timeout("corr-timeout", Duration::from_secs(300))
            .unwrap();

        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:corr-timeout] Long running response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert!(response.is_ok());
        let content = response.unwrap().unwrap();
        assert_eq!(content, "Long running response");
    }

    #[test]
    fn test_rpc_parse_prefix() {
        // Valid prefix
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:abc-123] Hello world").unwrap();
        assert_eq!(id, "abc-123");
        assert_eq!(rest, "Hello world");

        // Valid prefix with no trailing content
        let (id, _rest) = RPCChannel::parse_rpc_prefix("[rpc:id]").unwrap();
        assert_eq!(id, "id");

        // Invalid: no prefix
        assert!(RPCChannel::parse_rpc_prefix("Hello world").is_none());

        // Invalid: malformed prefix
        assert!(RPCChannel::parse_rpc_prefix("[rpc:").is_none());

        // Invalid: empty ID
        assert!(RPCChannel::parse_rpc_prefix("[rpc:] content").is_none());
    }

    #[test]
    fn test_extract_correlation_id() {
        assert_eq!(
            RPCChannel::extract_correlation_id("[rpc:corr-123] Hello"),
            Some("corr-123".to_string())
        );
        assert_eq!(RPCChannel::extract_correlation_id("No prefix"), None);
    }

    #[test]
    fn test_strip_rpc_prefix() {
        assert_eq!(
            RPCChannel::strip_rpc_prefix("[rpc:corr-123] Hello world"),
            "Hello world"
        );
        assert_eq!(RPCChannel::strip_rpc_prefix("No prefix"), "No prefix");
    }

    #[tokio::test]
    async fn test_rpc_input_duplicate_fails() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input("dup-id").unwrap();
        let result = ch.input("dup-id");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_send_channel_mismatch_ignored() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input("corr-mismatch").unwrap();

        // Message to wrong channel should be ignored
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:corr-mismatch] Should be ignored".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        // Pending request should still exist
        assert_eq!(ch.pending_count(), 1);
    }

    #[tokio::test]
    async fn test_rpc_send_no_pending_request() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        // Send response for a correlation ID that has no pending request
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:unknown-id] Orphan response".to_string(),
            message_type: String::new(),
        };
        // Should not error, just log a warning
        ch.send(msg).await.unwrap();
    }

    #[tokio::test]
    async fn test_rpc_cleanup_expired() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_millis(50),
            cleanup_interval: Duration::from_millis(10),
        };
        let ch = RPCChannel::new(config);
        ch.start().await.unwrap();

        // Create a pending request that will expire
        let _rx = ch.input("expire-me").unwrap();
        assert_eq!(ch.pending_count(), 1);

        // Wait for timeout to pass
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Run cleanup
        ch.cleanup_expired();

        // The expired request should be removed
        assert_eq!(ch.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_rpc_cleanup_delivered_kept_until_timeout() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_millis(100),
            cleanup_interval: Duration::from_millis(10),
        };
        let ch = RPCChannel::new(config);
        ch.start().await.unwrap();

        let rx = ch.input("delivered-me").unwrap();

        // Deliver the response
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:delivered-me] Response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert!(response.is_ok());

        // Right after delivery, the request should still be in the map (marked delivered)
        assert_eq!(ch.pending_count(), 1);

        // Wait for timeout and cleanup
        tokio::time::sleep(Duration::from_millis(150)).await;
        ch.cleanup_expired();

        // Now the delivered request should be cleaned up
        assert_eq!(ch.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_rpc_stop_clears_pending() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input("stop-1").unwrap();
        let _rx2 = ch.input("stop-2").unwrap();
        assert_eq!(ch.pending_count(), 2);

        ch.stop().await.unwrap();
        assert_eq!(ch.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_rpc_auto_generated_correlation_id() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        // Input with empty correlation ID should auto-generate one
        let rx = ch.input("").unwrap();
        assert_eq!(ch.pending_count(), 1);

        // Verify we can find the auto-generated ID
        let map = ch.pending.read();
        let generated_id = map.keys().next().unwrap().clone();
        drop(map);

        assert!(generated_id.starts_with("rpc-"));

        // Deliver response
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: format!("[rpc:{}] Auto response", generated_id),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let response = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert!(response.is_ok());
        assert_eq!(response.unwrap().unwrap(), "Auto response");
    }

    #[tokio::test]
    async fn test_pending_count() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        assert_eq!(ch.pending_count(), 0);

        let _rx1 = ch.input("count-1").unwrap();
        assert_eq!(ch.pending_count(), 1);

        let _rx2 = ch.input("count-2").unwrap();
        assert_eq!(ch.pending_count(), 2);
    }

    #[tokio::test]
    async fn test_rpc_send_no_correlation_id_prefix() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input("no-prefix-test").unwrap();

        // Send message without [rpc:...] prefix - should not deliver
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "No prefix here".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        // Pending request should still exist (not delivered)
        assert_eq!(ch.pending_count(), 1);
    }

    #[tokio::test]
    async fn test_rpc_send_empty_correlation_id_prefix() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        // Send message with [rpc:] empty ID - should not match
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:] No ID".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
        // Should not crash
    }

    #[test]
    fn test_rpc_parse_prefix_special_chars() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:test-123_abc] Response").unwrap();
        assert_eq!(id, "test-123_abc");
        assert_eq!(rest, "Response");
    }

    #[test]
    fn test_rpc_parse_prefix_only_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:only-id]").unwrap();
        assert_eq!(id, "only-id");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_rpc_parse_prefix_no_space_after_bracket() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:corr-123]NoSpace").unwrap();
        assert_eq!(id, "corr-123");
        assert_eq!(rest, "NoSpace");
    }

    #[test]
    fn test_strip_rpc_prefix_with_space() {
        assert_eq!(
            RPCChannel::strip_rpc_prefix("[rpc:test] Hello"),
            "Hello"
        );
    }

    #[test]
    fn test_strip_rpc_prefix_without_space() {
        assert_eq!(
            RPCChannel::strip_rpc_prefix("[rpc:test]Hello"),
            "Hello"
        );
    }

    #[test]
    fn test_strip_rpc_prefix_no_match() {
        assert_eq!(
            RPCChannel::strip_rpc_prefix("No prefix here"),
            "No prefix here"
        );
    }

    #[tokio::test]
    async fn test_rpc_multiple_requests() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input("multi-1").unwrap();
        let rx2 = ch.input("multi-2").unwrap();
        let _rx3 = ch.input("multi-3").unwrap();
        assert_eq!(ch.pending_count(), 3);

        // Deliver response to second request
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:multi-2] Response to 2".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx2).await;
        assert!(resp.is_ok());
        assert_eq!(resp.unwrap().unwrap(), "Response to 2");

        // Others should still be pending
        assert_eq!(ch.pending_count(), 3); // still in map but delivered=true for multi-2
    }

    #[tokio::test]
    async fn test_rpc_input_auto_generates_unique_ids() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input("").unwrap();
        let _rx2 = ch.input("").unwrap();
        assert_eq!(ch.pending_count(), 2);

        // Both should have unique IDs
        let map = ch.pending.read();
        let ids: Vec<&String> = map.keys().collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn test_rpc_start_stop_clears_pending() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input("clear-me").unwrap();
        assert_eq!(ch.pending_count(), 1);

        ch.stop().await.unwrap();
        assert_eq!(ch.pending_count(), 0);

        // Restart should work
        ch.start().await.unwrap();
        assert_eq!(ch.pending_count(), 0);
    }

    #[tokio::test]
    async fn test_rpc_config_default() {
        let config = RPCChannelConfig::default();
        assert_eq!(config.request_timeout, Duration::from_secs(60));
        assert_eq!(config.cleanup_interval, Duration::from_secs(30));
    }

    #[tokio::test]
    async fn test_rpc_channel_records_received() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        assert_eq!(ch.base.messages_received(), 0);
        let _rx = ch.input("stats-test").unwrap();
        assert_eq!(ch.base.messages_received(), 1);
    }

    #[tokio::test]
    async fn test_rpc_channel_records_sent() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        assert_eq!(ch.base.messages_sent(), 0);

        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "chat-1".to_string(),
            content: "[rpc:any] Response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
        assert_eq!(ch.base.messages_sent(), 1);
    }

    // ---- Additional comprehensive RPC channel tests ----

    // === Parse prefix edge cases ===

    #[test]
    fn test_parse_prefix_with_url_correlation_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:https://example.com/id] Response").unwrap();
        assert_eq!(id, "https://example.com/id");
        assert_eq!(rest, "Response");
    }

    #[test]
    fn test_parse_prefix_with_path_correlation_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:/path/to/id] Content").unwrap();
        assert_eq!(id, "/path/to/id");
        assert_eq!(rest, "Content");
    }

    #[test]
    fn test_parse_prefix_unicode_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:テスト-123] Response").unwrap();
        assert_eq!(id, "テスト-123");
        assert_eq!(rest, "Response");
    }

    #[test]
    fn test_parse_prefix_emoji_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:test-🎉-123] Response").unwrap();
        assert_eq!(id, "test-🎉-123");
        assert_eq!(rest, "Response");
    }

    #[test]
    fn test_parse_prefix_very_long_id() {
        let long_id = "a".repeat(1000);
        let content = format!("[rpc:{}] Content", long_id);
        let (id, rest) = RPCChannel::parse_rpc_prefix(&content).unwrap();
        assert_eq!(id, long_id);
        assert_eq!(rest, "Content");
    }

    #[test]
    fn test_parse_prefix_multiple_brackets() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:id1] [rpc:id2] Content").unwrap();
        assert_eq!(id, "id1");
        assert_eq!(rest, "[rpc:id2] Content");
    }

    #[test]
    fn test_parse_prefix_nested_brackets() {
        let (id, _rest) = RPCChannel::parse_rpc_prefix("[rpc:[nested]] Content").unwrap();
        assert_eq!(id, "[nested");
    }

    #[test]
    fn test_parse_prefix_missing_closing_bracket() {
        assert!(RPCChannel::parse_rpc_prefix("[rpc:test Content").is_none());
    }

    #[test]
    fn test_parse_prefix_empty_content_after_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:id]").unwrap();
        assert_eq!(id, "id");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_parse_prefix_whitespace_after_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:id]   ").unwrap();
        assert_eq!(id, "id");
        assert_eq!(rest, "");
    }

    #[test]
    fn test_parse_prefix_newlines_in_content() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:id] Line1\nLine2\nLine3").unwrap();
        assert_eq!(id, "id");
        assert_eq!(rest, "Line1\nLine2\nLine3");
    }

    #[test]
    fn test_parse_prefix_tabs_in_content() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:id]\tTabbed\tcontent").unwrap();
        assert_eq!(id, "id");
        assert_eq!(rest, "Tabbed\tcontent");
    }

    #[test]
    fn test_strip_prefix_complex_cases() {
        assert_eq!(RPCChannel::strip_rpc_prefix("[rpc:test] Line1\nLine2"), "Line1\nLine2");
        // Whitespace-only content gets trimmed
        assert_eq!(RPCChannel::strip_rpc_prefix("[rpc:test]   \n\t  "), "");
    }

    // === Input edge cases ===

    #[tokio::test]
    async fn test_rpc_input_records_received_per_call() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input("stats-1").unwrap();
        let _rx2 = ch.input("stats-2").unwrap();
        let _rx3 = ch.input("stats-3").unwrap();

        assert_eq!(ch.base.messages_received(), 3);
    }

    #[tokio::test]
    async fn test_rpc_input_with_custom_timeout_records_received() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input_with_timeout("timeout-stats", Duration::from_secs(10)).unwrap();
        assert_eq!(ch.base.messages_received(), 1);
    }

    #[tokio::test]
    async fn test_rpc_input_with_timeout_duplicate_fails() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx1 = ch.input_with_timeout("dup-timeout", Duration::from_secs(10)).unwrap();
        let result = ch.input_with_timeout("dup-timeout", Duration::from_secs(20));
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_rpc_input_with_timeout_delivers_response() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input_with_timeout("custom-timeout-resp", Duration::from_secs(10)).unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:custom-timeout-resp] Custom timeout response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "Custom timeout response");
    }

    // === Send edge cases ===

    #[tokio::test]
    async fn test_rpc_send_records_sent_on_every_call() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        for i in 0..5 {
            let msg = OutboundMessage {
                channel: "rpc".to_string(),
                chat_id: "c".to_string(),
                content: format!("[rpc:id{}] msg", i),
                message_type: String::new(),
            };
            ch.send(msg).await.unwrap();
        }
        assert_eq!(ch.base.messages_sent(), 5);
    }

    #[tokio::test]
    async fn test_rpc_send_with_empty_content_after_prefix() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("empty-resp").unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:empty-resp]".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "");
    }

    #[tokio::test]
    async fn test_rpc_send_with_unicode_content() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("unicode-resp").unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:unicode-resp] 你好世界 🌍 مرحبا".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "你好世界 🌍 مرحبا");
    }

    #[tokio::test]
    async fn test_rpc_send_with_multiline_content() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("multi-line").unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:multi-line] Line1\nLine2\nLine3".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "Line1\nLine2\nLine3");
    }

    #[tokio::test]
    async fn test_rpc_send_with_large_content() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("large-resp").unwrap();
        let large = "x".repeat(100_000);
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: format!("[rpc:large-resp] {}", large),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(2), rx).await;
        assert_eq!(resp.unwrap().unwrap().len(), 100_000);
    }

    // === Multiple concurrent requests ===

    #[tokio::test]
    async fn test_rpc_concurrent_requests_delivered_correctly() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let mut receivers = Vec::new();
        for i in 0..10 {
            let rx = ch.input(&format!("concurrent-{}", i)).unwrap();
            receivers.push(rx);
        }
        assert_eq!(ch.pending_count(), 10);

        // Deliver responses in random order
        for idx in [3, 7, 1, 9, 0, 5, 2, 8, 4, 6] {
            let msg = OutboundMessage {
                channel: "rpc".to_string(),
                chat_id: "c".to_string(),
                content: format!("[rpc:concurrent-{}] Answer {}", idx, idx),
                message_type: String::new(),
            };
            ch.send(msg).await.unwrap();
        }

        for (i, rx) in receivers.into_iter().enumerate() {
            let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
            assert_eq!(resp.unwrap().unwrap(), format!("Answer {}", i));
        }
    }

    // === Cleanup edge cases ===

    #[tokio::test]
    async fn test_rpc_cleanup_mixed_expired_and_valid() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_millis(50),
            cleanup_interval: Duration::from_millis(10),
        };
        let ch = RPCChannel::new(config);
        ch.start().await.unwrap();

        // Create one that will expire
        let _rx_expire = ch.input("will-expire").unwrap();

        // Create one with custom longer timeout
        let _rx_survive = ch.input_with_timeout("will-survive", Duration::from_secs(10)).unwrap();

        assert_eq!(ch.pending_count(), 2);

        // Wait for default timeout to pass
        tokio::time::sleep(Duration::from_millis(100)).await;
        ch.cleanup_expired();

        // Only the custom timeout one should survive
        assert_eq!(ch.pending_count(), 1);
    }

    #[tokio::test]
    async fn test_rpc_cleanup_no_expired_requests() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_secs(60),
            cleanup_interval: Duration::from_secs(10),
        };
        let ch = RPCChannel::new(config);
        ch.start().await.unwrap();

        let _rx = ch.input("no-expire").unwrap();
        assert_eq!(ch.pending_count(), 1);

        ch.cleanup_expired();
        assert_eq!(ch.pending_count(), 1); // still there
    }

    // === Stop edge cases ===

    #[tokio::test]
    async fn test_rpc_stop_drops_pending_senders() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx1 = ch.input("drop-1").unwrap();
        let rx2 = ch.input("drop-2").unwrap();
        let rx3 = ch.input("drop-3").unwrap();

        ch.stop().await.unwrap();
        assert_eq!(ch.pending_count(), 0);

        // All receivers should get errors (sender dropped)
        assert!(rx1.await.is_err());
        assert!(rx2.await.is_err());
        assert!(rx3.await.is_err());
    }

    #[tokio::test]
    async fn test_rpc_stop_idempotent() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        ch.stop().await.unwrap();
        ch.stop().await.unwrap(); // second stop should succeed
        assert_eq!(ch.pending_count(), 0);
    }

    // === Config edge cases ===

    #[test]
    fn test_rpc_config_custom_values() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_secs(300),
            cleanup_interval: Duration::from_secs(15),
        };
        assert_eq!(config.request_timeout, Duration::from_secs(300));
        assert_eq!(config.cleanup_interval, Duration::from_secs(15));
    }

    #[test]
    fn test_rpc_config_zero_timeout() {
        let config = RPCChannelConfig {
            request_timeout: Duration::ZERO,
            cleanup_interval: Duration::ZERO,
        };
        let ch = RPCChannel::new(config);
        assert_eq!(ch.name(), "rpc");
    }

    // === Generated correlation ID format ===

    #[tokio::test]
    async fn test_rpc_auto_id_format() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input("").unwrap();
        assert_eq!(ch.pending_count(), 1);
    }

    // === Receiver dropped before send ===

    #[tokio::test]
    async fn test_rpc_send_to_dropped_receiver() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        {
            let _rx = ch.input("dropped-rx").unwrap();
            // rx goes out of scope - receiver dropped
        }

        // Send should not panic, just log warning
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:dropped-rx] Response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
    }

    // === Dispatch loop integration ===

    #[tokio::test]
    async fn test_rpc_send_with_special_chars_in_correlation_id() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("test-123_abc.xyz").unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:test-123_abc.xyz] Response".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "Response");
    }

    // === Pending count accuracy ===

    #[tokio::test]
    async fn test_rpc_pending_count_after_delivery() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("count-deliver").unwrap();
        assert_eq!(ch.pending_count(), 1);

        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:count-deliver] ok".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        // After delivery, pending count stays 1 (marked delivered, not removed yet)
        assert_eq!(ch.pending_count(), 1);

        // Consume the response
        let _ = rx.await;

        // Still 1 until cleanup
        assert_eq!(ch.pending_count(), 1);
    }

    // ---- New tests for coverage improvement ----

    // === input_with_timeout with empty correlation ID ===

    #[tokio::test]
    async fn test_rpc_input_with_timeout_auto_generates_id() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input_with_timeout("", Duration::from_secs(10)).unwrap();
        assert_eq!(ch.pending_count(), 1);

        // Verify auto-generated ID format
        let map = ch.pending.read();
        let id = map.keys().next().unwrap();
        assert!(id.starts_with("rpc-"));
    }

    // === start_cleanup_task with Arc ===

    #[tokio::test]
    async fn test_rpc_start_cleanup_task() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_millis(50),
            cleanup_interval: Duration::from_millis(20),
        };
        let ch = Arc::new(RPCChannel::new(config));
        ch.start().await.unwrap();

        // Start cleanup task
        ch.start_cleanup_task();

        // Create a request that will expire
        let _rx = ch.input("cleanup-task-test").unwrap();
        assert_eq!(ch.pending_count(), 1);

        // Wait for cleanup
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be cleaned up
        assert_eq!(ch.pending_count(), 0);

        ch.stop().await.unwrap();
    }

    // === stop with active cleanup task ===

    #[tokio::test]
    async fn test_rpc_stop_aborts_cleanup_task() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_secs(60),
            cleanup_interval: Duration::from_millis(10),
        };
        let ch = Arc::new(RPCChannel::new(config));
        ch.start().await.unwrap();
        ch.start_cleanup_task();

        let _rx = ch.input("abort-test").unwrap();

        // Stop should abort the cleanup task and clear pending
        ch.stop().await.unwrap();
        assert_eq!(ch.pending_count(), 0);
    }

    // === Multiple input/send cycles ===

    #[tokio::test]
    async fn test_rpc_multiple_cycles() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        // First cycle
        let rx1 = ch.input("cycle-1").unwrap();
        ch.send(OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:cycle-1] First".to_string(),
            message_type: String::new(),
        }).await.unwrap();
        let resp1 = tokio::time::timeout(Duration::from_secs(1), rx1).await;
        assert_eq!(resp1.unwrap().unwrap(), "First");

        // Second cycle with same channel
        let rx2 = ch.input("cycle-2").unwrap();
        ch.send(OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:cycle-2] Second".to_string(),
            message_type: String::new(),
        }).await.unwrap();
        let resp2 = tokio::time::timeout(Duration::from_secs(1), rx2).await;
        assert_eq!(resp2.unwrap().unwrap(), "Second");

        // Stats accumulated
        assert_eq!(ch.base.messages_received(), 2);
        assert_eq!(ch.base.messages_sent(), 2);
    }

    // === Send with different message types ===

    #[tokio::test]
    async fn test_rpc_send_ignores_message_type() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let rx = ch.input("msg-type").unwrap();
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:msg-type] Response".to_string(),
            message_type: "history".to_string(),
        };
        ch.send(msg).await.unwrap();

        let resp = tokio::time::timeout(Duration::from_secs(1), rx).await;
        assert_eq!(resp.unwrap().unwrap(), "Response");
    }

    // === extract_correlation_id edge cases ===

    #[test]
    fn test_extract_correlation_id_various() {
        // Valid
        assert_eq!(
            RPCChannel::extract_correlation_id("[rpc:abc] content"),
            Some("abc".to_string())
        );
        // No prefix
        assert_eq!(RPCChannel::extract_correlation_id("no prefix"), None);
        // Empty
        assert_eq!(RPCChannel::extract_correlation_id(""), None);
        // Just prefix, no content
        assert_eq!(
            RPCChannel::extract_correlation_id("[rpc:test]"),
            Some("test".to_string())
        );
    }

    // === strip_rpc_prefix edge cases ===

    #[test]
    fn test_strip_rpc_prefix_various() {
        assert_eq!(RPCChannel::strip_rpc_prefix("[rpc:abc] Hello"), "Hello");
        assert_eq!(RPCChannel::strip_rpc_prefix("[rpc:abc]"), "");
        assert_eq!(RPCChannel::strip_rpc_prefix("no prefix"), "no prefix");
        assert_eq!(RPCChannel::strip_rpc_prefix(""), "");
        assert_eq!(
            RPCChannel::strip_rpc_prefix("[rpc:abc] Multi\nLine\nContent"),
            "Multi\nLine\nContent"
        );
    }

    // === parse_rpc_prefix with various correlation IDs ===

    #[test]
    fn test_parse_rpc_prefix_uuid_like() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:550e8400-e29b-41d4-a716-446655440000] Response").unwrap();
        assert_eq!(id, "550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(rest, "Response");
    }

    #[test]
    fn test_parse_rpc_prefix_numeric_id() {
        let (id, rest) = RPCChannel::parse_rpc_prefix("[rpc:12345] ok").unwrap();
        assert_eq!(id, "12345");
        assert_eq!(rest, "ok");
    }

    #[test]
    fn test_parse_rpc_prefix_empty_string() {
        assert!(RPCChannel::parse_rpc_prefix("").is_none());
    }

    // === Concurrent input and send ===

    #[tokio::test]
    async fn test_rpc_concurrent_input_and_send() {
        let ch = Arc::new(RPCChannel::new(RPCChannelConfig::default()));
        ch.start().await.unwrap();

        let mut handles = vec![];
        for i in 0..10 {
            let ch_clone = ch.clone();
            handles.push(tokio::spawn(async move {
                let cid = format!("concurrent-{}", i);
                let rx = ch_clone.input(&cid).unwrap();
                ch_clone.send(OutboundMessage {
                    channel: "rpc".to_string(),
                    chat_id: "c".to_string(),
                    content: format!("[rpc:{}] Answer {}", cid, i),
                    message_type: String::new(),
                }).await.unwrap();
                let resp = tokio::time::timeout(Duration::from_secs(2), rx).await;
                assert_eq!(resp.unwrap().unwrap(), format!("Answer {}", i));
            }));
        }

        for h in handles {
            h.await.unwrap();
        }
        assert_eq!(ch.base.messages_received(), 10);
        assert_eq!(ch.base.messages_sent(), 10);
    }

    // === Stop then restart with new requests ===

    #[tokio::test]
    async fn test_rpc_stop_restart_new_requests() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        // Create and deliver
        let rx = ch.input("before-stop").unwrap();
        ch.send(OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:before-stop] Before".to_string(),
            message_type: String::new(),
        }).await.unwrap();
        let _ = rx.await;

        ch.stop().await.unwrap();

        // Restart
        ch.start().await.unwrap();

        // New request after restart
        let rx2 = ch.input("after-restart").unwrap();
        ch.send(OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:after-restart] After".to_string(),
            message_type: String::new(),
        }).await.unwrap();
        let resp = tokio::time::timeout(Duration::from_secs(1), rx2).await;
        assert_eq!(resp.unwrap().unwrap(), "After");
    }

    // === Config clone ===

    #[test]
    fn test_rpc_config_clone() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_secs(120),
            cleanup_interval: Duration::from_secs(45),
        };
        let cloned = config.clone();
        assert_eq!(cloned.request_timeout, Duration::from_secs(120));
        assert_eq!(cloned.cleanup_interval, Duration::from_secs(45));
    }

    // === Config debug ===

    #[test]
    fn test_rpc_config_debug() {
        let config = RPCChannelConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("60s") || debug_str.contains("60"));
    }

    // === generate_correlation_id uniqueness ===

    #[test]
    fn test_rpc_generate_unique_ids_stress() {
        let mut ids = std::collections::HashSet::new();
        for _ in 0..1000 {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let rand_part = rand::thread_rng().gen_range(0..10000);
            let id = format!("rpc-{}-{:04}", ts, rand_part);
            assert!(ids.insert(id), "Duplicate correlation ID generated");
        }
        assert_eq!(ids.len(), 1000);
    }

    // === Send message to mismatched channel still records sent ===

    #[tokio::test]
    async fn test_rpc_send_mismatch_channel_still_records_sent() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:any] content".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();
        // Even on mismatch, sent counter should increment
        assert_eq!(ch.base.messages_sent(), 1);
    }

    // === Cleanup with only delivered requests ===

    #[tokio::test]
    async fn test_rpc_cleanup_only_delivered() {
        let config = RPCChannelConfig {
            request_timeout: Duration::from_millis(50),
            cleanup_interval: Duration::from_millis(10),
        };
        let ch = RPCChannel::new(config);
        ch.start().await.unwrap();

        let rx = ch.input("delivered-only").unwrap();
        ch.send(OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "[rpc:delivered-only] Response".to_string(),
            message_type: String::new(),
        }).await.unwrap();
        let _ = rx.await;

        // Wait for timeout
        tokio::time::sleep(Duration::from_millis(100)).await;
        ch.cleanup_expired();

        assert_eq!(ch.pending_count(), 0);
    }

    // === Pending count after multiple deliveries ===

    #[tokio::test]
    async fn test_rpc_pending_count_many_deliveries() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        for i in 0..5 {
            let rx = ch.input(&format!("many-{}", i)).unwrap();
            ch.send(OutboundMessage {
                channel: "rpc".to_string(),
                chat_id: "c".to_string(),
                content: format!("[rpc:many-{}] ok", i),
                message_type: String::new(),
            }).await.unwrap();
            let _ = rx.await;
        }

        // All 5 still in map (delivered but not cleaned up)
        assert_eq!(ch.pending_count(), 5);
    }

    // === Send with content that has rpc-like but not prefix ===

    #[tokio::test]
    async fn test_rpc_send_rpc_like_but_not_prefix() {
        let ch = RPCChannel::new(RPCChannelConfig::default());
        ch.start().await.unwrap();

        let _rx = ch.input("test-prefix").unwrap();

        // Content has rpc: but not as [rpc:...] prefix
        let msg = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c".to_string(),
            content: "rpc:something not a prefix".to_string(),
            message_type: String::new(),
        };
        ch.send(msg).await.unwrap();

        // Should not deliver since no [rpc:...] prefix
        assert_eq!(ch.pending_count(), 1);
    }
}
