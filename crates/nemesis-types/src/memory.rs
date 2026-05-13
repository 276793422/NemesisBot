//! Memory-related types.

use serde::{Deserialize, Serialize};

/// Memory type classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MemoryType {
    ShortTerm,
    LongTerm,
    Episodic,
    Graph,
    Daily,
}

impl std::fmt::Display for MemoryType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ShortTerm => write!(f, "short_term"),
            Self::LongTerm => write!(f, "long_term"),
            Self::Episodic => write!(f, "episodic"),
            Self::Graph => write!(f, "graph"),
            Self::Daily => write!(f, "daily"),
        }
    }
}

/// Memory entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: String,
    pub memory_type: MemoryType,
    pub key: String,
    pub content: String,
    pub tags: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub relevance_score: Option<f64>,
}

/// Memory query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryQueryResult {
    pub entries: Vec<MemoryEntry>,
    pub total: usize,
}
