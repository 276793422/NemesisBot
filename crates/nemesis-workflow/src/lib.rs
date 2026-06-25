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

pub use triggers::{TriggerManager, TriggerConfig};
pub use context::WorkflowContext;
pub use events::{WorkflowEvent, WorkflowEventManager, WorkflowObserver};
