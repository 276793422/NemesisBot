//! Security-related types.

use serde::{Deserialize, Serialize};

/// Risk level for operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// Security verdict from the security pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityVerdict {
    pub allowed: bool,
    pub risk_level: RiskLevel,
    pub reason: Option<String>,
    pub blocked_by: Option<String>,
}

/// Security operation being evaluated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub action: String,
    pub target: Option<String>,
    pub parameters: serde_json::Value,
    pub channel: String,
    pub sender_id: String,
}

/// Audit event for the integrity chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEvent {
    pub id: String,
    pub timestamp: String,
    pub operation: Operation,
    pub verdict: SecurityVerdict,
    pub hash: String,
    pub prev_hash: String,
}
