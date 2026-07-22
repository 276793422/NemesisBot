//! Channel trait and BaseChannel implementation.
//!
//! Defines the core `Channel` trait that all channel adapters must implement,
//! and provides `BaseChannel` with common functionality (name, enabled flag, stats,
//! allow-list filtering, and sync-target management).

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tracing::{debug, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

/// Channel adapter trait for sending outbound messages.
///
/// Each channel implementation must provide a name, lifecycle hooks (start/stop),
/// and a `send` method for delivering outbound messages.
#[async_trait]
pub trait Channel: Send + Sync {
    /// Returns the unique name of this channel (e.g. "rpc", "web", "websocket").
    fn name(&self) -> &str;

    /// Starts the channel, initializing any resources or connections.
    async fn start(&self) -> Result<()>;

    /// Stops the channel, releasing resources and closing connections.
    async fn stop(&self) -> Result<()>;

    /// Sends an outbound message through this channel.
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

    /// Checks whether a sender is allowed to use this channel.
    ///
    /// Returns `true` if the allow-list is empty (all senders allowed),
    /// or if the sender matches an entry in the allow-list.
    fn is_allowed(&self, _sender_id: &str) -> bool {
        true
    }

    /// Returns whether the channel is currently running.
    ///
    /// A channel is running after `start()` succeeds and until `stop()` is called.
    /// Mirrors Go's `Channel.IsRunning()` interface method.
    fn is_running(&self) -> bool {
        false
    }

    /// Adds a channel as a sync target for broadcasting messages.
    ///
    /// Returns an error if the target name matches this channel's own name
    /// (self-sync prevention).
    fn add_sync_target(&self, _name: &str, _channel: Arc<dyn Channel>) -> Result<()> {
        Ok(())
    }

    /// Removes a previously added sync target by name.
    fn remove_sync_target(&self, _name: &str) {}
}

// ---------------------------------------------------------------------------
// VoiceTranscriber trait (dependency injection for voice transcription)
// ---------------------------------------------------------------------------

/// Trait for voice transcription services.
///
/// Mirrors Go's `voice.GroqTranscriber` but decoupled from the voice module
/// to avoid circular dependencies. Implementations (e.g., `nemesis-voice::Transcriber`)
/// are injected into channels via `set_transcriber()`.
pub trait VoiceTranscriber: Send + Sync {
    /// Check if the transcriber is available (e.g., has API key configured).
    fn is_available(&self) -> bool;

    /// Transcribe an audio file at the given path.
    ///
    /// Returns the transcribed text on success, or an error description on failure.
    fn transcribe(
        &self,
        file_path: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>,
    >;
}

/// Statistics for a channel instance.
#[derive(Debug, Default)]
pub struct ChannelStats {
    /// Number of messages sent (outbound).
    pub messages_sent: AtomicU64,
    /// Number of messages received (inbound).
    pub messages_received: AtomicU64,
}

/// Base channel providing common functionality.
///
/// Wraps name, enabled flag, message counters, allow-list, and sync targets.
/// Concrete channel implementations can embed this and delegate common operations.
pub struct BaseChannel {
    name: String,
    enabled: parking_lot::RwLock<bool>,
    running: parking_lot::RwLock<bool>,
    stats: Arc<ChannelStats>,
    /// Allow-list for sender filtering. If empty, all senders are allowed.
    allow_list: Vec<String>,
    /// Sync targets for broadcasting messages to other channels.
    sync_targets: parking_lot::RwLock<HashMap<String, Arc<dyn Channel>>>,
}

impl fmt::Debug for BaseChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BaseChannel")
            .field("name", &self.name)
            .field("enabled", &self.is_enabled())
            .field("running", &self.is_running())
            .field("allow_list", &self.allow_list)
            .field(
                "sync_targets",
                &self.sync_targets.read().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl Clone for BaseChannel {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            enabled: parking_lot::RwLock::new(*self.enabled.read()),
            running: parking_lot::RwLock::new(*self.running.read()),
            stats: Arc::clone(&self.stats),
            allow_list: self.allow_list.clone(),
            sync_targets: parking_lot::RwLock::new(self.sync_targets.read().clone()),
        }
    }
}

/// Splits a compound sender ID (e.g. `"123456|username"`) into its ID and username parts.
///
/// Returns `(id_part, user_part)`. If there is no `|` separator, `user_part` is empty.
fn split_sender_id(sender_id: &str) -> (&str, &str) {
    match sender_id.find('|') {
        Some(idx) if idx > 0 => (&sender_id[..idx], &sender_id[idx + 1..]),
        _ => (sender_id, ""),
    }
}

impl BaseChannel {
    /// Creates a new `BaseChannel` with the given name and an empty allow-list.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            enabled: parking_lot::RwLock::new(true),
            running: parking_lot::RwLock::new(false),
            stats: Arc::new(ChannelStats::default()),
            allow_list: Vec::new(),
            sync_targets: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Creates a new `BaseChannel` with the given name and allow-list.
    pub fn with_allow_list(name: impl Into<String>, allow_list: Vec<String>) -> Self {
        Self {
            name: name.into(),
            enabled: parking_lot::RwLock::new(true),
            running: parking_lot::RwLock::new(false),
            stats: Arc::new(ChannelStats::default()),
            allow_list,
            sync_targets: parking_lot::RwLock::new(HashMap::new()),
        }
    }

    /// Returns the channel name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns whether the channel is enabled.
    pub fn is_enabled(&self) -> bool {
        *self.enabled.read()
    }

    /// Sets the enabled flag.
    pub fn set_enabled(&self, value: bool) {
        *self.enabled.write() = value;
    }

    /// Returns whether the channel is currently running.
    ///
    /// Mirrors Go's `BaseChannel.IsRunning()`. A channel is running
    /// after `start()` succeeds and until `stop()` is called.
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    /// Sets the running flag.
    ///
    /// Mirrors Go's `BaseChannel.setRunning()`. Should be called by
    /// concrete channel implementations in their `start()` and `stop()` methods.
    pub fn set_running(&self, value: bool) {
        *self.running.write() = value;
    }

    /// Increments the sent message counter.
    pub fn record_sent(&self) {
        self.stats.messages_sent.fetch_add(1, Ordering::Relaxed);
    }

    /// Increments the received message counter.
    pub fn record_received(&self) {
        self.stats.messages_received.fetch_add(1, Ordering::Relaxed);
    }

    /// Returns the number of messages sent.
    pub fn messages_sent(&self) -> u64 {
        self.stats.messages_sent.load(Ordering::Relaxed)
    }

    /// Returns the number of messages received.
    pub fn messages_received(&self) -> u64 {
        self.stats.messages_received.load(Ordering::Relaxed)
    }

    /// Checks whether a sender is allowed based on the allow-list.
    ///
    /// If the allow-list is empty, all senders are allowed. Otherwise, the sender
    /// ID is checked against each allowed entry using compound-ID matching that
    /// supports formats like `"123456|username"` and `"@username"`.
    pub fn is_allowed(&self, sender_id: &str) -> bool {
        if self.allow_list.is_empty() {
            return true;
        }

        let (id_part, user_part) = split_sender_id(sender_id);

        for allowed in &self.allow_list {
            // Strip leading '@' from allowed value for username matching
            let trimmed = allowed.trim_start_matches('@');
            let (allowed_id, allowed_user) = split_sender_id(trimmed);

            if sender_id == allowed
                || id_part == allowed
                || sender_id == trimmed
                || id_part == trimmed
                || id_part == allowed_id
                || (!allowed_user.is_empty() && sender_id == allowed_user)
                || (!user_part.is_empty()
                    && (user_part == allowed || user_part == trimmed || user_part == allowed_user))
            {
                return true;
            }
        }

        false
    }

    /// Adds a channel as a sync target.
    ///
    /// Returns an error if attempting to add a target with the same name as
    /// this channel (self-sync prevention).
    pub fn add_sync_target(&self, name: &str, channel: Arc<dyn Channel>) -> Result<()> {
        if name == self.name {
            return Err(NemesisError::Channel(
                "channel cannot sync to itself".to_string(),
            ));
        }
        self.sync_targets.write().insert(name.to_string(), channel);
        debug!(target = %name, "[Channel] sync target added");
        Ok(())
    }

    /// Removes a sync target by name.
    ///
    /// No-op if the target was not previously added.
    pub fn remove_sync_target(&self, name: &str) {
        self.sync_targets.write().remove(name);
        debug!(target = %name, "[Channel] sync target removed");
    }

    /// Sends a message to all configured sync targets.
    ///
    /// Skips any target whose name matches this channel's name (double-check).
    /// For the `"web"` target, uses `"web:broadcast"` as the chat ID.
    /// Each send has a 3-second timeout to avoid blocking.
    pub async fn sync_to_targets(&self, content: &str) {
        // Collect targets into a Vec within a block scope so the RwLockReadGuard
        // is dropped before any .await. parking_lot::RwLockReadGuard is !Send,
        // which would make the future !Send if held across an await point.
        let entries: Vec<(String, Arc<dyn Channel>)> = {
            let targets = self.sync_targets.read();
            if targets.is_empty() {
                return;
            }
            targets
                .iter()
                .filter(|(target_name, _)| *target_name != &self.name)
                .map(|(k, v)| (k.clone(), Arc::clone(v)))
                .collect()
        };

        debug!(
            channel = %self.name,
            content_len = content.len(),
            num_targets = entries.len(),
            "[Channel] syncing to targets"
        );

        for (target_name, target_ch) in entries {
            let mut chat_id = String::new();
            if target_name == "web" {
                chat_id = "web:broadcast".to_string();
            }

            let msg = OutboundMessage::new(&target_name, &chat_id, content);

            debug!(
                channel = %self.name,
                target = %target_name,
                content_len = content.len(),
                chat_id = %chat_id,
                "[Channel] sending to sync target"
            );

            match tokio::time::timeout(std::time::Duration::from_secs(3), target_ch.send(msg)).await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(
                        channel = %self.name,
                        target = %target_name,
                        error = %e,
                        "[Channel] failed to sync to target"
                    );
                }
                Err(_) => {
                    warn!(
                        channel = %self.name,
                        target = %target_name,
                        "[Channel] sync to target timed out"
                    );
                }
            }
        }
    }

    /// Handles an inbound message after checking the allow-list.
    ///
    /// If the sender is not allowed, the message is silently dropped.
    /// Otherwise, the message is published to the inbound bus (when a bus
    /// reference is available via the concrete channel implementation).
    ///
    /// Returns `true` if the message was accepted, `false` if blocked.
    pub fn handle_message(&self, sender_id: &str) -> bool {
        if !self.is_allowed(sender_id) {
            return false;
        }
        self.record_received();
        true
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
