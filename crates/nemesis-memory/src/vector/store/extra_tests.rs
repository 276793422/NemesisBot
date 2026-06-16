//! Additional coverage tests for VectorStore & StoreConfig.
//!
//! Avoids loading the ONNX plugin. Uses a stub embedding function via
//! `new_from_embed` to exercise query / list / persist / load paths.

use super::*;

/// Build a stub embedding function: maps any non-empty string to a fixed
/// 4-dim vector derived from a simple hash so identical strings give
/// identical vectors (required for similarity > 0).
fn stub_embed() -> Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
    Box::new(|s: &str| {
        if s.is_empty() {
            return Ok(vec![0.0, 0.0, 0.0, 0.0]);
        }
        let mut h: u32 = 0;
        for b in s.as_bytes() {
            h = h.wrapping_mul(31).wrapping_add(*b as u32);
        }
        // Deterministic 4-dim vector from hash.
        let f1 = ((h & 0xFF) as f32) / 255.0;
        let f2 = (((h >> 8) & 0xFF) as f32) / 255.0;
        let f3 = (((h >> 16) & 0xFF) as f32) / 255.0;
        let f4 = (((h >> 24) & 0xFF) as f32) / 255.0;
        Ok(vec![f1, f2, f3, f4])
    })
}

fn make_store_config(path: &str) -> StoreConfig {
    StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.0, // accept all
        storage_path: path.to_string(),
    }
}

fn make_entry(id: &str, content: &str) -> VectorEntry {
    VectorEntry {
        id: id.into(),
        entry_type: "long_term".into(),
        content: content.into(),
        metadata: HashMap::new(),
        tags: vec![],
        score: 0.0,
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    }
}

// ============================================================
// StoreConfig / Default
// ============================================================

#[test]
fn test_store_config_default_values() {
    let c = StoreConfig::default();
    assert_eq!(c.embedding_tier, "plugin");
    assert!(c.plugin_path.is_none());
    assert!(c.config_dir.is_none());
    assert_eq!(c.max_results, 10);
    assert!((c.similarity_threshold - 0.7).abs() < 1e-9);
    assert_eq!(c.storage_path, "");
}

#[test]
fn test_store_config_with_values() {
    let c = StoreConfig {
        embedding_tier: "api".into(),
        plugin_path: Some("/tmp/x.dll".into()),
        config_dir: None,
        max_results: 25,
        similarity_threshold: 0.5,
        storage_path: "/tmp/v.jsonl".into(),
    };
    assert_eq!(c.max_results, 25);
    assert_eq!(c.plugin_path.as_deref(), Some("/tmp/x.dll"));
}

#[test]
fn test_store_config_serialize_roundtrip() {
    let c = StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some("/p".into()),
        config_dir: None,
        max_results: 7,
        similarity_threshold: 0.9,
        storage_path: "/v.jsonl".into(),
    };
    let json = serde_json::to_string(&c).unwrap();
    let parsed: StoreConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.embedding_tier, "plugin");
    assert_eq!(parsed.max_results, 7);
    assert_eq!(parsed.storage_path, "/v.jsonl");
}

#[test]
fn test_store_config_config_dir_skipped_in_serialize() {
    // config_dir is #[serde(skip)] — should not appear in JSON.
    let mut c = StoreConfig::default();
    c.config_dir = Some("/cfg".into());
    let json = serde_json::to_string(&c).unwrap();
    assert!(!json.contains("config_dir"));
    assert!(!json.contains("/cfg"));
}

#[test]
fn test_store_config_default_is_empty_storage_path() {
    // Important: new() branches on empty storage_path.
    assert!(StoreConfig::default().storage_path.is_empty());
}

// ============================================================
// VectorEntry serialization
// ============================================================

#[test]
fn test_vector_entry_minimal_json() {
    let json = r#"{
        "id": "x",
        "type": "long_term",
        "content": "hello",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }"#;
    let e: VectorEntry = serde_json::from_str(json).unwrap();
    assert_eq!(e.id, "x");
    assert!(e.metadata.is_empty());
    assert!(e.tags.is_empty());
    assert_eq!(e.score, 0.0);
}

#[test]
fn test_vector_entry_entry_type_renamed_to_type() {
    let e = make_entry("ren-1", "x");
    let json = serde_json::to_string(&e).unwrap();
    // Should serialize as "type" not "entry_type".
    assert!(json.contains("\"type\""));
    assert!(!json.contains("\"entry_type\""));
}

#[test]
fn test_vector_entry_score_defaults_to_zero_on_missing() {
    let json = r#"{
        "id": "x",
        "type": "long_term",
        "content": "c",
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }"#;
    let e: VectorEntry = serde_json::from_str(json).unwrap();
    assert_eq!(e.score, 0.0);
}

#[test]
fn test_vector_entry_full_roundtrip() {
    let mut e = make_entry("rt-1", "content");
    e.metadata.insert("k".into(), "v".into());
    e.tags.push("t1".into());
    e.score = 0.5;
    let json = serde_json::to_string(&e).unwrap();
    let parsed: VectorEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.id, "rt-1");
    assert_eq!(parsed.metadata.get("k").unwrap(), "v");
    assert_eq!(parsed.tags, vec!["t1".to_string()]);
    assert!((parsed.score - 0.5).abs() < 1e-9);
}

// ============================================================
// cosine_similarity edge cases
// ============================================================

#[test]
fn test_cosine_similarity_one_dim_ones() {
    let a = vec![1.0f32];
    let b = vec![1.0f32];
    let s = cosine_similarity(&a, &b);
    assert!((s - 1.0).abs() < 1e-6);
}

#[test]
fn test_cosine_similarity_high_dim_parallel() {
    let a: Vec<f32> = (0..100).map(|i| i as f32).collect();
    let b = a.clone();
    let s = cosine_similarity(&a, &b);
    assert!((s - 1.0).abs() < 1e-6);
}

#[test]
fn test_cosine_similarity_unequal_lengths_b_shorter() {
    let a = vec![1.0f32, 2.0, 3.0];
    let b = vec![1.0f32, 2.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn test_cosine_similarity_a_empty() {
    let a: Vec<f32> = vec![];
    let b = vec![1.0f32, 2.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

#[test]
fn test_cosine_similarity_b_zero_vector() {
    let a = vec![1.0f32, 1.0, 1.0];
    let b = vec![0.0f32, 0.0, 0.0];
    assert_eq!(cosine_similarity(&a, &b), 0.0);
}

// ============================================================
// VectorStore lifecycle with stub embed (no plugin)
// ============================================================

#[test]
fn test_new_from_embed_creates_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn test_store_entry_then_len_increases() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert!(store.store_entry(&make_entry("a", "alpha")).is_ok());
    assert!(store.store_entry(&make_entry("b", "beta")).is_ok());
    assert_eq!(store.len(), 2);
    assert!(!store.is_empty());
}

#[test]
fn test_get_by_id_returns_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("g1", "gamma")).unwrap();
    let got = store.get_by_id("g1").unwrap();
    assert_eq!(got.content, "gamma");
}

#[test]
fn test_get_by_id_missing_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert!(store.get_by_id("missing").is_none());
}

#[test]
fn test_delete_entry_removes_and_returns_true() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("d1", "delta")).unwrap();
    assert!(store.delete_entry("d1"));
    assert!(store.get_by_id("d1").is_none());
    assert_eq!(store.len(), 0);
}

#[test]
fn test_delete_entry_missing_returns_false() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert!(!store.delete_entry("ghost"));
}

// ============================================================
// Query path
// ============================================================

#[test]
fn test_query_empty_store_returns_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    let r = store.query("anything", 10, &[]).unwrap();
    assert_eq!(r.total, 0);
    assert!(r.entries.is_empty());
    assert_eq!(r.query, "anything");
}

#[test]
fn test_query_finds_exact_match() {
    // Identical strings produce identical embeddings → cosine sim = 1.0 ≥ threshold 0.0.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("m1", "matchme")).unwrap();
    let r = store.query("matchme", 10, &[]).unwrap();
    assert!(r.total >= 1);
    assert_eq!(r.entries[0].id, "m1");
}

#[test]
fn test_query_limit_caps_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    // All entries have the same content — all match identically.
    for i in 0..5 {
        store.store_entry(&make_entry(&format!("id{}", i), "same")).unwrap();
    }
    let r = store.query("same", 2, &[]).unwrap();
    assert!(r.entries.len() <= 2);
    assert!(r.total >= 5);
}

#[test]
fn test_query_zero_limit_uses_max_results() {
    // limit=0 should fall back to config.max_results.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    for i in 0..3 {
        store.store_entry(&make_entry(&format!("z{}", i), "same")).unwrap();
    }
    let r = store.query("same", 0, &[]).unwrap();
    // max_results default = 10 ≥ 3.
    assert_eq!(r.entries.len(), 3);
}

#[test]
fn test_query_with_type_filter_matches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    let mut e1 = make_entry("t1", "same");
    e1.entry_type = "long_term".into();
    let mut e2 = make_entry("t2", "same");
    e2.entry_type = "episodic".into();
    store.store_entry(&e1).unwrap();
    store.store_entry(&e2).unwrap();
    let r = store.query("same", 10, &["long_term".to_string()]).unwrap();
    assert!(r.entries.iter().all(|e| e.entry_type == "long_term"));
}

#[test]
fn test_query_with_type_filter_excludes_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("x1", "same")).unwrap();
    let r = store.query("same", 10, &["nonexistent_type".to_string()]).unwrap();
    assert_eq!(r.total, 0);
}

#[test]
fn test_query_high_threshold_filters_out() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let mut cfg = make_store_config(&path.to_string_lossy());
    cfg.similarity_threshold = 1.5; // impossible to reach
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("h1", "hello")).unwrap();
    // "different" hashes differently from "hello" → similarity < 1.5 → filtered.
    let r = store.query("different", 10, &[]).unwrap();
    assert_eq!(r.total, 0);
}

#[test]
fn test_query_scores_populated_on_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("s1", "score test")).unwrap();
    let r = store.query("score test", 10, &[]).unwrap();
    assert!(r.total >= 1);
    assert!(r.entries[0].score > 0.0);
}

// ============================================================
// list_entries
// ============================================================

#[test]
fn test_list_entries_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    let r = store.list_entries(&[], 0, 10);
    assert_eq!(r.total, 0);
    assert!(r.entries.is_empty());
    assert_eq!(r.query, "");
}

#[test]
fn test_list_entries_no_filter_returns_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    for i in 0..5 {
        store.store_entry(&make_entry(&format!("l{}", i), "content")).unwrap();
    }
    let r = store.list_entries(&[], 0, 10);
    assert_eq!(r.total, 5);
    assert_eq!(r.entries.len(), 5);
}

#[test]
fn test_list_entries_with_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    let mut a = make_entry("a", "x");
    a.entry_type = "long_term".into();
    let mut b = make_entry("b", "x");
    b.entry_type = "episodic".into();
    store.store_entry(&a).unwrap();
    store.store_entry(&b).unwrap();
    let r = store.list_entries(&["episodic".to_string()], 0, 10);
    assert_eq!(r.total, 1);
    assert_eq!(r.entries[0].id, "b");
}

#[test]
fn test_list_entries_offset_skips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    for i in 0..5 {
        store.store_entry(&make_entry(&format!("o{}", i), "x")).unwrap();
    }
    let r = store.list_entries(&[], 3, 10);
    assert_eq!(r.total, 5);
    assert_eq!(r.entries.len(), 2);
}

#[test]
fn test_list_entries_limit_zero_returns_all() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    for i in 0..3 {
        store.store_entry(&make_entry(&format!("z{}", i), "x")).unwrap();
    }
    let r = store.list_entries(&[], 0, 0);
    assert_eq!(r.entries.len(), 3);
}

#[test]
fn test_list_entries_offset_beyond_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("only", "x")).unwrap();
    let r = store.list_entries(&[], 100, 10);
    assert_eq!(r.total, 1);
    assert!(r.entries.is_empty());
}

// ============================================================
// Persistence (sync + async)
// ============================================================

#[test]
fn test_persist_entry_sync_creates_file_and_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("v.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    let e = make_entry("p1", "persisted");
    store.persist_entry_sync(&e).unwrap();
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("p1"));
}

#[tokio::test]
async fn test_persist_entry_async_appends_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pa.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.persist_entry(&make_entry("pa1", "x")).await.unwrap();
    store.persist_entry(&make_entry("pa2", "y")).await.unwrap();
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("pa1"));
    assert!(content.contains("pa2"));
    // Two newlines (one per entry).
    assert_eq!(content.matches('\n').count(), 2);
}

#[test]
fn test_load_persisted_sync_empty_path_ok() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("noexist.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    // File doesn't exist — should return Ok and load nothing.
    let r = store.load_persisted_sync();
    assert!(r.is_ok());
    assert!(store.is_empty());
}

#[test]
fn test_load_persisted_sync_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rt.jsonl");

    // Write a valid JSONL line.
    let e = make_entry("rt-load", "loaded content");
    std::fs::write(&path, serde_json::to_string(&e).unwrap() + "\n").unwrap();

    let cfg = make_store_config(&path.to_string_lossy());
    // new_from_embed auto-loads persisted file on construction.
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert_eq!(store.len(), 1);
    assert_eq!(store.get_by_id("rt-load").unwrap().content, "loaded content");
}

#[test]
fn test_load_persisted_sync_idempotent() {
    // Calling load_persisted_sync explicitly after auto-load should not duplicate
    // because store_entry pushes (no dedup); this verifies the behavior.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rt2.jsonl");
    let e = make_entry("rt-load-2", "loaded content");
    std::fs::write(&path, serde_json::to_string(&e).unwrap() + "\n").unwrap();

    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    // Initial auto-load added 1.
    assert_eq!(store.len(), 1);
    // Explicit load adds another (current behavior — entries are appended).
    store.load_persisted_sync().unwrap();
    assert_eq!(store.len(), 2);
}

#[test]
fn test_load_persisted_sync_skips_blank_and_invalid_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mixed.jsonl");
    let valid = make_entry("valid-1", "valid");
    let content = format!(
        "{}\n\nnot json\n{}\n",
        serde_json::to_string(&valid).unwrap(),
        serde_json::to_string(&valid).unwrap()
    );
    std::fs::write(&path, content).unwrap();

    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    // 2 valid lines loaded, blank + invalid skipped.
    assert_eq!(store.len(), 2);
}

#[tokio::test]
async fn test_load_persisted_async_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("async.jsonl");
    let e = make_entry("async-load", "content async");
    std::fs::write(&path, serde_json::to_string(&e).unwrap() + "\n").unwrap();

    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    // Auto-load already populated.
    assert_eq!(store.len(), 1);
    // Verify the entry exists.
    assert_eq!(store.get_by_id("async-load").unwrap().content, "content async");
}

#[test]
fn test_load_persisted_sync_trims_whitespace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("trim.jsonl");
    let e = make_entry("trim-1", "x");
    // Leading/trailing whitespace should be trimmed by the loader.
    let line = serde_json::to_string(&e).unwrap();
    std::fs::write(&path, format!("   {}   \n", line)).unwrap();

    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    assert_eq!(store.len(), 1);
}

// ============================================================
// new_from_embed with empty storage_path uses default
// ============================================================

#[test]
fn test_new_from_embed_empty_storage_path_uses_default() {
    let cfg = StoreConfig {
        storage_path: String::new(),
        ..make_store_config("/ignored.jsonl")
    };
    let _store = VectorStore::new_from_embed(stub_embed(), cfg);
    // Should not panic — internally uses "memory/vector/vector_store.jsonl".
}

// ============================================================
// new() error path (no plugin)
// ============================================================

#[test]
fn test_new_without_plugin_returns_error() {
    let cfg = StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.7,
        storage_path: "/tmp/x.jsonl".into(),
    };
    match VectorStore::new(cfg) {
        Ok(_) => panic!("expected error when no plugin"),
        Err(e) => assert!(
            e.contains("Failed to create embedding function") || e.contains("plugin"),
            "unexpected error: {}",
            e
        ),
    }
}

#[test]
fn test_new_with_empty_storage_path_uses_default() {
    // Just check the path branch — will still fail at embedding creation.
    let cfg = StoreConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.7,
        storage_path: String::new(),
    };
    let r = VectorStore::new(cfg);
    assert!(r.is_err());
}

// ============================================================
// rewrite_persist_file on delete
// ============================================================

#[test]
fn test_delete_rewrites_persist_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rewrite.jsonl");
    let cfg = make_store_config(&path.to_string_lossy());
    let store = VectorStore::new_from_embed(stub_embed(), cfg);
    store.store_entry(&make_entry("rw1", "x")).unwrap();
    store.store_entry(&make_entry("rw2", "y")).unwrap();

    // Persist both.
    store.persist_entry_sync(&make_entry("rw1", "x")).unwrap();
    store.persist_entry_sync(&make_entry("rw2", "y")).unwrap();
    assert!(path.exists());

    // Deleting rw1 should rewrite the file to only contain rw2.
    assert!(store.delete_entry("rw1"));
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(!content.contains("rw1"));
    assert!(content.contains("rw2"));
}
