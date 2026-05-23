//! History types for chat history pagination.

use serde::{Deserialize, Serialize};

/// A single message in chat history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryMessage {
    pub role: String,
    pub content: String,
}

/// A page of history messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryPage {
    pub messages: Vec<HistoryMessage>,
    pub has_more: bool,
    pub oldest_index: i64,
    pub total_count: i64,
}

/// History request data payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryRequestData {
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_index: Option<i64>,
}

#[cfg(test)]
mod tests;
