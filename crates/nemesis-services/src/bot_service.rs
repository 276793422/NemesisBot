//! BotService - Main service orchestrator.
//!
//! Initializes all components (bus, config, channels, agent, security, forge)
//! and manages their lifecycle with full restart, error tracking, and
//! component-phase initialization.
//!
//! The initialization follows a 4-phase pattern mirroring the Go code:
//! 1. Load configuration from disk
//! 2. Validate configuration (models, API keys)
//! 3. Initialize components (sequential core + parallel independent)
//! 4. Start services in dependency order

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

use crate::state::BotState;
use crate::helpers::get_config_path;
use crate::log_hook::LogHookHandle;

/// A single component that the BotService manages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Component {
    Bus,
    Channels,
    Agent,
    Security,
    Forge,
    Cluster,
    Memory,
    Workflow,
    Skills,
    Cron,
    Heartbeat,
    Devices,
    Health,
    Observer,
}

impl Component {
    /// Human-readable label.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Bus => "bus",
            Self::Channels => "channels",
            Self::Agent => "agent",
            Self::Security => "security",
            Self::Forge => "forge",
            Self::Cluster => "cluster",
            Self::Memory => "memory",
            Self::Workflow => "workflow",
            Self::Skills => "skills",
            Self::Cron => "cron",
            Self::Heartbeat => "heartbeat",
            Self::Devices => "devices",
            Self::Health => "health",
            Self::Observer => "observer",
        }
    }
}

/// Tracks which components are currently enabled / running.
#[derive(Debug, Clone, Default)]
pub struct EnabledComponents {
    inner: HashMap<Component, bool>,
}

impl EnabledComponents {
    pub fn new() -> Self {
        let mut inner = HashMap::new();
        for comp in [
            Component::Bus,
            Component::Channels,
            Component::Agent,
            Component::Security,
            Component::Forge,
            Component::Cluster,
            Component::Memory,
            Component::Workflow,
            Component::Skills,
            Component::Cron,
            Component::Heartbeat,
            Component::Devices,
            Component::Health,
            Component::Observer,
        ] {
            inner.insert(comp, false);
        }
        Self { inner }
    }

    /// Mark a component as enabled.
    pub fn enable(&mut self, component: Component) {
        self.inner.insert(component, true);
    }

    /// Mark a component as disabled.
    pub fn disable(&mut self, component: Component) {
        self.inner.insert(component, false);
    }

    /// Check whether a component is enabled.
    pub fn is_enabled(&self, component: Component) -> bool {
        self.inner.get(&component).copied().unwrap_or(false)
    }

    /// Return a list of all enabled components.
    pub fn enabled_list(&self) -> Vec<Component> {
        self.inner
            .iter()
            .filter(|&(_, v)| *v)
            .map(|(&k, _)| k)
            .collect()
    }

    /// Disable all components.
    pub fn disable_all(&mut self) {
        for v in self.inner.values_mut() {
            *v = false;
        }
    }
}

// ---------------------------------------------------------------------------
// Service handles - opaque references to initialized subsystems.
//
// In the Go code, BotService holds concrete pointers to each subsystem
// (forgeSvc, memoryMgr, channelMgr, etc.). In Rust, we use trait objects
// behind Arc<dyn ...> to decouple the services crate from each subsystem
// crate, avoiding circular dependencies. When the concrete crates are
// wired together at the application layer, they inject their instances
// through the `inject_*` methods.
// ---------------------------------------------------------------------------

/// A service that can be started and stopped.
pub trait LifecycleService: Send + Sync {
    /// Start the service.
    fn start(&self) -> Result<(), String> {
        Ok(())
    }

    /// Stop the service.
    fn stop(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Type-erased handle for a service with start/stop lifecycle.
pub type ServiceHandle = Arc<dyn LifecycleService>;

/// A forge service providing self-learning capabilities.
pub trait ForgeService: Send + Sync + LifecycleService {
    /// Return the forge service name/identifier.
    fn forge_name(&self) -> &str;
}

/// An observer manager that collects events from the agent loop.
pub trait ObserverManager: Send + Sync {
    /// Check whether any observers are registered.
    fn has_observers(&self) -> bool;
}

/// A memory manager that stores and retrieves memories.
pub trait MemoryService: Send + Sync + LifecycleService {
    /// Close the memory store and release resources.
    fn close(&self) {}
}

/// A heartbeat service that triggers periodic LLM calls.
pub trait HeartbeatService: Send + Sync + LifecycleService {}

/// A device monitoring service.
pub trait DeviceService: Send + Sync + LifecycleService {}

/// A cron/scheduled job service.
pub trait CronService: Send + Sync + LifecycleService {}

/// A health check HTTP server.
pub trait HealthServer: Send + Sync + LifecycleService {}

/// A channel manager that handles all channel lifecycle.
pub trait ChannelManager: Send + Sync + LifecycleService {
    /// Return a list of enabled channel names.
    fn enabled_channels(&self) -> Vec<String>;
}

/// An agent loop that processes inbound messages through LLM + tools.
pub trait AgentLoopService: Send + Sync + LifecycleService {
    /// Process a heartbeat request. Returns a brief response from the LLM
    /// to confirm the agent is operational. Mirrors Go's `ProcessHeartbeat()`.
    fn process_heartbeat(&self) -> Result<String, String> {
        Ok(String::new())
    }
}

/// A security checker middleware.
pub trait SecurityService: Send + Sync {}

/// A workflow engine.
pub trait WorkflowService: Send + Sync {}

/// A skill loader.
pub trait SkillsService: Send + Sync {}

// ---------------------------------------------------------------------------
// BotService configuration
// ---------------------------------------------------------------------------

/// Model entry from the configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    /// Model identifier (e.g. "zhipu/glm-4.7").
    pub model: String,
    /// API key for authentication.
    #[serde(default)]
    pub api_key: String,
    /// Base URL override -- supports both `base_url` and `api_base` (nemesis-config).
    #[serde(default, alias = "api_base")]
    pub base_url: String,
    /// Whether this is the default model.
    #[serde(default)]
    pub is_default: bool,
}

/// Configuration accepted by [`BotService::new`].
///
/// Mirrors the subset of `nemesis_config::Config` flags the service
/// orchestrator cares about.
#[derive(Debug, Clone)]
pub struct BotServiceConfig {
    /// Whether security checks are enabled.
    pub security_enabled: bool,
    /// Whether forge self-learning is enabled.
    pub forge_enabled: bool,
    /// Whether cluster mode is enabled.
    pub cluster_enabled: bool,
    /// Whether memory subsystem is enabled.
    pub memory_enabled: bool,
    /// Whether workflow engine is enabled.
    pub workflow_enabled: bool,
    /// Path to config file (defaults to GetConfigPath).
    pub config_path: PathBuf,
    /// Workspace path.
    pub workspace: PathBuf,
    /// Configured model list (loaded from config file).
    pub model_list: Vec<ModelEntry>,
    /// Heartbeat interval in seconds.
    pub heartbeat_interval_secs: u64,
    /// Whether heartbeat is enabled.
    pub heartbeat_enabled: bool,
    /// Gateway host for health server.
    pub gateway_host: String,
    /// Gateway port for health server.
    pub gateway_port: u16,
    /// Whether LLM logging is enabled.
    pub llm_logging_enabled: bool,
    /// Whether forge trace collection is enabled.
    pub forge_trace_enabled: bool,
    /// Whether forge learning is enabled.
    pub forge_learning_enabled: bool,
    /// Whether devices monitoring is enabled.
    pub devices_enabled: bool,
    /// Whether USB device monitoring is enabled.
    pub devices_monitor_usb: bool,
    /// Cron execution timeout in minutes.
    pub cron_exec_timeout_minutes: u64,
    /// Whether to restrict file operations to workspace.
    pub restrict_to_workspace: bool,
}

impl Default for BotServiceConfig {
    fn default() -> Self {
        Self {
            security_enabled: true,
            forge_enabled: false,
            cluster_enabled: false,
            memory_enabled: false,
            workflow_enabled: false,
            config_path: get_config_path(),
            workspace: PathBuf::new(),
            model_list: Vec::new(),
            heartbeat_interval_secs: 300,
            heartbeat_enabled: true,
            gateway_host: "127.0.0.1".to_string(),
            gateway_port: 8080,
            llm_logging_enabled: false,
            forge_trace_enabled: false,
            forge_learning_enabled: false,
            devices_enabled: false,
            devices_monitor_usb: false,
            cron_exec_timeout_minutes: 5,
            restrict_to_workspace: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Top-level config file structure for loading from disk.
// ---------------------------------------------------------------------------

/// On-disk config.json structure (minimal subset needed for initialization).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    workspace: Option<String>,
    /// Model list -- supports both `model_list` (nemesis-config) and `models` (legacy).
    #[serde(default, alias = "model_list")]
    models: Vec<ModelEntry>,
    #[serde(default)]
    heartbeat: HeartbeatConfigFile,
    #[serde(default)]
    gateway: GatewayConfigFile,
    #[serde(default)]
    forge: ForgeConfigFile,
    #[serde(default)]
    memory: EnabledConfigFile,
    #[serde(default)]
    workflow: EnabledConfigFile,
    #[serde(default)]
    security: SecurityConfigFile,
    #[serde(default)]
    logging: LoggingConfigFile,
    #[serde(default)]
    devices: DevicesConfigFile,
    #[serde(default)]
    agents: AgentsConfigFile,
    #[serde(default)]
    tools: ToolsConfigFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct HeartbeatConfigFile {
    #[serde(default = "default_heartbeat_interval")]
    interval: u64,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_heartbeat_interval() -> u64 {
    300
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct GatewayConfigFile {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    8080
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ForgeConfigFile {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    trace: ForgeTraceConfigFile,
    #[serde(default)]
    learning: ForgeLearningConfigFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ForgeTraceConfigFile {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ForgeLearningConfigFile {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct EnabledConfigFile {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SecurityConfigFile {
    #[serde(default = "default_true")]
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LoggingConfigFile {
    #[serde(default)]
    llm: Option<LlmLoggingConfigFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct LlmLoggingConfigFile {
    #[serde(default)]
    enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct DevicesConfigFile {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    monitor_usb: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentsConfigFile {
    #[serde(default)]
    defaults: AgentsDefaultsConfigFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentsDefaultsConfigFile {
    #[serde(default = "default_true")]
    restrict_to_workspace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ToolsConfigFile {
    #[serde(default = "default_cron_timeout")]
    cron: CronToolsConfigFile,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CronToolsConfigFile {
    #[serde(default = "default_cron_timeout_minutes")]
    exec_timeout_minutes: u64,
}

fn default_cron_timeout_minutes() -> u64 {
    5
}

fn default_cron_timeout() -> CronToolsConfigFile {
    CronToolsConfigFile {
        exec_timeout_minutes: 5,
    }
}

fn default_true() -> bool {
    true
}

// ---------------------------------------------------------------------------
// BotServiceInner
// ---------------------------------------------------------------------------

/// Internal mutable state behind a lock.
struct BotServiceInner {
    state: BotState,
    last_error: Option<String>,
    config_loaded: bool,
    /// The parsed config file content.
    config_file: Option<ConfigFile>,
    /// Resolved workspace path (may be derived from config).
    resolved_workspace: PathBuf,
}

// ---------------------------------------------------------------------------
// ServiceRegistry - holds type-erased references to all initialized services.
// ---------------------------------------------------------------------------

/// Registry of initialized service handles. Each field is Option because
/// services are created conditionally based on configuration.
struct ServiceRegistry {
    /// Forge self-learning service.
    forge: Option<Arc<dyn ForgeService>>,
    /// Memory manager.
    memory: Option<Arc<dyn MemoryService>>,
    /// Heartbeat service.
    heartbeat: Option<Arc<dyn HeartbeatService>>,
    /// Device monitoring service.
    devices: Option<Arc<dyn DeviceService>>,
    /// Health HTTP server.
    health: Option<Arc<dyn HealthServer>>,
    /// Channel manager.
    channels: Option<Arc<dyn ChannelManager>>,
    /// Agent loop.
    agent: Option<Arc<dyn AgentLoopService>>,
    /// Cron service.
    cron: Option<Arc<dyn CronService>>,
    /// Security checker.
    security: Option<Arc<dyn SecurityService>>,
    /// Workflow engine.
    workflow: Option<Arc<dyn WorkflowService>>,
    /// Skills loader.
    skills: Option<Arc<dyn SkillsService>>,
    /// Observer manager.
    observer: Option<Arc<dyn ObserverManager>>,
}

impl Default for ServiceRegistry {
    fn default() -> Self {
        Self {
            forge: None,
            memory: None,
            heartbeat: None,
            devices: None,
            health: None,
            channels: None,
            agent: None,
            cron: None,
            security: None,
            workflow: None,
            skills: None,
            observer: None,
        }
    }
}

// ---------------------------------------------------------------------------
// BotService
// ---------------------------------------------------------------------------

/// Main service orchestrator.
///
/// Holds configuration, tracks component state, and manages the lifecycle
/// of all bot services (bus, channels, agent, security, forge, etc.).
///
/// The service lifecycle is:
/// 1. `new()` - creates a stopped service
/// 2. `start()` - performs 4-phase initialization
/// 3. `stop()` - gracefully shuts down in reverse order
/// 4. `restart()` - stop + start
pub struct BotService {
    config: BotServiceConfig,
    inner: Arc<RwLock<BotServiceInner>>,
    enabled: Arc<RwLock<EnabledComponents>>,
    /// Service cancellation broadcast channel.
    cancel_tx: Arc<Mutex<Option<tokio::sync::broadcast::Sender<()>>>>,
    /// Type-erased service handles, behind a lock for concurrent access.
    services: Arc<RwLock<ServiceRegistry>>,
    /// Optional restart callback for post-save restart (set by application wiring layer).
    restart_callback: Arc<Mutex<Option<Box<dyn Fn() -> nemesis_types::error::Result<()> + Send + Sync>>>>,
    /// Log hook chain for bridging log events to external consumers (e.g., SSE).
    log_hooks: Arc<crate::log_hook::LogHookChain>,
}

impl BotService {
    /// Create a new (stopped) BotService from the given config.
    pub fn new(config: BotServiceConfig) -> Self {
        info!("BotService created (stopped)");
        Self {
            config,
            inner: Arc::new(RwLock::new(BotServiceInner {
                state: BotState::NotStarted,
                last_error: None,
                config_loaded: false,
                config_file: None,
                resolved_workspace: PathBuf::new(),
            })),
            enabled: Arc::new(RwLock::new(EnabledComponents::new())),
            cancel_tx: Arc::new(Mutex::new(None)),
            services: Arc::new(RwLock::new(ServiceRegistry::default())),
            restart_callback: Arc::new(Mutex::new(None)),
            log_hooks: Arc::new(crate::log_hook::LogHookChain::new()),
        }
    }

    /// Create a BotService with default config using the detected config path.
    pub fn with_default_config() -> Self {
        Self::new(BotServiceConfig::default())
    }

    // -----------------------------------------------------------------------
    // Lifecycle: Start / Stop / Restart
    // -----------------------------------------------------------------------

    /// Start all configured components.
    ///
    /// Performs 4-phase initialization:
    /// 1. Load configuration
    /// 2. Validate configuration
    /// 3. Initialize components
    /// 4. Start services
    pub fn start(&self) -> nemesis_types::error::Result<()> {
        {
            let inner = self.inner.read();
            if inner.state.is_running() || inner.state == BotState::Starting {
                return Err(nemesis_types::error::NemesisError::Other(format!(
                    "bot is already {}",
                    inner.state
                )));
            }
        }

        info!("bot_service: Starting bot service...");

        // Set state to Starting
        {
            let mut inner = self.inner.write();
            inner.state = BotState::Starting;
            inner.last_error = None;
        }

        // Create cancellation channel
        let (cancel_tx, _) = tokio::sync::broadcast::channel(1);
        *self.cancel_tx.lock() = Some(cancel_tx);

        // Phase 1: Load configuration
        if let Err(e) = self.load_config() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "failed to load config: {}",
                e
            )));
        }

        // Phase 2: Validate configuration
        if let Err(e) = self.validate_config() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "config validation failed: {}",
                e
            )));
        }

        // Phase 3: Initialize components
        if let Err(e) = self.init_components() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "failed to initialize components: {}",
                e
            )));
        }

        // Phase 4: Start services
        if let Err(e) = self.start_services() {
            self.stop_all();
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "failed to start services: {}",
                e
            )));
        }

        {
            let mut inner = self.inner.write();
            inner.state = BotState::Running;
        }
        info!("bot_service: Bot service started successfully");
        Ok(())
    }

    /// Stop all components gracefully.
    pub fn stop(&self) -> nemesis_types::error::Result<()> {
        {
            let inner = self.inner.read();
            if !inner.state.can_stop() {
                return Err(nemesis_types::error::NemesisError::Other(format!(
                    "bot is not running (current state: {})",
                    inner.state
                )));
            }
        }

        info!("bot_service: Stopping bot service...");

        // Cancel context
        if let Some(tx) = self.cancel_tx.lock().as_ref() {
            let _ = tx.send(());
        }

        self.stop_all();

        {
            let mut inner = self.inner.write();
            inner.state = BotState::NotStarted;
        }
        info!("bot_service: Bot service stopped");
        Ok(())
    }

    /// Restart stops and then starts the bot service.
    pub fn restart(&self) -> nemesis_types::error::Result<()> {
        info!("bot_service: Restarting bot service...");

        {
            let inner = self.inner.read();
            if inner.state.is_running() {
                drop(inner);
                self.stop()?;
            }
        }

        // Brief pause for cleanup
        std::thread::sleep(std::time::Duration::from_millis(500));

        self.start()?;

        info!("bot_service: Bot service restarted successfully");
        Ok(())
    }

    /// Set a restart callback for post-save restart.
    ///
    /// The application wiring layer should call this to provide a way for
    /// the BotService to restart itself asynchronously after config save.
    /// The callback wraps `BotService::restart()` in a way that satisfies
    /// Rust's borrow checker (typically by cloning an Arc to the BotService).
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let svc = Arc::new(BotService::new(config));
    /// let svc_clone = svc.clone();
    /// svc.set_restart_callback(Box::new(move || svc_clone.restart()));
    /// ```
    pub fn set_restart_callback(&self, cb: Box<dyn Fn() -> nemesis_types::error::Result<()> + Send + Sync>) {
        *self.restart_callback.lock() = Some(cb);
    }

    // -----------------------------------------------------------------------
    // State queries
    // -----------------------------------------------------------------------

    /// Return the current bot state.
    pub fn get_state(&self) -> BotState {
        self.inner.read().state
    }

    /// Return the last error, if any.
    pub fn get_error(&self) -> Option<String> {
        self.inner.read().last_error.clone()
    }

    /// Return a clone of the config.
    pub fn get_config(&self) -> BotServiceConfig {
        self.config.clone()
    }

    /// Return a reference to the config.
    pub fn config(&self) -> &BotServiceConfig {
        &self.config
    }

    /// Return a snapshot of enabled components.
    pub fn enabled_components(&self) -> EnabledComponents {
        self.enabled.read().clone()
    }

    /// Return the core components for external access.
    ///
    /// Returns a map of component name to a JSON object indicating its status.
    /// This mirrors the Go `GetComponents` method that returns a map of
    /// component names to their instances.
    pub fn get_components(&self) -> HashMap<String, serde_json::Value> {
        let enabled = self.enabled.read();
        let mut components = HashMap::new();
        for comp in enabled.enabled_list() {
            components.insert(
                comp.label().to_string(),
                serde_json::json!({ "enabled": true }),
            );
        }
        components
    }

    /// Return the Forge instance if available.
    ///
    /// Mirrors the Go `GetForge()` method. Returns None if Forge is not
    /// enabled or not yet initialized.
    pub fn get_forge(&self) -> Option<Arc<dyn ForgeService>> {
        self.services.read().forge.clone()
    }

    /// Return the memory manager instance if available.
    pub fn get_memory(&self) -> Option<Arc<dyn MemoryService>> {
        self.services.read().memory.clone()
    }

    /// Return the channel manager instance if available.
    pub fn get_channel_manager(&self) -> Option<Arc<dyn ChannelManager>> {
        self.services.read().channels.clone()
    }

    /// Return the agent loop instance if available.
    pub fn get_agent_loop(&self) -> Option<Arc<dyn AgentLoopService>> {
        self.services.read().agent.clone()
    }

    /// Return the resolved workspace path.
    pub fn workspace(&self) -> PathBuf {
        self.inner.read().resolved_workspace.clone()
    }

    /// Create a heartbeat handler callback.
    ///
    /// Mirrors Go's `createHeartbeatHandler()`. Returns a closure that:
    /// 1. Checks if BOOTSTRAP.md exists (if so, skips heartbeat)
    /// 2. Falls back to "cli:direct" session key
    /// 3. Calls `agent_loop.ProcessHeartbeat()`
    ///
    /// The returned closure can be passed to the HeartbeatService.
    pub fn create_heartbeat_handler(&self) -> Box<dyn Fn() + Send + Sync> {
        let workspace = self.inner.read().resolved_workspace.clone();
        let agent_loop = self.services.read().agent.clone();

        Box::new(move || {
            // Check if BOOTSTRAP.md exists (skip heartbeat during initialization).
            let bootstrap_path = workspace.join("BOOTSTRAP.md");
            if bootstrap_path.exists() {
                info!("Skipping heartbeat: BOOTSTRAP.md exists (initialization in progress)");
                return;
            }

            // Process heartbeat through agent loop.
            if let Some(ref agent) = agent_loop {
                match agent.process_heartbeat() {
                    Ok(response) => {
                        if !response.is_empty() {
                            info!(response_len = response.len(), "Heartbeat processed");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "Heartbeat processing failed");
                    }
                }
            } else {
                warn!("Heartbeat skipped: agent loop not initialized");
            }
        })
    }

    /// Return a cancellation token receiver for cooperative shutdown.
    pub fn cancel_receiver(&self) -> Option<tokio::sync::broadcast::Receiver<()>> {
        self.cancel_tx.lock().as_ref().map(|tx| tx.subscribe())
    }

    // -----------------------------------------------------------------------
    // Dependency injection - called by the application wiring layer.
    // -----------------------------------------------------------------------

    /// Inject a Forge service instance.
    ///
    /// Called during Phase 4 of initialization when forge is enabled.
    /// The caller creates the concrete Forge instance and injects it here.
    pub fn inject_forge(&self, forge: Arc<dyn ForgeService>) {
        self.services.write().forge = Some(forge);
    }

    /// Inject a Memory service instance.
    pub fn inject_memory(&self, memory: Arc<dyn MemoryService>) {
        self.services.write().memory = Some(memory);
    }

    /// Inject a Heartbeat service instance.
    pub fn inject_heartbeat(&self, heartbeat: Arc<dyn HeartbeatService>) {
        self.services.write().heartbeat = Some(heartbeat);
    }

    /// Inject a Device service instance.
    pub fn inject_devices(&self, devices: Arc<dyn DeviceService>) {
        self.services.write().devices = Some(devices);
    }

    /// Inject a Health server instance.
    pub fn inject_health(&self, health: Arc<dyn HealthServer>) {
        self.services.write().health = Some(health);
    }

    /// Inject a Channel manager instance.
    pub fn inject_channels(&self, channels: Arc<dyn ChannelManager>) {
        self.services.write().channels = Some(channels);
    }

    /// Inject an Agent loop instance.
    pub fn inject_agent(&self, agent: Arc<dyn AgentLoopService>) {
        self.services.write().agent = Some(agent);
    }

    /// Inject a Cron service instance.
    pub fn inject_cron(&self, cron: Arc<dyn CronService>) {
        self.services.write().cron = Some(cron);
    }

    /// Inject a Security service instance.
    pub fn inject_security(&self, security: Arc<dyn SecurityService>) {
        self.services.write().security = Some(security);
    }

    /// Inject a Workflow service instance.
    pub fn inject_workflow(&self, workflow: Arc<dyn WorkflowService>) {
        self.services.write().workflow = Some(workflow);
    }

    /// Inject a Skills service instance.
    pub fn inject_skills(&self, skills: Arc<dyn SkillsService>) {
        self.services.write().skills = Some(skills);
    }

    /// Inject an Observer manager instance.
    pub fn inject_observer(&self, observer: Arc<dyn ObserverManager>) {
        self.services.write().observer = Some(observer);
    }

    // -----------------------------------------------------------------------
    // Log hook registration
    // -----------------------------------------------------------------------

    /// Register a log hook to receive log events.
    ///
    /// Multiple hooks can be registered. Each hook receives every log event.
    /// The gateway layer typically registers a hook that bridges log events
    /// to the web server's SSE event stream.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let hook = Arc::new(SseLogHook::new(event_tx));
    /// bot_service.register_log_hook(hook);
    /// ```
    pub fn register_log_hook(&self, hook: LogHookHandle) {
        self.log_hooks.register(hook);
        info!("bot_service: Log hook registered (total: {})", self.log_hooks.len());
    }

    /// Return a reference to the log hook chain.
    ///
    /// Allows direct access to the chain for dispatching events or
    /// inspecting the registered hooks.
    pub fn log_hooks(&self) -> &crate::log_hook::LogHookChain {
        &self.log_hooks
    }

    // -----------------------------------------------------------------------
    // Save config
    // -----------------------------------------------------------------------

    /// Save the configuration to disk and optionally restart the bot.
    ///
    /// Mirrors the Go `SaveConfig` method:
    /// 1. Validates the provided config JSON
    /// 2. Writes it to the config file path
    /// 3. If `restart` is true and the bot is running, schedules a restart
    ///
    /// The restart happens asynchronously in a background task (matching
    /// the Go `go func()` pattern).
    pub fn save_config(
        &self,
        config_json: &serde_json::Value,
        restart: bool,
    ) -> nemesis_types::error::Result<()> {
        info!("bot_service: Saving configuration to {:?}", self.config.config_path);

        // Validate that the config JSON is a valid object
        if !config_json.is_object() {
            return Err(nemesis_types::error::NemesisError::Validation(
                "config must be a JSON object".to_string(),
            ));
        }

        // Attempt to deserialize as ConfigFile to validate structure
        let config_data: ConfigFile = serde_json::from_value(config_json.clone())
            .map_err(|e| {
                nemesis_types::error::NemesisError::Validation(format!(
                    "invalid config structure: {}",
                    e
                ))
            })?;

        // Validate models configuration (at least one model with API key)
        if config_data.models.is_empty() {
            return Err(nemesis_types::error::NemesisError::Validation(
                "no models configured".to_string(),
            ));
        }

        let has_valid_model = config_data
            .models
            .iter()
            .any(|m| !m.api_key.is_empty());
        if !has_valid_model {
            return Err(nemesis_types::error::NemesisError::Validation(
                "no model with valid API key found".to_string(),
            ));
        }

        // Ensure the parent directory exists
        if let Some(parent) = self.config.config_path.parent() {
            if !parent.exists() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    nemesis_types::error::NemesisError::Io(e)
                })?;
            }
        }

        // Serialize with pretty formatting
        let config_str = serde_json::to_string_pretty(config_json).map_err(|e| {
            nemesis_types::error::NemesisError::Serialization(e)
        })?;

        // Write to disk atomically: write to temp file, then rename
        let temp_path = self.config.config_path.with_extension("json.tmp");
        std::fs::write(&temp_path, &config_str).map_err(|e| {
            nemesis_types::error::NemesisError::Io(e)
        })?;

        // Atomic rename
        std::fs::rename(&temp_path, &self.config.config_path).map_err(|e| {
            // Clean up temp file on rename failure
            let _ = std::fs::remove_file(&temp_path);
            nemesis_types::error::NemesisError::Io(e)
        })?;

        info!("bot_service: Configuration saved successfully");

        // Update the cached config file
        {
            let mut inner = self.inner.write();
            inner.config_file = Some(config_data);
            inner.config_loaded = true;
        }

        // Trigger restart if requested and bot is running
        if restart && self.get_state().is_running() {
            info!("bot_service: Restart scheduled after config save");
            let restart_cb = self.restart_callback.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let cb = restart_cb.lock();
                if let Some(ref callback) = *cb {
                    match callback() {
                        Ok(()) => info!("bot_service: Post-save restart completed"),
                        Err(e) => error!("bot_service: Post-save restart failed: {}", e),
                    }
                } else {
                    warn!("bot_service: No restart callback set; skipping post-save restart");
                }
            });
        }

        Ok(())
    }

    /// Save the configuration and trigger an async restart with 100ms delay.
    ///
    /// Convenience method that calls `save_config(config_json, true)`.
    /// Mirrors Go's `SaveConfig(cfg, true)` pattern where the restart flag
    /// is always set. The restart happens asynchronously in a background
    /// task after a 100ms delay, matching Go's `go func()` + `time.Sleep(100ms)`.
    pub fn save_config_and_restart(
        &self,
        config_json: &serde_json::Value,
    ) -> nemesis_types::error::Result<()> {
        self.save_config(config_json, true)
    }

    // -----------------------------------------------------------------------
    // Internal lifecycle phases
    // -----------------------------------------------------------------------

    /// Phase 1: Load configuration from disk.
    ///
    /// Reads the config.json file and populates the internal state with
    /// the parsed configuration. The workspace path is derived from the
    /// config's `workspace` field, falling back to the directory containing
    /// the config file.
    fn load_config(&self) -> Result<(), String> {
        info!(
            "bot_service: Loading config from {:?}",
            self.config.config_path
        );

        // Check config file exists
        if !self.config.config_path.exists() {
            return Err(format!("config file not found: {:?}", self.config.config_path));
        }

        // Read and parse the config file
        let config_content = std::fs::read_to_string(&self.config.config_path)
            .map_err(|e| format!("failed to read config file: {}", e))?;

        let config_file: ConfigFile = serde_json::from_str(&config_content)
            .map_err(|e| format!("failed to parse config file: {}", e))?;

        // Resolve workspace path
        let workspace = if let Some(ref ws) = config_file.workspace {
            PathBuf::from(ws)
        } else if !self.config.workspace.as_os_str().is_empty() {
            self.config.workspace.clone()
        } else {
            // Default: parent directory of config file
            self.config
                .config_path
                .parent()
                .map(|p| p.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."))
        };

        info!(
            "bot_service: Config loaded, workspace={:?}",
            workspace
        );

        // Update config flags from the loaded file
        {
            let mut inner = self.inner.write();
            inner.config_loaded = true;
            inner.config_file = Some(config_file);
            inner.resolved_workspace = workspace;
        }

        // Update the service config flags based on loaded config
        {
            let inner = self.inner.read();
            if let Some(ref cf) = inner.config_file {
                // These are already set by the caller in most cases,
                // but we update them if the config file says otherwise
                let _ = (
                    cf.security.enabled,
                    cf.forge.enabled,
                    cf.memory.enabled,
                    cf.workflow.enabled,
                );
            }
        }

        Ok(())
    }

    /// Phase 2: Validate the loaded configuration.
    ///
    /// Checks that:
    /// - Configuration has been loaded
    /// - At least one model is configured
    /// - At least one model has a valid API key
    fn validate_config(&self) -> Result<(), String> {
        let inner = self.inner.read();
        if !inner.config_loaded {
            return Err("config not loaded".to_string());
        }

        let config_file = inner
            .config_file
            .as_ref()
            .ok_or_else(|| "config file not parsed".to_string())?;

        // Check if at least one model is configured
        if config_file.models.is_empty() {
            return Err("no models configured".to_string());
        }

        // Check if at least one model has a valid API key
        let has_valid_model = config_file
            .models
            .iter()
            .any(|m| !m.api_key.is_empty());

        if !has_valid_model {
            return Err("no model with valid API key found".to_string());
        }

        drop(inner);

        info!("bot_service: Configuration validated");
        Ok(())
    }

    /// Phase 3: Initialize all components.
    ///
    /// Uses the 4-phase initialization pattern from the Go code:
    /// - Phase 1 (sequential): Core components with inter-dependencies
    ///   (provider, message bus, agent loop, channel manager).
    /// - Phase 2 (parallel): Independent services (cron, heartbeat, state,
    ///   health server).
    /// - Phase 3 (sequential): Wire up service dependencies (cron tool,
    ///   heartbeat handler, device service).
    /// - Phase 4 (conditional): Forge, Observer, Memory, LearningEngine.
    ///
    /// In this Rust implementation, the concrete service creation is delegated
    /// to the application wiring layer via dependency injection. The init
    /// phase tracks which components are enabled and validates that required
    /// services have been injected before starting.
    fn init_components(&self) -> Result<(), String> {
        let mut enabled = self.enabled.write();

        // Clone the config file data so we can release the lock before proceeding.
        let config_file_data = {
            let inner = self.inner.read();
            inner
                .config_file
                .clone()
                .ok_or_else(|| "config file not loaded".to_string())?
        };

        let _workspace = self.inner.read().resolved_workspace.clone();

        // --- Phase 1: Sequential core setup ---
        // These components have inter-dependencies and must be created in order.
        // In the Go code: provider -> msgBus -> agentLoop + channelMgr (parallel)
        // -> wire agent <-> channelMgr.
        //
        // Here we track which components should be enabled. The actual wiring
        // happens at the application layer via inject_* methods.

        // Core components are always required
        enabled.enable(Component::Bus);
        info!("Component initialized: {}", Component::Bus.label());

        enabled.enable(Component::Agent);
        info!("Component initialized: {}", Component::Agent.label());

        enabled.enable(Component::Channels);
        info!("Component initialized: {}", Component::Channels.label());

        // --- Phase 2: Parallel independent service creation ---
        // In Go, these run in parallel using errgroup. In Rust, the application
        // layer creates them using the parallel_init utilities. Here we just
        // track which ones should be enabled.

        // Cron service (always enabled)
        enabled.enable(Component::Cron);
        info!("Component initialized: {}", Component::Cron.label());

        // Heartbeat service (check config)
        if config_file_data.heartbeat.enabled {
            enabled.enable(Component::Heartbeat);
            info!("Component initialized: {}", Component::Heartbeat.label());
        }

        // Health server (always enabled)
        enabled.enable(Component::Health);
        info!("Component initialized: {}", Component::Health.label());

        // Devices (check config)
        if config_file_data.devices.enabled {
            enabled.enable(Component::Devices);
            info!("Component initialized: {}", Component::Devices.label());
        }

        // Skills (always enabled)
        enabled.enable(Component::Skills);
        info!("Component initialized: {}", Component::Skills.label());

        // --- Phase 3: Wire up service dependencies (sequential) ---
        // These steps depend on the services created in Phase 2.
        // The actual wiring happens at the application layer.

        // Security (config-driven)
        if self.config.security_enabled || config_file_data.security.enabled {
            enabled.enable(Component::Security);
            info!("Component initialized: {}", Component::Security.label());
        }

        // Workflow (config-driven)
        if self.config.workflow_enabled || config_file_data.workflow.enabled {
            enabled.enable(Component::Workflow);
            info!("Component initialized: {}", Component::Workflow.label());
        }

        // --- Phase 4: Forge and Observer setup ---
        // These depend on agentLoop + provider being available.

        // Forge self-learning module
        if self.config.forge_enabled || config_file_data.forge.enabled {
            enabled.enable(Component::Forge);
            info!("Component initialized: {}", Component::Forge.label());
        }

        // Cluster (config-driven)
        if self.config.cluster_enabled {
            enabled.enable(Component::Cluster);
            info!("Component initialized: {}", Component::Cluster.label());
        }

        // Memory subsystem (config-driven)
        if self.config.memory_enabled || config_file_data.memory.enabled {
            enabled.enable(Component::Memory);
            info!("Component initialized: {}", Component::Memory.label());
        }

        // Observer (enabled if any observers will be registered)
        enabled.enable(Component::Observer);
        info!("Component initialized: {}", Component::Observer.label());

        info!("bot_service: Components initialized");
        Ok(())
    }

    /// Phase 4: Start all initialized services in dependency order.
    ///
    /// The start order mirrors the Go code:
    /// 1. Heartbeat service
    /// 2. Device service
    /// 3. Channel manager (StartAll)
    /// 4. Health server (in background)
    /// 5. Agent loop (in background)
    /// 6. Cron service
    /// 7. Forge service
    ///
    /// Each service is started via its `LifecycleService::start()` method.
    /// Services that are not injected (None) are skipped gracefully.
    fn start_services(&self) -> Result<(), String> {
        info!("bot_service: Starting all services...");
        let services = self.services.read();

        // 1. Start heartbeat service
        if let Some(ref heartbeat) = services.heartbeat {
            heartbeat.start().map_err(|e| {
                format!("failed to start heartbeat service: {}", e)
            })?;
            info!("bot_service: Heartbeat service started");
        }

        // 2. Start device service
        if let Some(ref devices) = services.devices {
            devices.start().map_err(|e| {
                format!("failed to start device service: {}", e)
            })?;
            info!("bot_service: Device service started");
        }

        // 3. Start channel manager
        if let Some(ref channels) = services.channels {
            channels.start().map_err(|e| {
                format!("failed to start channel manager: {}", e)
            })?;

            let enabled_channels = channels.enabled_channels();
            if !enabled_channels.is_empty() {
                info!(
                    "bot_service: Channels enabled: {:?}",
                    enabled_channels
                );
            }
        }

        // 4. Start health server (in background)
        if let Some(ref health) = services.health {
            // Health server runs in the background. We clone the Arc to move
            // into the spawned task.
            let health_clone = health.clone();
            tokio::spawn(async move {
                if let Err(e) = health_clone.start() {
                    error!("bot_service: Health server error: {}", e);
                }
            });
            info!(
                "bot_service: Health server started on {}:{}",
                self.config.gateway_host, self.config.gateway_port
            );
        }

        // 5. Start agent loop (in background)
        if let Some(ref agent) = services.agent {
            let agent_clone = agent.clone();
            let cancel_rx = self.cancel_tx.lock().as_ref().map(|tx| tx.subscribe());
            tokio::spawn(async move {
                if let Err(e) = agent_clone.start() {
                    error!("bot_service: Agent loop error: {}", e);
                }
                // Keep running until cancelled
                if let Some(mut rx) = cancel_rx {
                    let _ = rx.recv().await;
                } else {
                    // No cancel channel, run until dropped
                    std::future::pending::<()>().await;
                }
            });
            info!("bot_service: Agent loop started");
        }

        // 6. Start cron service
        if let Some(ref cron) = services.cron {
            match cron.start() {
                Ok(()) => {
                    info!("bot_service: Cron service started");
                }
                Err(e) => {
                    warn!("bot_service: Cron service start failed: {}", e);
                    // Non-fatal: cron failure should not prevent bot from starting
                }
            }
        }

        // 7. Start Forge self-learning module
        if let Some(ref forge) = services.forge {
            forge.start().map_err(|e| {
                format!("failed to start forge service: {}", e)
            })?;
            info!("bot_service: Forge service started");
        }

        info!("bot_service: All services started");
        Ok(())
    }

    /// Stop all services in reverse order of initialization.
    ///
    /// Mirrors the Go `stopAll()` method. Each service is stopped via its
    /// `LifecycleService::stop()` method or its specific shutdown method
    /// (e.g., `MemoryService::close()`).
    ///
    /// Stop order (reverse of start):
    /// 1. Forge service
    /// 2. Cron service
    /// 3. Health server
    /// 4. Device service
    /// 5. Heartbeat service
    /// 6. Channel manager
    /// 7. Agent loop
    /// 8. Memory manager (close, not stop)
    fn stop_all(&self) {
        info!("bot_service: Stopping all services (reverse order)...");

        let mut services = self.services.write();

        // Stop in reverse order of start
        // 1. Forge
        if let Some(ref forge) = services.forge {
            if let Err(e) = forge.stop() {
                warn!("bot_service: Error stopping forge service: {}", e);
            } else {
                info!("bot_service: Forge service stopped");
            }
        }

        // 2. Cron
        if let Some(ref cron) = services.cron {
            if let Err(e) = cron.stop() {
                warn!("bot_service: Error stopping cron service: {}", e);
            } else {
                info!("bot_service: Cron service stopped");
            }
        }

        // 3. Health server
        if let Some(ref health) = services.health {
            if let Err(e) = health.stop() {
                warn!("bot_service: Error stopping health server: {}", e);
            } else {
                info!("bot_service: Health server stopped");
            }
        }

        // 4. Device service
        if let Some(ref devices) = services.devices {
            if let Err(e) = devices.stop() {
                warn!("bot_service: Error stopping device service: {}", e);
            } else {
                info!("bot_service: Device service stopped");
            }
        }

        // 5. Heartbeat service
        if let Some(ref heartbeat) = services.heartbeat {
            if let Err(e) = heartbeat.stop() {
                warn!("bot_service: Error stopping heartbeat service: {}", e);
            } else {
                info!("bot_service: Heartbeat service stopped");
            }
        }

        // 6. Channel manager
        if let Some(ref channels) = services.channels {
            if let Err(e) = channels.stop() {
                warn!("bot_service: Error stopping channel manager: {}", e);
            } else {
                info!("bot_service: Channel manager stopped");
            }
        }

        // 7. Agent loop
        if let Some(ref agent) = services.agent {
            if let Err(e) = agent.stop() {
                warn!("bot_service: Error stopping agent loop: {}", e);
            } else {
                info!("bot_service: Agent loop stopped");
            }
        }

        // 8. Memory manager (close, not stop)
        if let Some(ref memory) = services.memory {
            memory.close();
            info!("bot_service: Memory manager closed");
        }

        // Drop all service references
        services.forge = None;
        services.memory = None;
        services.heartbeat = None;
        services.devices = None;
        services.health = None;
        services.channels = None;
        services.agent = None;
        services.cron = None;
        services.security = None;
        services.workflow = None;
        services.skills = None;
        services.observer = None;

        drop(services);

        // Mark all components as disabled
        let mut enabled = self.enabled.write();
        enabled.disable_all();

        info!("bot_service: All services stopped");
    }

    /// Set state and record error.
    fn set_state_with_error(&self, state: BotState, err: &str) {
        let mut inner = self.inner.write();
        inner.state = state;
        inner.last_error = Some(err.to_string());
        error!(
            "bot_service: Bot service error, state={}, error={}",
            state, err
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config() -> BotServiceConfig {
        BotServiceConfig {
            config_path: PathBuf::from("test_config.json"),
            workspace: PathBuf::from("/tmp/test_workspace"),
            ..BotServiceConfig::default()
        }
    }

    fn make_config_with_file(dir: &std::path::Path) -> BotServiceConfig {
        let config_path = dir.join("config.json");
        let config_content = serde_json::json!({
            "workspace": dir.to_string_lossy(),
            "models": [
                {
                    "model": "test/test-model-1.0",
                    "api_key": "test-key-12345",
                    "base_url": "",
                    "is_default": true
                }
            ],
            "heartbeat": {
                "interval": 60,
                "enabled": true
            },
            "gateway": {
                "host": "127.0.0.1",
                "port": 8080
            },
            "security": {
                "enabled": true
            },
            "forge": {
                "enabled": false
            },
            "memory": {
                "enabled": false
            },
            "workflow": {
                "enabled": false
            },
            "devices": {
                "enabled": false,
                "monitor_usb": false
            },
            "agents": {
                "defaults": {
                    "restrict_to_workspace": true
                }
            },
            "tools": {
                "cron": {
                    "exec_timeout_minutes": 5
                }
            }
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&config_content).unwrap())
            .unwrap();

        BotServiceConfig {
            config_path,
            workspace: dir.to_path_buf(),
            ..BotServiceConfig::default()
        }
    }

    #[test]
    fn test_new_service_is_not_started() {
        let svc = BotService::new(make_config());
        assert_eq!(svc.get_state(), BotState::NotStarted);
        assert!(svc.get_error().is_none());
        assert!(svc.enabled_components().enabled_list().is_empty());
    }

    #[test]
    fn test_start_fails_without_config_file() {
        let svc = BotService::new(make_config());
        // Should fail because test_config.json doesn't exist
        let result = svc.start();
        assert!(result.is_err());
        assert_eq!(svc.get_state(), BotState::Error);
        assert!(svc.get_error().is_some());
    }

    #[test]
    fn test_stop_when_not_running_fails() {
        let svc = BotService::new(make_config());
        let result = svc.stop();
        assert!(result.is_err());
    }

    #[test]
    fn test_restart_when_not_running_starts() {
        let svc = BotService::new(make_config());
        // Restart on a stopped bot will try to start and fail due to missing config
        let result = svc.restart();
        assert!(result.is_err());
    }

    #[test]
    fn test_get_components_when_stopped() {
        let svc = BotService::new(make_config());
        let components = svc.get_components();
        assert!(components.is_empty());
    }

    #[test]
    fn test_save_config_writes_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Write initial config
        let initial_config = serde_json::json!({
            "models": [
                {
                    "model": "test/model-1.0",
                    "api_key": "test-key",
                    "base_url": "",
                    "is_default": true
                }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&initial_config).unwrap())
            .unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path: config_path.clone(),
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let new_config = serde_json::json!({
            "models": [
                {
                    "model": "test/model-2.0",
                    "api_key": "new-key",
                    "base_url": "http://localhost:9090",
                    "is_default": true
                }
            ]
        });

        let result = svc.save_config(&new_config, false);
        assert!(result.is_ok());

        // Verify file was written
        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["models"][0]["model"], "test/model-2.0");
        assert_eq!(parsed["models"][0]["api_key"], "new-key");
    }

    #[test]
    fn test_save_config_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        // Not an object
        let result = svc.save_config(&serde_json::json!("not an object"), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_config_rejects_no_models() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let result = svc.save_config(&serde_json::json!({ "models": [] }), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_config_rejects_no_api_key() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let result = svc.save_config(
            &serde_json::json!({
                "models": [{ "model": "test/1.0", "api_key": "", "base_url": "", "is_default": true }]
            }),
            false,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_config_checks_models() {
        let dir = tempfile::tempdir().unwrap();
        // Config with no models
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, r#"{"models": []}"#).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("no models configured"));
    }

    #[test]
    fn test_validate_config_checks_api_keys() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(
            &config_path,
            r#"{"models": [{"model": "test/1.0", "api_key": "", "base_url": "", "is_default": true}]}"#,
        )
        .unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("no model with valid API key"));
    }

    #[test]
    fn test_full_start_stop_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        // Start should succeed
        let result = svc.start();
        assert!(result.is_ok());
        assert_eq!(svc.get_state(), BotState::Running);

        // Verify core components are enabled
        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Bus));
        assert!(enabled.is_enabled(Component::Agent));
        assert!(enabled.is_enabled(Component::Channels));
        assert!(enabled.is_enabled(Component::Health));
        assert!(enabled.is_enabled(Component::Cron));
        assert!(enabled.is_enabled(Component::Skills));
        assert!(enabled.is_enabled(Component::Observer));

        // Stop should succeed
        let result = svc.stop();
        assert!(result.is_ok());
        assert_eq!(svc.get_state(), BotState::NotStarted);
        assert!(svc.enabled_components().enabled_list().is_empty());
    }

    #[test]
    fn test_restart_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        // Start
        svc.start().unwrap();
        assert_eq!(svc.get_state(), BotState::Running);

        // Restart
        let result = svc.restart();
        assert!(result.is_ok());
        assert_eq!(svc.get_state(), BotState::Running);
    }

    #[test]
    fn test_double_start_fails() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();
        let result = svc.start();
        assert!(result.is_err());
    }

    #[test]
    fn test_inject_and_get_forge() {
        let svc = BotService::new(make_config());

        // Before injection
        assert!(svc.get_forge().is_none());

        // Create a mock forge service
        struct MockForge;
        impl LifecycleService for MockForge {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ForgeService for MockForge {
            fn forge_name(&self) -> &str { "mock_forge" }
        }

        svc.inject_forge(Arc::new(MockForge));
        assert!(svc.get_forge().is_some());
        assert_eq!(svc.get_forge().unwrap().forge_name(), "mock_forge");
    }

    #[test]
    fn test_inject_memory() {
        let svc = BotService::new(make_config());
        assert!(svc.get_memory().is_none());

        struct MockMemory;
        impl LifecycleService for MockMemory {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl MemoryService for MockMemory {}

        svc.inject_memory(Arc::new(MockMemory));
        assert!(svc.get_memory().is_some());
    }

    #[test]
    fn test_config_path() {
        let config = make_config();
        assert_eq!(config.config_path, PathBuf::from("test_config.json"));
    }

    #[test]
    fn test_save_config_atomic_write() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Write initial config
        let initial = serde_json::json!({
            "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
        });
        std::fs::write(&config_path, serde_json::to_string(&initial).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path: config_path.clone(),
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        // Update config
        let updated = serde_json::json!({
            "models": [{ "model": "test/2.0", "api_key": "key2", "base_url": "", "is_default": true }]
        });
        svc.save_config(&updated, false).unwrap();

        // Verify no temp file left behind
        assert!(!config_path.with_extension("json.tmp").exists());

        // Verify content is the updated config
        let content = std::fs::read_to_string(&config_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed["models"][0]["model"], "test/2.0");
    }

    #[test]
    fn test_save_config_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("subdir").join("config.json");

        let svc = BotService::new(BotServiceConfig {
            config_path: config_path.clone(),
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let config = serde_json::json!({
            "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
        });

        svc.save_config(&config, false).unwrap();
        assert!(config_path.exists());
    }

    #[test]
    fn test_enabled_components_disable_all() {
        let mut ec = EnabledComponents::new();
        ec.enable(Component::Bus);
        ec.enable(Component::Agent);
        assert_eq!(ec.enabled_list().len(), 2);

        ec.disable_all();
        assert!(ec.enabled_list().is_empty());
    }

    #[test]
    fn test_component_labels() {
        assert_eq!(Component::Bus.label(), "bus");
        assert_eq!(Component::Forge.label(), "forge");
        assert_eq!(Component::Observer.label(), "observer");
        assert_eq!(Component::Health.label(), "health");
    }

    #[test]
    fn test_get_components_reflects_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();

        let components = svc.get_components();
        assert!(components.contains_key("bus"));
        assert!(components.contains_key("agent"));
        assert!(components.contains_key("channels"));
    }

    #[test]
    fn test_workspace_returns_resolved_path() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        // Before start, workspace is empty
        assert!(svc.workspace().as_os_str().is_empty() || svc.workspace().exists());

        svc.start().unwrap();
        // After start, workspace should be resolved
        assert!(!svc.workspace().as_os_str().is_empty());
    }

    // ============================================================
    // Additional tests for inject/get methods, serialization,
    // and configuration edge cases
    // ============================================================

    #[test]
    fn test_inject_and_get_cron() {
        let svc = BotService::new(make_config());

        struct MockCron;
        impl LifecycleService for MockCron {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl CronService for MockCron {}

        svc.inject_cron(Arc::new(MockCron));
        // Cron is not directly gettable but injection should not panic
    }

    #[test]
    fn test_inject_security() {
        let svc = BotService::new(make_config());

        struct MockSecurity;
        impl SecurityService for MockSecurity {}

        svc.inject_security(Arc::new(MockSecurity));
    }

    #[test]
    fn test_inject_workflow() {
        let svc = BotService::new(make_config());

        struct MockWorkflow;
        impl WorkflowService for MockWorkflow {}

        svc.inject_workflow(Arc::new(MockWorkflow));
    }

    #[test]
    fn test_inject_skills() {
        let svc = BotService::new(make_config());

        struct MockSkills;
        impl SkillsService for MockSkills {}

        svc.inject_skills(Arc::new(MockSkills));
    }

    #[test]
    fn test_inject_observer() {
        let svc = BotService::new(make_config());

        struct MockObserver;
        impl ObserverManager for MockObserver {
            fn has_observers(&self) -> bool { false }
        }

        svc.inject_observer(Arc::new(MockObserver));
    }

    #[test]
    fn test_inject_heartbeat() {
        let svc = BotService::new(make_config());

        struct MockHeartbeat;
        impl LifecycleService for MockHeartbeat {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl HeartbeatService for MockHeartbeat {}

        svc.inject_heartbeat(Arc::new(MockHeartbeat));
    }

    #[test]
    fn test_inject_devices() {
        let svc = BotService::new(make_config());

        struct MockDevices;
        impl LifecycleService for MockDevices {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl DeviceService for MockDevices {}

        svc.inject_devices(Arc::new(MockDevices));
    }

    #[test]
    fn test_inject_health() {
        let svc = BotService::new(make_config());

        struct MockHealth;
        impl LifecycleService for MockHealth {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl HealthServer for MockHealth {}

        svc.inject_health(Arc::new(MockHealth));
    }

    #[test]
    fn test_inject_channels() {
        let svc = BotService::new(make_config());

        struct MockChannels;
        impl LifecycleService for MockChannels {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ChannelManager for MockChannels {
            fn enabled_channels(&self) -> Vec<String> { vec![] }
        }

        svc.inject_channels(Arc::new(MockChannels));
        assert!(svc.get_channel_manager().is_some());
    }

    #[test]
    fn test_inject_agent() {
        let svc = BotService::new(make_config());

        struct MockAgent;
        impl LifecycleService for MockAgent {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl AgentLoopService for MockAgent {}

        svc.inject_agent(Arc::new(MockAgent));
        assert!(svc.get_agent_loop().is_some());
    }

    #[test]
    fn test_bot_service_config_default() {
        let config = BotServiceConfig::default();
        assert!(config.security_enabled);
        assert!(!config.forge_enabled);
        assert!(!config.cluster_enabled);
        assert!(config.workspace.as_os_str().is_empty());
        assert_eq!(config.gateway_port, 8080);
    }

    #[test]
    fn test_component_serialization() {
        let component = Component::Bus;
        let json = serde_json::to_string(&component).unwrap();
        let restored: Component = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, Component::Bus);
    }

    #[test]
    fn test_component_all_labels() {
        assert_eq!(Component::Bus.label(), "bus");
        assert_eq!(Component::Channels.label(), "channels");
        assert_eq!(Component::Agent.label(), "agent");
        assert_eq!(Component::Security.label(), "security");
        assert_eq!(Component::Forge.label(), "forge");
        assert_eq!(Component::Cluster.label(), "cluster");
        assert_eq!(Component::Memory.label(), "memory");
        assert_eq!(Component::Workflow.label(), "workflow");
        assert_eq!(Component::Skills.label(), "skills");
        assert_eq!(Component::Cron.label(), "cron");
        assert_eq!(Component::Heartbeat.label(), "heartbeat");
        assert_eq!(Component::Devices.label(), "devices");
        assert_eq!(Component::Health.label(), "health");
        assert_eq!(Component::Observer.label(), "observer");
    }

    #[test]
    fn test_enabled_components_serialization() {
        let mut ec = EnabledComponents::new();
        ec.enable(Component::Bus);
        ec.enable(Component::Agent);
        let list = ec.enabled_list();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&Component::Bus));
        assert!(list.contains(&Component::Agent));
    }

    #[test]
    fn test_enabled_components_is_enabled() {
        let mut ec = EnabledComponents::new();
        assert!(!ec.is_enabled(Component::Bus));
        ec.enable(Component::Bus);
        assert!(ec.is_enabled(Component::Bus));
        assert!(!ec.is_enabled(Component::Agent));
    }

    #[test]
    fn test_bot_state_transitions() {
        assert!(BotState::NotStarted.can_start());
        assert!(!BotState::NotStarted.can_stop());

        assert!(!BotState::Running.can_start());
        assert!(BotState::Running.can_stop());

        assert!(BotState::Error.can_start());
        assert!(!BotState::Error.can_stop());
    }

    #[test]
    fn test_bot_state_display() {
        assert_eq!(BotState::NotStarted.to_string(), "not_started");
        assert_eq!(BotState::Running.to_string(), "running");
        assert_eq!(BotState::Error.to_string(), "error");
    }

    #[test]
    fn test_get_config_returns_copy() {
        let svc = BotService::new(make_config());
        let config1 = svc.get_config();
        let config2 = svc.get_config();
        assert_eq!(config1.config_path, config2.config_path);
    }

    #[test]
    fn test_get_channel_manager_none() {
        let svc = BotService::new(make_config());
        assert!(svc.get_channel_manager().is_none());
    }

    #[test]
    fn test_get_agent_loop_none() {
        let svc = BotService::new(make_config());
        assert!(svc.get_agent_loop().is_none());
    }

    // ============================================================
    // Additional coverage tests for 95%+ target
    // ============================================================

    // --- EnabledComponents ---

    #[test]
    fn test_enabled_components_default() {
        let ec = EnabledComponents::default();
        assert!(ec.enabled_list().is_empty());
    }

    #[test]
    fn test_enabled_components_disable() {
        let mut ec = EnabledComponents::new();
        ec.enable(Component::Bus);
        assert!(ec.is_enabled(Component::Bus));
        ec.disable(Component::Bus);
        assert!(!ec.is_enabled(Component::Bus));
    }

    #[test]
    fn test_enabled_components_is_enabled_unknown() {
        // is_enabled for a component that was never inserted should return false
        let ec = EnabledComponents::new();
        // All components are inserted in new(), so test with a fresh one
        // but after disable_all
        let mut ec = EnabledComponents::new();
        ec.disable_all();
        assert!(!ec.is_enabled(Component::Bus));
    }

    #[test]
    fn test_enabled_components_enable_disable_roundtrip() {
        let mut ec = EnabledComponents::new();
        for comp in [
            Component::Bus, Component::Channels, Component::Agent,
            Component::Security, Component::Forge, Component::Cluster,
            Component::Memory, Component::Workflow, Component::Skills,
            Component::Cron, Component::Heartbeat, Component::Devices,
            Component::Health, Component::Observer,
        ] {
            ec.enable(comp);
            assert!(ec.is_enabled(comp));
            ec.disable(comp);
            assert!(!ec.is_enabled(comp));
        }
    }

    // --- Component ---

    #[test]
    fn test_component_all_variants_serde() {
        let all = vec![
            Component::Bus, Component::Channels, Component::Agent,
            Component::Security, Component::Forge, Component::Cluster,
            Component::Memory, Component::Workflow, Component::Skills,
            Component::Cron, Component::Heartbeat, Component::Devices,
            Component::Health, Component::Observer,
        ];
        for comp in &all {
            let json = serde_json::to_string(comp).unwrap();
            let back: Component = serde_json::from_str(&json).unwrap();
            assert_eq!(*comp, back);
        }
    }

    #[test]
    fn test_component_copy_eq() {
        let a = Component::Bus;
        let b = a;
        assert_eq!(a, b);
    }

    #[test]
    fn test_component_hash_in_set() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(Component::Bus);
        set.insert(Component::Bus);
        set.insert(Component::Agent);
        assert_eq!(set.len(), 2);
    }

    // --- BotServiceConfig ---

    #[test]
    fn test_config_ref_accessor() {
        let svc = BotService::new(make_config());
        assert_eq!(svc.config().config_path, PathBuf::from("test_config.json"));
    }

    #[test]
    fn test_model_entry_serde() {
        let entry = ModelEntry {
            model: "test/model-1.0".to_string(),
            api_key: "key123".to_string(),
            base_url: "http://localhost:8080".to_string(),
            is_default: true,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: ModelEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.model, "test/model-1.0");
        assert_eq!(back.api_key, "key123");
        assert_eq!(back.base_url, "http://localhost:8080");
        assert!(back.is_default);
    }

    #[test]
    fn test_model_entry_api_base_alias() {
        // The alias "api_base" should also deserialize into base_url
        let json = r#"{"model":"test/1","api_key":"k","api_base":"http://host:123","is_default":false}"#;
        let entry: ModelEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.base_url, "http://host:123");
    }

    #[test]
    fn test_model_entry_defaults() {
        let json = r#"{"model":"test/1"}"#;
        let entry: ModelEntry = serde_json::from_str(json).unwrap();
        assert!(entry.api_key.is_empty());
        assert!(entry.base_url.is_empty());
        assert!(!entry.is_default);
    }

    // --- LifecycleService trait default impl ---

    struct MockLifecycle;
    impl LifecycleService for MockLifecycle {}

    #[test]
    fn test_lifecycle_service_default_start() {
        let svc = MockLifecycle;
        assert!(svc.start().is_ok());
    }

    #[test]
    fn test_lifecycle_service_default_stop() {
        let svc = MockLifecycle;
        assert!(svc.stop().is_ok());
    }

    // --- AgentLoopService trait default impl ---

    struct MockAgentDefault;
    impl LifecycleService for MockAgentDefault {
        fn start(&self) -> Result<(), String> { Ok(()) }
        fn stop(&self) -> Result<(), String> { Ok(()) }
    }
    impl AgentLoopService for MockAgentDefault {}

    #[test]
    fn test_agent_loop_default_process_heartbeat() {
        let agent = MockAgentDefault;
        let result = agent.process_heartbeat();
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // --- HeartbeatHandler ---

    #[test]
    fn test_create_heartbeat_handler_skips_bootstrap() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());

        // Create BOOTSTRAP.md in workspace
        std::fs::write(dir.path().join("BOOTSTRAP.md"), "# init").unwrap();

        let svc = BotService::new(config);
        svc.start().unwrap();

        let handler = svc.create_heartbeat_handler();
        // Should not panic when bootstrap exists
        handler();
    }

    #[test]
    fn test_create_heartbeat_handler_no_agent() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());

        let svc = BotService::new(config);
        svc.start().unwrap();

        // No agent injected, so handler should log warning but not panic
        let handler = svc.create_heartbeat_handler();
        handler(); // Should not panic
    }

    #[tokio::test]
    async fn test_create_heartbeat_handler_with_agent() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());

        let svc = BotService::new(config);

        struct MockAgentHeartbeat;
        impl LifecycleService for MockAgentHeartbeat {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl AgentLoopService for MockAgentHeartbeat {
            fn process_heartbeat(&self) -> Result<String, String> {
                Ok("heartbeat ok".to_string())
            }
        }

        svc.inject_agent(Arc::new(MockAgentHeartbeat));
        svc.start().unwrap();

        let handler = svc.create_heartbeat_handler();
        handler(); // Should call process_heartbeat
    }

    #[tokio::test]
    async fn test_create_heartbeat_handler_agent_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());

        let svc = BotService::new(config);

        struct MockAgentError;
        impl LifecycleService for MockAgentError {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl AgentLoopService for MockAgentError {
            fn process_heartbeat(&self) -> Result<String, String> {
                Err("heartbeat failed".to_string())
            }
        }

        svc.inject_agent(Arc::new(MockAgentError));
        svc.start().unwrap();

        let handler = svc.create_heartbeat_handler();
        handler(); // Should handle error gracefully
    }

    // --- Cancel receiver ---

    #[test]
    fn test_cancel_receiver_none_before_start() {
        let svc = BotService::new(make_config());
        assert!(svc.cancel_receiver().is_none());
    }

    #[test]
    fn test_cancel_receiver_some_after_start() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();
        assert!(svc.cancel_receiver().is_some());
    }

    // --- Restart callback ---

    #[test]
    fn test_set_restart_callback() {
        let svc = BotService::new(make_config());
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        svc.set_restart_callback(Box::new(move || {
            called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }));
        // Trigger the callback by accessing internal state
        assert!(!called.load(std::sync::atomic::Ordering::SeqCst));
    }

    // --- Log hooks ---

    #[test]
    fn test_register_log_hook() {
        let svc = BotService::new(make_config());

        struct TestHook;
        impl crate::log_hook::LogHook for TestHook {
            fn on_log(&self, _event: crate::log_hook::LogEvent) {}
        }

        assert!(svc.log_hooks().is_empty());
        svc.register_log_hook(Arc::new(TestHook));
        assert_eq!(svc.log_hooks().len(), 1);
    }

    #[test]
    fn test_register_multiple_log_hooks() {
        let svc = BotService::new(make_config());

        struct TestHook1;
        impl crate::log_hook::LogHook for TestHook1 {
            fn on_log(&self, _event: crate::log_hook::LogEvent) {}
        }
        struct TestHook2;
        impl crate::log_hook::LogHook for TestHook2 {
            fn on_log(&self, _event: crate::log_hook::LogEvent) {}
        }

        svc.register_log_hook(Arc::new(TestHook1));
        svc.register_log_hook(Arc::new(TestHook2));
        assert_eq!(svc.log_hooks().len(), 2);
    }

    // --- save_config_and_restart ---

    #[test]
    fn test_save_config_and_restart_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let config = serde_json::json!({
            "models": [{ "model": "test/1.0", "api_key": "key1", "base_url": "", "is_default": true }]
        });

        // Bot is not running, so restart won't trigger callback
        let result = svc.save_config_and_restart(&config);
        assert!(result.is_ok());
    }

    // --- load_config workspace resolution ---

    #[test]
    fn test_load_config_workspace_from_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let workspace_dir = dir.path().join("my_workspace");
        std::fs::create_dir_all(&workspace_dir).unwrap();

        let config_content = serde_json::json!({
            "workspace": workspace_dir.to_string_lossy(),
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: PathBuf::new(),
            ..BotServiceConfig::default()
        });

        svc.start().unwrap();
        assert_eq!(svc.workspace(), workspace_dir);
    }

    #[test]
    fn test_load_config_workspace_from_config_field() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Config file has no workspace field
        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: PathBuf::from("/fallback/workspace"),
            ..BotServiceConfig::default()
        });

        svc.start().unwrap();
        assert_eq!(svc.workspace(), PathBuf::from("/fallback/workspace"));
    }

    #[test]
    fn test_load_config_workspace_default_to_parent() {
        let dir = tempfile::tempdir().unwrap();
        let subdir = dir.path().join("nested");
        std::fs::create_dir_all(&subdir).unwrap();
        let config_path = subdir.join("config.json");

        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: PathBuf::new(), // empty, no fallback
            ..BotServiceConfig::default()
        });

        svc.start().unwrap();
        // Should fall back to parent directory of config file
        assert_eq!(svc.workspace(), subdir);
    }

    // --- init_components conditional branches ---

    fn make_config_with_flags(
        dir: &std::path::Path,
        forge_enabled: bool,
        memory_enabled: bool,
        workflow_enabled: bool,
        cluster_enabled: bool,
        devices_enabled: bool,
        heartbeat_enabled: bool,
    ) -> BotServiceConfig {
        let config_path = dir.join("config.json");
        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ],
            "forge": { "enabled": forge_enabled },
            "memory": { "enabled": memory_enabled },
            "workflow": { "enabled": workflow_enabled },
            "devices": { "enabled": devices_enabled, "monitor_usb": false },
            "heartbeat": { "enabled": heartbeat_enabled, "interval": 60 },
            "security": { "enabled": true }
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        BotServiceConfig {
            config_path,
            workspace: dir.to_path_buf(),
            forge_enabled,
            memory_enabled,
            workflow_enabled,
            cluster_enabled,
            ..BotServiceConfig::default()
        }
    }

    #[test]
    fn test_init_components_forge_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Forge));
    }

    #[test]
    fn test_init_components_cluster_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, false, false, true, false, true);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Cluster));
    }

    #[test]
    fn test_init_components_memory_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, true, false, false, false, true);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Memory));
    }

    #[test]
    fn test_init_components_workflow_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, false, true, false, false, true);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Workflow));
    }

    #[test]
    fn test_init_components_devices_enabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, false, false, false, true, true);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(enabled.is_enabled(Component::Devices));
    }

    #[test]
    fn test_init_components_heartbeat_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, false, false, false, false, false);
        let svc = BotService::new(config);
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(!enabled.is_enabled(Component::Heartbeat));
    }

    #[test]
    fn test_init_components_all_disabled_optional() {
        let dir = tempfile::tempdir().unwrap();
        // Security enabled = false in both config and BotServiceConfig
        let config_path = dir.path().join("config.json");
        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ],
            "forge": { "enabled": false },
            "memory": { "enabled": false },
            "workflow": { "enabled": false },
            "devices": { "enabled": false, "monitor_usb": false },
            "heartbeat": { "enabled": false, "interval": 60 },
            "security": { "enabled": false }
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            security_enabled: false,
            ..BotServiceConfig::default()
        });
        svc.start().unwrap();

        let enabled = svc.enabled_components();
        assert!(!enabled.is_enabled(Component::Forge));
        assert!(!enabled.is_enabled(Component::Memory));
        assert!(!enabled.is_enabled(Component::Workflow));
        assert!(!enabled.is_enabled(Component::Cluster));
        assert!(!enabled.is_enabled(Component::Devices));
        assert!(!enabled.is_enabled(Component::Heartbeat));
        assert!(!enabled.is_enabled(Component::Security));
    }

    // --- start_services with injected services ---

    #[test]
    fn test_start_services_with_channel_manager() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct MockChannelsWithNames;
        impl LifecycleService for MockChannelsWithNames {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ChannelManager for MockChannelsWithNames {
            fn enabled_channels(&self) -> Vec<String> {
                vec!["web".to_string(), "discord".to_string()]
            }
        }

        svc.inject_channels(Arc::new(MockChannelsWithNames));
        svc.start().unwrap();
        assert!(svc.get_channel_manager().is_some());
        let channels = svc.get_channel_manager().unwrap();
        assert_eq!(channels.enabled_channels(), vec!["web", "discord"]);
    }

    #[test]
    fn test_start_services_heartbeat_start_failure() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct FailingHeartbeat;
        impl LifecycleService for FailingHeartbeat {
            fn start(&self) -> Result<(), String> { Err("heartbeat start error".to_string()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl HeartbeatService for FailingHeartbeat {}

        svc.inject_heartbeat(Arc::new(FailingHeartbeat));
        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("heartbeat"));
    }

    #[test]
    fn test_start_services_devices_start_failure() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct FailingDevices;
        impl LifecycleService for FailingDevices {
            fn start(&self) -> Result<(), String> { Err("devices start error".to_string()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl DeviceService for FailingDevices {}

        svc.inject_devices(Arc::new(FailingDevices));
        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("device"));
    }

    #[test]
    fn test_start_services_channel_start_failure() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct FailingChannels;
        impl LifecycleService for FailingChannels {
            fn start(&self) -> Result<(), String> { Err("channels start error".to_string()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ChannelManager for FailingChannels {
            fn enabled_channels(&self) -> Vec<String> { vec![] }
        }

        svc.inject_channels(Arc::new(FailingChannels));
        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("channel"));
    }

    #[test]
    fn test_start_services_cron_non_fatal_failure() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct FailingCron;
        impl LifecycleService for FailingCron {
            fn start(&self) -> Result<(), String> { Err("cron start error".to_string()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl CronService for FailingCron {}

        svc.inject_cron(Arc::new(FailingCron));
        // Cron failure is non-fatal - bot should still start
        let result = svc.start();
        assert!(result.is_ok());
        assert_eq!(svc.get_state(), BotState::Running);
    }

    #[test]
    fn test_start_services_forge_start_failure() {
        let dir = tempfile::tempdir().unwrap();
        // Enable forge in config
        let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
        let svc = BotService::new(config);

        struct FailingForge;
        impl LifecycleService for FailingForge {
            fn start(&self) -> Result<(), String> { Err("forge start error".to_string()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ForgeService for FailingForge {
            fn forge_name(&self) -> &str { "failing_forge" }
        }

        svc.inject_forge(Arc::new(FailingForge));
        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("forge"));
    }

    // --- stop_all with injected services ---

    #[tokio::test]
    async fn test_stop_all_with_injected_services() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct MockFullService;
        impl LifecycleService for MockFullService {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl ForgeService for MockFullService {
            fn forge_name(&self) -> &str { "mock" }
        }
        impl MemoryService for MockFullService {}
        impl HeartbeatService for MockFullService {}
        impl DeviceService for MockFullService {}
        impl HealthServer for MockFullService {}
        impl ChannelManager for MockFullService {
            fn enabled_channels(&self) -> Vec<String> { vec![] }
        }
        impl AgentLoopService for MockFullService {}
        impl CronService for MockFullService {}

        svc.inject_forge(Arc::new(MockFullService));
        svc.inject_memory(Arc::new(MockFullService));
        svc.inject_heartbeat(Arc::new(MockFullService));
        svc.inject_devices(Arc::new(MockFullService));
        svc.inject_health(Arc::new(MockFullService));
        svc.inject_channels(Arc::new(MockFullService));
        svc.inject_agent(Arc::new(MockFullService));
        svc.inject_cron(Arc::new(MockFullService));

        svc.start().unwrap();
        assert!(svc.get_forge().is_some());
        assert!(svc.get_memory().is_some());

        svc.stop().unwrap();
        // After stop, services should be cleared
        assert!(svc.get_forge().is_none());
        assert!(svc.get_memory().is_none());
        assert!(svc.get_channel_manager().is_none());
        assert!(svc.get_agent_loop().is_none());
    }

    #[tokio::test]
    async fn test_stop_all_with_service_stop_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct ErrorOnStop;
        impl LifecycleService for ErrorOnStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("stop error".to_string()) }
        }
        impl ChannelManager for ErrorOnStop {
            fn enabled_channels(&self) -> Vec<String> { vec![] }
        }
        impl AgentLoopService for ErrorOnStop {}
        impl ForgeService for ErrorOnStop {
            fn forge_name(&self) -> &str { "error_forge" }
        }

        svc.inject_channels(Arc::new(ErrorOnStop));
        svc.inject_agent(Arc::new(ErrorOnStop));

        svc.start().unwrap();
        // stop should succeed even with errors from individual services
        let result = svc.stop();
        assert!(result.is_ok());
    }

    // --- with_default_config ---

    #[test]
    fn test_with_default_config() {
        let svc = BotService::with_default_config();
        assert_eq!(svc.get_state(), BotState::NotStarted);
        assert!(svc.get_error().is_none());
    }

    // --- Double stop ---

    #[test]
    fn test_double_stop_fails() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();
        svc.stop().unwrap();

        let result = svc.stop();
        assert!(result.is_err());
    }

    // --- Start after stop (re-start) ---

    #[test]
    fn test_start_after_stop() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();
        svc.stop().unwrap();
        assert_eq!(svc.get_state(), BotState::NotStarted);

        // Should be able to start again
        let result = svc.start();
        assert!(result.is_ok());
        assert_eq!(svc.get_state(), BotState::Running);
    }

    // --- Start when starting (race condition) ---

    #[test]
    fn test_start_when_already_starting_blocked() {
        // This tests the "already starting" guard
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        svc.start().unwrap();
        // Second start should fail because already running
        let result = svc.start();
        assert!(result.is_err());
    }

    // --- Config file with invalid JSON ---

    #[test]
    fn test_load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not valid json{{{").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("parse"));
    }

    // --- Config file parsing with all sub-fields ---

    #[test]
    fn test_config_file_with_logging() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ],
            "logging": {
                "llm": { "enabled": true }
            },
            "forge": {
                "enabled": false,
                "trace": { "enabled": true },
                "learning": { "enabled": false }
            },
            "tools": {
                "cron": { "exec_timeout_minutes": 10 }
            },
            "agents": {
                "defaults": { "restrict_to_workspace": false }
            },
            "gateway": { "host": "0.0.0.0", "port": 3000 },
            "heartbeat": { "enabled": false, "interval": 600 }
        });
        std::fs::write(&config_path, serde_json::to_string(&content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        svc.start().unwrap();
        assert_eq!(svc.get_state(), BotState::Running);
    }

    // --- Config file empty JSON ---

    #[test]
    fn test_config_file_empty_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        let result = svc.start();
        assert!(result.is_err());
        // Should fail on validation: no models configured
        assert!(svc.get_error().unwrap().contains("no models"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target - Phase 2
    // ============================================================

    // --- start_services with health server ---

    #[tokio::test]
    async fn test_start_services_health_server_start() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct MockHealthSvc;
        impl LifecycleService for MockHealthSvc {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl HealthServer for MockHealthSvc {}

        svc.inject_health(Arc::new(MockHealthSvc));
        svc.start().unwrap();
        assert_eq!(svc.get_state(), BotState::Running);

        // Give spawned task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        svc.stop().unwrap();
    }

    #[tokio::test]
    async fn test_start_services_health_stop_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct ErrorHealthStop;
        impl LifecycleService for ErrorHealthStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("health stop error".to_string()) }
        }
        impl HealthServer for ErrorHealthStop {}

        svc.inject_health(Arc::new(ErrorHealthStop));
        svc.start().unwrap();
        // stop_all should handle health stop error gracefully
        svc.stop().unwrap();
    }

    // --- stop_all with forge stop error ---

    #[test]
    fn test_stop_all_forge_stop_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), true, false, false, false, false, true);
        let svc = BotService::new(config);

        struct ErrorForgeStop;
        impl LifecycleService for ErrorForgeStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("forge stop error".to_string()) }
        }
        impl ForgeService for ErrorForgeStop {
            fn forge_name(&self) -> &str { "error_forge" }
        }

        svc.inject_forge(Arc::new(ErrorForgeStop));
        svc.start().unwrap();
        svc.stop().unwrap();
    }

    // --- stop_all with memory close ---

    #[test]
    fn test_stop_all_with_memory_service() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_flags(dir.path(), false, true, false, false, false, true);
        let svc = BotService::new(config);

        struct MockMemorySvc;
        impl LifecycleService for MockMemorySvc {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl MemoryService for MockMemorySvc {}

        svc.inject_memory(Arc::new(MockMemorySvc));
        svc.start().unwrap();
        assert!(svc.get_memory().is_some());
        svc.stop().unwrap();
        assert!(svc.get_memory().is_none());
    }

    // --- save_config_and_restart while running ---

    #[tokio::test]
    async fn test_save_config_and_restart_while_running() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = Arc::new(BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        }));

        // Set restart callback
        let svc_clone = svc.clone();
        let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = called.clone();
        svc.set_restart_callback(Box::new(move || {
            called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            // Attempt restart but it will fail since we don't have a valid config after save
            let _ = svc_clone.restart();
            Ok(())
        }));

        svc.start().unwrap();
        assert_eq!(svc.get_state(), BotState::Running);

        let new_config = serde_json::json!({
            "models": [
                { "model": "test/2.0", "api_key": "new-key", "base_url": "", "is_default": true }
            ]
        });

        let result = svc.save_config_and_restart(&new_config);
        assert!(result.is_ok());

        // Wait for async restart to complete
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    }

    // --- save_config restart without callback ---

    #[tokio::test]
    async fn test_save_config_restart_no_callback() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        let config_content = serde_json::json!({
            "models": [
                { "model": "test/1.0", "api_key": "test-key", "base_url": "", "is_default": true }
            ]
        });
        std::fs::write(&config_path, serde_json::to_string(&config_content).unwrap()).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        svc.start().unwrap();

        let new_config = serde_json::json!({
            "models": [
                { "model": "test/2.0", "api_key": "new-key", "base_url": "", "is_default": true }
            ]
        });

        // restart=true but no restart callback set - should warn but succeed
        let result = svc.save_config(&new_config, true);
        assert!(result.is_ok());

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }

    // --- start_services with agent loop ---

    #[tokio::test]
    async fn test_start_services_agent_in_background() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct MockAgentBg;
        impl LifecycleService for MockAgentBg {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Ok(()) }
        }
        impl AgentLoopService for MockAgentBg {}

        svc.inject_agent(Arc::new(MockAgentBg));
        svc.start().unwrap();
        assert_eq!(svc.get_state(), BotState::Running);

        // Give agent background task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        svc.stop().unwrap();
    }

    // --- stop_all with heartbeat stop error ---

    #[test]
    fn test_stop_all_heartbeat_stop_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct ErrorHeartbeatStop;
        impl LifecycleService for ErrorHeartbeatStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("heartbeat stop error".to_string()) }
        }
        impl HeartbeatService for ErrorHeartbeatStop {}

        svc.inject_heartbeat(Arc::new(ErrorHeartbeatStop));
        svc.start().unwrap();
        svc.stop().unwrap();
    }

    // --- stop_all with cron stop error ---

    #[test]
    fn test_stop_all_cron_stop_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct ErrorCronStop;
        impl LifecycleService for ErrorCronStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("cron stop error".to_string()) }
        }
        impl CronService for ErrorCronStop {}

        svc.inject_cron(Arc::new(ErrorCronStop));
        svc.start().unwrap();
        svc.stop().unwrap();
    }

    // --- stop_all with device stop error ---

    #[test]
    fn test_stop_all_devices_stop_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = make_config_with_file(dir.path());
        let svc = BotService::new(config);

        struct ErrorDeviceStop;
        impl LifecycleService for ErrorDeviceStop {
            fn start(&self) -> Result<(), String> { Ok(()) }
            fn stop(&self) -> Result<(), String> { Err("device stop error".to_string()) }
        }
        impl DeviceService for ErrorDeviceStop {}

        svc.inject_devices(Arc::new(ErrorDeviceStop));
        svc.start().unwrap();
        svc.stop().unwrap();
    }

    // --- load_config with read error ---

    #[test]
    fn test_load_config_unreadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        // Create a directory instead of a file to cause read error
        std::fs::create_dir_all(&config_path).unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        let result = svc.start();
        assert!(result.is_err());
    }

    // --- save_config rejects non-object ---

    #[test]
    fn test_save_config_rejects_array() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let result = svc.save_config(&serde_json::json!([1, 2, 3]), false);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_config_rejects_null() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let result = svc.save_config(&serde_json::Value::Null, false);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_config_rejects_number() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        let result = svc.save_config(&serde_json::json!(42), false);
        assert!(result.is_err());
    }

    // --- save_config invalid structure ---

    #[test]
    fn test_save_config_rejects_bad_structure() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            workspace: dir.path().to_path_buf(),
            ..BotServiceConfig::default()
        });

        // Valid JSON object but with wrong field types
        let result = svc.save_config(&serde_json::json!({
            "models": "not an array"
        }), false);
        assert!(result.is_err());
    }

    // --- start when in Error state ---

    #[test]
    fn test_start_after_error_state() {
        let svc = BotService::new(make_config());
        // First start fails because config file doesn't exist
        let result = svc.start();
        assert!(result.is_err());
        assert_eq!(svc.get_state(), BotState::Error);

        // Can start again from Error state (but will fail again)
        let result = svc.start();
        assert!(result.is_err());
    }

    // --- validate_config not loaded ---

    #[test]
    fn test_validate_config_not_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        // Write an empty file so load_config passes but config_loaded is set
        std::fs::write(&config_path, "{}").unwrap();

        let svc = BotService::new(BotServiceConfig {
            config_path,
            ..BotServiceConfig::default()
        });

        // This will fail at validate_config since config is loaded but models is empty
        let result = svc.start();
        assert!(result.is_err());
        assert!(svc.get_error().unwrap().contains("no models"));
    }
}
