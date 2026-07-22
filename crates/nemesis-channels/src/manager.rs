//! Channel manager: lifecycle management and outbound message routing.
//!
//! `ChannelManager` owns all channel instances, provides registration,
//! start/stop lifecycle, and dispatches outbound messages to the correct
//! channel by name. Supports an optional allowed-channels filter and
//! runs a background dispatch loop consuming from an outbound receiver.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{RwLock, broadcast, mpsc};
use tracing::{debug, error, info, warn};

use nemesis_types::channel::OutboundMessage;
use nemesis_types::error::{NemesisError, Result};

use crate::base::Channel;
use nemesis_types::channel::InboundMessage;

/// Internal channel names that should not be dispatched externally.
/// Messages targeting these channels are silently consumed by the
/// agent loop or other subsystems and must not be forwarded to
/// external channel adapters.
/// Matches Go's constants.INTERNAL_CHANNELS: ["cli", "system", "subagent"].
const INTERNAL_CHANNELS: &[&str] = &["cli", "system", "subagent"];

/// Manager-level metrics.
#[derive(Debug, Default)]
pub struct ManagerMetrics {
    /// Total outbound messages dispatched.
    pub dispatched: std::sync::atomic::AtomicU64,
    /// Messages dropped because the channel was not found.
    pub dropped_not_found: std::sync::atomic::AtomicU64,
    /// Messages dropped because the channel was filtered out.
    pub dropped_filtered: std::sync::atomic::AtomicU64,
    /// Messages dropped because they targeted an internal channel.
    pub dropped_internal: std::sync::atomic::AtomicU64,
    /// Messages that failed to send.
    pub send_errors: std::sync::atomic::AtomicU64,
}

/// Channel status information for reporting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelStatus {
    /// Whether the channel is enabled.
    pub enabled: bool,
    /// Whether the channel is currently running.
    pub running: bool,
}

/// Configuration for channel sync relationships.
///
/// Maps a source channel name to a list of target channel names that should
/// receive synced copies of messages. Mirrors Go's per-channel `SyncTo` config.
#[derive(Debug, Clone, Default)]
pub struct ChannelSyncConfig {
    /// Source channel name -> list of target channel names.
    pub targets: HashMap<String, Vec<String>>,
}

/// Manages the lifecycle of all channel instances.
pub struct ChannelManager {
    channels: RwLock<HashMap<String, Arc<dyn Channel>>>,
    /// If set, only messages targeting these channel names are dispatched.
    allowed_channels: Option<Vec<String>>,
    /// Outbound message sender for the dispatch loop.
    outbound_tx: mpsc::Sender<OutboundMessage>,
    /// Outbound message receiver (taken by the dispatch loop).
    outbound_rx: parking_lot::Mutex<Option<mpsc::Receiver<OutboundMessage>>>,
    /// Aggregate metrics.
    metrics: Arc<ManagerMetrics>,
    /// Sync targets: source channel name -> list of target channel names.
    /// Populated by `setup_sync_targets()`. Mirrors Go's sync target logic.
    sync_targets: RwLock<HashMap<String, Vec<String>>>,
    /// Whether the dispatch loop has been started.
    /// Used by `start_all()` to auto-start the dispatch loop if not yet started.
    dispatch_started: std::sync::atomic::AtomicBool,
    /// Shutdown flag for the dispatch loop.
    /// When set, the dispatch loop exits even if cloned senders are still alive.
    shutdown: std::sync::atomic::AtomicBool,
}

impl ChannelManager {
    /// Creates a new, empty `ChannelManager` with an internal outbound queue.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel::<OutboundMessage>(256);
        Self {
            channels: RwLock::new(HashMap::new()),
            allowed_channels: None,
            outbound_tx: tx,
            outbound_rx: parking_lot::Mutex::new(Some(rx)),
            metrics: Arc::new(ManagerMetrics::default()),
            sync_targets: RwLock::new(HashMap::new()),
            dispatch_started: std::sync::atomic::AtomicBool::new(false),
            shutdown: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Creates a manager with an allowed-channels filter.
    pub fn with_allowed_channels(allowed: Vec<String>) -> Self {
        let mut mgr = Self::new();
        mgr.allowed_channels = if allowed.is_empty() {
            None
        } else {
            Some(allowed)
        };
        mgr
    }

    /// Returns a clone of the outbound sender for publishing messages.
    pub fn outbound_sender(&self) -> mpsc::Sender<OutboundMessage> {
        self.outbound_tx.clone()
    }

    /// Spawns a background task that reads from the outbound receiver
    /// and dispatches messages to the appropriate channel.
    /// Must be called exactly once after registration and before start.
    pub fn start_dispatch_loop(self: &Arc<Self>) -> Result<()> {
        let rx =
            self.outbound_rx.lock().take().ok_or_else(|| {
                NemesisError::Channel("dispatch loop already started".to_string())
            })?;

        self.dispatch_started
            .store(true, std::sync::atomic::Ordering::Release);

        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.dispatch_loop(rx).await;
        });

        info!("[ChannelManager] channel manager dispatch loop started");
        Ok(())
    }

    /// Internal dispatch loop running as a background task.
    ///
    /// Continuously reads outbound messages from the receiver and dispatches
    /// each one to the appropriate channel. Skips internal channels and
    /// channels not in the allowed list. The loop exits when the sender
    /// is dropped (all clones go out of scope) **or** when the shutdown
    /// flag is set (explicit `stop_all()` call).
    async fn dispatch_loop(&self, mut rx: mpsc::Receiver<OutboundMessage>) {
        info!("[ChannelManager] outbound dispatcher started");
        loop {
            // Check shutdown flag before each recv, so stop_all() can break
            // the loop even if cloned senders are still alive.
            if self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                info!("[ChannelManager] dispatch loop shutting down (shutdown flag set)");
                break;
            }

            match rx.recv().await {
                Some(msg) => {
                    // Skip internal channels silently
                    if is_internal_channel(&msg.channel) {
                        debug!(
                            channel = %msg.channel,
                            chat_id = %msg.chat_id,
                            "[ChannelManager] skipping internal channel"
                        );
                        self.metrics
                            .dropped_internal
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        continue;
                    }

                    debug!(
                        channel = %msg.channel,
                        chat_id = %msg.chat_id,
                        content_len = msg.content.len(),
                        "[ChannelManager] received outbound message from bus"
                    );

                    if let Err(e) = self.dispatch_outbound(msg).await {
                        warn!(error = %e, "[ChannelManager] dispatch loop failed to send outbound message");
                    }
                }
                None => {
                    info!("[ChannelManager] dispatch loop shutting down (channel closed)");
                    break;
                }
            }
        }
    }

    /// Returns a reference to the manager metrics.
    pub fn metrics(&self) -> &ManagerMetrics {
        &self.metrics
    }

    /// Registers a channel. If a channel with the same name already exists,
    /// it is replaced and an error is returned.
    pub async fn register(&self, channel: Arc<dyn Channel>) -> Result<()> {
        let name = channel.name().to_string();
        let mut map = self.channels.write().await;
        if map.contains_key(&name) {
            return Err(NemesisError::Channel(format!(
                "channel '{}' already registered",
                name
            )));
        }
        info!(name = %name, "[ChannelManager] registered channel");
        map.insert(name, channel);
        Ok(())
    }

    /// Registers a channel, replacing any existing one with the same name.
    pub async fn register_or_replace(&self, channel: Arc<dyn Channel>) {
        let name = channel.name().to_string();
        let existed = {
            let mut map = self.channels.write().await;
            let existed = map.contains_key(&name);
            map.insert(name.clone(), channel);
            existed
        };
        if existed {
            warn!(name = %name, "[ChannelManager] replaced existing channel registration");
        } else {
            info!(name = %name, "[ChannelManager] registered channel");
        }
    }

    /// Unregisters a channel by name.
    pub async fn unregister(&self, name: &str) -> bool {
        let mut map = self.channels.write().await;
        map.remove(name).is_some()
    }

    /// Starts all registered channels.
    ///
    /// Iterates through all registered channels and calls `start()` on each.
    /// If any channel fails to start, logs the error but continues starting
    /// the remaining channels. This matches Go's behavior where a single
    /// channel failure does not prevent others from starting.
    ///
    /// Also automatically starts the dispatch loop if it hasn't been started
    /// yet. This mirrors Go's behavior where the dispatch loop is started
    /// inside `StartAll()` rather than requiring a separate call.
    pub async fn start_all(self: &Arc<Self>) -> Result<()> {
        // Auto-start dispatch loop if not yet started (M3).
        // In Go, StartAll() starts the dispatch goroutine internally.
        if !self
            .dispatch_started
            .load(std::sync::atomic::Ordering::Acquire)
        {
            self.start_dispatch_loop()?;
        }

        let map = self.channels.read().await;
        if map.is_empty() {
            warn!("[ChannelManager] no channels to start");
            return Ok(());
        }

        info!("[ChannelManager] starting all channels");

        let mut started_count = 0u32;
        for (name, ch) in map.iter() {
            info!(name = %name, "[ChannelManager] starting channel");
            if let Err(e) = ch.start().await {
                error!(name = %name, error = %e, "[ChannelManager] failed to start channel");
                // Continue starting remaining channels
            } else {
                started_count += 1;
                info!(name = %name, "[ChannelManager] started channel");
            }
        }

        info!(
            "[ChannelManager] All channels started, count={}",
            started_count
        );
        Ok(())
    }

    /// Stops all registered channels.
    ///
    /// Iterates through all registered channels and calls `stop()` on each.
    /// Errors are logged but do not prevent stopping remaining channels.
    /// Also sets the shutdown flag so the dispatch loop exits even if
    /// cloned senders are still alive.
    pub async fn stop_all(&self) -> Result<()> {
        // Set shutdown flag so dispatch loop exits (M4).
        // In Go, cancelling the context stops the dispatch goroutine.
        // In Rust, cloned senders keep the mpsc channel alive, so we
        // need an explicit signal.
        self.shutdown
            .store(true, std::sync::atomic::Ordering::Release);

        let map = self.channels.read().await;

        info!("[ChannelManager] stopping all channels");

        for (name, ch) in map.iter() {
            debug!(name = %name, "[ChannelManager] stopping channel");
            if let Err(e) = ch.stop().await {
                error!(name = %name, error = %e, "[ChannelManager] failed to stop channel");
                // Continue stopping remaining channels even on error.
            }
            info!(name = %name, "[ChannelManager] stopped channel");
        }

        info!("[ChannelManager] All channels stopped");
        Ok(())
    }

    /// Returns a reference to a channel by name, if registered.
    pub async fn get(&self, name: &str) -> Option<Arc<dyn Channel>> {
        let map = self.channels.read().await;
        map.get(name).cloned()
    }

    /// Dispatches an outbound message to the correct channel based on `msg.channel`.
    ///
    /// If `allowed_channels` is set, messages targeting non-allowed channels
    /// are silently dropped. Looks up the channel by name and calls `send`.
    /// Returns an error if the channel is not found or the send fails.
    pub async fn dispatch_outbound(&self, msg: OutboundMessage) -> Result<()> {
        let channel_name = msg.channel.clone();

        // Apply allowed-channels filter
        if let Some(ref allowed) = self.allowed_channels {
            if !allowed.contains(&channel_name) {
                self.metrics
                    .dropped_filtered
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    channel = %channel_name,
                    "[ChannelManager] dropping outbound message: channel not in allowed list"
                );
                return Ok(());
            }
        }

        let map = self.channels.read().await;
        match map.get(&channel_name) {
            Some(ch) => {
                debug!(channel = %channel_name, chat_id = %msg.chat_id, "[ChannelManager] dispatching outbound message");
                self.metrics
                    .dispatched
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match ch.send(msg).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
                        error!(
                            "[ChannelManager] Failed to dispatch outbound message to {}: {}",
                            channel_name, e
                        );
                        self.metrics
                            .send_errors
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        Err(e)
                    }
                }
            }
            None => {
                self.metrics
                    .dropped_not_found
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                warn!(
                    "[ChannelManager] No channel found for outbound message, target={}",
                    channel_name
                );
                Err(NemesisError::Channel(format!(
                    "channel '{}' not found for outbound message",
                    channel_name
                )))
            }
        }
    }

    /// Configure sync targets from a `ChannelSyncConfig`.
    ///
    /// Reads the config, validates source/target channels, and calls
    /// `source_channel.add_sync_target(target_name, target_channel)` on each
    /// valid pair to establish the sync relationship at the channel level.
    /// Mirrors Go's `setupSyncTargets()`.
    pub async fn setup_sync_targets(&self, config: &ChannelSyncConfig) {
        info!("[ChannelManager] setting up sync targets");

        let channels = self.channels.read().await;
        let mut targets = HashMap::new();

        for (source_name, target_names) in &config.targets {
            // Skip if the source channel is not registered
            let source_ch = match channels.get(source_name) {
                Some(ch) => Arc::clone(ch),
                None => {
                    warn!(source = %source_name, "[ChannelManager] sync source channel not registered, skipping");
                    continue;
                }
            };

            let mut valid_targets = Vec::new();
            for target_name in target_names {
                // Prevent self-sync
                if target_name == source_name {
                    warn!(channel = %source_name, "[ChannelManager] channel cannot sync to itself, skipping");
                    continue;
                }

                // Check target exists and get reference
                let target_ch = match channels.get(target_name) {
                    Some(ch) => Arc::clone(ch),
                    None => {
                        warn!(source = %source_name, target = %target_name, "[ChannelManager] sync target not found, skipping");
                        continue;
                    }
                };

                // Establish sync relationship on the source channel (matches Go's AddSyncTarget)
                if let Err(e) = source_ch.add_sync_target(target_name, target_ch) {
                    warn!(source = %source_name, target = %target_name, error = %e, "[ChannelManager] failed to add sync target");
                    continue;
                }

                valid_targets.push(target_name.clone());
            }

            if !valid_targets.is_empty() {
                info!(source = %source_name, targets = ?valid_targets, "[ChannelManager] configured sync targets");
                targets.insert(source_name.clone(), valid_targets);
            }
        }

        *self.sync_targets.write().await = targets;
        info!("[ChannelManager] sync targets setup completed");
    }

    /// Get the list of sync target channel names for a given source channel.
    ///
    /// Returns an empty vector if no sync targets are configured for the channel.
    /// Mirrors Go's `getSyncTargets()`.
    pub async fn get_sync_targets(&self, channel_name: &str) -> Vec<String> {
        self.sync_targets
            .read()
            .await
            .get(channel_name)
            .cloned()
            .unwrap_or_default()
    }

    /// Returns the number of registered channels.
    pub async fn channel_count(&self) -> usize {
        self.channels.read().await.len()
    }

    /// Returns the names of all registered channels.
    pub async fn channel_names(&self) -> Vec<String> {
        self.channels.read().await.keys().cloned().collect()
    }

    /// Returns the status of all registered channels (enabled, running).
    pub async fn get_status(&self) -> HashMap<String, ChannelStatus> {
        let map = self.channels.read().await;
        let mut status = HashMap::new();
        for (name, ch) in map.iter() {
            status.insert(
                name.clone(),
                ChannelStatus {
                    enabled: true,
                    running: ch.is_running(),
                },
            );
        }
        status
    }

    /// Sends a message directly to a channel by name, bypassing the dispatch loop.
    ///
    /// This is useful for programmatic message sending where you don't want
    /// to go through the bus. Returns an error if the channel is not found.
    pub async fn send_to_channel(
        &self,
        channel_name: &str,
        chat_id: &str,
        content: &str,
    ) -> Result<()> {
        let map = self.channels.read().await;
        match map.get(channel_name) {
            Some(ch) => {
                let msg = OutboundMessage {
                    channel: channel_name.to_string(),
                    chat_id: chat_id.to_string(),
                    content: content.to_string(),
                    message_type: String::new(),
                    meta: Default::default(),
                };
                ch.send(msg).await
            }
            None => Err(NemesisError::Channel(format!(
                "channel '{}' not found",
                channel_name
            ))),
        }
    }
}

/// Configuration for initializing channels from a config source.
///
/// Mirrors Go's `config.Channels` structure. Each field holds the channel-specific
/// configuration plus an `enabled` flag. The `init_channels` method reads this
/// config and creates/registers the appropriate channel instances.
///
/// Channel-specific configs use `Option` so that only the enabled channels
/// need to have their fields populated.
#[derive(Clone, Default)]
pub struct ChannelInitConfig {
    /// Telegram channel configuration.
    #[cfg(feature = "telegram")]
    pub telegram: Option<crate::telegram::TelegramConfig>,
    /// Discord channel configuration.
    #[cfg(feature = "discord")]
    pub discord: Option<crate::discord::DiscordConfig>,
    /// Slack channel configuration.
    #[cfg(feature = "slack")]
    pub slack: Option<crate::slack::SlackConfig>,
    /// WhatsApp channel configuration.
    #[cfg(feature = "whatsapp")]
    pub whatsapp: Option<crate::whatsapp::WhatsAppConfig>,
    /// Feishu channel configuration.
    #[cfg(feature = "feishu")]
    pub feishu: Option<crate::feishu::FeishuConfig>,
    /// DingTalk channel configuration.
    #[cfg(feature = "dingtalk")]
    pub dingtalk: Option<crate::dingtalk::DingTalkConfig>,
    /// QQ channel configuration.
    #[cfg(feature = "tencent")]
    pub qq: Option<crate::qq::QQConfig>,
    /// Email channel configuration.
    #[cfg(feature = "email")]
    pub email: Option<crate::email::EmailConfig>,
    /// Matrix channel configuration.
    #[cfg(feature = "matrix")]
    pub matrix: Option<crate::matrix::MatrixConfig>,
    /// IRC channel configuration.
    #[cfg(feature = "irc")]
    pub irc: Option<crate::irc::IRCConfig>,
    /// Signal channel configuration.
    #[cfg(feature = "signal")]
    pub signal: Option<crate::signal::SignalConfig>,
    /// Mastodon channel configuration.
    #[cfg(feature = "mastodon")]
    pub mastodon: Option<crate::mastodon::MastodonConfig>,
    /// Bluesky channel configuration.
    #[cfg(feature = "bluesky")]
    pub bluesky: Option<crate::bluesky::BlueskyConfig>,
    /// OneBot channel configuration.
    #[cfg(feature = "onebot")]
    pub onebot: Option<crate::onebot::OneBotConfig>,
    /// LINE channel configuration.
    pub line: Option<crate::line::LineConfig>,
    /// External channel configuration.
    pub external: Option<crate::external::ExternalConfig>,
    /// MaixCam channel configuration.
    pub maixcam: Option<crate::maixcam::MaixCamConfig>,
    /// Web channel configuration.
    pub web: Option<crate::web::WebChannelConfig>,
    /// WebServerOps for injecting into the WebChannel (outbound delivery).
    pub web_server_ops: Option<std::sync::Arc<dyn crate::web::WebServerOps>>,
    /// WebSocket channel configuration.
    pub websocket: Option<crate::websocket::WebSocketChannelConfig>,
}

impl std::fmt::Debug for ChannelInitConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChannelInitConfig")
            .field("web", &self.web)
            .field(
                "web_server_ops",
                &self.web_server_ops.as_ref().map(|_| "Some(...)"),
            )
            .finish()
    }
}

impl ChannelManager {
    /// Initializes channels from a `ChannelInitConfig` and registers them.
    ///
    /// Mirrors Go's `Manager.initChannels()`. For each channel that has
    /// configuration provided (i.e., the `Option` is `Some`), this method
    /// attempts to create a channel instance and register it. Failures are
    /// logged but do not prevent other channels from being initialized.
    ///
    /// The `bus_sender` is passed to channels that need it for publishing
    /// inbound messages (e.g., Telegram, Discord, Slack).
    pub async fn init_channels(
        &self,
        config: &ChannelInitConfig,
        bus_sender: broadcast::Sender<InboundMessage>,
    ) -> Result<()> {
        info!("[ChannelManager] initializing channel manager");

        // Telegram
        #[cfg(feature = "telegram")]
        {
            if let Some(ref cfg) = config.telegram {
                info!("[ChannelManager] attempting to initialize Telegram channel");
                match crate::telegram::TelegramChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Telegram channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Telegram channel");
                    }
                }
            }
        }

        // Discord
        #[cfg(feature = "discord")]
        {
            if let Some(ref cfg) = config.discord {
                info!("[ChannelManager] attempting to initialize Discord channel");
                match crate::discord::DiscordChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Discord channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Discord channel");
                    }
                }
            }
        }

        // Slack
        #[cfg(feature = "slack")]
        {
            if let Some(ref cfg) = config.slack {
                info!("[ChannelManager] attempting to initialize Slack channel");
                match crate::slack::SlackChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Slack channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Slack channel");
                    }
                }
            }
        }

        // WhatsApp
        #[cfg(feature = "whatsapp")]
        {
            if let Some(ref cfg) = config.whatsapp {
                info!("[ChannelManager] attempting to initialize WhatsApp channel");
                match crate::whatsapp::WhatsAppChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] WhatsApp channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize WhatsApp channel");
                    }
                }
            }
        }

        // Feishu
        #[cfg(feature = "feishu")]
        {
            if let Some(ref cfg) = config.feishu {
                info!("[ChannelManager] attempting to initialize Feishu channel");
                match crate::feishu::FeishuChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Feishu channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Feishu channel");
                    }
                }
            }
        }

        // DingTalk
        #[cfg(feature = "dingtalk")]
        {
            if let Some(ref cfg) = config.dingtalk {
                info!("[ChannelManager] attempting to initialize DingTalk channel");
                match crate::dingtalk::DingTalkChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] DingTalk channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize DingTalk channel");
                    }
                }
            }
        }

        // QQ
        #[cfg(feature = "tencent")]
        {
            if let Some(ref cfg) = config.qq {
                info!("[ChannelManager] attempting to initialize QQ channel");
                match crate::qq::QQChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] QQ channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize QQ channel");
                    }
                }
            }
        }

        // Email
        #[cfg(feature = "email")]
        {
            if let Some(ref cfg) = config.email {
                info!("[ChannelManager] attempting to initialize Email channel");
                match crate::email::EmailChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Email channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Email channel");
                    }
                }
            }
        }

        // Matrix
        #[cfg(feature = "matrix")]
        {
            if let Some(ref cfg) = config.matrix {
                info!("[ChannelManager] attempting to initialize Matrix channel");
                match crate::matrix::MatrixChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Matrix channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Matrix channel");
                    }
                }
            }
        }

        // IRC
        #[cfg(feature = "irc")]
        {
            if let Some(ref cfg) = config.irc {
                info!("[ChannelManager] attempting to initialize IRC channel");
                match crate::irc::IRCChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] IRC channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize IRC channel");
                    }
                }
            }
        }

        // Signal
        #[cfg(feature = "signal")]
        {
            if let Some(ref cfg) = config.signal {
                info!("[ChannelManager] attempting to initialize Signal channel");
                match crate::signal::SignalChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Signal channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Signal channel");
                    }
                }
            }
        }

        // Mastodon
        #[cfg(feature = "mastodon")]
        {
            if let Some(ref cfg) = config.mastodon {
                info!("[ChannelManager] attempting to initialize Mastodon channel");
                match crate::mastodon::MastodonChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Mastodon channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Mastodon channel");
                    }
                }
            }
        }

        // Bluesky
        #[cfg(feature = "bluesky")]
        {
            if let Some(ref cfg) = config.bluesky {
                info!("[ChannelManager] attempting to initialize Bluesky channel");
                match crate::bluesky::BlueskyChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] Bluesky channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize Bluesky channel");
                    }
                }
            }
        }

        // OneBot
        #[cfg(feature = "onebot")]
        {
            if let Some(ref cfg) = config.onebot {
                info!("[ChannelManager] attempting to initialize OneBot channel");
                match crate::onebot::OneBotChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] OneBot channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize OneBot channel");
                    }
                }
            }
        }

        // LINE
        {
            if let Some(ref cfg) = config.line {
                info!("[ChannelManager] attempting to initialize LINE channel");
                match crate::line::LineChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] LINE channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize LINE channel");
                    }
                }
            }
        }

        // External
        {
            if let Some(ref cfg) = config.external {
                info!("[ChannelManager] attempting to initialize External channel");
                match crate::external::ExternalChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] External channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize External channel");
                    }
                }
            }
        }

        // MaixCam
        {
            if let Some(ref cfg) = config.maixcam {
                info!("[ChannelManager] attempting to initialize MaixCam channel");
                match crate::maixcam::MaixCamChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("[ChannelManager] MaixCam channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "[ChannelManager] failed to initialize MaixCam channel");
                    }
                }
            }
        }

        // Web (always available, enabled by default)
        {
            if let Some(ref cfg) = config.web {
                info!("[ChannelManager] attempting to initialize Web channel");
                let ch = crate::web::WebChannel::new(cfg.clone());
                if let Some(ref ops) = config.web_server_ops {
                    ch.set_server(ops.clone());
                }
                self.register_or_replace(Arc::new(ch)).await;
                info!("[ChannelManager] Web channel enabled successfully");
            }
        }

        // WebSocket
        {
            if let Some(ref cfg) = config.websocket {
                info!("[ChannelManager] attempting to initialize WebSocket channel");
                let ch = crate::websocket::WebSocketChannel::new(cfg.clone(), bus_sender.clone());
                self.register_or_replace(Arc::new(ch)).await;
                info!("[ChannelManager] WebSocket channel enabled successfully");
            }
        }

        let count = self.channel_count().await;
        info!(
            enabled_channels = count,
            "[ChannelManager] channel initialization completed"
        );

        Ok(())
    }
}

impl Default for ChannelManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Checks whether a channel name is an internal channel that should not be
/// dispatched to external channel adapters.
fn is_internal_channel(channel: &str) -> bool {
    INTERNAL_CHANNELS.contains(&channel)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
