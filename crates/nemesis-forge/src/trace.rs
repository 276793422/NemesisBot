//! Trace collector - observer for conversation-level trace collection.
//!
//! Collects tool call traces, session signals, and LLM interaction data
//! for the forge reflector to analyze.

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

/// Session-level signal detected from traces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSignal {
    /// Signal type (retry, backtrack).
    pub signal_type: String,
    /// Tool name involved.
    pub tool_name: String,
    /// Timestamp.
    pub timestamp: String,
    /// Session key.
    pub session_key: String,
}

/// Statistics derived from collected traces.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceStats {
    /// Total number of traces.
    pub total_traces: usize,
    /// Average LLM rounds per conversation.
    pub avg_rounds: f64,
    /// Efficiency score (tool steps per round).
    pub efficiency_score: f64,
    /// Tool chain patterns detected.
    #[serde(default)]
    pub tool_chain_patterns: Vec<ToolChainPattern>,
    /// Retry patterns detected.
    #[serde(default)]
    pub retry_patterns: Vec<RetryPattern>,
    /// Summary of signals.
    #[serde(default)]
    pub signal_summary: std::collections::HashMap<String, u32>,
}

/// A detected tool chain pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolChainPattern {
    /// Chain description (e.g., "file_read -> file_write -> memory_store").
    pub chain: String,
    /// Occurrence count.
    pub count: u32,
    /// Average rounds.
    pub avg_rounds: f64,
    /// Success rate.
    pub success_rate: f64,
}

/// A detected retry pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPattern {
    /// Tool name.
    pub tool_name: String,
    /// Retry count.
    pub retry_count: u32,
    /// Success rate.
    pub success_rate: f64,
}

impl Default for TraceStats {
    fn default() -> Self {
        Self {
            total_traces: 0,
            avg_rounds: 0.0,
            efficiency_score: 0.0,
            tool_chain_patterns: Vec::new(),
            retry_patterns: Vec::new(),
            signal_summary: std::collections::HashMap::new(),
        }
    }
}

/// The trace collector accumulates trace events for analysis.
pub struct TraceCollector {
    events: parking_lot::Mutex<Vec<TraceEvent>>,
    signals: parking_lot::Mutex<Vec<SessionSignal>>,
}

impl TraceCollector {
    /// Create a new trace collector.
    pub fn new() -> Self {
        Self {
            events: parking_lot::Mutex::new(Vec::new()),
            signals: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// Record a trace event.
    pub fn record_event(&self, event: TraceEvent) {
        self.events.lock().push(event);
    }

    /// Record a session signal.
    pub fn record_signal(&self, signal: SessionSignal) {
        self.signals.lock().push(signal);
    }

    /// Get all collected events.
    pub fn events(&self) -> Vec<TraceEvent> {
        self.events.lock().clone()
    }

    /// Get all collected signals.
    pub fn signals(&self) -> Vec<SessionSignal> {
        self.signals.lock().clone()
    }

    /// Compute statistics from collected events and signals.
    pub fn compute_stats(&self) -> TraceStats {
        let events = self.events.lock();
        let signals = self.signals.lock();

        let total_traces = events.len();

        // Count unique sessions and rounds
        let mut session_rounds: std::collections::HashMap<String, u32> =
            std::collections::HashMap::new();
        for event in events.iter() {
            if event.event_type == "llm_response" {
                *session_rounds.entry(event.session_key.clone()).or_insert(0) += 1;
            }
        }

        let avg_rounds = if !session_rounds.is_empty() {
            session_rounds.values().sum::<u32>() as f64 / session_rounds.len() as f64
        } else {
            0.0
        };

        let tool_calls = events
            .iter()
            .filter(|e| e.event_type == "tool_call")
            .count() as f64;
        let efficiency_score = if total_traces > 0 {
            tool_calls / total_traces as f64
        } else {
            0.0
        };

        // Signal summary
        let mut signal_summary = std::collections::HashMap::new();
        for signal in signals.iter() {
            *signal_summary
                .entry(signal.signal_type.clone())
                .or_insert(0u32) += 1;
        }

        TraceStats {
            total_traces,
            avg_rounds,
            efficiency_score,
            tool_chain_patterns: Vec::new(),
            retry_patterns: Vec::new(),
            signal_summary,
        }
    }

    /// Clear all collected data.
    pub fn clear(&self) {
        self.events.lock().clear();
        self.signals.lock().clear();
    }

    /// Return the number of collected events.
    pub fn len(&self) -> usize {
        self.events.lock().len()
    }

    /// Return whether the collector is empty.
    pub fn is_empty(&self) -> bool {
        self.events.lock().is_empty()
    }
}

impl Default for TraceCollector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
