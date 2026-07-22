//! Checkpoint module — durable snapshots of in-flight workflow executions.
//!
//! Built in milestone 1b-A1. Stores enough state to resume an execution
//! after gateway restart or human-review pause.
//!
//! Layout:
//! - [`types`]: serialisable structs (`Checkpoint`, `SerializableContext`, …)
//! - [`store`]: `CheckpointStore` trait + `InMemoryCheckpointStore`
//! - [`file_store`]: `FileCheckpointStore` (JSON files under
//!   `{home}/workspace/workflow/checkpoints/`)

pub mod file_store;
pub mod store;
pub mod types;

pub use file_store::FileCheckpointStore;
pub use store::{CheckpointStore, InMemoryCheckpointStore, StoreError};
pub use types::{
    Checkpoint, CheckpointMeta, SerializableContext, SerializableNodeResult, parse_state,
};
