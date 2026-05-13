//! Error types for the NemesisBot system.

use thiserror::Error;

/// Top-level error type for NemesisBot operations.
#[derive(Error, Debug)]
pub enum NemesisError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Security violation: {0}")]
    Security(String),

    #[error("Provider error: {0}")]
    Provider(String),

    #[error("Channel error: {0}")]
    Channel(String),

    #[error("Agent error: {0}")]
    Agent(String),

    #[error("Cluster error: {0}")]
    Cluster(String),

    #[error("Memory error: {0}")]
    Memory(String),

    #[error("Tool error: {0}")]
    Tool(String),

    #[error("Workflow error: {0}")]
    Workflow(String),

    #[error("Forge error: {0}")]
    Forge(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Timeout: {0}")]
    Timeout(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, NemesisError>;
