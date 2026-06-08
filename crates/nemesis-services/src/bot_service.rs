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
use tracing::{debug, info, warn, error};

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

    /// Check whether the service is currently running.
    fn is_running(&self) -> bool {
        false
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
        info!("[BotService] BotService created (stopped)");
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

        info!("[BotService] Starting bot service...");

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
        debug!("[BotService] Starting component: {}", "load_config");
        if let Err(e) = self.load_config() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "failed to load config: {}",
                e
            )));
        }

        // Phase 2: Validate configuration
        debug!("[BotService] Starting component: {}", "validate_config");
        if let Err(e) = self.validate_config() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "config validation failed: {}",
                e
            )));
        }

        // Phase 3: Initialize components
        debug!("[BotService] Starting component: {}", "init_components");
        if let Err(e) = self.init_components() {
            self.set_state_with_error(BotState::Error, &e);
            return Err(nemesis_types::error::NemesisError::Other(format!(
                "failed to initialize components: {}",
                e
            )));
        }

        // Phase 4: Start services
        debug!("[BotService] Starting component: {}", "start_services");
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
        info!("[BotService] Bot service started successfully");
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

        info!("[BotService] Stopping bot service...");

        // Cancel context
        if let Some(tx) = self.cancel_tx.lock().as_ref() {
            let _ = tx.send(());
        }

        self.stop_all();

        {
            let mut inner = self.inner.write();
            inner.state = BotState::NotStarted;
        }
        info!("[BotService] Bot service stopped");
        Ok(())
    }

    /// Restart stops and then starts the bot service.
    pub fn restart(&self) -> nemesis_types::error::Result<()> {
        info!("[BotService] Restarting bot service...");

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

        info!("[BotService] Bot service restarted successfully");
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
                info!("[BotService] Skipping heartbeat: BOOTSTRAP.md exists (initialization in progress)");
                return;
            }

            // Process heartbeat through agent loop.
            if let Some(ref agent) = agent_loop {
                match agent.process_heartbeat() {
                    Ok(response) => {
                        if !response.is_empty() {
                            info!(response_len = response.len(), "[BotService] Heartbeat processed");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "[BotService] Heartbeat processing failed");
                    }
                }
            } else {
                warn!("[BotService] Heartbeat skipped: agent loop not initialized");
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
        debug!("[BotService] Injecting {} service", "forge");
        self.services.write().forge = Some(forge);
    }

    /// Inject a Memory service instance.
    pub fn inject_memory(&self, memory: Arc<dyn MemoryService>) {
        debug!("[BotService] Injecting {} service", "memory");
        self.services.write().memory = Some(memory);
    }

    /// Inject a Heartbeat service instance.
    pub fn inject_heartbeat(&self, heartbeat: Arc<dyn HeartbeatService>) {
        debug!("[BotService] Injecting {} service", "heartbeat");
        self.services.write().heartbeat = Some(heartbeat);
    }

    /// Inject a Device service instance.
    pub fn inject_devices(&self, devices: Arc<dyn DeviceService>) {
        debug!("[BotService] Injecting {} service", "devices");
        self.services.write().devices = Some(devices);
    }

    /// Inject a Health server instance.
    pub fn inject_health(&self, health: Arc<dyn HealthServer>) {
        debug!("[BotService] Injecting {} service", "health_server");
        self.services.write().health = Some(health);
    }

    /// Inject a Channel manager instance.
    pub fn inject_channels(&self, channels: Arc<dyn ChannelManager>) {
        debug!("[BotService] Injecting {} service", "channels");
        self.services.write().channels = Some(channels);
    }

    /// Inject an Agent loop instance.
    pub fn inject_agent(&self, agent: Arc<dyn AgentLoopService>) {
        debug!("[BotService] Injecting {} service", "agent");
        self.services.write().agent = Some(agent);
    }

    /// Inject a Cron service instance.
    pub fn inject_cron(&self, cron: Arc<dyn CronService>) {
        debug!("[BotService] Injecting {} service", "cron");
        self.services.write().cron = Some(cron);
    }

    /// Inject a Security service instance.
    pub fn inject_security(&self, security: Arc<dyn SecurityService>) {
        debug!("[BotService] Injecting {} service", "security");
        self.services.write().security = Some(security);
    }

    /// Inject a Workflow service instance.
    pub fn inject_workflow(&self, workflow: Arc<dyn WorkflowService>) {
        debug!("[BotService] Injecting {} service", "workflow");
        self.services.write().workflow = Some(workflow);
    }

    /// Inject a Skills service instance.
    pub fn inject_skills(&self, skills: Arc<dyn SkillsService>) {
        debug!("[BotService] Injecting {} service", "skills");
        self.services.write().skills = Some(skills);
    }

    /// Inject an Observer manager instance.
    pub fn inject_observer(&self, observer: Arc<dyn ObserverManager>) {
        debug!("[BotService] Injecting {} service", "observer");
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
        info!("[BotService] Log hook registered (total: {})", self.log_hooks.len());
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
        info!("[BotService] Saving configuration to {:?}", self.config.config_path);

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

        info!("[BotService] Configuration saved successfully");

        // Update the cached config file
        {
            let mut inner = self.inner.write();
            inner.config_file = Some(config_data);
            inner.config_loaded = true;
        }

        // Trigger restart if requested and bot is running
        if restart && self.get_state().is_running() {
            info!("[BotService] Restart scheduled after config save");
            let restart_cb = self.restart_callback.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                let cb = restart_cb.lock();
                if let Some(ref callback) = *cb {
                    match callback() {
                        Ok(()) => info!("[BotService] Post-save restart completed"),
                        Err(e) => error!("[BotService] Post-save restart failed: {}", e),
                    }
                } else {
                    warn!("[BotService] No restart callback set; skipping post-save restart");
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
            "[BotService] Loading config from {:?}",
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
            "[BotService] Config loaded, workspace={:?}",
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

        info!("[BotService] Configuration validated");
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
        info!("[BotService] Component initialized: {}", Component::Bus.label());

        enabled.enable(Component::Agent);
        info!("[BotService] Component initialized: {}", Component::Agent.label());

        enabled.enable(Component::Channels);
        info!("[BotService] Component initialized: {}", Component::Channels.label());

        // --- Phase 2: Parallel independent service creation ---
        // In Go, these run in parallel using errgroup. In Rust, the application
        // layer creates them using the parallel_init utilities. Here we just
        // track which ones should be enabled.

        // Cron service (always enabled)
        enabled.enable(Component::Cron);
        info!("[BotService] Component initialized: {}", Component::Cron.label());

        // Heartbeat service (check config)
        if config_file_data.heartbeat.enabled {
            enabled.enable(Component::Heartbeat);
            info!("[BotService] Component initialized: {}", Component::Heartbeat.label());
        }

        // Health server (always enabled)
        enabled.enable(Component::Health);
        info!("[BotService] Component initialized: {}", Component::Health.label());

        // Devices (check config)
        if config_file_data.devices.enabled {
            enabled.enable(Component::Devices);
            info!("[BotService] Component initialized: {}", Component::Devices.label());
        }

        // Skills (always enabled)
        enabled.enable(Component::Skills);
        info!("[BotService] Component initialized: {}", Component::Skills.label());

        // --- Phase 3: Wire up service dependencies (sequential) ---
        // These steps depend on the services created in Phase 2.
        // The actual wiring happens at the application layer.

        // Security (config-driven)
        if self.config.security_enabled || config_file_data.security.enabled {
            enabled.enable(Component::Security);
            info!("[BotService] Component initialized: {}", Component::Security.label());
        }

        // Workflow (config-driven)
        if self.config.workflow_enabled || config_file_data.workflow.enabled {
            enabled.enable(Component::Workflow);
            info!("[BotService] Component initialized: {}", Component::Workflow.label());
        }

        // --- Phase 4: Forge and Observer setup ---
        // These depend on agentLoop + provider being available.

        // Forge self-learning module
        if self.config.forge_enabled || config_file_data.forge.enabled {
            enabled.enable(Component::Forge);
            info!("[BotService] Component initialized: {}", Component::Forge.label());
        }

        // Cluster (config-driven)
        if self.config.cluster_enabled {
            enabled.enable(Component::Cluster);
            info!("[BotService] Component initialized: {}", Component::Cluster.label());
        }

        // Memory subsystem (config-driven)
        if self.config.memory_enabled || config_file_data.memory.enabled {
            enabled.enable(Component::Memory);
            info!("[BotService] Component initialized: {}", Component::Memory.label());
        }

        // Observer (enabled if any observers will be registered)
        enabled.enable(Component::Observer);
        info!("[BotService] Component initialized: {}", Component::Observer.label());

        info!("[BotService] Components initialized");
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
        info!("[BotService] Starting all services...");
        let services = self.services.read();

        // 1. Start heartbeat service
        if let Some(ref heartbeat) = services.heartbeat {
            heartbeat.start().map_err(|e| {
                format!("failed to start heartbeat service: {}", e)
            })?;
            info!("[BotService] Heartbeat service started");
        }

        // 2. Start device service
        if let Some(ref devices) = services.devices {
            devices.start().map_err(|e| {
                format!("failed to start device service: {}", e)
            })?;
            info!("[BotService] Device service started");
        }

        // 3. Start channel manager
        if let Some(ref channels) = services.channels {
            channels.start().map_err(|e| {
                format!("failed to start channel manager: {}", e)
            })?;

            let enabled_channels = channels.enabled_channels();
            if !enabled_channels.is_empty() {
                info!(
                    "[BotService] Channels enabled: {:?}",
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
                    error!("[BotService] Health server error: {}", e);
                }
            });
            info!(
                "[BotService] Health server started on {}:{}",
                self.config.gateway_host, self.config.gateway_port
            );
        }

        // 5. Start agent loop (in background)
        if let Some(ref agent) = services.agent {
            let agent_clone = agent.clone();
            let cancel_rx = self.cancel_tx.lock().as_ref().map(|tx| tx.subscribe());
            tokio::spawn(async move {
                if let Err(e) = agent_clone.start() {
                    error!("[BotService] Agent loop error: {}", e);
                }
                // Keep running until cancelled
                if let Some(mut rx) = cancel_rx {
                    let _ = rx.recv().await;
                } else {
                    // No cancel channel, run until dropped
                    std::future::pending::<()>().await;
                }
            });
            info!("[BotService] Agent loop started");
        }

        // 6. Start cron service
        if let Some(ref cron) = services.cron {
            match cron.start() {
                Ok(()) => {
                    info!("[BotService] Cron service started");
                }
                Err(e) => {
                    warn!("[BotService] Cron service start failed: {}", e);
                    // Non-fatal: cron failure should not prevent bot from starting
                }
            }
        }

        // 7. Start Forge self-learning module
        if let Some(ref forge) = services.forge {
            forge.start().map_err(|e| {
                format!("failed to start forge service: {}", e)
            })?;
            info!("[BotService] Forge service started");
        }

        info!("[BotService] All services started");
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
        info!("[BotService] Stopping all services (reverse order)...");

        let mut services = self.services.write();

        // Stop in reverse order of start
        // 1. Forge
        if let Some(ref forge) = services.forge {
            if let Err(e) = forge.stop() {
                warn!("[BotService] Error stopping forge service: {}", e);
            } else {
                info!("[BotService] Forge service stopped");
            }
        }

        // 2. Cron
        if let Some(ref cron) = services.cron {
            if let Err(e) = cron.stop() {
                warn!("[BotService] Error stopping cron service: {}", e);
            } else {
                info!("[BotService] Cron service stopped");
            }
        }

        // 3. Health server
        if let Some(ref health) = services.health {
            if let Err(e) = health.stop() {
                warn!("[BotService] Error stopping health server: {}", e);
            } else {
                info!("[BotService] Health server stopped");
            }
        }

        // 4. Device service
        if let Some(ref devices) = services.devices {
            if let Err(e) = devices.stop() {
                warn!("[BotService] Error stopping device service: {}", e);
            } else {
                info!("[BotService] Device service stopped");
            }
        }

        // 5. Heartbeat service
        if let Some(ref heartbeat) = services.heartbeat {
            if let Err(e) = heartbeat.stop() {
                warn!("[BotService] Error stopping heartbeat service: {}", e);
            } else {
                info!("[BotService] Heartbeat service stopped");
            }
        }

        // 6. Channel manager
        if let Some(ref channels) = services.channels {
            if let Err(e) = channels.stop() {
                warn!("[BotService] Error stopping channel manager: {}", e);
            } else {
                info!("[BotService] Channel manager stopped");
            }
        }

        // 7. Agent loop
        if let Some(ref agent) = services.agent {
            if let Err(e) = agent.stop() {
                warn!("[BotService] Error stopping agent loop: {}", e);
            } else {
                info!("[BotService] Agent loop stopped");
            }
        }

        // 8. Memory manager (close, not stop)
        if let Some(ref memory) = services.memory {
            memory.close();
            info!("[BotService] Memory manager closed");
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

        info!("[BotService] All services stopped");
    }

    /// Set state and record error.
    fn set_state_with_error(&self, state: BotState, err: &str) {
        let mut inner = self.inner.write();
        inner.state = state;
        inner.last_error = Some(err.to_string());
        error!(
            "[BotService] Bot service error, state={}, error={}",
            state, err
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
