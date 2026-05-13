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
pub mod service_manager;
pub mod state;
pub mod helpers;
pub mod parallel;
pub mod log_hook;

pub use bot_service::{
    BotService, BotServiceConfig, Component, EnabledComponents, ModelEntry,
    // Lifecycle traits for dependency injection
    LifecycleService, ServiceHandle,
    ForgeService, MemoryService, HeartbeatService, DeviceService,
    HealthServer, ChannelManager, AgentLoopService, CronService,
    SecurityService, WorkflowService, SkillsService, ObserverManager,
};
pub use service_manager::ServiceManager;
pub use state::BotState;
pub use helpers::{get_config_path, should_skip_heartbeat_for_bootstrap};
pub use parallel::{parallel_init, parallel_init_blocking, sequential_init, BoundedParallelInit};
pub use log_hook::{LogHook, LogHookChain, LogHookHandle, LogEvent, NoopLogHook};
