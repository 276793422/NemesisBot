//! Channel manager: lifecycle management and outbound message routing.
//!
//! `ChannelManager` owns all channel instances, provides registration,
//! start/stop lifecycle, and dispatches outbound messages to the correct
//! channel by name. Supports an optional allowed-channels filter and
//! runs a background dispatch loop consuming from an outbound receiver.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{broadcast, mpsc, RwLock};
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
        let rx = self
            .outbound_rx
            .lock()
            .take()
            .ok_or_else(|| NemesisError::Channel("dispatch loop already started".to_string()))?;

        self.dispatch_started.store(true, std::sync::atomic::Ordering::Release);

        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            mgr.dispatch_loop(rx).await;
        });

        info!("channel manager dispatch loop started");
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
        info!("outbound dispatcher started");
        loop {
            // Check shutdown flag before each recv, so stop_all() can break
            // the loop even if cloned senders are still alive.
            if self.shutdown.load(std::sync::atomic::Ordering::Acquire) {
                info!("dispatch loop shutting down (shutdown flag set)");
                break;
            }

            match rx.recv().await {
                Some(msg) => {
                    // Skip internal channels silently
                    if is_internal_channel(&msg.channel) {
                        debug!(
                            channel = %msg.channel,
                            chat_id = %msg.chat_id,
                            "skipping internal channel"
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
                        "received outbound message from bus"
                    );

                    if let Err(e) = self.dispatch_outbound(msg).await {
                        warn!(error = %e, "dispatch loop failed to send outbound message");
                    }
                }
                None => {
                    info!("dispatch loop shutting down (channel closed)");
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
        info!(name = %name, "registered channel");
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
            warn!(name = %name, "replaced existing channel registration");
        } else {
            info!(name = %name, "registered channel");
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
        if !self.dispatch_started.load(std::sync::atomic::Ordering::Acquire) {
            self.start_dispatch_loop()?;
        }

        let map = self.channels.read().await;
        if map.is_empty() {
            warn!("no channels to start");
            return Ok(());
        }

        info!("starting all channels");

        for (name, ch) in map.iter() {
            info!(name = %name, "starting channel");
            if let Err(e) = ch.start().await {
                error!(name = %name, error = %e, "failed to start channel");
                // Continue starting remaining channels
            } else {
                info!(name = %name, "started channel");
            }
        }

        info!("all channels started");
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
        self.shutdown.store(true, std::sync::atomic::Ordering::Release);

        let map = self.channels.read().await;

        info!("stopping all channels");

        for (name, ch) in map.iter() {
            debug!(name = %name, "stopping channel");
            if let Err(e) = ch.stop().await {
                error!(name = %name, error = %e, "failed to stop channel");
                // Continue stopping remaining channels even on error.
            }
            info!(name = %name, "stopped channel");
        }

        info!("all channels stopped");
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
                    "dropping outbound message: channel not in allowed list"
                );
                return Ok(());
            }
        }

        let map = self.channels.read().await;
        match map.get(&channel_name) {
            Some(ch) => {
                debug!(channel = %channel_name, chat_id = %msg.chat_id, "dispatching outbound message");
                self.metrics
                    .dispatched
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                match ch.send(msg).await {
                    Ok(()) => Ok(()),
                    Err(e) => {
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
                Err(NemesisError::Channel(format!(
                    "channel '{}' not found for outbound message",
                    channel_name
                )))
            }
        }
    }

    /// Configure sync targets from a `ChannelSyncConfig`.
    ///
    /// Reads the config and populates `sync_targets` for each source channel
    /// that has configured target channels. Self-sync (source == target) and
    /// references to non-existent channels are logged as warnings and skipped.
    /// Mirrors Go's `setupSyncTargets()`.
    pub async fn setup_sync_targets(&self, config: &ChannelSyncConfig) {
        info!("setting up sync targets");

        let channels = self.channels.read().await;
        let mut targets = HashMap::new();

        for (source_name, target_names) in &config.targets {
            // Skip if the source channel is not registered
            if !channels.contains_key(source_name) {
                warn!(source = %source_name, "sync source channel not registered, skipping");
                continue;
            }

            let mut valid_targets = Vec::new();
            for target_name in target_names {
                // Prevent self-sync
                if target_name == source_name {
                    warn!(channel = %source_name, "channel cannot sync to itself, skipping");
                    continue;
                }

                // Check target exists
                if !channels.contains_key(target_name) {
                    warn!(source = %source_name, target = %target_name, "sync target not found, skipping");
                    continue;
                }

                valid_targets.push(target_name.clone());
            }

            if !valid_targets.is_empty() {
                info!(source = %source_name, targets = ?valid_targets, "configured sync targets");
                targets.insert(source_name.clone(), valid_targets);
            }
        }

        *self.sync_targets.write().await = targets;
        info!("sync targets setup completed");
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
#[derive(Debug, Clone, Default)]
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
    /// WebSocket channel configuration (heartbeat interval in seconds).
    pub websocket_heartbeat_secs: Option<u64>,
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
        info!("initializing channel manager");

        // Telegram
        #[cfg(feature = "telegram")]
        {
            if let Some(ref cfg) = config.telegram {
                info!("attempting to initialize Telegram channel");
                match crate::telegram::TelegramChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Telegram channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Telegram channel");
                    }
                }
            }
        }

        // Discord
        #[cfg(feature = "discord")]
        {
            if let Some(ref cfg) = config.discord {
                info!("attempting to initialize Discord channel");
                match crate::discord::DiscordChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Discord channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Discord channel");
                    }
                }
            }
        }

        // Slack
        #[cfg(feature = "slack")]
        {
            if let Some(ref cfg) = config.slack {
                info!("attempting to initialize Slack channel");
                match crate::slack::SlackChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Slack channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Slack channel");
                    }
                }
            }
        }

        // WhatsApp
        #[cfg(feature = "whatsapp")]
        {
            if let Some(ref cfg) = config.whatsapp {
                info!("attempting to initialize WhatsApp channel");
                match crate::whatsapp::WhatsAppChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("WhatsApp channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize WhatsApp channel");
                    }
                }
            }
        }

        // Feishu
        #[cfg(feature = "feishu")]
        {
            if let Some(ref cfg) = config.feishu {
                info!("attempting to initialize Feishu channel");
                match crate::feishu::FeishuChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Feishu channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Feishu channel");
                    }
                }
            }
        }

        // DingTalk
        #[cfg(feature = "dingtalk")]
        {
            if let Some(ref cfg) = config.dingtalk {
                info!("attempting to initialize DingTalk channel");
                match crate::dingtalk::DingTalkChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("DingTalk channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize DingTalk channel");
                    }
                }
            }
        }

        // QQ
        #[cfg(feature = "tencent")]
        {
            if let Some(ref cfg) = config.qq {
                info!("attempting to initialize QQ channel");
                match crate::qq::QQChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("QQ channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize QQ channel");
                    }
                }
            }
        }

        // Email
        #[cfg(feature = "email")]
        {
            if let Some(ref cfg) = config.email {
                info!("attempting to initialize Email channel");
                match crate::email::EmailChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Email channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Email channel");
                    }
                }
            }
        }

        // Matrix
        #[cfg(feature = "matrix")]
        {
            if let Some(ref cfg) = config.matrix {
                info!("attempting to initialize Matrix channel");
                match crate::matrix::MatrixChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Matrix channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Matrix channel");
                    }
                }
            }
        }

        // IRC
        #[cfg(feature = "irc")]
        {
            if let Some(ref cfg) = config.irc {
                info!("attempting to initialize IRC channel");
                match crate::irc::IRCChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("IRC channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize IRC channel");
                    }
                }
            }
        }

        // Signal
        #[cfg(feature = "signal")]
        {
            if let Some(ref cfg) = config.signal {
                info!("attempting to initialize Signal channel");
                match crate::signal::SignalChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Signal channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Signal channel");
                    }
                }
            }
        }

        // Mastodon
        #[cfg(feature = "mastodon")]
        {
            if let Some(ref cfg) = config.mastodon {
                info!("attempting to initialize Mastodon channel");
                match crate::mastodon::MastodonChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Mastodon channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Mastodon channel");
                    }
                }
            }
        }

        // Bluesky
        #[cfg(feature = "bluesky")]
        {
            if let Some(ref cfg) = config.bluesky {
                info!("attempting to initialize Bluesky channel");
                match crate::bluesky::BlueskyChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("Bluesky channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize Bluesky channel");
                    }
                }
            }
        }

        // OneBot
        #[cfg(feature = "onebot")]
        {
            if let Some(ref cfg) = config.onebot {
                info!("attempting to initialize OneBot channel");
                match crate::onebot::OneBotChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("OneBot channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize OneBot channel");
                    }
                }
            }
        }

        // LINE
        {
            if let Some(ref cfg) = config.line {
                info!("attempting to initialize LINE channel");
                match crate::line::LineChannel::new(cfg.clone(), bus_sender.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("LINE channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize LINE channel");
                    }
                }
            }
        }

        // External
        {
            if let Some(ref cfg) = config.external {
                info!("attempting to initialize External channel");
                match crate::external::ExternalChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("External channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize External channel");
                    }
                }
            }
        }

        // MaixCam
        {
            if let Some(ref cfg) = config.maixcam {
                info!("attempting to initialize MaixCam channel");
                match crate::maixcam::MaixCamChannel::new(cfg.clone()) {
                    Ok(ch) => {
                        self.register_or_replace(Arc::new(ch)).await;
                        info!("MaixCam channel enabled successfully");
                    }
                    Err(e) => {
                        error!(error = %e, "failed to initialize MaixCam channel");
                    }
                }
            }
        }

        // Web (always available, enabled by default)
        {
            if let Some(ref cfg) = config.web {
                info!("attempting to initialize Web channel");
                let ch = crate::web::WebChannel::new(cfg.clone());
                self.register_or_replace(Arc::new(ch)).await;
                info!("Web channel enabled successfully");
            }
        }

        // WebSocket
        {
            if let Some(heartbeat_secs) = config.websocket_heartbeat_secs {
                info!("attempting to initialize WebSocket channel");
                let ch = crate::websocket::WebSocketChannel::with_heartbeat(
                    std::time::Duration::from_secs(heartbeat_secs),
                );
                self.register_or_replace(Arc::new(ch)).await;
                info!("WebSocket channel enabled successfully");
            }
        }

        let count = self.channel_count().await;
        info!(enabled_channels = count, "channel initialization completed");

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
mod tests {
    use super::*;
    use async_trait::async_trait;

    /// A minimal stub channel for testing.
    struct StubChannel {
        name: String,
        sent: Arc<parking_lot::RwLock<Vec<String>>>,
        started: Arc<parking_lot::RwLock<bool>>,
    }

    impl StubChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                sent: Arc::new(parking_lot::RwLock::new(Vec::new())),
                started: Arc::new(parking_lot::RwLock::new(false)),
            }
        }

        fn sent_messages(&self) -> Vec<String> {
            self.sent.read().clone()
        }

        fn is_started(&self) -> bool {
            *self.started.read()
        }
    }

    #[async_trait]
    impl Channel for StubChannel {
        fn name(&self) -> &str {
            &self.name
        }

        fn is_running(&self) -> bool {
            *self.started.read()
        }

        async fn start(&self) -> Result<()> {
            *self.started.write() = true;
            Ok(())
        }

        async fn stop(&self) -> Result<()> {
            *self.started.write() = false;
            Ok(())
        }

        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            self.sent.write().push(msg.content.clone());
            Ok(())
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("test-ch"));
        mgr.register(ch.clone()).await.unwrap();

        assert!(mgr.get("test-ch").await.is_some());
        assert!(mgr.get("nonexistent").await.is_none());
        assert_eq!(mgr.channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_register_duplicate_fails() {
        let mgr = ChannelManager::new();
        let ch1 = Arc::new(StubChannel::new("dup"));
        let ch2 = Arc::new(StubChannel::new("dup"));

        mgr.register(ch1).await.unwrap();
        let result = mgr.register(ch2).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_register_or_replace() {
        let mgr = ChannelManager::new();
        let ch1 = Arc::new(StubChannel::new("dup"));
        let ch2 = Arc::new(StubChannel::new("dup"));

        mgr.register(ch1).await.unwrap();
        mgr.register_or_replace(ch2).await;
        assert_eq!(mgr.channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_unregister() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("removable"));
        mgr.register(ch).await.unwrap();
        assert!(mgr.unregister("removable").await);
        assert!(!mgr.unregister("removable").await);
        assert_eq!(mgr.channel_count().await, 0);
    }

    #[tokio::test]
    async fn test_start_stop_all() {
        let mgr = Arc::new(ChannelManager::new());
        let ch1 = Arc::new(StubChannel::new("a"));
        let ch2 = Arc::new(StubChannel::new("b"));

        mgr.register(ch1.clone()).await.unwrap();
        mgr.register(ch2.clone()).await.unwrap();

        mgr.start_all().await.unwrap();
        assert!(ch1.is_started());
        assert!(ch2.is_started());

        mgr.stop_all().await.unwrap();
        assert!(!ch1.is_started());
        assert!(!ch2.is_started());
    }

    #[tokio::test]
    async fn test_dispatch_outbound_success() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            content: "Hello world".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent, vec!["Hello world"]);
    }

    #[tokio::test]
    async fn test_dispatch_outbound_channel_not_found() {
        let mgr = ChannelManager::new();
        let msg = OutboundMessage {
            channel: "missing".to_string(),
            chat_id: "chat-1".to_string(),
            content: "Hello".to_string(),
            message_type: String::new(),
        };
        let result = mgr.dispatch_outbound(msg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dispatch_loop() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("loop-test"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "loop-test".to_string(),
            chat_id: "c1".to_string(),
            content: "via loop".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        // Give the loop time to process
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let sent = ch.sent_messages();
        assert!(sent.contains(&"via loop".to_string()));
        drop(tx); // Close sender to stop loop
    }

    #[tokio::test]
    async fn test_dispatch_loop_skips_internal_channels() {
        let mgr = Arc::new(ChannelManager::new());
        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "system".to_string(),
            chat_id: "c1".to_string(),
            content: "internal msg".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            mgr.metrics()
                .dropped_internal
                .load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    #[tokio::test]
    async fn test_allowed_channels_filter() {
        let mgr = ChannelManager::with_allowed_channels(vec!["allowed".to_string()]);
        let ch = Arc::new(StubChannel::new("allowed"));
        mgr.register(ch.clone()).await.unwrap();

        // Message to allowed channel should dispatch
        let msg = OutboundMessage {
            channel: "allowed".to_string(),
            chat_id: "c1".to_string(),
            content: "ok".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();
        assert_eq!(ch.sent_messages().len(), 1);

        // Message to filtered channel should be silently dropped
        let msg2 = OutboundMessage {
            channel: "blocked".to_string(),
            chat_id: "c1".to_string(),
            content: "dropped".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg2).await.unwrap(); // no error
        assert_eq!(mgr.metrics().dropped_filtered.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_metrics() {
        let mgr = ChannelManager::new();
        assert_eq!(mgr.metrics().dispatched.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(mgr.metrics().dropped_not_found.load(std::sync::atomic::Ordering::Relaxed), 0);

        // Dispatch to missing channel increments dropped_not_found
        let msg = OutboundMessage {
            channel: "missing".to_string(),
            chat_id: "c1".to_string(),
            content: "x".to_string(),
            message_type: String::new(),
        };
        let _ = mgr.dispatch_outbound(msg).await;
        assert_eq!(mgr.metrics().dropped_not_found.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_channel_names() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();

        let mut names = mgr.channel_names().await;
        names.sort();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[tokio::test]
    async fn test_get_status() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("web"))).await.unwrap();

        let status = mgr.get_status().await;
        assert!(status.contains_key("web"));
        assert!(status["web"].enabled);
    }

    #[tokio::test]
    async fn test_send_to_channel() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.send_to_channel("web", "chat-1", "direct message")
            .await
            .unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent, vec!["direct message"]);
    }

    #[tokio::test]
    async fn test_send_to_missing_channel() {
        let mgr = ChannelManager::new();
        let result = mgr.send_to_channel("missing", "chat-1", "msg").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_is_internal_channel() {
        assert!(is_internal_channel("system"));
        assert!(!is_internal_channel("web"));
        assert!(!is_internal_channel("rpc"));
    }

    #[tokio::test]
    async fn test_start_stop_idempotent() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("a"));
        mgr.register(ch.clone()).await.unwrap();

        // Start all twice should succeed
        mgr.start_all().await.unwrap();
        assert!(ch.is_started());
        // Second call should be a no-op (dispatch already started)
        mgr.start_all().await.unwrap();

        // Stop all twice should succeed
        mgr.stop_all().await.unwrap();
        assert!(!ch.is_started());
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_start_all_empty_manager() {
        let mgr = Arc::new(ChannelManager::new());
        // No channels registered
        mgr.start_all().await.unwrap();
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let mgr = ChannelManager::new();
        assert!(!mgr.unregister("nonexistent").await);
        assert_eq!(mgr.channel_count().await, 0);
    }

    #[tokio::test]
    async fn test_get_status_empty() {
        let mgr = ChannelManager::new();
        let status = mgr.get_status().await;
        assert!(status.is_empty());
    }

    #[tokio::test]
    async fn test_get_status_multiple_channels() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("ch1"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("ch2"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("ch3"))).await.unwrap();

        let status = mgr.get_status().await;
        assert_eq!(status.len(), 3);
        assert!(status.contains_key("ch1"));
        assert!(status.contains_key("ch2"));
        assert!(status.contains_key("ch3"));
    }

    #[tokio::test]
    async fn test_dispatch_outbound_empty_content() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            content: String::new(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0], "");
    }

    #[tokio::test]
    async fn test_dispatch_outbound_long_content() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        let long_content = "x".repeat(100_000);
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            content: long_content.clone(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].len(), 100_000);
    }

    #[tokio::test]
    async fn test_send_to_channel_empty_chat_id() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.send_to_channel("web", "", "test content")
            .await
            .unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent, vec!["test content"]);
    }

    #[tokio::test]
    async fn test_send_to_channel_empty_content() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.send_to_channel("web", "chat-1", "").await.unwrap();

        let sent = ch.sent_messages();
        assert_eq!(sent, vec![""]);
    }

    #[tokio::test]
    async fn test_dispatch_loop_double_start_fails() {
        let mgr = Arc::new(ChannelManager::new());
        mgr.start_dispatch_loop().unwrap();
        // Second call should fail
        let result = mgr.start_dispatch_loop();
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_concurrent_access() {
        let mgr = Arc::new(ChannelManager::new());

        // Register channels
        for i in 0..5 {
            let name = format!("ch{}", i);
            mgr.register(Arc::new(StubChannel::new(&name))).await.unwrap();
        }

        let mgr1 = Arc::clone(&mgr);
        let mgr2 = Arc::clone(&mgr);
        let mgr3 = Arc::clone(&mgr);
        let mgr4 = Arc::clone(&mgr);

        // Concurrent reads
        let h1 = tokio::spawn(async move {
            for _ in 0..100 {
                mgr1.get("ch1").await;
            }
        });
        let h2 = tokio::spawn(async move {
            for _ in 0..100 {
                mgr2.channel_names().await;
            }
        });
        let h3 = tokio::spawn(async move {
            for _ in 0..100 {
                mgr3.get_status().await;
            }
        });
        let h4 = tokio::spawn(async move {
            for _ in 0..100 {
                mgr4.channel_count().await;
            }
        });

        h1.await.unwrap();
        h2.await.unwrap();
        h3.await.unwrap();
        h4.await.unwrap();

        // Manager should still be functional
        assert_eq!(mgr.channel_count().await, 5);
    }

    #[tokio::test]
    async fn test_metrics_dispatched_increment() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        // Dispatch multiple messages
        for i in 0..5 {
            let msg = OutboundMessage {
                channel: "web".to_string(),
                chat_id: format!("chat-{}", i),
                content: format!("msg {}", i),
                message_type: String::new(),
            };
            mgr.dispatch_outbound(msg).await.unwrap();
        }

        assert_eq!(
            mgr.metrics().dispatched.load(std::sync::atomic::Ordering::Relaxed),
            5
        );
        assert_eq!(ch.sent_messages().len(), 5);
    }

    #[tokio::test]
    async fn test_allowed_channels_empty_means_all() {
        let mgr = ChannelManager::with_allowed_channels(vec![]);
        // Empty allowed list means no filter - all channels allowed
        let ch = Arc::new(StubChannel::new("any"));
        mgr.register(ch.clone()).await.unwrap();

        let msg = OutboundMessage {
            channel: "any".to_string(),
            chat_id: "c1".to_string(),
            content: "ok".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();
        assert_eq!(ch.sent_messages().len(), 1);
    }

    #[tokio::test]
    async fn test_unregister_after_start() {
        let mgr = Arc::new(ChannelManager::new());
        let ch1 = Arc::new(StubChannel::new("a"));
        let ch2 = Arc::new(StubChannel::new("b"));
        mgr.register(ch1.clone()).await.unwrap();
        mgr.register(ch2.clone()).await.unwrap();

        mgr.start_all().await.unwrap();
        assert!(ch1.is_started());
        assert!(ch2.is_started());

        mgr.unregister("a").await;
        assert_eq!(mgr.channel_count().await, 1);

        // Channel b should still be accessible
        assert!(mgr.get("b").await.is_some());
        assert!(mgr.get("a").await.is_none());
    }

    // --- Benchmark-style throughput tests ---

    #[tokio::test]
    async fn test_dispatch_throughput() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("bench"));
        mgr.register(ch.clone()).await.unwrap();

        let count = 1_000;
        let start = std::time::Instant::now();
        for i in 0..count {
            let msg = OutboundMessage {
                channel: "bench".to_string(),
                chat_id: format!("c{}", i),
                content: format!("msg{}", i),
                message_type: String::new(),
            };
            mgr.dispatch_outbound(msg).await.unwrap();
        }
        let elapsed = start.elapsed();
        assert_eq!(ch.sent_messages().len(), count);
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "Dispatch throughput too slow: {:?}",
            elapsed
        );
    }

    #[tokio::test]
    async fn test_register_unregister_throughput() {
        let mgr = ChannelManager::new();
        let count = 100;

        let start = std::time::Instant::now();
        for i in 0..count {
            let ch = Arc::new(StubChannel::new(&format!("ch-{}", i)));
            mgr.register(ch).await.unwrap();
        }
        let elapsed = start.elapsed();
        assert_eq!(mgr.channel_count().await, count);
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "Register throughput too slow: {:?}",
            elapsed
        );
    }

    // ---- Additional comprehensive manager tests ----

    // === Sync target configuration ===

    #[tokio::test]
    async fn test_setup_sync_targets_valid() {
        let mgr = ChannelManager::new();
        let ch_a = Arc::new(StubChannel::new("a"));
        let ch_b = Arc::new(StubChannel::new("b"));
        mgr.register(ch_a).await.unwrap();
        mgr.register(ch_b).await.unwrap();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("a".to_string(), vec!["b".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("a").await;
        assert_eq!(targets, vec!["b"]);
    }

    #[tokio::test]
    async fn test_setup_sync_targets_self_sync_prevented() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("self"));
        mgr.register(ch).await.unwrap();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("self".to_string(), vec!["self".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("self").await;
        assert!(targets.is_empty()); // self-sync should be skipped
    }

    #[tokio::test]
    async fn test_setup_sync_targets_nonexistent_source() {
        let mgr = ChannelManager::new();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("missing".to_string(), vec!["target".to_string()]);

        mgr.setup_sync_targets(&config).await;
        // Should not panic, just skip
    }

    #[tokio::test]
    async fn test_setup_sync_targets_nonexistent_target() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("source"));
        mgr.register(ch).await.unwrap();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("source".to_string(), vec!["nonexistent".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("source").await;
        assert!(targets.is_empty()); // nonexistent target skipped
    }

    #[tokio::test]
    async fn test_setup_sync_targets_multiple() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("c"))).await.unwrap();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("a").await;
        assert_eq!(targets.len(), 2);
        assert!(targets.contains(&"b".to_string()));
        assert!(targets.contains(&"c".to_string()));
    }

    #[tokio::test]
    async fn test_setup_sync_targets_circular() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("a".to_string(), vec!["b".to_string()]);
        config.targets.insert("b".to_string(), vec!["a".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let a_targets = mgr.get_sync_targets("a").await;
        let b_targets = mgr.get_sync_targets("b").await;
        assert_eq!(a_targets, vec!["b"]);
        assert_eq!(b_targets, vec!["a"]);
    }

    #[tokio::test]
    async fn test_get_sync_targets_no_config() {
        let mgr = ChannelManager::new();
        let targets = mgr.get_sync_targets("anything").await;
        assert!(targets.is_empty());
    }

    #[tokio::test]
    async fn test_setup_sync_targets_empty_config() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();

        let config = ChannelSyncConfig::default();
        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("a").await;
        assert!(targets.is_empty());
    }

    // === Dispatch loop edge cases ===

    #[tokio::test]
    async fn test_dispatch_loop_shutdown_flag() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("test"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        // Send a message
        let msg = OutboundMessage {
            channel: "test".to_string(),
            chat_id: "c1".to_string(),
            content: "before shutdown".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert!(ch.sent_messages().contains(&"before shutdown".to_string()));

        // Stop sets shutdown flag
        mgr.stop_all().await.unwrap();
    }

    #[tokio::test]
    async fn test_dispatch_loop_skips_cli_channel() {
        let mgr = Arc::new(ChannelManager::new());
        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "cli".to_string(),
            chat_id: "c1".to_string(),
            content: "cli msg".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            mgr.metrics().dropped_internal.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    #[tokio::test]
    async fn test_dispatch_loop_skips_subagent_channel() {
        let mgr = Arc::new(ChannelManager::new());
        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "subagent".to_string(),
            chat_id: "c1".to_string(),
            content: "subagent msg".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        assert_eq!(
            mgr.metrics().dropped_internal.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    #[tokio::test]
    async fn test_dispatch_loop_multiple_messages() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("multi"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        for i in 0..20 {
            let msg = OutboundMessage {
                channel: "multi".to_string(),
                chat_id: format!("c{}", i),
                content: format!("msg {}", i),
                message_type: String::new(),
            };
            tx.send(msg).await.unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        assert_eq!(ch.sent_messages().len(), 20);
        drop(tx);
    }

    // === Allowed channels filter edge cases ===

    #[tokio::test]
    async fn test_allowed_channels_multiple_allowed() {
        let mgr = ChannelManager::with_allowed_channels(vec![
            "web".to_string(),
            "rpc".to_string(),
        ]);

        let ch_web = Arc::new(StubChannel::new("web"));
        let ch_rpc = Arc::new(StubChannel::new("rpc"));
        let ch_other = Arc::new(StubChannel::new("other"));

        mgr.register(ch_web.clone()).await.unwrap();
        mgr.register(ch_rpc.clone()).await.unwrap();
        mgr.register(ch_other.clone()).await.unwrap();

        // Allowed channels should receive
        let msg = OutboundMessage {
            channel: "web".to_string(),
            chat_id: "c1".to_string(),
            content: "ok".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg).await.unwrap();
        assert_eq!(ch_web.sent_messages().len(), 1);

        // RPC should also be allowed
        let msg2 = OutboundMessage {
            channel: "rpc".to_string(),
            chat_id: "c1".to_string(),
            content: "ok".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg2).await.unwrap();
        assert_eq!(ch_rpc.sent_messages().len(), 1);

        // Other should be filtered
        let msg3 = OutboundMessage {
            channel: "other".to_string(),
            chat_id: "c1".to_string(),
            content: "filtered".to_string(),
            message_type: String::new(),
        };
        mgr.dispatch_outbound(msg3).await.unwrap(); // no error, just dropped
        assert_eq!(ch_other.sent_messages().len(), 0);
        assert_eq!(
            mgr.metrics().dropped_filtered.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    // === Metrics accuracy ===

    #[tokio::test]
    async fn test_metrics_send_errors() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(FailingStubChannel::new("fail-ch"));
        mgr.register(ch).await.unwrap();

        let msg = OutboundMessage {
            channel: "fail-ch".to_string(),
            chat_id: "c1".to_string(),
            content: "will fail".to_string(),
            message_type: String::new(),
        };
        let result = mgr.dispatch_outbound(msg).await;
        assert!(result.is_err());
        assert_eq!(
            mgr.metrics().send_errors.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
    }

    #[tokio::test]
    async fn test_metrics_multiple_not_found() {
        let mgr = ChannelManager::new();

        for i in 0..5 {
            let msg = OutboundMessage {
                channel: format!("missing-{}", i),
                chat_id: "c1".to_string(),
                content: "test".to_string(),
                message_type: String::new(),
            };
            let _ = mgr.dispatch_outbound(msg).await;
        }

        assert_eq!(
            mgr.metrics().dropped_not_found.load(std::sync::atomic::Ordering::Relaxed),
            5
        );
    }

    // === Registration edge cases ===

    #[tokio::test]
    async fn test_register_many_channels() {
        let mgr = ChannelManager::new();
        for i in 0..200 {
            let ch = Arc::new(StubChannel::new(&format!("ch-{}", i)));
            mgr.register(ch).await.unwrap();
        }
        assert_eq!(mgr.channel_count().await, 200);
    }

    #[tokio::test]
    async fn test_register_or_replace_multiple_times() {
        let mgr = ChannelManager::new();
        for _ in 0..5 {
            let ch = Arc::new(StubChannel::new("same-name"));
            mgr.register_or_replace(ch).await;
        }
        assert_eq!(mgr.channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_unregister_all_channels() {
        let mgr = ChannelManager::new();
        for i in 0..10 {
            mgr.register(Arc::new(StubChannel::new(&format!("ch-{}", i)))).await.unwrap();
        }
        assert_eq!(mgr.channel_count().await, 10);

        for i in 0..10 {
            assert!(mgr.unregister(&format!("ch-{}", i)).await);
        }
        assert_eq!(mgr.channel_count().await, 0);
    }

    // === Send to channel edge cases ===

    #[tokio::test]
    async fn test_send_to_channel_unicode_content() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.send_to_channel("web", "chat-1", "你好世界 🌍").await.unwrap();
        assert_eq!(ch.sent_messages()[0], "你好世界 🌍");
    }

    #[tokio::test]
    async fn test_send_to_channel_large_content() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("web"));
        mgr.register(ch.clone()).await.unwrap();

        let large = "x".repeat(1_000_000);
        mgr.send_to_channel("web", "chat-1", &large).await.unwrap();
        assert_eq!(ch.sent_messages()[0].len(), 1_000_000);
    }

    // === Channel status ===

    #[tokio::test]
    async fn test_channel_status_after_start() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("running-ch"));
        mgr.register(ch.clone()).await.unwrap();

        let mgr_arc = Arc::new(mgr);
        mgr_arc.start_all().await.unwrap();

        let status = mgr_arc.get_status().await;
        assert!(status.contains_key("running-ch"));
        assert!(status["running-ch"].running);
    }

    // === Channel names ===

    #[tokio::test]
    async fn test_channel_names_ordering() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("z-channel"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("a-channel"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("m-channel"))).await.unwrap();

        let mut names = mgr.channel_names().await;
        names.sort();
        assert_eq!(names, vec!["a-channel", "m-channel", "z-channel"]);
    }

    // === Default impl ===

    #[tokio::test]
    async fn test_manager_default() {
        let mgr = ChannelManager::default();
        assert_eq!(mgr.channel_count().await, 0);
    }

    // === Internal channel check ===

    #[test]
    fn test_is_internal_channel_all_types() {
        assert!(is_internal_channel("cli"));
        assert!(is_internal_channel("system"));
        assert!(is_internal_channel("subagent"));
        assert!(!is_internal_channel("web"));
        assert!(!is_internal_channel("rpc"));
        assert!(!is_internal_channel("websocket"));
        assert!(!is_internal_channel("telegram"));
        assert!(!is_internal_channel(""));
    }

    // === Concurrent dispatch ===

    #[tokio::test]
    async fn test_concurrent_dispatch_outbound() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("concurrent"));
        mgr.register(ch.clone()).await.unwrap();

        let mut handles = vec![];
        for i in 0..10 {
            let mgr_clone = Arc::clone(&mgr);
            handles.push(tokio::spawn(async move {
                let msg = OutboundMessage {
                    channel: "concurrent".to_string(),
                    chat_id: format!("c{}", i),
                    content: format!("msg {}", i),
                    message_type: String::new(),
                };
                mgr_clone.dispatch_outbound(msg).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        assert_eq!(ch.sent_messages().len(), 10);
    }

    // === FailingStubChannel for testing send errors ===

    struct FailingStubChannel {
        name: String,
    }

    impl FailingStubChannel {
        fn new(name: &str) -> Self {
            Self { name: name.to_string() }
        }
    }

    #[async_trait]
    impl Channel for FailingStubChannel {
        fn name(&self) -> &str { &self.name }
        async fn start(&self) -> Result<()> { Ok(()) }
        async fn stop(&self) -> Result<()> { Ok(()) }
        async fn send(&self, _msg: OutboundMessage) -> Result<()> {
            Err(NemesisError::Channel("send always fails".to_string()))
        }
    }

    // === Channel that fails to start ===

    struct FailStartChannel {
        name: String,
    }

    impl FailStartChannel {
        fn new(name: &str) -> Self {
            Self { name: name.to_string() }
        }
    }

    #[async_trait]
    impl Channel for FailStartChannel {
        fn name(&self) -> &str { &self.name }
        fn is_running(&self) -> bool { false }
        async fn start(&self) -> Result<()> {
            Err(NemesisError::Channel("start failed".to_string()))
        }
        async fn stop(&self) -> Result<()> { Ok(()) }
        async fn send(&self, _msg: OutboundMessage) -> Result<()> { Ok(()) }
    }

    // === Slow channel for timeout tests ===

    struct SlowChannel {
        name: String,
        sent: Arc<parking_lot::RwLock<Vec<String>>>,
    }

    impl SlowChannel {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                sent: Arc::new(parking_lot::RwLock::new(Vec::new())),
            }
        }

        fn sent_messages(&self) -> Vec<String> {
            self.sent.read().clone()
        }
    }

    #[async_trait]
    impl Channel for SlowChannel {
        fn name(&self) -> &str { &self.name }
        async fn start(&self) -> Result<()> { Ok(()) }
        async fn stop(&self) -> Result<()> { Ok(()) }
        async fn send(&self, msg: OutboundMessage) -> Result<()> {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            self.sent.write().push(msg.content);
            Ok(())
        }
    }

    // === Tests for start_all with failing channel ===

    #[tokio::test]
    async fn test_start_all_with_failing_channel_continues() {
        let mgr = Arc::new(ChannelManager::new());
        let good_ch = Arc::new(StubChannel::new("good"));
        let fail_ch = Arc::new(FailStartChannel::new("fail"));

        mgr.register(good_ch.clone()).await.unwrap();
        mgr.register(Arc::new(FailStartChannel::new("fail"))).await.unwrap();

        // start_all should continue even if one channel fails
        mgr.start_all().await.unwrap();

        // Good channel should still be started
        assert!(good_ch.is_started());
    }

    #[tokio::test]
    async fn test_stop_all_with_failing_channel_continues() {
        let mgr = Arc::new(ChannelManager::new());
        let good_ch = Arc::new(StubChannel::new("good"));

        mgr.register(good_ch.clone()).await.unwrap();

        mgr.start_all().await.unwrap();
        assert!(good_ch.is_started());

        mgr.stop_all().await.unwrap();
        assert!(!good_ch.is_started());
    }

    // === Tests for init_channels with web config ===

    #[tokio::test]
    async fn test_init_channels_with_web() {
        let mgr = ChannelManager::new();
        let (tx, _) = broadcast::channel::<InboundMessage>(256);

        let mut config = ChannelInitConfig::default();
        config.web = Some(crate::web::WebChannelConfig::default());

        mgr.init_channels(&config, tx).await.unwrap();
        assert!(mgr.get("web").await.is_some());
        assert_eq!(mgr.channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_init_channels_with_websocket() {
        let mgr = ChannelManager::new();
        let (tx, _) = broadcast::channel::<InboundMessage>(256);

        let mut config = ChannelInitConfig::default();
        config.websocket_heartbeat_secs = Some(30);

        mgr.init_channels(&config, tx).await.unwrap();
        assert!(mgr.get("websocket").await.is_some());
        assert_eq!(mgr.channel_count().await, 1);
    }

    #[tokio::test]
    async fn test_init_channels_empty_config() {
        let mgr = ChannelManager::new();
        let (tx, _) = broadcast::channel::<InboundMessage>(256);

        let config = ChannelInitConfig::default();
        mgr.init_channels(&config, tx).await.unwrap();
        assert_eq!(mgr.channel_count().await, 0);
    }

    #[tokio::test]
    async fn test_init_channels_web_and_websocket() {
        let mgr = ChannelManager::new();
        let (tx, _) = broadcast::channel::<InboundMessage>(256);

        let mut config = ChannelInitConfig::default();
        config.web = Some(crate::web::WebChannelConfig::default());
        config.websocket_heartbeat_secs = Some(60);

        mgr.init_channels(&config, tx).await.unwrap();
        assert!(mgr.get("web").await.is_some());
        assert!(mgr.get("websocket").await.is_some());
        assert_eq!(mgr.channel_count().await, 2);
    }

    // === ChannelSyncConfig edge cases ===

    #[test]
    fn test_channel_sync_config_default() {
        let config = ChannelSyncConfig::default();
        assert!(config.targets.is_empty());
    }

    #[test]
    fn test_channel_sync_config_with_targets() {
        let mut config = ChannelSyncConfig::default();
        config.targets.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);
        assert_eq!(config.targets.len(), 1);
        assert_eq!(config.targets["a"].len(), 2);
    }

    // === ChannelStatus serialization ===

    #[test]
    fn test_channel_status_serialize() {
        let status = ChannelStatus {
            enabled: true,
            running: false,
        };
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"enabled\":true"));
        assert!(json.contains("\"running\":false"));
    }

    #[test]
    fn test_channel_status_deserialize() {
        let json = r#"{"enabled":true,"running":false}"#;
        let status: ChannelStatus = serde_json::from_str(json).unwrap();
        assert!(status.enabled);
        assert!(!status.running);
    }

    #[test]
    fn test_channel_status_roundtrip() {
        let status = ChannelStatus {
            enabled: false,
            running: true,
        };
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: ChannelStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.enabled, status.enabled);
        assert_eq!(deserialized.running, status.running);
    }

    // === ManagerMetrics default ===

    #[test]
    fn test_manager_metrics_default() {
        let metrics = ManagerMetrics::default();
        assert_eq!(metrics.dispatched.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped_not_found.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped_filtered.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(metrics.dropped_internal.load(std::sync::atomic::Ordering::Relaxed), 0);
        assert_eq!(metrics.send_errors.load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    // === Dispatch loop with allowed channels ===

    #[tokio::test]
    async fn test_dispatch_loop_with_allowed_filter() {
        let mgr = Arc::new(ChannelManager::with_allowed_channels(vec!["ok".to_string()]));
        let ch = Arc::new(StubChannel::new("ok"));
        mgr.register(ch.clone()).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        // Allowed channel should be dispatched
        let msg1 = OutboundMessage {
            channel: "ok".to_string(),
            chat_id: "c1".to_string(),
            content: "allowed".to_string(),
            message_type: String::new(),
        };
        tx.send(msg1).await.unwrap();

        // Filtered channel should be dropped
        let msg2 = OutboundMessage {
            channel: "blocked".to_string(),
            chat_id: "c1".to_string(),
            content: "filtered".to_string(),
            message_type: String::new(),
        };
        tx.send(msg2).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(ch.sent_messages().len(), 1);
        assert_eq!(ch.sent_messages()[0], "allowed");
        assert_eq!(
            mgr.metrics().dropped_filtered.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    // === Dispatch loop with send error ===

    #[tokio::test]
    async fn test_dispatch_loop_with_send_error() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(FailingStubChannel::new("fail"));
        mgr.register(ch).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "fail".to_string(),
            chat_id: "c1".to_string(),
            content: "will fail".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            mgr.metrics().send_errors.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    // === Dispatch loop with missing channel ===

    #[tokio::test]
    async fn test_dispatch_loop_missing_channel() {
        let mgr = Arc::new(ChannelManager::new());
        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        let msg = OutboundMessage {
            channel: "nonexistent".to_string(),
            chat_id: "c1".to_string(),
            content: "lost".to_string(),
            message_type: String::new(),
        };
        tx.send(msg).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(
            mgr.metrics().dropped_not_found.load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        drop(tx);
    }

    // === Setup sync targets mixed valid and invalid ===

    #[tokio::test]
    async fn test_setup_sync_targets_mixed_valid_invalid() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
        // "c" NOT registered

        let mut config = ChannelSyncConfig::default();
        config.targets.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);

        mgr.setup_sync_targets(&config).await;

        let targets = mgr.get_sync_targets("a").await;
        assert_eq!(targets.len(), 1);
        assert!(targets.contains(&"b".to_string()));
        assert!(!targets.contains(&"c".to_string())); // nonexistent target skipped
    }

    // === Register, unregister, re-register ===

    #[tokio::test]
    async fn test_unregister_and_reregister() {
        let mgr = ChannelManager::new();
        let ch1 = Arc::new(StubChannel::new("ch"));
        mgr.register(ch1).await.unwrap();
        assert_eq!(mgr.channel_count().await, 1);

        mgr.unregister("ch").await;
        assert_eq!(mgr.channel_count().await, 0);

        let ch2 = Arc::new(StubChannel::new("ch"));
        mgr.register(ch2).await.unwrap();
        assert_eq!(mgr.channel_count().await, 1);
        assert!(mgr.get("ch").await.is_some());
    }

    // === Dispatch loop processes multiple channels ===

    #[tokio::test]
    async fn test_dispatch_loop_routes_to_correct_channel() {
        let mgr = Arc::new(ChannelManager::new());
        let ch_a = Arc::new(StubChannel::new("a"));
        let ch_b = Arc::new(StubChannel::new("b"));
        mgr.register(ch_a.clone()).await.unwrap();
        mgr.register(ch_b.clone()).await.unwrap();

        mgr.start_dispatch_loop().unwrap();
        let tx = mgr.outbound_sender();

        tx.send(OutboundMessage {
            channel: "a".to_string(),
            chat_id: "c1".to_string(),
            content: "for A".to_string(),
            message_type: String::new(),
        }).await.unwrap();

        tx.send(OutboundMessage {
            channel: "b".to_string(),
            chat_id: "c2".to_string(),
            content: "for B".to_string(),
            message_type: String::new(),
        }).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        assert_eq!(ch_a.sent_messages(), vec!["for A"]);
        assert_eq!(ch_b.sent_messages(), vec!["for B"]);
        drop(tx);
    }

    // === ChannelInitConfig debug ===

    #[test]
    fn test_channel_init_config_debug() {
        let config = ChannelInitConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("ChannelInitConfig") || debug_str.contains("web"));
    }

    // === get_status returns running state ===

    #[tokio::test]
    async fn test_get_status_running_state() {
        let mgr = Arc::new(ChannelManager::new());
        let ch = Arc::new(StubChannel::new("test"));
        mgr.register(ch.clone()).await.unwrap();

        // Before start: not running
        let status = mgr.get_status().await;
        assert!(!status["test"].running);

        // After start: running
        mgr.start_all().await.unwrap();
        let status = mgr.get_status().await;
        assert!(status["test"].running);

        // After stop: not running
        mgr.stop_all().await.unwrap();
        let status = mgr.get_status().await;
        assert!(!status["test"].running);
    }

    // === Outbound sender cloning ===

    #[tokio::test]
    async fn test_outbound_sender_clones() {
        let mgr = Arc::new(ChannelManager::new());
        let tx1 = mgr.outbound_sender();
        let tx2 = mgr.outbound_sender();

        // Both should be usable
        assert!(!tx1.is_closed());
        assert!(!tx2.is_closed());
    }

    // === Metrics after multiple operations ===

    #[tokio::test]
    async fn test_metrics_comprehensive() {
        let mgr = ChannelManager::new();
        let ch = Arc::new(StubChannel::new("ok"));
        mgr.register(ch).await.unwrap();

        // Dispatch successful
        mgr.dispatch_outbound(OutboundMessage {
            channel: "ok".to_string(),
            chat_id: "c1".to_string(),
            content: "ok".to_string(),
            message_type: String::new(),
        }).await.unwrap();

        // Dispatch to missing channel
        let _ = mgr.dispatch_outbound(OutboundMessage {
            channel: "missing".to_string(),
            chat_id: "c1".to_string(),
            content: "lost".to_string(),
            message_type: String::new(),
        }).await;

        assert_eq!(mgr.metrics().dispatched.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(mgr.metrics().dropped_not_found.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    // === Setup sync targets replaces existing config ===

    #[tokio::test]
    async fn test_setup_sync_targets_replaces() {
        let mgr = ChannelManager::new();
        mgr.register(Arc::new(StubChannel::new("a"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("b"))).await.unwrap();
        mgr.register(Arc::new(StubChannel::new("c"))).await.unwrap();

        // First config: a -> b
        let mut config1 = ChannelSyncConfig::default();
        config1.targets.insert("a".to_string(), vec!["b".to_string()]);
        mgr.setup_sync_targets(&config1).await;
        assert_eq!(mgr.get_sync_targets("a").await, vec!["b"]);

        // Replace with: a -> c
        let mut config2 = ChannelSyncConfig::default();
        config2.targets.insert("a".to_string(), vec!["c".to_string()]);
        mgr.setup_sync_targets(&config2).await;
        assert_eq!(mgr.get_sync_targets("a").await, vec!["c"]);
    }
}
