//! NemesisBot - Workflow Engine
//!
//! DAG-based workflow engine with node types, topological execution,
//! condition evaluation, trigger management, and JSONL persistence.

pub mod call_stack;
pub mod chat_secrets;
pub mod checkpoint;
pub mod conditions;
pub mod context;
pub mod driver_status;
pub mod engine;
pub mod event_dispatcher;
pub mod events;
pub mod nodes;
pub mod parser;
pub mod persistence;
pub mod scheduler;
pub mod triggers;
pub mod types;
pub mod workflow_chat_state;

pub use call_stack::{CallFrame, WorkflowCallStack};
pub use context::WorkflowContext;
pub use driver_status::{
    TriggerDriverStatus, all_driver_statuses, all_known_trigger_types, driver_status_for,
};
pub use event_dispatcher::{EventDispatcher, TriggerEvent};
pub use events::{WorkflowEvent, WorkflowEventManager, WorkflowObserver};
pub use triggers::{CronTimezone, TriggerConfig, TriggerManager};
pub use types::MAX_RECURSION_DEPTH;
