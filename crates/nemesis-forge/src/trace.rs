//! Trace event types for conversation-level trace collection.
//!
//! `TraceEvent` is the JSONL record persisted by
//! [`crate::trace_store:TraceStore`] (fed from the tool-dispatch path via
//! `Forge::record_trace_event`) and consumed by the deployment monitor's
//! efficacy analysis. The in-memory `TraceCollector` and its aggregate types
//! (`SessionSignal` / `TraceStats` / `ToolChainPattern` / `RetryPattern`)
//! were removed 2026-11-05 — they had zero production readers after the
//! 2026-10-22 audit detached the in-memory feed (write-only state); the
//! reflector defines its own analysis-side types.

use serde::{Deserialize, Serialize};

/// A single trace event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEvent {
    /// Unique event ID.
    pub id: String,
    /// Event type (conversation_start, conversation_end, llm_request, llm_response, tool_call).
    pub event_type: String,
    /// Session key (hashed for privacy).
    pub session_key: String,
    /// Event timestamp.
    pub timestamp: String,
    /// Event data (JSON).
    pub data: serde_json::Value,
}
