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
mod tests {
    use super::*;

    #[test]
    fn test_retriable_errors() {
        let err = FailoverError::RateLimit {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            retry_after: None,
        };
        assert!(err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::RateLimit);

        let err = FailoverError::Auth {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            status: 401,
        };
        assert!(!err.is_retriable());
    }

    #[test]
    fn test_from_status() {
        let err = FailoverError::from_status("openai", "gpt-4", 401, "unauthorized");
        assert!(matches!(err, FailoverError::Auth { .. }));

        let err = FailoverError::from_status("openai", "gpt-4", 429, "slow down");
        assert!(matches!(err, FailoverError::RateLimit { .. }));

        let err = FailoverError::from_status("openai", "gpt-4", 503, "overloaded");
        assert!(matches!(err, FailoverError::Overloaded { .. }));

        let err = FailoverError::from_status("openai", "gpt-4", 500, "internal error");
        assert!(matches!(err, FailoverError::Unknown { .. }));
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_retriable_timeout() {
        let err = FailoverError::Timeout {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
        };
        assert!(err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::Timeout);
    }

    #[test]
    fn test_retriable_overloaded() {
        let err = FailoverError::Overloaded {
            provider: "openai".to_string(),
        };
        assert!(err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::Overloaded);
    }

    #[test]
    fn test_not_retriable_billing() {
        let err = FailoverError::Billing {
            provider: "openai".to_string(),
        };
        assert!(!err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::Billing);
    }

    #[test]
    fn test_not_retriable_format() {
        let err = FailoverError::Format {
            provider: "openai".to_string(),
            message: "bad format".to_string(),
        };
        assert!(!err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::Format);
    }

    #[test]
    fn test_not_retriable_unknown() {
        let err = FailoverError::Unknown {
            provider: "openai".to_string(),
            message: "something weird".to_string(),
        };
        assert!(!err.is_retriable());
        assert_eq!(err.reason(), FailoverReason::Unknown);
    }

    #[test]
    fn test_from_status_401() {
        let err = FailoverError::from_status("anthropic", "claude-3", 401, "bad key");
        assert!(matches!(err, FailoverError::Auth { status: 401, .. }));
    }

    #[test]
    fn test_from_status_403() {
        let err = FailoverError::from_status("anthropic", "claude-3", 403, "forbidden");
        assert!(matches!(err, FailoverError::Auth { status: 403, .. }));
    }

    #[test]
    fn test_from_status_402() {
        let err = FailoverError::from_status("openai", "gpt-4", 402, "payment required");
        assert!(matches!(err, FailoverError::Billing { .. }));
    }

    #[test]
    fn test_from_status_502() {
        let err = FailoverError::from_status("openai", "gpt-4", 502, "bad gateway");
        assert!(matches!(err, FailoverError::Overloaded { .. }));
    }

    #[test]
    fn test_from_status_400() {
        let err = FailoverError::from_status("openai", "gpt-4", 400, "bad request");
        assert!(matches!(err, FailoverError::Unknown { .. }));
    }

    #[test]
    fn test_from_status_404() {
        let err = FailoverError::from_status("openai", "gpt-4", 404, "not found");
        assert!(matches!(err, FailoverError::Unknown { .. }));
    }

    #[test]
    fn test_error_display() {
        let err = FailoverError::Auth {
            provider: "openai".to_string(),
            model: "gpt-4".to_string(),
            status: 401,
        };
        let msg = format!("{}", err);
        assert!(msg.contains("openai"));
        assert!(msg.contains("gpt-4"));
        assert!(msg.contains("401"));
    }

    #[test]
    fn test_rate_limit_display() {
        let err = FailoverError::RateLimit {
            provider: "anthropic".to_string(),
            model: "claude-3".to_string(),
            retry_after: Some(60),
        };
        let msg = format!("{}", err);
        assert!(msg.contains("anthropic"));
        assert!(msg.contains("claude-3"));
    }

    #[test]
    fn test_reason_all_variants() {
        assert_eq!(FailoverError::Auth { provider: "p".to_string(), model: "m".to_string(), status: 0 }.reason(), FailoverReason::Auth);
        assert_eq!(FailoverError::RateLimit { provider: "p".to_string(), model: "m".to_string(), retry_after: None }.reason(), FailoverReason::RateLimit);
        assert_eq!(FailoverError::Billing { provider: "p".to_string() }.reason(), FailoverReason::Billing);
        assert_eq!(FailoverError::Timeout { provider: "p".to_string(), model: "m".to_string() }.reason(), FailoverReason::Timeout);
        assert_eq!(FailoverError::Format { provider: "p".to_string(), message: "m".to_string() }.reason(), FailoverReason::Format);
        assert_eq!(FailoverError::Overloaded { provider: "p".to_string() }.reason(), FailoverReason::Overloaded);
        assert_eq!(FailoverError::Unknown { provider: "p".to_string(), message: "m".to_string() }.reason(), FailoverReason::Unknown);
    }
}
