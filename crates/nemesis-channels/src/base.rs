//! Channel trait and BaseChannel implementation.
//!
//! Defines the core `Channel` trait that all channel adapters must implement,
//! and provides `BaseChannel` with common functionality (name, enabled flag, stats,
//! allow-list filtering, and sync-target management).

use async_trait::async_trait;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>>;
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
                    && (user_part == allowed
                        || user_part == trimmed
                        || user_part == allowed_user))
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
        debug!(target = %name, "sync target added");
        Ok(())
    }

    /// Removes a sync target by name.
    ///
    /// No-op if the target was not previously added.
    pub fn remove_sync_target(&self, name: &str) {
        self.sync_targets.write().remove(name);
        debug!(target = %name, "sync target removed");
    }

    /// Sends a message to all configured sync targets.
    ///
    /// Skips any target whose name matches this channel's name (double-check).
    /// For the `"web"` target, uses `"web:broadcast"` as the chat ID.
    /// Each send has a 3-second timeout to avoid blocking.
    pub async fn sync_to_targets(&self, content: &str) {
        let targets = self.sync_targets.read();
        if targets.is_empty() {
            return;
        }

        // Clone the entries to avoid holding the lock across await points.
        let entries: Vec<(String, Arc<dyn Channel>)> = targets
            .iter()
            .filter(|(target_name, _)| *target_name != &self.name)
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        drop(targets);

        debug!(
            channel = %self.name,
            content_len = content.len(),
            num_targets = entries.len(),
            "syncing to targets"
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
                "sending to sync target"
            );

            match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                target_ch.send(msg),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    warn!(
                        channel = %self.name,
                        target = %target_name,
                        error = %e,
                        "failed to sync to target"
                    );
                }
                Err(_) => {
                    warn!(
                        channel = %self.name,
                        target = %target_name,
                        "sync to target timed out"
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
mod tests {
    use super::*;

    #[test]
    fn test_base_channel_new() {
        let ch = BaseChannel::new("test-channel");
        assert_eq!(ch.name(), "test-channel");
        assert!(ch.is_enabled());
        assert_eq!(ch.messages_sent(), 0);
        assert_eq!(ch.messages_received(), 0);
    }

    #[test]
    fn test_base_channel_enabled_toggle() {
        let ch = BaseChannel::new("toggle");
        assert!(ch.is_enabled());

        ch.set_enabled(false);
        assert!(!ch.is_enabled());

        ch.set_enabled(true);
        assert!(ch.is_enabled());
    }

    #[test]
    fn test_base_channel_stats() {
        let ch = BaseChannel::new("stats");

        ch.record_sent();
        ch.record_sent();
        ch.record_sent();
        ch.record_received();

        assert_eq!(ch.messages_sent(), 3);
        assert_eq!(ch.messages_received(), 1);
    }

    // ---- is_allowed tests ----

    #[test]
    fn test_is_allowed_empty_list_allows_all() {
        let ch = BaseChannel::new("test");
        assert!(ch.is_allowed("any-user"));
    }

    #[test]
    fn test_is_allowed_exact_match() {
        let ch = BaseChannel::with_allow_list("test", vec!["user1".into(), "user2".into()]);
        assert!(ch.is_allowed("user1"));
        assert!(!ch.is_allowed("user3"));
    }

    #[test]
    fn test_is_allowed_compound_sender_id() {
        // Compound senderID "123456|username" with simple ID in allowlist
        let ch = BaseChannel::with_allow_list("test", vec!["123456".into(), "user1".into()]);
        assert!(ch.is_allowed("123456|username"));
    }

    #[test]
    fn test_is_allowed_compound_both_sides() {
        // Both senderID and allowlist entry are compound
        let ch = BaseChannel::with_allow_list(
            "test",
            vec!["123456|username".into(), "user2".into()],
        );
        assert!(ch.is_allowed("123456|username"));
    }

    #[test]
    fn test_is_allowed_at_prefix() {
        // Allowlist with @ prefix should match username
        let ch = BaseChannel::with_allow_list("test", vec!["@user1".into(), "@user2".into()]);
        assert!(ch.is_allowed("user1"));
    }

    #[test]
    fn test_is_allowed_at_prefix_compound_sender() {
        // @username in allowlist matches compound senderID username part
        let ch = BaseChannel::with_allow_list("test", vec!["@username".into()]);
        assert!(ch.is_allowed("123456|username"));
    }

    #[test]
    fn test_is_allowed_compound_allowlist_matches_id_part() {
        // Compound allowlist entry "123456|correct_user" matches senderID "123456|username"
        // because id_part (123456) == allowed_id (123456)
        let ch = BaseChannel::with_allow_list(
            "test",
            vec!["123456|correct_user".into()],
        );
        assert!(ch.is_allowed("123456|username"));
    }

    #[test]
    fn test_is_allowed_username_part_match() {
        // Simple "username" in allowlist matches the user part of compound senderID
        let ch = BaseChannel::with_allow_list("test", vec!["username".into()]);
        assert!(ch.is_allowed("123456|username"));
    }

    #[test]
    fn test_is_allowed_not_in_list() {
        let ch = BaseChannel::with_allow_list("test", vec!["user1".into(), "user2".into()]);
        assert!(!ch.is_allowed("user3"));
    }

    // ---- split_sender_id tests ----

    #[test]
    fn test_split_sender_id_simple() {
        let (id, user) = split_sender_id("123456");
        assert_eq!(id, "123456");
        assert_eq!(user, "");
    }

    #[test]
    fn test_split_sender_id_compound() {
        let (id, user) = split_sender_id("123456|username");
        assert_eq!(id, "123456");
        assert_eq!(user, "username");
    }

    #[test]
    fn test_split_sender_id_pipe_at_start() {
        // Pipe at position 0, so idx > 0 fails, returns whole string as id
        let (id, user) = split_sender_id("|value");
        assert_eq!(id, "|value");
        assert_eq!(user, "");
    }

    // ---- add_sync_target / remove_sync_target tests ----

    #[test]
    fn test_add_sync_target() {
        let ch = BaseChannel::new("test");
        let target = Arc::new(MockChannel::new("target"));
        assert!(ch.add_sync_target("target", target).is_ok());
    }

    #[test]
    fn test_add_sync_target_prevents_self_sync() {
        let ch = BaseChannel::new("source");
        let target = Arc::new(MockChannel::new("source"));
        let result = ch.add_sync_target("source", target);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("cannot sync to itself"));
    }

    #[test]
    fn test_remove_sync_target() {
        let ch = BaseChannel::new("test");
        let target = Arc::new(MockChannel::new("target"));
        ch.add_sync_target("target", target).unwrap();

        ch.remove_sync_target("target");
        // Removing non-existent should not panic
        ch.remove_sync_target("non-existent");
    }

    #[test]
    fn test_remove_sync_target_noop_for_missing() {
        let ch = BaseChannel::new("test");
        // Should not panic
        ch.remove_sync_target("nonexistent");
    }

    // ---- sync_to_targets tests ----

    #[tokio::test]
    async fn test_sync_to_targets_no_targets() {
        let ch = BaseChannel::new("source");
        // Should not panic or block
        ch.sync_to_targets("test message").await;
    }

    #[tokio::test]
    async fn test_sync_to_targets_sends_to_all() {
        let ch = BaseChannel::new("source");
        let target1 = Arc::new(MockChannel::new("target1"));
        let target2 = Arc::new(MockChannel::new("target2"));

        ch.add_sync_target("target1", Arc::clone(&target1) as Arc<dyn Channel>)
            .unwrap();
        ch.add_sync_target("target2", Arc::clone(&target2) as Arc<dyn Channel>)
            .unwrap();

        ch.sync_to_targets("Test message").await;

        let msgs1 = target1.sent_messages();
        let msgs2 = target2.sent_messages();

        assert_eq!(msgs1.len(), 1);
        assert_eq!(msgs1[0].content, "Test message");
        assert_eq!(msgs2.len(), 1);
        assert_eq!(msgs2[0].content, "Test message");
    }

    #[tokio::test]
    async fn test_sync_to_targets_web_broadcast() {
        let ch = BaseChannel::new("source");
        let web_target = Arc::new(MockChannel::new("web"));

        ch.add_sync_target("web", Arc::clone(&web_target) as Arc<dyn Channel>)
            .unwrap();

        ch.sync_to_targets("Broadcast message").await;

        let msgs = web_target.sent_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat_id, "web:broadcast");
        assert_eq!(msgs[0].content, "Broadcast message");
    }

    // ---- handle_message tests ----

    #[test]
    fn test_handle_message_allowed() {
        let ch = BaseChannel::new("test");
        assert!(ch.handle_message("sender123"));
        assert_eq!(ch.messages_received(), 1);
    }

    #[test]
    fn test_handle_message_blocked() {
        let ch = BaseChannel::with_allow_list("test", vec!["allowed-user".into()]);
        assert!(!ch.handle_message("blocked-user"));
        assert_eq!(ch.messages_received(), 0);
    }

    // ---- Debug impl test ----

    #[test]
    fn test_is_running_default_false() {
        let ch = BaseChannel::new("test");
        assert!(!ch.is_running());
    }

    #[test]
    fn test_set_running_toggle() {
        let ch = BaseChannel::new("test");
        assert!(!ch.is_running());

        ch.set_running(true);
        assert!(ch.is_running());

        ch.set_running(false);
        assert!(!ch.is_running());
    }

    #[test]
    fn test_debug_impl() {
        let ch = BaseChannel::new("test-debug");
        let debug_str = format!("{:?}", ch);
        assert!(debug_str.contains("test-debug"));
    }

    #[test]
    fn test_with_allow_list_creates_properly() {
        let ch = BaseChannel::with_allow_list("test", vec!["user1".into(), "user2".into()]);
        assert_eq!(ch.name(), "test");
        assert!(ch.is_enabled());
        assert!(ch.is_allowed("user1"));
        assert!(ch.is_allowed("user2"));
        assert!(!ch.is_allowed("user3"));
    }

    #[test]
    fn test_handle_message_records_stats() {
        let ch = BaseChannel::new("test");
        assert!(ch.handle_message("sender1"));
        assert!(ch.handle_message("sender2"));
        assert!(ch.handle_message("sender3"));
        assert_eq!(ch.messages_received(), 3);
    }

    #[test]
    fn test_handle_message_blocked_no_stats() {
        let ch = BaseChannel::with_allow_list("test", vec!["allowed".into()]);
        assert!(!ch.handle_message("blocked1"));
        assert!(!ch.handle_message("blocked2"));
        assert!(ch.handle_message("allowed"));
        assert_eq!(ch.messages_received(), 1);
    }

    #[test]
    fn test_stats_increment_independent() {
        let ch = BaseChannel::new("test");
        ch.record_sent();
        ch.record_sent();
        ch.record_received();
        ch.record_received();
        ch.record_received();
        assert_eq!(ch.messages_sent(), 2);
        assert_eq!(ch.messages_received(), 3);
    }

    #[tokio::test]
    async fn test_sync_to_targets_with_failing_target() {
        let ch = BaseChannel::new("source");
        let failing_target = Arc::new(FailingMockChannel::new("fail"));
        ch.add_sync_target("fail", failing_target as Arc<dyn Channel>)
            .unwrap();

        // Should not panic when target fails
        ch.sync_to_targets("test message").await;
    }

    #[tokio::test]
    async fn test_sync_to_targets_skips_self() {
        let ch = BaseChannel::new("self");

        let other = Arc::new(MockChannel::new("other"));
        ch.add_sync_target("other", other.clone() as Arc<dyn Channel>).unwrap();

        ch.sync_to_targets("test").await;
        let msgs = other.sent_messages();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn test_split_sender_id_no_pipe() {
        let (id, user) = split_sender_id("simple");
        assert_eq!(id, "simple");
        assert_eq!(user, "");
    }

    #[test]
    fn test_split_sender_id_multiple_pipes() {
        let (id, user) = split_sender_id("123|user|extra");
        assert_eq!(id, "123");
        assert_eq!(user, "user|extra");
    }

    #[test]
    fn test_is_allowed_empty_string_sender() {
        let ch = BaseChannel::with_allow_list("test", vec!["user1".into()]);
        assert!(!ch.is_allowed(""));
    }

    #[test]
    fn test_is_allowed_empty_string_allowed() {
        let ch = BaseChannel::with_allow_list("test", vec![String::new()]);
        assert!(ch.is_allowed(""));
    }

    // ---- Mock channel that always fails ----

    struct FailingMockChannel {
        name: String,
    }

    impl FailingMockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
            }
        }
    }

    impl fmt::Debug for FailingMockChannel {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("FailingMockChannel")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl Channel for FailingMockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn start(&self) -> Result<()> {
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }

        async fn send(&self, _msg: OutboundMessage) -> Result<()> {
            Err(NemesisError::Channel("send always fails".to_string()))
        }
    }

    // ---- Mock channel for testing ----

    /// A minimal mock channel for testing sync targets.
    struct MockChannel {
        name: String,
        sent: parking_lot::RwLock<Vec<OutboundMessage>>,
    }

    impl MockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                sent: parking_lot::RwLock::new(Vec::new()),
            }
        }

        fn sent_messages(&self) -> Vec<OutboundMessage> {
            self.sent.read().clone()
        }
    }

    impl fmt::Debug for MockChannel {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("MockChannel")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl Channel for MockChannel {
        fn name(&self) -> &str {
            &self.name
        }

        async fn start(&self) -> Result<()> {
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            Ok(())
        }

        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            self.sent.write().push(msg);
            Ok(())
        }
    }

    // ---- Additional comprehensive tests ----

    // === Allow-list edge cases ===

    #[test]
    fn test_is_allowed_pipe_only_sender() {
        // Pipe at position 0 should be treated as whole string
        let ch = BaseChannel::with_allow_list("test", vec!["|value".into()]);
        assert!(ch.is_allowed("|value"));
        assert!(!ch.is_allowed("value"));
    }

    #[test]
    fn test_is_allowed_sender_with_at_prefix() {
        let ch = BaseChannel::with_allow_list("test", vec!["@user1".into()]);
        assert!(ch.is_allowed("@user1"));
        // @ prefix requires matching @ in allowlist
    }

    #[test]
    fn test_is_allowed_at_prefix_in_both() {
        let ch = BaseChannel::with_allow_list("test", vec!["@user1".into()]);
        assert!(ch.is_allowed("@user1"));
    }

    #[test]
    fn test_is_allowed_compound_sender_username_part_match() {
        // "username" in allowlist matches user_part of "123456|username"
        let ch = BaseChannel::with_allow_list("test", vec!["alice".into()]);
        assert!(ch.is_allowed("999|alice"));
    }

    #[test]
    fn test_is_allowed_compound_sender_no_match() {
        let ch = BaseChannel::with_allow_list("test", vec!["bob".into()]);
        assert!(!ch.is_allowed("999|alice"));
    }

    #[test]
    fn test_is_allowed_compound_allowlist_id_match() {
        // "123" in allowlist matches id_part of "123|username"
        let ch = BaseChannel::with_allow_list("test", vec!["123".into()]);
        assert!(ch.is_allowed("123|someone"));
    }

    #[test]
    fn test_is_allowed_long_sender_id() {
        let ch = BaseChannel::with_allow_list("test", vec!["user123".into()]);
        assert!(ch.is_allowed("user123"));
    }

    #[test]
    fn test_is_allowed_multiple_entries_partial() {
        let ch = BaseChannel::with_allow_list(
            "test",
            vec!["user1".into(), "user2".into(), "user3".into()],
        );
        assert!(ch.is_allowed("user2"));
        assert!(!ch.is_allowed("user4"));
    }

    #[test]
    fn test_is_allowed_case_sensitive() {
        let ch = BaseChannel::with_allow_list("test", vec!["User1".into()]);
        assert!(!ch.is_allowed("user1"));
        assert!(ch.is_allowed("User1"));
    }

    #[test]
    fn test_is_allowed_at_username_matches_compound_user_part() {
        // @alice in allowlist matches "123|alice" via user_part == allowed_user
        let ch = BaseChannel::with_allow_list("test", vec!["@alice".into()]);
        assert!(ch.is_allowed("123|alice"));
    }

    #[test]
    fn test_is_allowed_at_username_no_match_different_user() {
        let ch = BaseChannel::with_allow_list("test", vec!["@alice".into()]);
        assert!(!ch.is_allowed("123|bob"));
    }

    // === split_sender_id edge cases ===

    #[test]
    fn test_split_sender_id_single_char_id() {
        let (id, user) = split_sender_id("a|bob");
        assert_eq!(id, "a");
        assert_eq!(user, "bob");
    }

    #[test]
    fn test_split_sender_id_pipe_at_end() {
        let (id, user) = split_sender_id("12345|");
        assert_eq!(id, "12345");
        assert_eq!(user, "");
    }

    #[test]
    fn test_split_sender_id_many_pipes() {
        let (id, user) = split_sender_id("1|2|3|4");
        assert_eq!(id, "1");
        assert_eq!(user, "2|3|4");
    }

    // === Stats edge cases ===

    #[test]
    fn test_stats_large_count() {
        let ch = BaseChannel::new("test");
        for _ in 0..1000 {
            ch.record_sent();
        }
        for _ in 0..500 {
            ch.record_received();
        }
        assert_eq!(ch.messages_sent(), 1000);
        assert_eq!(ch.messages_received(), 500);
    }

    #[test]
    fn test_stats_concurrent_increment() {
        use std::sync::Arc;
        use std::thread;

        let ch = Arc::new(BaseChannel::new("test"));
        let mut handles = vec![];

        for _ in 0..10 {
            let ch_clone = Arc::clone(&ch);
            handles.push(thread::spawn(move || {
                for _ in 0..100 {
                    ch_clone.record_sent();
                    ch_clone.record_received();
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(ch.messages_sent(), 1000);
        assert_eq!(ch.messages_received(), 1000);
    }

    // === Sync target edge cases ===

    #[tokio::test]
    async fn test_add_sync_target_replaces_existing() {
        let ch = BaseChannel::new("source");
        let target1 = Arc::new(MockChannel::new("target"));
        let target2 = Arc::new(MockChannel::new("target"));

        ch.add_sync_target("target", target1).unwrap();
        ch.add_sync_target("target", target2).unwrap();

        // Should replace - only one target with that name
        ch.sync_to_targets("test").await;
        // Not crashing is the test
    }

    #[tokio::test]
    async fn test_sync_to_targets_empty_content() {
        let ch = BaseChannel::new("source");
        let target = Arc::new(MockChannel::new("target"));
        ch.add_sync_target("target", target.clone() as Arc<dyn Channel>).unwrap();

        ch.sync_to_targets("").await;
        let msgs = target.sent_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, "");
    }

    #[tokio::test]
    async fn test_sync_to_targets_large_content() {
        let ch = BaseChannel::new("source");
        let target = Arc::new(MockChannel::new("target"));
        ch.add_sync_target("target", target.clone() as Arc<dyn Channel>).unwrap();

        let large = "x".repeat(1_000_000);
        ch.sync_to_targets(&large).await;
        let msgs = target.sent_messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content.len(), 1_000_000);
    }

    #[tokio::test]
    async fn test_sync_to_targets_unicode_content() {
        let ch = BaseChannel::new("source");
        let target = Arc::new(MockChannel::new("target"));
        ch.add_sync_target("target", target.clone() as Arc<dyn Channel>).unwrap();

        let unicode_content = "Hello 世界 🌍 مرحبا";
        ch.sync_to_targets(unicode_content).await;
        let msgs = target.sent_messages();
        assert_eq!(msgs[0].content, unicode_content);
    }

    #[tokio::test]
    async fn test_sync_to_targets_special_chars_content() {
        let ch = BaseChannel::new("source");
        let target = Arc::new(MockChannel::new("target"));
        ch.add_sync_target("target", target.clone() as Arc<dyn Channel>).unwrap();

        let special = "Test <>&\"'`\n\t\r";
        ch.sync_to_targets(special).await;
        let msgs = target.sent_messages();
        assert_eq!(msgs[0].content, special);
    }

    #[tokio::test]
    async fn test_sync_multiple_targets() {
        let ch = BaseChannel::new("source");
        let t1 = Arc::new(MockChannel::new("t1"));
        let t2 = Arc::new(MockChannel::new("t2"));
        let t3 = Arc::new(MockChannel::new("t3"));

        ch.add_sync_target("t1", t1.clone() as Arc<dyn Channel>).unwrap();
        ch.add_sync_target("t2", t2.clone() as Arc<dyn Channel>).unwrap();
        ch.add_sync_target("t3", t3.clone() as Arc<dyn Channel>).unwrap();

        ch.sync_to_targets("multi-target").await;

        assert_eq!(t1.sent_messages().len(), 1);
        assert_eq!(t2.sent_messages().len(), 1);
        assert_eq!(t3.sent_messages().len(), 1);
    }

    #[tokio::test]
    async fn test_sync_failing_target_does_not_block_others() {
        let ch = BaseChannel::new("source");
        let fail_target = Arc::new(FailingMockChannel::new("fail"));
        let ok_target = Arc::new(MockChannel::new("ok"));

        ch.add_sync_target("fail", fail_target as Arc<dyn Channel>).unwrap();
        ch.add_sync_target("ok", ok_target.clone() as Arc<dyn Channel>).unwrap();

        ch.sync_to_targets("test").await;

        // ok_target should still get the message even though fail_target fails
        assert_eq!(ok_target.sent_messages().len(), 1);
    }

    #[test]
    fn test_sync_target_self_name_filtered() {
        let ch = BaseChannel::new("source");
        // Cannot add self as sync target
        let target = Arc::new(MockChannel::new("source"));
        assert!(ch.add_sync_target("source", target).is_err());
    }

    // === handle_message edge cases ===

    #[test]
    fn test_handle_message_empty_sender() {
        let ch = BaseChannel::new("test");
        assert!(ch.handle_message(""));
        assert_eq!(ch.messages_received(), 1);
    }

    #[test]
    fn test_handle_message_empty_sender_with_allowlist() {
        let ch = BaseChannel::with_allow_list("test", vec!["user1".into()]);
        assert!(!ch.handle_message(""));
    }

    #[test]
    fn test_handle_message_unicode_sender() {
        let ch = BaseChannel::with_allow_list("test", vec!["用户123".into()]);
        assert!(ch.handle_message("用户123"));
        assert!(!ch.handle_message("用户456"));
    }

    #[test]
    fn test_handle_message_rapid_calls() {
        let ch = BaseChannel::new("test");
        for i in 0..1000 {
            assert!(ch.handle_message(&format!("sender{}", i)));
        }
        assert_eq!(ch.messages_received(), 1000);
    }

    // === Running state tests ===

    #[test]
    fn test_running_state_transitions() {
        let ch = BaseChannel::new("test");
        assert!(!ch.is_running());

        ch.set_running(true);
        assert!(ch.is_running());

        ch.set_running(true); // idempotent
        assert!(ch.is_running());

        ch.set_running(false);
        assert!(!ch.is_running());
    }

    // === Enabled state tests ===

    #[test]
    fn test_enabled_state_transitions() {
        let ch = BaseChannel::new("test");
        assert!(ch.is_enabled());

        ch.set_enabled(false);
        assert!(!ch.is_enabled());

        ch.set_enabled(false); // idempotent
        assert!(!ch.is_enabled());

        ch.set_enabled(true);
        assert!(ch.is_enabled());
    }

    // === Debug format tests ===

    #[test]
    fn test_debug_format_includes_sync_targets() {
        let ch = BaseChannel::new("debug-test");
        let target = Arc::new(MockChannel::new("tgt"));
        ch.add_sync_target("tgt", target as Arc<dyn Channel>).unwrap();

        let debug_str = format!("{:?}", ch);
        assert!(debug_str.contains("debug-test"));
        assert!(debug_str.contains("tgt"));
    }

    #[test]
    fn test_debug_format_empty_sync_targets() {
        let ch = BaseChannel::new("empty-sync");
        let debug_str = format!("{:?}", ch);
        assert!(debug_str.contains("empty-sync"));
    }

    #[test]
    fn test_debug_format_disabled() {
        let ch = BaseChannel::new("disabled-ch");
        ch.set_enabled(false);
        let debug_str = format!("{:?}", ch);
        assert!(debug_str.contains("enabled: false"));
    }

    // === Constructor edge cases ===

    #[test]
    fn test_new_with_empty_name() {
        let ch = BaseChannel::new("");
        assert_eq!(ch.name(), "");
    }

    #[test]
    fn test_new_with_special_chars_name() {
        let ch = BaseChannel::new("test-channel_v2.0");
        assert_eq!(ch.name(), "test-channel_v2.0");
    }

    #[test]
    fn test_with_allow_list_empty_vec() {
        let ch = BaseChannel::with_allow_list("test", vec![]);
        assert!(ch.is_allowed("anyone")); // empty list = all allowed
    }

    #[test]
    fn test_with_allow_list_single_entry() {
        let ch = BaseChannel::with_allow_list("test", vec!["only-me".into()]);
        assert!(ch.is_allowed("only-me"));
        assert!(!ch.is_allowed("not-me"));
    }

    #[test]
    fn test_with_allow_list_many_entries() {
        let entries: Vec<String> = (0..100).map(|i| format!("user{}", i)).collect();
        let ch = BaseChannel::with_allow_list("test", entries);
        assert!(ch.is_allowed("user50"));
        assert!(!ch.is_allowed("user100"));
    }

    // === Remove sync target edge cases ===

    #[test]
    fn test_remove_sync_target_twice() {
        let ch = BaseChannel::new("test");
        let target = Arc::new(MockChannel::new("tgt"));
        ch.add_sync_target("tgt", target as Arc<dyn Channel>).unwrap();

        ch.remove_sync_target("tgt");
        ch.remove_sync_target("tgt"); // second removal - no panic
    }

    #[test]
    fn test_add_remove_add_sync_target() {
        let ch = BaseChannel::new("test");
        let t1 = Arc::new(MockChannel::new("tgt"));
        let t2 = Arc::new(MockChannel::new("tgt"));

        ch.add_sync_target("tgt", t1 as Arc<dyn Channel>).unwrap();
        ch.remove_sync_target("tgt");
        ch.add_sync_target("tgt", t2 as Arc<dyn Channel>).unwrap();
    }

    // ---- New tests for coverage improvement ----

    // === Channel trait default implementations ===

    struct BareChannel;

    #[async_trait]
    impl Channel for BareChannel {
        fn name(&self) -> &str { "bare" }
        async fn start(&self) -> Result<()> { Ok(()) }
        async fn stop(&self) -> Result<()> { Ok(()) }
        async fn send(&self, _msg: OutboundMessage) -> Result<()> { Ok(()) }
    }

    #[test]
    fn test_channel_default_is_allowed() {
        let ch = BareChannel;
        // Default is_allowed should return true for any sender
        assert!(ch.is_allowed("anyone"));
        assert!(ch.is_allowed(""));
        assert!(ch.is_allowed("user@domain"));
    }

    #[test]
    fn test_channel_default_is_running() {
        let ch = BareChannel;
        // Default is_running should return false
        assert!(!ch.is_running());
    }

    #[test]
    fn test_channel_default_add_sync_target() {
        let ch = BareChannel;
        let target = Arc::new(MockChannel::new("target"));
        // Default add_sync_target should return Ok
        assert!(ch.add_sync_target("target", target as Arc<dyn Channel>).is_ok());
    }

    #[test]
    fn test_channel_default_remove_sync_target() {
        let ch = BareChannel;
        // Default remove_sync_target should be a no-op (not panic)
        ch.remove_sync_target("anything");
    }

    // === Slow mock for sync timeout ===

    struct SlowMockChannel {
        name: String,
        sent: parking_lot::RwLock<Vec<OutboundMessage>>,
    }

    impl SlowMockChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                sent: parking_lot::RwLock::new(Vec::new()),
            }
        }
    }

    impl fmt::Debug for SlowMockChannel {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("SlowMockChannel")
                .field("name", &self.name)
                .finish()
        }
    }

    #[async_trait]
    impl Channel for SlowMockChannel {
        fn name(&self) -> &str { &self.name }
        async fn start(&self) -> Result<()> { Ok(()) }
        async fn stop(&self) -> Result<()> { Ok(()) }
        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            // Sleep longer than the 3-second sync timeout
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            self.sent.write().push(msg);
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_sync_to_targets_timeout() {
        let ch = BaseChannel::new("source");
        let slow_target = Arc::new(SlowMockChannel::new("slow"));
        ch.add_sync_target("slow", slow_target as Arc<dyn Channel>).unwrap();

        // This should timeout after 3 seconds but not block forever
        let start = tokio::time::Instant::now();
        ch.sync_to_targets("timeout test").await;
        let elapsed = start.elapsed();

        // Should have timed out within ~4 seconds
        assert!(elapsed < std::time::Duration::from_secs(4));
    }

    // === sync_to_targets with mix of fast, slow, and failing ===

    #[tokio::test]
    async fn test_sync_to_targets_mixed_quality() {
        let ch = BaseChannel::new("source");
        let ok = Arc::new(MockChannel::new("ok"));
        let fail = Arc::new(FailingMockChannel::new("fail"));
        let slow = Arc::new(SlowMockChannel::new("slow"));

        ch.add_sync_target("ok", ok.clone() as Arc<dyn Channel>).unwrap();
        ch.add_sync_target("fail", fail as Arc<dyn Channel>).unwrap();
        ch.add_sync_target("slow", slow as Arc<dyn Channel>).unwrap();

        // Should complete (with timeout on slow, error on fail, success on ok)
        let start = tokio::time::Instant::now();
        ch.sync_to_targets("mixed test").await;
        let elapsed = start.elapsed();

        // ok target should have received the message
        assert_eq!(ok.sent_messages().len(), 1);
        assert!(elapsed < std::time::Duration::from_secs(4));
    }

    // === BaseChannel with special name characters ===

    #[test]
    fn test_base_channel_unicode_name() {
        let ch = BaseChannel::new("テスト-channel");
        assert_eq!(ch.name(), "テスト-channel");
    }

    #[test]
    fn test_base_channel_name_with_spaces() {
        let ch = BaseChannel::new("test channel");
        assert_eq!(ch.name(), "test channel");
    }

    // === sync_to_targets with target named same as source but added via map ===

    #[tokio::test]
    async fn test_sync_to_targets_filters_self_name_in_entries() {
        let ch = BaseChannel::new("self");
        // Directly insert into sync_targets map to bypass the self-sync check
        // This tests the double-check filter in sync_to_targets
        let other = Arc::new(MockChannel::new("other"));
        let self_target = Arc::new(MockChannel::new("self"));
        ch.sync_targets.write().insert("other".to_string(), other.clone() as Arc<dyn Channel>);
        ch.sync_targets.write().insert("self".to_string(), self_target as Arc<dyn Channel>);

        ch.sync_to_targets("test").await;

        // "other" should get the message, "self" should be filtered out
        assert_eq!(other.sent_messages().len(), 1);
    }

    // === Stats with reset (via new) ===

    #[test]
    fn test_stats_independent_per_channel() {
        let ch1 = BaseChannel::new("ch1");
        let ch2 = BaseChannel::new("ch2");

        ch1.record_sent();
        ch1.record_sent();
        ch2.record_received();

        assert_eq!(ch1.messages_sent(), 2);
        assert_eq!(ch2.messages_sent(), 0);
        assert_eq!(ch1.messages_received(), 0);
        assert_eq!(ch2.messages_received(), 1);
    }

    // === is_allowed with pipe at position 0 in sender ===

    #[test]
    fn test_is_allowed_pipe_at_start_sender() {
        let ch = BaseChannel::with_allow_list("test", vec!["|value".into()]);
        // "|value" as sender - pipe at position 0, split_sender_id returns ("|value", "")
        assert!(ch.is_allowed("|value"));
    }

    // === is_allowed with allowed_user matching user_part ===

    #[test]
    fn test_is_allowed_allowed_user_matches_user_part() {
        // When allowlist has compound entry "123|alice", and sender is "456|alice"
        // allowed_user = "alice" should match user_part "alice"
        let ch = BaseChannel::with_allow_list("test", vec!["123|alice".into()]);
        // This matches because id_part("456") != allowed_id("123")
        // but user_part("alice") == allowed_user("alice")
        assert!(ch.is_allowed("456|alice"));
    }

    // === is_allowed with @trimmed matching id_part ===

    #[test]
    fn test_is_allowed_at_trimmed_matches_id_part() {
        // @123 in allowlist, trimmed to "123", matches id_part of "123|user"
        let ch = BaseChannel::with_allow_list("test", vec!["@123".into()]);
        assert!(ch.is_allowed("123|user"));
    }

    // === handle_message boundary ===

    #[test]
    fn test_handle_message_with_allowed_user() {
        let ch = BaseChannel::with_allow_list("test", vec!["alice".into()]);
        assert!(ch.handle_message("alice"));
        assert_eq!(ch.messages_received(), 1);
    }
}
