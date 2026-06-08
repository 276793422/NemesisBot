//! Webhook inbound channel (HTTP POST, API key auth, request-response blocking).
//!
//! Receives HTTP POST requests, publishes them to the message bus as
//! InboundMessages, and returns the resulting OutboundMessage content
//! as the HTTP response. Supports API key authentication and path-based
//! routing.

use async_trait::async_trait;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::{BaseChannel, Channel};

/// Webhook inbound channel configuration.
#[derive(Debug, Clone)]
pub struct WebhookInboundConfig {
    /// Listen address (e.g. ":9090").
    pub listen_addr: String,
    /// Webhook path (e.g. "/webhook/incoming").
    pub path: String,
    /// API key for authentication (empty = no auth).
    pub api_key: String,
    /// Allowed sender IDs.
    pub allow_from: Vec<String>,
}

impl Default for WebhookInboundConfig {
    fn default() -> Self {
        Self {
            listen_addr: ":9090".to_string(),
            path: "/webhook/incoming".to_string(),
            api_key: String::new(),
            allow_from: Vec::new(),
        }
    }
}

/// Webhook request body.
#[derive(Debug, Deserialize)]
pub struct WebhookRequest {
    pub content: String,
    pub sender_id: Option<String>,
    pub chat_id: Option<String>,
    pub metadata: Option<HashMap<String, serde_json::Value>>,
}

/// Webhook response body.
#[derive(Serialize)]
pub struct WebhookResponse {
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Pending request waiting for response.
struct PendingRequest {
    tx: tokio::sync::oneshot::Sender<String>,
    deadline: tokio::time::Instant,
}

/// Webhook inbound channel.
pub struct WebhookInboundChannel {
    base: BaseChannel,
    config: WebhookInboundConfig,
    running: Arc<parking_lot::RwLock<bool>>,
    pending: dashmap::DashMap<String, PendingRequest>,
    outbound_queue: RwLock<Vec<OutboundMessage>>,
}

impl WebhookInboundChannel {
    /// Creates a new `WebhookInboundChannel`.
    pub fn new(config: WebhookInboundConfig) -> Result<Self> {
        let listen_addr = if config.listen_addr.is_empty() {
            ":9090".to_string()
        } else {
            config.listen_addr.clone()
        };
        let path = if config.path.is_empty() {
            "/webhook/incoming".to_string()
        } else {
            config.path.clone()
        };

        Ok(Self {
            base: BaseChannel::new("webhook"),
            config: WebhookInboundConfig {
                listen_addr,
                path,
                ..config
            },
            running: Arc::new(parking_lot::RwLock::new(false)),
            pending: dashmap::DashMap::new(),
            outbound_queue: RwLock::new(Vec::new()),
        })
    }

    /// Returns the listen address.
    pub fn listen_addr(&self) -> &str {
        &self.config.listen_addr
    }

    /// Returns the webhook path.
    pub fn path(&self) -> &str {
        &self.config.path
    }

    /// Validates the API key from a request header.
    pub fn validate_api_key(&self, provided_key: &str) -> bool {
        if self.config.api_key.is_empty() {
            return true; // no auth required
        }
        self.config.api_key == provided_key
    }

    /// Registers a pending request and returns the receiver.
    pub fn register_pending(
        &self,
        chat_id: String,
        timeout: Duration,
    ) -> tokio::sync::oneshot::Receiver<String> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.insert(
            chat_id,
            PendingRequest {
                tx,
                deadline: tokio::time::Instant::now() + timeout,
            },
        );
        rx
    }

    /// Extracts path segments for routing.
    /// Pattern: /webhook/{channel_name}/{chat_id}
    pub fn extract_routing<'a>(path: &'a str, base_path: &str) -> (Option<&'a str>, Option<&'a str>) {
        let base = base_path.trim_end_matches('/');
        let remaining = match path.strip_prefix(base) {
            Some(r) => r,
            None => return (None, None),
        };
        let remaining = remaining.trim_start_matches('/');
        if remaining.is_empty() {
            return (None, None);
        }

        let parts: Vec<&str> = remaining.splitn(2, '/').collect();
        let channel = parts.first().copied().filter(|s| !s.is_empty());
        let chat_id = parts.get(1).copied().filter(|s| !s.is_empty());

        (channel, chat_id)
    }

    /// Processes a webhook request.
    pub fn process_request(&self, req: &WebhookRequest) -> (String, String, HashMap<String, String>) {
        let sender_id = req.sender_id.clone().unwrap_or_else(|| "webhook".to_string());
        let chat_id = req.chat_id.clone().unwrap_or_else(|| "webhook:default".to_string());

        let mut metadata = HashMap::new();
        metadata.insert("platform".to_string(), "webhook_inbound".to_string());

        if let Some(ref extra) = req.metadata {
            for (k, v) in extra {
                let val = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                metadata.insert(k.clone(), val);
            }
        }

        (sender_id, chat_id, metadata)
    }

    /// Drains all queued outbound messages (for testing).
    pub fn drain_outbound(&self) -> Vec<OutboundMessage> {
        self.outbound_queue.write().drain(..).collect()
    }

    /// Cleans up expired pending requests.
    pub fn cleanup_expired(&self) {
        let now = tokio::time::Instant::now();
        self.pending
            .retain(|_, pr| now < pr.deadline);
    }
}

#[async_trait]
impl Channel for WebhookInboundChannel {
    fn name(&self) -> &str {
        self.base.name()
    }

    fn is_running(&self) -> bool {
        self.base.is_running()
    }

    async fn start(&self) -> Result<()> {
        info!(
            listen_addr = %self.config.listen_addr,
            path = %self.config.path,
            auth = !self.config.api_key.is_empty(),
            "[WebhookInboundChannel] starting webhook inbound channel"
        );
        *self.running.write() = true;
        self.base.set_enabled(true);
        info!("[WebhookInboundChannel] Webhook inbound channel started");
        Ok(())
    }

    async fn stop(&self) -> Result<()> {
        info!("[WebhookInboundChannel] stopping webhook inbound channel");
        *self.running.write() = false;
        self.base.set_enabled(false);
        self.pending.clear();
        Ok(())
    }

    async fn send(&self, msg: OutboundMessage) -> Result<()> {
        if !*self.running.read() {
            return Err(NemesisError::Channel(
                "webhook inbound channel not running".to_string(),
            ));
        }

        self.base.record_sent();

        // Try to resolve a pending request
        if let Some((_, pending)) = self.pending.remove(&msg.chat_id) {
            let _ = pending.tx.send(msg.content);
            return Ok(());
        }

        // No pending request, queue for testing
        debug!(chat_id = %msg.chat_id, "[WebhookInboundChannel] no pending request, queueing");
        self.outbound_queue.write().push(msg);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
