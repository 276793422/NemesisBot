use super::*;

fn make_turn(role: &str, content: impl Into<String>) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.into(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-04-29T12:00:00Z".to_string(),
        reasoning_content: None,
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
        memory
            .turns
            .push(make_turn("user", "a".repeat(200) + &format!(" {}", i)));
        memory
            .turns
            .push(make_turn("assistant", "b".repeat(200) + &format!(" {}", i)));
    }

    // Before summarization: we should have 13 turns (1 system + 12 user/assistant).
    assert_eq!(memory.len(), 13);

    // Trigger summarization.
    let removed = memory.summarize();
    assert!(
        removed > 0,
        "Expected some turns to be removed, but removed={}",
        removed
    );
    // System prompt should still be there.
    assert_eq!(memory.get_context()[0].role, "system");
    // Some turns should have been removed.
    assert!(
        memory.len() < 13,
        "Expected fewer turns after summarization, got {}",
        memory.len()
    );
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
        memory.turns.push(make_turn(
            "user",
            format!("Long message {} with padding content to exceed limits", i),
        ));
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
