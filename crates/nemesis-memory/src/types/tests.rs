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
    let mut meta = BTreeMap::new();
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
    let mut meta = BTreeMap::new();
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
    assert!(entry.created_at <= chrono::Local::now());
    assert!(entry.updated_at <= chrono::Local::now());
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
    let s1 = ScoredEntry {
        entry: e1,
        score: 0.9,
    };
    let s2 = ScoredEntry {
        entry: e2,
        score: 0.5,
    };
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
    let entry =
        Entry::new(MemoryType::LongTerm, "no meta".to_string()).with_metadata(BTreeMap::new());
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

/// BUG-20261122-045 回归：Entry.metadata BTreeMap 化——vector store
/// rewrite_persist_file 全量重写直接 `serde_json::to_string` 落盘，
/// 两个独立反序列化实例字节必须恒等（HashMap 每进程随机种子曾使
/// 同内容相邻两次 rewrite 行内键序漂移）。
#[test]
fn entry_metadata_roundtrip_bytes_deterministic() {
    let raw = r#"{"id":"e1","type":"LongTerm","content":"c","metadata":{"zz":"1","aa":"2","mm":"3"},"tags":[],"created_at":"2026-11-22T10:00:00+08:00","updated_at":"2026-11-22T10:00:00+08:00"}"#;
    let a: Entry = serde_json::from_str(raw).unwrap();
    let b: Entry = serde_json::from_str(raw).unwrap();
    let sa = serde_json::to_string(&a).unwrap();
    let sb = serde_json::to_string(&b).unwrap();
    assert_eq!(sa, sb, "两个独立实例的序列化字节必须恒等");
    let pos_aa = sa.find(r#""aa":"2""#).expect("aa key");
    let pos_mm = sa.find(r#""mm":"3""#).expect("mm key");
    let pos_zz = sa.find(r#""zz":"1""#).expect("zz key");
    assert!(
        pos_aa < pos_mm && pos_mm < pos_zz,
        "metadata 键序必须字典序（jsonl 落盘字节确定性）"
    );
}
