//! NemesisBot - Workflow Engine
//!
//! DAG-based workflow engine with node types, topological execution,
//! condition evaluation, trigger management, and JSONL persistence.

pub mod types;
pub mod engine;
pub mod events;
pub mod nodes;
pub mod persistence;
pub mod scheduler;
pub mod triggers;
pub mod parser;
pub mod context;
pub mod conditions;
pub mod checkpoint;
pub mod call_stack;
pub mod driver_status;
pub mod event_dispatcher;
pub mod workflow_chat_state;
pub mod chat_secrets;

pub use triggers::{CronTimezone, TriggerManager, TriggerConfig};
pub use context::WorkflowContext;
pub use events::{WorkflowEvent, WorkflowEventManager, WorkflowObserver};
pub use types::MAX_RECURSION_DEPTH;
pub use call_stack::{CallFrame, WorkflowCallStack};
pub use driver_status::{
    all_driver_statuses, all_known_trigger_types, driver_status_for, TriggerDriverStatus,
};
pub use event_dispatcher::{EventDispatcher, TriggerEvent};
