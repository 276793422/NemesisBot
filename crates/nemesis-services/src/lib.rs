//! NemesisBot - Service Management
//!
//! BotService lifecycle, service orchestration, parallel initialization,
//! and graceful shutdown.
//!
//! # Module overview
//!
//! - [`bot_service`] - Main service orchestrator with 4-phase initialization
//! - [`service_manager`] - Lifecycle management with graceful shutdown
//! - [`state`] - BotState state machine
//! - [`helpers`] - Configuration path resolution
//! - [`parallel`] - Parallel initialization utilities (errgroup-style)
//! - [`log_hook`] - LogHook trait and chain for bridging logs to SSE

pub mod bot_service;
pub mod helpers;
pub mod log_hook;
pub mod parallel;
pub mod service_manager;
pub mod state;

pub use bot_service::{
    AgentLoopService,
    BotService,
    BotServiceConfig,
    ChannelManager,
    Component,
    CronService,
    DeviceService,
    EnabledComponents,
    ForgeService,
    HealthServer,
    HeartbeatService,
    // Lifecycle traits for dependency injection
    LifecycleService,
    MemoryService,
    ModelEntry,
    ObserverManager,
    SecurityService,
    ServiceHandle,
    SkillsService,
    WorkflowService,
};
pub use helpers::{get_config_path, should_skip_heartbeat_for_bootstrap};
pub use log_hook::{LogEvent, LogHook, LogHookChain, LogHookHandle, NoopLogHook};
pub use parallel::{BoundedParallelInit, parallel_init, parallel_init_blocking, sequential_init};
pub use service_manager::ServiceManager;
pub use state::BotState;
