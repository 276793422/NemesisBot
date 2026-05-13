//! Conversation memory: manages context window sizing and message summarization.
//!
//! Two memory systems:
//! - `ConversationMemory`: in-memory LLM context window with token-based truncation.
//! - `MemoryStore`: file-based persistent memory (MEMORY.md + daily notes).

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Local;
use serde::{Deserialize, Serialize};
use tracing::debug;

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
            let remaining: Vec<ConversationTurn> =
                self.turns.drain(keep_from..).collect();
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
        self.memory_dir.join(month_dir).join(format!("{}.md", today))
    }

    /// Read the long-term memory file (`MEMORY.md`).
    ///
    /// Returns an empty string if the file does not exist.
    pub fn read_long_term(&self) -> String {
        fs::read_to_string(&self.memory_file).unwrap_or_default()
    }

    /// Write content to the long-term memory file.
    pub fn write_long_term(&self, content: &str) -> std::io::Result<()> {
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
        let path = self.today_file();

        // Ensure month directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let existing = fs::read_to_string(&path).unwrap_or_default();

        let new_content = if existing.is_empty() {
            format!(
                "# {}\n\n{}",
                Local::now().format("%Y-%m-%d"),
                content
            )
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
            let path = self.memory_dir.join(month_dir).join(format!("{}.md", date_str));

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
mod tests {
    use super::*;

    fn make_turn(role: &str, content: impl Into<String>) -> ConversationTurn {
        ConversationTurn {
            role: role.to_string(),
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-04-29T12:00:00Z".to_string(),
        }
    }

    #[test]
    fn add_and_get_context() {
        let mut memory = ConversationMemory::with_defaults();
        memory.add(make_turn("system", "You are helpful."));
        memory.add(make_turn("user", "Hello"));
        memory.add(make_turn("assistant", "Hi!"));

        assert_eq!(memory.len(), 3);
        let ctx = memory.get_context();
        assert_eq!(ctx[0].role, "system");
        assert_eq!(ctx[1].role, "user");
        assert_eq!(ctx[2].role, "assistant");
    }

    #[test]
    fn summarize_removes_old_turns() {
        let config = MemoryConfig {
            max_tokens: 100,
            keep_tokens: 50,
        };

        // Build turns manually without going through add() so we control
        // exactly when summarization fires.
        let mut memory = ConversationMemory::new(config);

        // Add a system prompt.
        memory.add(make_turn("system", "You are helpful.")); // 17 chars → 6 tokens

        // Add many turns directly to the internal vector, bypassing auto-truncation.
        // Each "a".repeat(200) + " N" is 202 chars → 80 tokens. Several turns will push us well over 100.
        for i in 0..6 {
            memory.turns.push(make_turn("user", "a".repeat(200) + &format!(" {}", i)));
            memory.turns.push(make_turn("assistant", "b".repeat(200) + &format!(" {}", i)));
        }

        // Before summarization: we should have 13 turns (1 system + 12 user/assistant).
        assert_eq!(memory.len(), 13);

        // Trigger summarization.
        let removed = memory.summarize();
        assert!(removed > 0, "Expected some turns to be removed, but removed={}", removed);
        // System prompt should still be there.
        assert_eq!(memory.get_context()[0].role, "system");
        // Some turns should have been removed.
        assert!(memory.len() < 13, "Expected fewer turns after summarization, got {}", memory.len());
    }

    #[test]
    fn search_finds_matching_turns() {
        let mut memory = ConversationMemory::with_defaults();
        memory.add(make_turn("user", "Tell me about Rust programming"));
        memory.add(make_turn("assistant", "Rust is a systems language"));
        memory.add(make_turn("user", "What about Python?"));
        memory.add(make_turn("assistant", "Python is a scripting language"));

        let results = memory.search("rust");
        // Case-insensitive search matches "Rust" in both turns.
        assert_eq!(results.len(), 2);
        assert!(results[0].content.contains("Rust"));
        assert!(results[1].content.contains("Rust"));
    }

    #[test]
    fn search_is_case_insensitive() {
        let mut memory = ConversationMemory::with_defaults();
        memory.add(make_turn("user", "Hello WORLD"));

        let results = memory.search("world");
        assert_eq!(results.len(), 1);

        let results = memory.search("HELLO");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn search_no_match_returns_empty() {
        let mut memory = ConversationMemory::with_defaults();
        memory.add(make_turn("user", "Hello world"));

        let results = memory.search("xyz");
        assert!(results.is_empty());
    }

    #[test]
    fn search_empty_memory_returns_empty() {
        let memory = ConversationMemory::with_defaults();
        let results = memory.search("anything");
        assert!(results.is_empty());
    }

    #[test]
    fn estimated_tokens_calculation() {
        let mut memory = ConversationMemory::with_defaults();
        memory.add(make_turn("system", "Hello"));
        memory.add(make_turn("user", "World"));

        let tokens = memory.estimated_tokens();
        // "Hello" = 5 chars, 5*2/5 = 2; "World" = 5 chars, 5*2/5 = 2; total = 4
        assert_eq!(tokens, 4);
    }

    #[test]
    fn summarize_on_empty_memory() {
        let mut memory = ConversationMemory::with_defaults();
        let removed = memory.summarize();
        assert_eq!(removed, 0);
    }

    #[test]
    fn summarize_keeps_system_prompt() {
        let config = MemoryConfig {
            max_tokens: 100,
            keep_tokens: 20,
        };
        let mut memory = ConversationMemory::new(config);
        memory.add(make_turn("system", "You are helpful."));
        for i in 0..10 {
            memory.turns.push(make_turn("user", format!("Long message {} with padding content to exceed limits", i)));
        }

        let removed = memory.summarize();
        assert!(removed > 0);
        assert_eq!(memory.get_context()[0].role, "system");
    }

    #[test]
    fn check_truncation_auto_triggers() {
        let config = MemoryConfig {
            max_tokens: 10,
            keep_tokens: 5,
        };
        let mut memory = ConversationMemory::new(config);
        memory.add(make_turn("system", "System"));
        // Add turns that exceed max_tokens
        memory.add(make_turn("user", "a".repeat(50))); // 50 chars = 20 tokens
        memory.add(make_turn("user", "b".repeat(50)));

        // After add, check_truncation should have fired and reduced the size
        // The system prompt should survive
        let ctx = memory.get_context();
        assert!(ctx.iter().any(|t| t.role == "system"));
    }

    #[test]
    fn memory_config_default() {
        let config = MemoryConfig::default();
        assert_eq!(config.max_tokens, 32000);
        assert_eq!(config.keep_tokens, 16000);
    }

    #[test]
    fn memory_config_serialization() {
        let config = MemoryConfig {
            max_tokens: 100,
            keep_tokens: 50,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: MemoryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.max_tokens, 100);
        assert_eq!(parsed.keep_tokens, 50);
    }

    // --- MemoryStore tests ---

    #[test]
    fn memory_store_new_creates_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("ws");
        let store = MemoryStore::new(workspace.to_str().unwrap());

        assert!(workspace.join("memory").exists());
        assert_eq!(store.memory_dir(), workspace.join("memory"));
        assert_eq!(store.memory_file(), workspace.join("memory/MEMORY.md"));
    }

    #[test]
    fn memory_store_read_write_long_term() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());

        assert!(store.read_long_term().is_empty());

        store.write_long_term("# My Memory\nSome notes.").unwrap();
        let content = store.read_long_term();
        assert!(content.contains("My Memory"));
        assert!(content.contains("Some notes."));
    }

    #[test]
    fn memory_store_overwrite_long_term() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());

        store.write_long_term("First").unwrap();
        store.write_long_term("Second").unwrap();
        assert_eq!(store.read_long_term(), "Second");
    }

    #[test]
    fn memory_store_append_today() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());

        // First append creates the file with header.
        store.append_today("First note.").unwrap();
        let content = store.read_today();
        assert!(content.contains("First note."));

        // Second append appends to existing.
        store.append_today("Second note.").unwrap();
        let content = store.read_today();
        assert!(content.contains("First note."));
        assert!(content.contains("Second note."));
    }

    #[test]
    fn memory_store_read_today_empty_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        assert!(store.read_today().is_empty());
    }

    #[test]
    fn memory_store_get_recent_daily_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());

        // Write today's note.
        store.append_today("Today's entry.").unwrap();

        let notes = store.get_recent_daily_notes(3);
        assert!(notes.contains("Today's entry."));
    }

    #[test]
    fn memory_store_get_recent_daily_notes_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        let notes = store.get_recent_daily_notes(7);
        assert!(notes.is_empty());
    }

    #[test]
    fn memory_store_get_memory_context_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        assert!(store.get_memory_context().is_empty());
    }

    #[test]
    fn memory_store_get_memory_context_with_long_term() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        store.write_long_term("Important fact.").unwrap();

        let ctx = store.get_memory_context();
        assert!(ctx.contains("# Memory"));
        assert!(ctx.contains("Long-term Memory"));
        assert!(ctx.contains("Important fact."));
    }

    #[test]
    fn memory_store_get_memory_context_with_notes() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        store.append_today("Daily update.").unwrap();

        let ctx = store.get_memory_context();
        assert!(ctx.contains("# Memory"));
        assert!(ctx.contains("Recent Daily Notes"));
        assert!(ctx.contains("Daily update."));
    }

    #[test]
    fn memory_store_get_memory_context_both() {
        let tmp = tempfile::tempdir().unwrap();
        let store = MemoryStore::new(tmp.path().to_str().unwrap());
        store.write_long_term("Long term.").unwrap();
        store.append_today("Today's note.").unwrap();

        let ctx = store.get_memory_context();
        assert!(ctx.contains("Long-term Memory"));
        assert!(ctx.contains("Long term."));
        assert!(ctx.contains("Recent Daily Notes"));
        assert!(ctx.contains("Today's note."));
        assert!(ctx.contains("---"));
    }

    #[test]
    fn memory_store_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let workspace = tmp.path().join("test_ws");
        let store = MemoryStore::new(workspace.to_str().unwrap());

        assert!(store.memory_dir().ends_with("memory"));
        assert!(store.memory_file().ends_with("memory/MEMORY.md"));
    }
}
