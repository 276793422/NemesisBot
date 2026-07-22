//! Conversation memory: manages context window sizing and message summarization.
//!
//! Two memory systems:
//! - `ConversationMemory`: in-memory LLM context window with token-based truncation.
//! - `MemoryStore`: file-based persistent memory (MEMORY.md + daily notes).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::types::ConversationTurn;

/// Configuration for conversation memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Maximum estimated token count before truncation is applied.
    pub max_tokens: usize,
    /// Number of tokens to keep after summarization (the most recent ones).
    pub keep_tokens: usize,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_tokens: 32000,
            keep_tokens: 16000,
        }
    }
}

/// Manages conversation context window with token-based truncation.
pub struct ConversationMemory {
    /// Stored conversation turns.
    turns: Vec<ConversationTurn>,
    /// Memory configuration.
    config: MemoryConfig,
}

impl ConversationMemory {
    /// Create a new conversation memory with the given configuration.
    pub fn new(config: MemoryConfig) -> Self {
        Self {
            turns: Vec::new(),
            config,
        }
    }

    /// Create memory with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(MemoryConfig::default())
    }

    /// Add a conversation turn to memory.
    pub fn add(&mut self, turn: ConversationTurn) {
        self.turns.push(turn);
        self.check_truncation();
    }

    /// Get the current conversation context as a list of turns.
    pub fn get_context(&self) -> &[ConversationTurn] {
        &self.turns
    }

    /// Get the total estimated token count for all stored turns.
    pub fn estimated_tokens(&self) -> usize {
        self.turns.iter().map(|t| estimate_tokens(&t.content)).sum()
    }

    /// Truncate old messages to bring the context within limits.
    ///
    /// Returns the number of turns that were removed.
    pub fn summarize(&mut self) -> usize {
        let original_len = self.turns.len();
        if original_len == 0 {
            return 0;
        }

        let target_tokens = self.config.keep_tokens;

        // Walk from the end backwards, accumulating tokens until we exceed target.
        let mut accumulated = 0usize;
        let mut keep_from = self.turns.len();
        for (i, turn) in self.turns.iter().enumerate().rev() {
            accumulated += estimate_tokens(&turn.content);
            if accumulated >= target_tokens {
                keep_from = i;
                break;
            }
        }

        // Always keep at least the first turn (system prompt).
        keep_from = keep_from.max(1);

        let removed = keep_from - 1;
        if removed > 0 {
            debug!(
                "Summarizing: removing {} old turns, keeping {}",
                removed,
                self.turns.len() - keep_from
            );
            // Keep turns from keep_from onward, plus turn 0 (system).
            let system = self.turns.first().cloned();
            let remaining: Vec<ConversationTurn> = self.turns.drain(keep_from..).collect();
            self.turns.truncate(1);
            self.turns.extend(remaining);
            // Edge case: if we didn't have a system prompt, don't keep an empty slot.
            if system.is_none() && self.turns.first().map_or(false, |t| t.role != "system") {
                // no-op
            }
        }

        original_len - self.turns.len()
    }

    /// Search turns by keyword match (case-insensitive).
    ///
    /// Returns all turns whose content contains the given keyword.
    pub fn search(&self, keyword: &str) -> Vec<&ConversationTurn> {
        let keyword_lower = keyword.to_lowercase();
        self.turns
            .iter()
            .filter(|t| t.content.to_lowercase().contains(&keyword_lower))
            .collect()
    }

    /// Returns the number of stored turns.
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Returns true if there are no stored turns.
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Check if truncation is needed and apply it.
    fn check_truncation(&mut self) {
        if self.estimated_tokens() > self.config.max_tokens {
            self.summarize();
        }
    }
}

/// Estimate the token count for a string.
///
/// Uses a heuristic of approximately 2.5 characters per token (chars * 2 / 5),
/// matching Go's `utf8.RuneCountInString(m.Content) * 2 / 5` formula.
/// This correctly handles CJK and other multi-byte text, unlike byte-based
/// division which overestimates by 3x for CJK content.
fn estimate_tokens(text: &str) -> usize {
    text.chars().count() * 2 / 5
}

// ---------------------------------------------------------------------------
// File-based persistent memory store (matches Go MemoryStore)
// ---------------------------------------------------------------------------

/// File-based persistent memory store.
///
/// - Long-term memory: `memory/MEMORY.md`
/// - Daily notes: `memory/YYYYMM/YYYYMMDD.md`
pub struct MemoryStore {
    #[allow(dead_code)] // Reserved for future workspace-relative operations
    workspace: PathBuf,
    memory_dir: PathBuf,
    memory_file: PathBuf,
}

impl MemoryStore {
    /// Create a new MemoryStore for the given workspace.
    ///
    /// Ensures the `memory/` directory exists.
    pub fn new(workspace: &str) -> Self {
        let workspace = PathBuf::from(workspace);
        let memory_dir = workspace.join("memory");
        let memory_file = memory_dir.join("MEMORY.md");

        // Ensure memory directory exists.
        let _ = fs::create_dir_all(&memory_dir);

        info!(
            "[MemoryStore] Initialized, memory_dir={}",
            memory_dir.display()
        );

        Self {
            workspace,
            memory_dir,
            memory_file,
        }
    }

    /// Return the path to today's daily note file (`memory/YYYYMM/YYYYMMDD.md`).
    fn today_file(&self) -> PathBuf {
        let today = Local::now().format("%Y%m%d").to_string(); // YYYYMMDD
        let month_dir = &today[..6]; // YYYYMM
        self.memory_dir
            .join(month_dir)
            .join(format!("{}.md", today))
    }

    /// Read the long-term memory file (`MEMORY.md`).
    ///
    /// Returns an empty string if the file does not exist.
    pub fn read_long_term(&self) -> String {
        fs::read_to_string(&self.memory_file).unwrap_or_default()
    }

    /// Write content to the long-term memory file.
    pub fn write_long_term(&self, content: &str) -> std::io::Result<()> {
        debug!(
            "[MemoryStore] Writing long-term memory, {} bytes",
            content.len()
        );
        if let Some(parent) = self.memory_file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&self.memory_file, content)
    }

    /// Read today's daily note.
    ///
    /// Returns an empty string if the file does not exist.
    pub fn read_today(&self) -> String {
        let path = self.today_file();
        fs::read_to_string(&path).unwrap_or_default()
    }

    /// Append content to today's daily note.
    ///
    /// If the file does not exist yet, it is created with a date header.
    pub fn append_today(&self, content: &str) -> std::io::Result<()> {
        debug!(
            "[MemoryStore] Appending to today's daily note, {} bytes",
            content.len()
        );
        let path = self.today_file();

        // Ensure month directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let existing = fs::read_to_string(&path).unwrap_or_default();

        let new_content = if existing.is_empty() {
            format!("# {}\n\n{}", Local::now().format("%Y-%m-%d"), content)
        } else {
            format!("{}\n{}", existing, content)
        };

        fs::write(&path, new_content)
    }

    /// Return daily notes from the last `days` days, joined with `---`.
    pub fn get_recent_daily_notes(&self, days: usize) -> String {
        let mut notes = Vec::new();
        let today = Local::now().date_naive();

        for i in 0..days {
            let date = today - chrono::Duration::days(i as i64);
            let date_str = date.format("%Y%m%d").to_string();
            let month_dir = &date_str[..6];
            let path = self
                .memory_dir
                .join(month_dir)
                .join(format!("{}.md", date_str));

            if let Ok(data) = fs::read_to_string(&path) {
                notes.push(data);
            }
        }

        notes.join("\n\n---\n\n")
    }

    /// Return formatted memory context suitable for injection into the agent prompt.
    ///
    /// Includes long-term memory and recent daily notes (last 3 days).
    pub fn get_memory_context(&self) -> String {
        let mut parts = Vec::new();

        let long_term = self.read_long_term();
        if !long_term.is_empty() {
            parts.push(format!("## Long-term Memory\n\n{}", long_term));
        }

        let recent_notes = self.get_recent_daily_notes(3);
        if !recent_notes.is_empty() {
            parts.push(format!("## Recent Daily Notes\n\n{}", recent_notes));
        }

        if parts.is_empty() {
            return String::new();
        }

        format!("# Memory\n\n{}", parts.join("\n\n---\n\n"))
    }

    /// Return the memory directory path.
    pub fn memory_dir(&self) -> &Path {
        &self.memory_dir
    }

    /// Return the long-term memory file path.
    pub fn memory_file(&self) -> &Path {
        &self.memory_file
    }
}

#[cfg(test)]
mod tests;
