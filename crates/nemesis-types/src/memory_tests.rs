//! Tests for memory.rs

use crate::memory::{MemoryEntry, MemoryQueryResult, MemoryType};
use serde_json::{from_value, to_value};

#[test]
fn test_memory_type_display() {
    assert_eq!(MemoryType::ShortTerm.to_string(), "short_term");
    assert_eq!(MemoryType::LongTerm.to_string(), "long_term");
    assert_eq!(MemoryType::Episodic.to_string(), "episodic");
    assert_eq!(MemoryType::Graph.to_string(), "graph");
    assert_eq!(MemoryType::Daily.to_string(), "daily");
}

#[test]
fn test_memory_type_clone_copy() {
    let mt = MemoryType::ShortTerm;
    let mt_copy = mt;
    let mt_clone = mt.clone();

    assert_eq!(mt, mt_copy);
    assert_eq!(mt, mt_clone);
}

#[test]
fn test_memory_type_serialization() {
    let mt = MemoryType::Episodic;
    let json = to_value(mt).unwrap();
    assert_eq!(json, "Episodic");

    let deserialized: MemoryType = from_value(json).unwrap();
    assert_eq!(deserialized, MemoryType::Episodic);
}

#[test]
fn test_memory_type_equality() {
    assert_eq!(MemoryType::ShortTerm, MemoryType::ShortTerm);
    assert_ne!(MemoryType::ShortTerm, MemoryType::LongTerm);
    assert_ne!(MemoryType::Graph, MemoryType::Daily);
}

#[test]
fn test_memory_entry_basic() {
    let entry = MemoryEntry {
        id: "test-id".to_string(),
        memory_type: MemoryType::ShortTerm,
        key: "test-key".to_string(),
        content: "test content".to_string(),
        tags: vec!["tag1".to_string(), "tag2".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: Some(0.95),
    };

    assert_eq!(entry.id, "test-id");
    assert_eq!(entry.memory_type, MemoryType::ShortTerm);
    assert_eq!(entry.tags.len(), 2);
    assert_eq!(entry.relevance_score, Some(0.95));
}

#[test]
fn test_memory_entry_no_score() {
    let entry = MemoryEntry {
        id: "test-id".to_string(),
        memory_type: MemoryType::LongTerm,
        key: "test-key".to_string(),
        content: "test content".to_string(),
        tags: vec![],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: None,
    };

    assert_eq!(entry.relevance_score, None);
    assert!(entry.tags.is_empty());
}

#[test]
fn test_memory_entry_clone() {
    let entry = MemoryEntry {
        id: "test-id".to_string(),
        memory_type: MemoryType::Episodic,
        key: "test-key".to_string(),
        content: "test content".to_string(),
        tags: vec!["tag1".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: Some(0.5),
    };

    let cloned = entry.clone();
    assert_eq!(entry.id, cloned.id);
    assert_eq!(entry.memory_type, cloned.memory_type);
    assert_eq!(entry.content, cloned.content);
}

#[test]
fn test_memory_entry_serialization() {
    let entry = MemoryEntry {
        id: "test-id".to_string(),
        memory_type: MemoryType::Graph,
        key: "test-key".to_string(),
        content: "test content".to_string(),
        tags: vec!["tag1".to_string(), "tag2".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: Some(0.75),
    };

    let json = to_value(&entry).unwrap();
    assert_eq!(json["id"], "test-id");
    assert_eq!(json["memory_type"], "Graph");
    assert_eq!(json["content"], "test content");
    assert_eq!(json["relevance_score"], 0.75);

    let deserialized: MemoryEntry = from_value(json).unwrap();
    assert_eq!(deserialized.id, entry.id);
    assert_eq!(deserialized.memory_type, entry.memory_type);
    assert_eq!(deserialized.content, entry.content);
}

#[test]
fn test_memory_query_result_basic() {
    let result = MemoryQueryResult {
        entries: vec![],
        total: 0,
    };

    assert_eq!(result.total, 0);
    assert!(result.entries.is_empty());
}

#[test]
fn test_memory_query_result_with_entries() {
    let entry1 = MemoryEntry {
        id: "id1".to_string(),
        memory_type: MemoryType::ShortTerm,
        key: "key1".to_string(),
        content: "content1".to_string(),
        tags: vec!["tag1".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: Some(0.9),
    };

    let entry2 = MemoryEntry {
        id: "id2".to_string(),
        memory_type: MemoryType::LongTerm,
        key: "key2".to_string(),
        content: "content2".to_string(),
        tags: vec!["tag2".to_string()],
        created_at: "2024-01-01T00:00:00Z".to_string(),
        updated_at: "2024-01-01T00:00:00Z".to_string(),
        relevance_score: Some(0.8),
    };

    let result = MemoryQueryResult {
        entries: vec![entry1, entry2],
        total: 2,
    };

    assert_eq!(result.total, 2);
    assert_eq!(result.entries.len(), 2);
}

#[test]
fn test_memory_query_result_serialization() {
    let result = MemoryQueryResult {
        entries: vec![],
        total: 0,
    };

    let json = to_value(&result).unwrap();
    assert_eq!(json["total"], 0);
    assert!(json["entries"].is_array());

    let deserialized: MemoryQueryResult = from_value(json).unwrap();
    assert_eq!(deserialized.total, 0);
    assert!(deserialized.entries.is_empty());
}

#[test]
fn test_all_memory_types() {
    // Test all MemoryType variants
    let types = vec![
        MemoryType::ShortTerm,
        MemoryType::LongTerm,
        MemoryType::Episodic,
        MemoryType::Graph,
        MemoryType::Daily,
    ];

    for mt in types {
        // Test display
        let display_str = mt.to_string();
        assert!(!display_str.is_empty());

        // Test serialization
        let json = to_value(mt).unwrap();
        let deserialized: MemoryType = from_value(json).unwrap();
        assert_eq!(mt, deserialized);
    }
}
