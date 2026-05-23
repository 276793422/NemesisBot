//! Failover system for LLM providers.

use thiserror::Error;

/// Reason for provider failover.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FailoverReason {
    Auth,
    RateLimit,
    Billing,
    Timeout,
    Format,
    Overloaded,
    Unknown,
}

use serde::{Deserialize, Serialize};

/// Failover error with context.
#[derive(Debug, Error)]
pub enum FailoverError {
    #[error("auth failure for provider {provider}/{model}: status {status}")]
    Auth {
        provider: String,
        model: String,
        status: u16,
    },
    #[error("rate limited by provider {provider}/{model}")]
    RateLimit {
        provider: String,
        model: String,
        retry_after: Option<u64>,
    },
    #[error("billing issue with provider {provider}")]
    Billing { provider: String },
    #[error("timeout calling provider {provider}/{model}")]
    Timeout { provider: String, model: String },
    #[error("format error from provider {provider}: {message}")]
    Format {
        provider: String,
        message: String,
    },
    #[error("provider {provider} is overloaded")]
    Overloaded { provider: String },
    #[error("unknown error from provider {provider}: {message}")]
    Unknown {
        provider: String,
        message: String,
    },
}

impl FailoverError {
    /// Check if this error is retriable with a different provider.
    pub fn is_retriable(&self) -> bool {
        matches!(
            self,
            FailoverError::RateLimit { .. }
                | FailoverError::Timeout { .. }
                | FailoverError::Overloaded { .. }
        )
    }

    /// Get the failover reason.
    pub fn reason(&self) -> FailoverReason {
        match self {
            FailoverError::Auth { .. } => FailoverReason::Auth,
            FailoverError::RateLimit { .. } => FailoverReason::RateLimit,
            FailoverError::Billing { .. } => FailoverReason::Billing,
            FailoverError::Timeout { .. } => FailoverReason::Timeout,
            FailoverError::Format { .. } => FailoverReason::Format,
            FailoverError::Overloaded { .. } => FailoverReason::Overloaded,
            FailoverError::Unknown { .. } => FailoverReason::Unknown,
        }
    }

    /// Create from HTTP status code.
    pub fn from_status(provider: &str, model: &str, status: u16, body: &str) -> Self {
        match status {
            401 | 403 => FailoverError::Auth {
                provider: provider.to_string(),
                model: model.to_string(),
                status,
            },
            429 => FailoverError::RateLimit {
                provider: provider.to_string(),
                model: model.to_string(),
                retry_after: None,
            },
            402 => FailoverError::Billing {
                provider: provider.to_string(),
            },
            503 | 502 => FailoverError::Overloaded {
                provider: provider.to_string(),
            },
            _ => FailoverError::Unknown {
                provider: provider.to_string(),
                message: format!("status {}: {}", status, body.chars().take(200).collect::<String>()),
            },
        }
    }
}

#[cfg(test)]
mod tests;
