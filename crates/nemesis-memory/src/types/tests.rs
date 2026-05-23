use super::*;

#[test]
fn entry_new_generates_valid_id() {
    let entry = Entry::new(MemoryType::LongTerm, "test content".to_string());
    assert!(!entry.id.is_empty());
    // UUID v4 format: 8-4-4-4-12
    assert_eq!(entry.id.len(), 36);
    assert_eq!(entry.typ, MemoryType::LongTerm);
    assert_eq!(entry.content, "test content");
}

#[test]
fn entry_builder_methods_work() {
    let mut meta = HashMap::new();
    meta.insert("source".to_string(), "test".to_string());

    let entry = Entry::new(MemoryType::ShortTerm, "hello".to_string())
        .with_tags(vec!["greeting".to_string(), "test".to_string()])
        .with_metadata(meta)
        .with_score(0.95);

    assert_eq!(entry.tags.len(), 2);
    assert_eq!(entry.metadata.get("source").unwrap(), "test");
    assert!((entry.score.unwrap() - 0.95).abs() < f64::EPSILON);
    assert!(entry.created_at <= entry.updated_at);
}

#[test]
fn memory_type_display() {
    assert_eq!(MemoryType::ShortTerm.to_string(), "short_term");
    assert_eq!(MemoryType::LongTerm.to_string(), "long_term");
    assert_eq!(MemoryType::Episodic.to_string(), "episodic");
    assert_eq!(MemoryType::Graph.to_string(), "graph");
    assert_eq!(MemoryType::Daily.to_string(), "daily");
}

#[test]
fn vector_config_default_values() {
    let config = VectorConfig::default();
    assert_eq!(config.embedding_tier, "plugin");
    assert!(config.plugin_path.is_none());
}

#[test]
fn entry_serialization_roundtrip() {
    let mut meta = HashMap::new();
    meta.insert("key1".to_string(), "value1".to_string());
    let entry = Entry::new(MemoryType::LongTerm, "test content".to_string())
        .with_tags(vec!["tag1".to_string(), "tag2".to_string()])
        .with_metadata(meta)
        .with_score(0.85);

    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: Entry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, entry.id);
    assert_eq!(deserialized.typ, entry.typ);
    assert_eq!(deserialized.content, entry.content);
    assert_eq!(deserialized.tags, entry.tags);
    assert!((deserialized.score.unwrap() - 0.85).abs() < f64::EPSILON);
    assert_eq!(deserialized.metadata.get("key1").unwrap(), "value1");
}

#[test]
fn entry_default_metadata_and_tags() {
    let entry = Entry::new(MemoryType::ShortTerm, "hello".to_string());
    assert!(entry.metadata.is_empty());
    assert!(entry.tags.is_empty());
    assert!(entry.score.is_none());
}

#[test]
fn entry_with_score_zero() {
    let entry = Entry::new(MemoryType::Daily, "daily note".to_string()).with_score(0.0);
    assert_eq!(entry.score.unwrap(), 0.0);
}

#[test]
fn entry_with_score_one() {
    let entry = Entry::new(MemoryType::Episodic, "episodic".to_string()).with_score(1.0);
    assert_eq!(entry.score.unwrap(), 1.0);
}

#[test]
fn entry_different_memory_types() {
    let types = vec![
        MemoryType::ShortTerm,
        MemoryType::LongTerm,
        MemoryType::Episodic,
        MemoryType::Graph,
        MemoryType::Daily,
    ];
    for mt in types {
        let entry = Entry::new(mt, format!("content for {:?}", mt));
        assert_eq!(entry.typ, mt);
    }
}

#[test]
fn entry_unique_ids() {
    let e1 = Entry::new(MemoryType::LongTerm, "a".to_string());
    let e2 = Entry::new(MemoryType::LongTerm, "b".to_string());
    assert_ne!(e1.id, e2.id);
}

#[test]
fn entry_timestamps_set() {
    let entry = Entry::new(MemoryType::LongTerm, "ts test".to_string());
    assert!(entry.created_at <= chrono::Utc::now());
    assert!(entry.updated_at <= chrono::Utc::now());
    assert!(entry.created_at <= entry.updated_at);
}

#[test]
fn search_result_serialization() {
    let entry = Entry::new(MemoryType::LongTerm, "test".to_string());
    let sr = SearchResult {
        entries: vec![ScoredEntry { entry, score: 0.95 }],
        total: 1,
    };
    let json = serde_json::to_string(&sr).unwrap();
    let deserialized: SearchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.total, 1);
    assert_eq!(deserialized.entries.len(), 1);
    assert!((deserialized.entries[0].score - 0.95).abs() < f64::EPSILON);
}

#[test]
fn search_result_empty() {
    let sr = SearchResult {
        entries: vec![],
        total: 0,
    };
    assert!(sr.entries.is_empty());
    assert_eq!(sr.total, 0);
    let json = serde_json::to_string(&sr).unwrap();
    let deserialized: SearchResult = serde_json::from_str(&json).unwrap();
    assert!(deserialized.entries.is_empty());
}

#[test]
fn scored_entry_ordering() {
    let e1 = Entry::new(MemoryType::LongTerm, "a".to_string());
    let e2 = Entry::new(MemoryType::LongTerm, "b".to_string());
    let s1 = ScoredEntry { entry: e1, score: 0.9 };
    let s2 = ScoredEntry { entry: e2, score: 0.5 };
    assert!(s1.score > s2.score);
}

#[test]
fn vector_config_custom() {
    let config = VectorConfig {
        embedding_tier: "plugin".to_string(),
        plugin_path: Some("/path/to/plugin".to_string()),
        config_dir: None,
        host_services: None,
    };
    assert_eq!(config.embedding_tier, "plugin");
    assert!(config.plugin_path.is_some());
}

#[test]
fn vector_config_serialization_roundtrip() {
    let config = VectorConfig {
        embedding_tier: "plugin".to_string(),
        plugin_path: None,
        config_dir: None,
        host_services: None,
    };
    let json = serde_json::to_string(&config).unwrap();
    let deserialized: VectorConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.embedding_tier, "plugin");
}

#[test]
fn entry_with_empty_tags() {
    let entry = Entry::new(MemoryType::LongTerm, "no tags".to_string()).with_tags(vec![]);
    assert!(entry.tags.is_empty());
}

#[test]
fn entry_with_empty_metadata() {
    let entry = Entry::new(MemoryType::LongTerm, "no meta".to_string()).with_metadata(HashMap::new());
    assert!(entry.metadata.is_empty());
}

#[test]
fn entry_content_with_special_chars() {
    let content = "Hello\n\t\"world\"\r\n{'key': 'value'}";
    let entry = Entry::new(MemoryType::LongTerm, content.to_string());
    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: Entry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.content, content);
}

#[test]
fn entry_content_unicode() {
    let content = "日本語テスト 🎉 Ñoño";
    let entry = Entry::new(MemoryType::LongTerm, content.to_string());
    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: Entry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.content, content);
}

#[test]
fn entry_content_very_long() {
    let content = "a".repeat(1_000_000);
    let entry = Entry::new(MemoryType::LongTerm, content.clone());
    assert_eq!(entry.content.len(), 1_000_000);
}
