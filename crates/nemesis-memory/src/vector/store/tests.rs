use super::*;

fn make_entry(id: &str, content: &str) -> VectorEntry {
    VectorEntry {
        id: id.into(),
        entry_type: "long_term".into(),
        content: content.into(),
        metadata: HashMap::new(),
        tags: vec![],
        score: 0.0,
        created_at: chrono::Local::now().to_rfc3339(),
        updated_at: chrono::Local::now().to_rfc3339(),
    }
}

// ============================================================
// Config and serialization tests (no plugin needed)
// ============================================================

#[test]
fn test_store_config_default() {
    let config = StoreConfig::default();
    assert_eq!(config.embedding_tier, "plugin");
    assert!(config.plugin_path.is_none());
    assert_eq!(config.max_results, 10);
    assert!((config.similarity_threshold - 0.7).abs() < f64::EPSILON);
}

#[test]
fn test_vector_entry_serialization() {
    let entry = make_entry("test-ser", "serialize me");
    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, "test-ser");
    assert_eq!(deserialized.content, "serialize me");
    assert_eq!(deserialized.entry_type, "long_term");
}

#[test]
fn test_vector_entry_with_metadata() {
    let mut entry = make_entry("meta-1", "with metadata");
    entry.metadata.insert("source".into(), "test".into());
    entry.metadata.insert("count".into(), "42".into());

    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.metadata.get("source").unwrap(), "test");
    assert_eq!(deserialized.metadata.get("count").unwrap(), "42");
}

#[test]
fn test_vector_entry_with_tags() {
    let mut entry = make_entry("tag-1", "tagged entry");
    entry.tags.push("important".into());
    entry.tags.push("review".into());

    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.tags, vec!["important", "review"]);
}

#[test]
fn test_cosine_similarity_same_vector() {
    let a = vec![1.0f32, 0.0, 0.0];
    let sim = cosine_similarity(&a, &a);
    assert!((sim - 1.0).abs() < 0.01);
}

#[test]
fn test_cosine_similarity_opposite_vectors() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![-1.0f32, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - (-1.0)).abs() < 0.01);
}

#[test]
fn test_cosine_similarity_orthogonal_vectors() {
    let a = vec![1.0f32, 0.0, 0.0];
    let b = vec![0.0f32, 1.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert!((sim - 0.0).abs() < 0.01);
}

#[test]
fn test_cosine_similarity_different_lengths() {
    let a = vec![1.0f32, 0.0];
    let b = vec![1.0f32, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

#[test]
fn test_cosine_similarity_empty_vectors() {
    let a: Vec<f32> = vec![];
    let b: Vec<f32> = vec![];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

#[test]
fn test_cosine_similarity_zero_vectors() {
    let a = vec![0.0f32, 0.0, 0.0];
    let b = vec![1.0f32, 0.0, 0.0];
    let sim = cosine_similarity(&a, &b);
    assert_eq!(sim, 0.0);
}

// ============================================================
// System Tests — VectorStore with real ONNX plugin
//
// ONNX Runtime cannot safely re-init after free, so all
// scenarios run inside a single test with one VectorStore
// lifecycle. The plugin store is created once, all scenarios
// are executed sequentially, then dropped at the end.
//
// Requires:
//   1. plugin_onnx.dll: cd plugins/plugin-onnx && cargo build --release
//   2. Test model: bash test-tools/plugin-onnx-test/scripts/setup-test.sh
//
// Run with:
//   cargo test -p nemesis-memory -- --ignored --test-threads=1
// ============================================================

#[test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM--v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
fn st_plugin_system_test_all_scenarios() {
    // Use shared plugin fixture — creates VectorStore without loading a new plugin
    let embed =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let store_config = crate::vector::test_fixture::plugin_store_config("")
        .expect("plugin DLL + model files required");
    let store = VectorStore::new_from_embed(embed, store_config);

    // === Scenario 1: Store creates and is empty ===
    {
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        println!("[P2] Scenario 1: Store creates empty — PASS");
    }

    // === Scenario 2: Single entry store ===
    {
        store
            .store_entry(&make_entry(
                "s2-1",
                "The quick brown fox jumps over the lazy dog",
            ))
            .unwrap();
        assert_eq!(store.len(), 1);
        println!("[P2] Scenario 2: Single entry store — PASS");
    }

    // Clear for next scenarios
    store.delete_entry("s2-1");
    assert!(store.is_empty());

    // === Scenario 3: Basic store + query with semantic ranking ===
    {
        store
            .store_entry(&make_entry(
                "s3-1",
                "Cats are independent animals that like to explore",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s3-2",
                "Dogs are loyal companions that love to play fetch",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s3-3",
                "The stock market showed mixed results today",
            ))
            .unwrap();

        let result = store.query("feline pets", 10, &[]).unwrap();
        assert!(
            result.total >= 1,
            "Expected at least 1 result, got {}",
            result.total
        );
        assert_eq!(
            result.entries[0].id, "s3-1",
            "Cat entry should be top result for 'feline pets'"
        );
        println!("[P2] Scenario 3: Basic query with semantic ranking — PASS");
    }

    // Clear
    for id in &["s3-1", "s3-2", "s3-3"] {
        store.delete_entry(id);
    }

    // === Scenario 4: Semantic ranking of diverse topics ===
    {
        store
            .store_entry(&make_entry(
                "s4-1",
                "Python is a popular programming language for data science",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s4-2",
                "Java is widely used for enterprise applications",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s4-3",
                "Bananas are a good source of potassium",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s4-4",
                "Machine learning models require training data",
            ))
            .unwrap();

        let result = store
            .query("software development and coding", 10, &[])
            .unwrap();
        assert!(
            result.total >= 2,
            "Expected at least 2 results, got {}",
            result.total
        );

        let ids: Vec<&str> = result.entries.iter().map(|e| e.id.as_str()).collect();
        let python_pos = ids.iter().position(|&id| id == "s4-1");
        let banana_pos = ids.iter().position(|&id| id == "s4-3");
        // If both are present, python should rank higher
        if let (Some(pp), Some(bp)) = (python_pos, banana_pos) {
            assert!(
                pp < bp,
                "Python entry should rank higher than banana for 'software development'"
            );
        }
        // Python should always be in results
        assert!(
            python_pos.is_some(),
            "Python entry should be in results for 'software development'"
        );
        println!("[P2] Scenario 4: Semantic ranking — PASS");
    }

    // Clear
    for id in &["s4-1", "s4-2", "s4-3", "s4-4"] {
        store.delete_entry(id);
    }

    // === Scenario 5: Similarity scores are valid ===
    {
        store
            .store_entry(&make_entry(
                "s5-1",
                "Machine learning is a subset of artificial intelligence",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s5-2",
                "Neural networks are inspired by the human brain",
            ))
            .unwrap();

        let result = store.query("AI and deep learning", 10, &[]).unwrap();
        assert!(result.total >= 1);
        for entry in &result.entries {
            assert!(entry.score > 0.0, "Score should be positive");
            assert!(
                entry.score <= 1.0,
                "Score should not exceed 1.0, got {}",
                entry.score
            );
        }
        println!("[P2] Scenario 5: Similarity scores valid — PASS");
    }

    for id in &["s5-1", "s5-2"] {
        store.delete_entry(id);
    }

    // === Scenario 6: Query with type filter ===
    {
        let mut e1 = make_entry("s6-1", "Important meeting about project timeline");
        e1.entry_type = "long_term".into();
        let mut e2 = make_entry("s6-2", "Meeting notes from standup");
        e2.entry_type = "episodic".into();
        let mut e3 = make_entry("s6-3", "Project deadline is next Friday");
        e3.entry_type = "long_term".into();

        store.store_entry(&e1).unwrap();
        store.store_entry(&e2).unwrap();
        store.store_entry(&e3).unwrap();

        let result = store
            .query("project meeting", 10, &["long_term".to_string()])
            .unwrap();
        assert!(
            result.entries.iter().all(|e| e.entry_type == "long_term"),
            "All results should be long_term type"
        );
        println!("[P2] Scenario 6: Type filter — PASS");
    }

    for id in &["s6-1", "s6-2", "s6-3"] {
        store.delete_entry(id);
    }

    // === Scenario 7: Query consistency (deterministic results) ===
    {
        store
            .store_entry(&make_entry("s7-1", "The weather is sunny and warm today"))
            .unwrap();
        store
            .store_entry(&make_entry("s7-2", "Programming in Rust is fun and safe"))
            .unwrap();

        let r1 = store.query("climate and sunshine", 10, &[]).unwrap();
        let r2 = store.query("climate and sunshine", 10, &[]).unwrap();

        assert_eq!(r1.total, r2.total, "Same query should return same count");
        for (a, b) in r1.entries.iter().zip(r2.entries.iter()) {
            assert_eq!(a.id, b.id, "Same query should return same entries");
            assert!(
                (a.score - b.score).abs() < 1e-6,
                "Same query should return same scores"
            );
        }
        println!("[P2] Scenario 7: Query consistency — PASS");
    }

    for id in &["s7-1", "s7-2"] {
        store.delete_entry(id);
    }

    // === Scenario 8: CRUD lifecycle ===
    {
        store
            .store_entry(&make_entry("s8-1", "First entry to test CRUD"))
            .unwrap();
        store
            .store_entry(&make_entry("s8-2", "Second entry for CRUD test"))
            .unwrap();
        assert_eq!(store.len(), 2);

        let entry = store.get_by_id("s8-1").unwrap();
        assert_eq!(entry.content, "First entry to test CRUD");

        assert!(store.delete_entry("s8-1"));
        assert_eq!(store.len(), 1);
        assert!(store.get_by_id("s8-1").is_none());

        let result = store.query("CRUD test", 10, &[]).unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.entries[0].id, "s8-2");
        println!("[P2] Scenario 8: CRUD lifecycle — PASS");
    }

    store.delete_entry("s8-2");

    // === Scenario 9: Plugin produces valid embeddings ===
    {
        store
            .store_entry(&make_entry("s9-1", "The cat sat on the mat"))
            .unwrap();
        let result = store.query("cat", 10, &[]).unwrap();
        assert!(
            result.total >= 1,
            "Plugin store should find results for 'cat'"
        );

        // Verify scores are valid
        for entry in &result.entries {
            assert!(entry.score > 0.0, "Score should be positive");
            assert!(entry.score <= 1.0, "Score should not exceed 1.0");
        }

        println!("[P2] Scenario 9: Plugin embeddings valid — PASS");
    }

    store.delete_entry("s9-1");

    // === Scenario 10: Semantic similarity with lexical variation ===
    {
        store
            .store_entry(&make_entry(
                "s10-1",
                "The automobile was traveling at high speed",
            ))
            .unwrap();
        store
            .store_entry(&make_entry("s10-2", "The vehicle was moving very fast"))
            .unwrap();
        store
            .store_entry(&make_entry("s10-3", "I enjoy cooking pasta for dinner"))
            .unwrap();

        let result = store.query("a car going quickly", 10, &[]).unwrap();

        // Car/speed entries should rank above cooking
        let ids: Vec<&str> = result.entries.iter().map(|e| e.id.as_str()).collect();
        let s3_pos = ids.iter().position(|&id| id == "s10-3");
        if let Some(pos) = s3_pos {
            let s1_pos = ids.iter().position(|&id| id == "s10-1").unwrap_or(99);
            let s2_pos = ids.iter().position(|&id| id == "s10-2").unwrap_or(99);
            assert!(
                s1_pos < pos && s2_pos < pos,
                "Car/speed entries should rank above cooking"
            );
        }

        // Both car entries should have meaningful similarity
        let p_s1 = result
            .entries
            .iter()
            .find(|e| e.id == "s10-1")
            .map(|e| e.score)
            .unwrap_or(0.0);
        let p_s2 = result
            .entries
            .iter()
            .find(|e| e.id == "s10-2")
            .map(|e| e.score)
            .unwrap_or(0.0);
        assert!(p_s1 > 0.3, "s1 should have meaningful similarity: {}", p_s1);
        assert!(p_s2 > 0.3, "s2 should have meaningful similarity: {}", p_s2);
        println!("[P2] Scenario 10: Semantic similarity with lexical variation — PASS");
    }

    for i in 1..=3 {
        store.delete_entry(&format!("s10-{}", i));
    }

    // === Scenario 11: Embed dimension matches config ===
    {
        store
            .store_entry(&make_entry("s11-1", "Dimension verification test"))
            .unwrap();
        let result = store.query("test", 10, &[]).unwrap();
        assert!(
            result.total >= 1,
            "Query should work with correct dimensions"
        );
        println!("[P2] Scenario 11: Embed dimension matches — PASS");
    }

    store.delete_entry("s11-1");

    // === Scenario 12: Large batch entries ===
    {
        for i in 0..20 {
            store
                .store_entry(&make_entry(
                    &format!("s12-{}", i),
                    &format!("Entry number {} about topic {}", i, i % 5),
                ))
                .unwrap();
        }
        assert_eq!(store.len(), 20);

        let result = store.query("topic 0", 10, &[]).unwrap();
        assert!(result.total >= 1, "Should find entries about topic 0");
        let top_ids: Vec<&str> = result
            .entries
            .iter()
            .take(4)
            .map(|e| e.id.as_str())
            .collect();
        assert!(
            top_ids.iter().any(|id| *id == "s12-0" || *id == "s12-5"),
            "Topic 0 entries should appear in top results"
        );
        println!("[P2] Scenario 12: Large batch (20 entries) — PASS");
    }

    for i in 0..20 {
        store.delete_entry(&format!("s12-{}", i));
    }

    // === Scenario 13: Multiple sequential queries produce stable results ===
    {
        store
            .store_entry(&make_entry(
                "s13-1",
                "Artificial intelligence is transforming technology",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s13-2",
                "Cooking recipes from around the world",
            ))
            .unwrap();
        store
            .store_entry(&make_entry(
                "s13-3",
                "Space exploration and Mars colonization",
            ))
            .unwrap();

        // Run 5 queries in sequence
        for _ in 0..5 {
            let r = store.query("AI and computers", 10, &[]).unwrap();
            assert!(r.total >= 1);
            assert_eq!(
                r.entries[0].id, "s13-1",
                "AI entry should consistently rank first"
            );
        }
        println!("[P2] Scenario 13: Sequential query stability — PASS");
    }

    // Store is dropped here — but shared plugin keeps running
    println!("[P2] All 13 scenarios PASSED");
}

#[tokio::test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM--v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
async fn st_plugin_persistence_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("plugin_vectors.jsonl");

    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..crate::vector::test_fixture::plugin_store_config("")
            .expect("plugin DLL + model files required")
    };

    // Store and persist (using shared plugin fixture)
    let embed1 =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let store = VectorStore::new_from_embed(embed1, config.clone());
    let e1 = make_entry("st-persist-1", "Persistent entry about machine learning");
    let e2 = make_entry(
        "st-persist-2",
        "Another entry about natural language processing",
    );
    store.store_entry(&e1).unwrap();
    store.store_entry(&e2).unwrap();
    store.persist_entry(&e1).await.unwrap();
    store.persist_entry(&e2).await.unwrap();
    assert_eq!(store.len(), 2);
    drop(store); // Drop VectorStore, shared plugin keeps running

    // Load into new store (using same shared plugin)
    let embed2 =
        crate::vector::test_fixture::shared_embed_func().expect("shared plugin not available");
    let store2 = VectorStore::new_from_embed(embed2, config);
    store2.load_persisted().await.unwrap();
    assert_eq!(store2.len(), 2, "Should load 2 persisted entries");

    let result = store2.query("AI and ML", 10, &[]).unwrap();
    assert!(result.total >= 1, "Should find results in loaded store");
    println!("[P2] Persistence roundtrip — PASS");
}

// ============================================================
// coverage: offline constructor paths (no plugin needed)
// ============================================================

/// VectorStore::new without any configured embedding backend must fail with
/// the combined guidance message (empty storage_path keeps the built-in
/// default persist-path arm; empty plugin_path trips the embedding factory's
/// first guard).
#[test]
fn new_without_plugin_reports_embedding_guidance_error() {
    let config = StoreConfig {
        ..Default::default()
    };
    let err = match VectorStore::new(config) {
        Err(e) => e,
        Ok(_) => panic!("constructor must fail without any plugin configured"),
    };
    assert!(
        err.contains("Failed to create embedding function"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("No plugin path configured"),
        "unexpected error: {err}"
    );
}

/// With an explicit (non-empty) storage_path the constructor takes the custom
/// persist-path arm before failing on the absent plugin DLL.
#[test]
fn new_with_explicit_storage_path_still_requires_plugin() {
    let dir = tempfile::tempdir().unwrap();
    let bogus_dll = dir.path().join("definitely-missing.dll");
    let config = StoreConfig {
        storage_path: dir
            .path()
            .join("vector_store.jsonl")
            .to_string_lossy()
            .to_string(),
        plugin_path: Some(bogus_dll.to_string_lossy().to_string()),
        ..Default::default()
    };
    let err = match VectorStore::new(config) {
        Err(e) => e,
        Ok(_) => panic!("constructor must fail when the plugin dll is absent"),
    };
    assert!(
        err.contains("Plugin DLL not found"),
        "unexpected error: {err}"
    );
    assert!(
        err.contains("Failed to create embedding function"),
        "unexpected error: {err}"
    );
}

// rewrite_persist_file 走唯一临时名权威 helper。旧固定
// `<persist>.jsonl.tmp` 被残留垃圾（目录形态最顽固）占用时全静默失败，
// 活文件永不重写。预种活文件 + 在旧固定名上种目录：旧实现必红（删除未
// 落盘，条目原样留在盘上），新实现经唯一临时名照常重写。
#[test]
fn bug59_rewrite_persist_survives_residue_at_fixed_tmp_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vector_store.jsonl");
    std::fs::create_dir(dir.path().join("vector_store.jsonl.tmp")).unwrap();

    let seeded = make_entry("bug59-1", "residue survivor entry");
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&seeded).unwrap()),
    )
    .unwrap();

    let embed: crate::vector::EmbeddingFunc = Box::new(|_: &str| Ok(vec![0.0; 4]));
    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = VectorStore::new_from_embed(embed, config);
    assert!(store.delete_entry("bug59-1"), "seeded entry must be found");

    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        !raw.contains("bug59-1"),
        "rewrite must be applied (old fixed-tmp shape silently skipped it): {raw:?}"
    );
}

// ============================================================
// BUG-20261112-087 回归锁：持久层加载失败/单条失败的数据保全
// ============================================================

// M1：持久文件整体不可读（非 UTF-8 → read_to_string Err）→ load 置降级
// 旗标 → 此后 rewrite 冻结全量覆写。旧实现：空索引照常覆写 = 磁盘历史
// 整体覆没（数据销毁）。断言：改写请求后磁盘字节逐位不变。
#[test]
fn bug87_load_failure_freezes_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vector_store.jsonl");
    let poisoned: &[u8] = &[0xFF, 0xFE, 0x80, 0x00, 0xC3];
    std::fs::write(&path, poisoned).unwrap();

    let embed: crate::vector::EmbeddingFunc = Box::new(|_: &str| Ok(vec![0.0; 4]));
    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..Default::default()
    };
    // new_from_embed 内部 load_persisted_sync：读失败 → 置旗标（错误被
    // 吞掉是构造器旧姿态，旗标是本轮新增的降级闸）。
    let store = VectorStore::new_from_embed(embed, config);

    // 内存操作照常：进一条 → 删掉 → 触发 rewrite（被旗标拦）。
    store
        .store_entry(&make_entry("bug87-1", "in-memory only entry"))
        .unwrap();
    assert!(store.delete_entry("bug87-1"), "in-memory entry must delete");

    let now = std::fs::read(&path).unwrap();
    assert_eq!(
        now, poisoned,
        "rewrite must be frozen after failed load (old shape destroyed on-disk history)"
    );
}

// M2 端到端：单条 embed 失败 → 原始行留档；无关条目的改写把留档行写回
// （不静默抹掉）。旧实现：失败条目被逐出索引，一次无关 rewrite 后从
// 磁盘永久消失。
#[test]
fn bug87_failed_embed_line_survives_unrelated_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vector_store.jsonl");
    let good = make_entry("bug87-good", "healthy entry about rust");
    let bad = make_entry("bug87-bad", "EMBED_FAIL poisoned content");
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&good).unwrap(),
            serde_json::to_string(&bad).unwrap()
        ),
    )
    .unwrap();

    let embed: crate::vector::EmbeddingFunc = Box::new(|c: &str| {
        if c.contains("EMBED_FAIL") {
            Err("embed boom".into())
        } else {
            Ok(vec![0.0; 4])
        }
    });
    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = VectorStore::new_from_embed(embed, config);
    assert_eq!(store.len(), 1, "only the embedable entry is indexed");

    // 无关改写：进一条新条目再删掉 → rewrite 发生。
    store
        .store_entry(&make_entry("bug87-tmp", "temporary entry"))
        .unwrap();
    assert!(store.delete_entry("bug87-tmp"));

    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        raw.contains("bug87-bad"),
        "embed-failed line must survive unrelated rewrites (old shape dropped it from disk): {raw:?}"
    );
    assert!(
        raw.contains("bug87-good"),
        "indexed entry must persist through rewrite: {raw:?}"
    );
    assert!(
        !raw.contains("bug87-tmp"),
        "deleted entry must be gone: {raw:?}"
    );
}

// M2 delete 面：留档行按 id 同步摘除——删除留档条目后 rewrite 不把
// 原始行写回（否则删除悄悄失效，条目复活）。
#[test]
fn bug87_delete_removes_unloaded_line_by_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vector_store.jsonl");
    let good = make_entry("bug87-good", "healthy entry about rust");
    let bad = make_entry("bug87-bad", "EMBED_FAIL poisoned content");
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&good).unwrap(),
            serde_json::to_string(&bad).unwrap()
        ),
    )
    .unwrap();

    let embed: crate::vector::EmbeddingFunc = Box::new(|c: &str| {
        if c.contains("EMBED_FAIL") {
            Err("embed boom".into())
        } else {
            Ok(vec![0.0; 4])
        }
    });
    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = VectorStore::new_from_embed(embed, config);

    // 删除留档条目（未入索引）：必须报命中并从磁盘摘除。
    assert!(
        store.delete_entry("bug87-bad"),
        "unloaded (kept-on-disk) entry must be deletable by id"
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(
        !raw.contains("bug87-bad"),
        "deleted unloaded entry must not resurrect via kept raw line: {raw:?}"
    );
    assert!(
        raw.contains("bug87-good"),
        "unrelated entry must survive: {raw:?}"
    );
    assert_eq!(store.len(), 1);
}

// M3（BUG-20261112-087）证伪钉子：子代理报告「嵌入模型切换维度不配 →
// cosine 恒 0.0 静默全废」——机制描述与代码结构吻合但运行时不可达：
// VectorEntry 不持久化 embedding，load 经 store_entry 按当前模型**重新
// embed** 全部条目，索引维度恒跟随当前模型。本测试钉住该真实行为：
// 4-d 索引换 8-d 模型重开后 query 正常（条目已按 8-d 重嵌），不存在
// 维度不配发生路径。若未来改成持久化 embedding，此测试即哨兵。
#[test]
fn bug87_reload_reembeds_entries_so_dimensions_follow_current_model() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vector_store.jsonl");

    let embed4: crate::vector::EmbeddingFunc = Box::new(|_: &str| Ok(vec![0.1; 4]));
    let config = StoreConfig {
        storage_path: path.to_string_lossy().to_string(),
        ..Default::default()
    };
    let store = VectorStore::new_from_embed(embed4, config.clone());
    store
        .store_entry(&make_entry("bug87-dim", "indexed with 4-d model"))
        .unwrap();
    store
        .persist_entry_sync(&make_entry("bug87-dim", "indexed with 4-d model"))
        .unwrap();

    // 换 8-d 模型重开同一持久文件：load 重嵌 → 索引条目 8-d。
    let embed8: crate::vector::EmbeddingFunc = Box::new(|_: &str| Ok(vec![0.1; 8]));
    let store2 = VectorStore::new_from_embed(embed8, config);
    assert_eq!(
        store2.len(),
        1,
        "entry must reload (re-embedded, not skipped)"
    );

    // 与当前模型同维度的 query 照常命中（相似度 1.0，恒 0 分假象不存在）。
    let res = store2
        .query("indexed with 4-d model", 5, &[])
        .expect("reload re-embeds entries; dimension mismatch has no occurrence path");
    assert!(res.total >= 1, "re-embedded entry must be retrievable");
}

// ============================================================
// BUG-20261114-032: query_with_embedding equivalence
// ============================================================

/// query_with_embedding 与 query_with_threshold 评分语义单一真相源：
/// 同一 embedding、同一查询喂给两个入口，结果（条目/顺序/分数/total）
/// 必须完全一致——前者是后者的 embed 拆分形态，任何语义漂移都会让
/// 守卫纪律调用方（manager 三处搜索）与便捷壳产生分叉。
#[test]
fn query_with_embedding_matches_query_with_threshold() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("equiv.jsonl");
    let embed: crate::vector::EmbeddingFunc =
        Box::new(|t: &str| Ok(vec![t.len() as f32, 1.0, 0.0]));
    let config = StoreConfig {
        embedding_tier: "test".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.1,
        storage_path: path.to_string_lossy().to_string(),
    };
    let store = VectorStore::new_from_embed(embed, config);

    store
        .store_entry(&make_entry("e1", "alpha memory about paris"))
        .unwrap();
    store
        .store_entry(&make_entry("e2", "beta memory about london"))
        .unwrap();
    store
        .store_entry(&make_entry("e3", "gamma note unrelated"))
        .unwrap();

    let query = "alpha memory";
    let emb = (store.embed_handle())(query).unwrap();

    let via_shell = store.query_with_threshold(query, 0, &[], 0.1).unwrap();
    let via_split = store
        .query_with_embedding(&emb, query, 0, &[], 0.1)
        .unwrap();

    assert_eq!(via_shell.total, via_split.total);
    assert_eq!(via_shell.entries.len(), via_split.entries.len());
    for (a, b) in via_shell.entries.iter().zip(via_split.entries.iter()) {
        assert_eq!(a.id, b.id);
        assert!(
            (a.score - b.score).abs() < 1e-12,
            "score drift between embed-split and shell entry points"
        );
    }
    assert_eq!(via_shell.query, via_split.query);
}
