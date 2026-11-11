use super::*;

#[tokio::test]
async fn add_and_query_triples() {
    let store = InMemoryGraphStore::new();

    store
        .add_triple(GraphTriple::new(
            "rust".into(),
            "is_a".into(),
            "language".into(),
        ))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "language".into(),
            "has_feature".into(),
            "memory_safety".into(),
        ))
        .await
        .unwrap();

    let result = store.query_bfs("rust", 3).await.unwrap();
    // Two paths: rust->language, and rust->language->memory_safety
    assert_eq!(result.paths.len(), 2);

    let direct: Vec<_> = result.paths.iter().filter(|p| p.len() == 1).collect();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0][0].object, "language");
}

#[tokio::test]
async fn entity_upsert_and_get() {
    let store = InMemoryGraphStore::new();

    store
        .upsert_entity(GraphEntity::new("rust".into(), "language".into()))
        .await
        .unwrap();

    let entity = store.get_entity("rust").await.unwrap().unwrap();
    assert_eq!(entity.name, "rust");
    assert_eq!(entity.typ, "language");

    // Upsert overwrites.
    store
        .upsert_entity(GraphEntity::new("rust".into(), "tool".into()))
        .await
        .unwrap();
    let updated = store.get_entity("rust").await.unwrap().unwrap();
    assert_eq!(updated.typ, "tool");
}

#[tokio::test]
async fn remove_triple() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
        .await
        .unwrap();

    let removed = store.remove_triple("a", "b", "c").await.unwrap();
    assert!(removed);

    let result = store.query_bfs("a", 1).await.unwrap();
    assert!(result.paths.is_empty());

    let removed_again = store.remove_triple("a", "b", "c").await.unwrap();
    assert!(!removed_again);
}

#[tokio::test]
async fn list_triples_for_entity() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("x".into(), "rel1".into(), "y".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("z".into(), "rel2".into(), "x".into()))
        .await
        .unwrap();

    let triples = store.list_triples("x").await.unwrap();
    assert_eq!(triples.len(), 2);
}

// -- Persistence tests --------------------------------------------------

#[tokio::test]
async fn persist_entities_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    store
        .upsert_entity(GraphEntity::new("alice".into(), "person".into()))
        .await
        .unwrap();
    store
        .upsert_entity(GraphEntity::new("bob".into(), "person".into()))
        .await
        .unwrap();

    let entities_path = dir.path().join("entities.jsonl");
    assert!(entities_path.exists());

    let data = std::fs::read_to_string(&entities_path).unwrap();
    let count = data.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn persist_triples_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    store
        .add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "b".into(),
            "works_with".into(),
            "c".into(),
        ))
        .await
        .unwrap();

    let triples_path = dir.path().join("triples.jsonl");
    assert!(triples_path.exists());

    let data = std::fs::read_to_string(&triples_path).unwrap();
    let count = data.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn reload_entities_from_disk() {
    let dir = tempfile::tempdir().unwrap();

    // Write data with first store instance.
    {
        let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
        store
            .upsert_entity(GraphEntity::new("rust".into(), "language".into()))
            .await
            .unwrap();
        store
            .upsert_entity(GraphEntity::new("go".into(), "language".into()))
            .await
            .unwrap();
    }

    // Create a new store -- should reload from disk.
    let store2 = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    let entity = store2.get_entity("rust").await.unwrap().unwrap();
    assert_eq!(entity.name, "rust");
    assert_eq!(entity.typ, "language");

    let go = store2.get_entity("go").await.unwrap().unwrap();
    assert_eq!(go.name, "go");

    let count = store2.entity_count().await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn reload_triples_from_disk() {
    let dir = tempfile::tempdir().unwrap();

    // Write data with first store instance.
    {
        let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
        store
            .add_triple(GraphTriple::new("x".into(), "rel".into(), "y".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new("y".into(), "rel".into(), "z".into()))
            .await
            .unwrap();
    }

    // Create a new store -- should reload from disk.
    let store2 = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    let triples = store2.list_triples("x").await.unwrap();
    assert_eq!(triples.len(), 1);
    assert_eq!(triples[0].object, "y");

    let count = store2.triple_count().await.unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn persist_after_delete_entity() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    store
        .upsert_entity(GraphEntity::new("target".into(), "thing".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "a".into(),
            "refers".into(),
            "target".into(),
        ))
        .await
        .unwrap();

    store.delete_entity("target").await.unwrap();

    // Verify files on disk reflect the deletion.
    let entities_data = std::fs::read_to_string(dir.path().join("entities.jsonl")).unwrap();
    assert!(
        !entities_data.contains("target"),
        "entity should be gone from persisted file"
    );

    let triples_data = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
    assert!(
        triples_data.trim().is_empty(),
        "triples referencing deleted entity should be gone"
    );
}

#[tokio::test]
async fn persist_after_remove_triple() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    store
        .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
        .await
        .unwrap();

    let triples_data_before = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
    assert!(triples_data_before.contains("b"));

    store.remove_triple("a", "b", "c").await.unwrap();

    let triples_data_after = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
    assert!(
        triples_data_after.trim().is_empty(),
        "removed triple should be gone from persisted file"
    );
}

#[tokio::test]
async fn no_persistence_without_dir() {
    let store = InMemoryGraphStore::new();
    store
        .upsert_entity(GraphEntity::new("x".into(), "thing".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
        .await
        .unwrap();

    // No files were written -- nothing to assert, just ensure no panic.
    assert_eq!(store.entity_count().await.unwrap(), 1);
    assert_eq!(store.triple_count().await.unwrap(), 1);
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[tokio::test]
async fn graph_entity_properties() {
    let store = InMemoryGraphStore::new();
    let mut entity = GraphEntity::new("rust".into(), "language".into());
    entity
        .properties
        .insert("paradigm".into(), "multi-paradigm".into());
    entity.properties.insert("year".into(), "2010".into());
    store.upsert_entity(entity).await.unwrap();

    let retrieved = store.get_entity("rust").await.unwrap().unwrap();
    assert_eq!(
        retrieved.properties.get("paradigm").unwrap(),
        "multi-paradigm"
    );
    assert_eq!(retrieved.properties.get("year").unwrap(), "2010");
}

#[tokio::test]
async fn graph_triple_confidence() {
    let store = InMemoryGraphStore::new();
    let triple = GraphTriple::new("a".into(), "rel".into(), "b".into()).with_confidence(0.75);
    store.add_triple(triple).await.unwrap();

    let triples = store.list_triples("a").await.unwrap();
    assert_eq!(triples.len(), 1);
    assert!((triples[0].confidence - 0.75).abs() < f64::EPSILON);
}

#[tokio::test]
async fn graph_default_confidence_is_one() {
    let triple = GraphTriple::new("x".into(), "y".into(), "z".into());
    assert!((triple.confidence - 1.0).abs() < f64::EPSILON);
}

#[tokio::test]
async fn graph_triple_metadata() {
    let store = InMemoryGraphStore::new();
    let mut triple = GraphTriple::new("alice".into(), "knows".into(), "bob".into());
    triple.metadata.insert("since".into(), "2020".into());
    store.add_triple(triple).await.unwrap();

    let triples = store.list_triples("alice").await.unwrap();
    assert_eq!(triples[0].metadata.get("since").unwrap(), "2020");
}

#[tokio::test]
async fn graph_bfs_empty_graph() {
    let store = InMemoryGraphStore::new();
    let result = store.query_bfs("nonexistent", 3).await.unwrap();
    assert!(result.paths.is_empty());
    assert!(result.entities.is_empty());
}

#[tokio::test]
async fn graph_bfs_depth_zero() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();

    let result = store.query_bfs("a", 0).await.unwrap();
    // Depth 0 means path.len() >= max_depth immediately, so no traversal
    assert!(result.paths.is_empty());
}

#[tokio::test]
async fn graph_bfs_multi_hop() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("b".into(), "rel".into(), "c".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "rel".into(), "d".into()))
        .await
        .unwrap();

    let result = store.query_bfs("a", 3).await.unwrap();
    // Should find paths: a->b, a->b->c, a->b->c->d
    assert!(result.paths.len() >= 3);
}

#[tokio::test]
async fn graph_bfs_cyclic_graph() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("b".into(), "rel".into(), "a".into()))
        .await
        .unwrap();

    let result = store.query_bfs("a", 5).await.unwrap();
    // Should not infinite loop; visited set prevents revisiting
    assert!(!result.paths.is_empty());
}

#[tokio::test]
async fn graph_search_case_insensitive() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new(
            "Rust".into(),
            "is_a".into(),
            "Language".into(),
        ))
        .await
        .unwrap();

    let results = store.search("rust", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn graph_search_by_predicate() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "hates".into(), "d".into()))
        .await
        .unwrap();

    let results = store.search("knows", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].subject, "a");
}

#[tokio::test]
async fn graph_search_by_object() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "paris".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("b".into(), "rel".into(), "london".into()))
        .await
        .unwrap();

    let results = store.search("paris", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn graph_search_empty_query() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
        .await
        .unwrap();

    let results = store.search("", 10).await.unwrap();
    // Empty query matches everything (contains(""))
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn graph_search_limit() {
    let store = InMemoryGraphStore::new();
    for i in 0..10 {
        store
            .add_triple(GraphTriple::new(
                format!("a{}", i),
                "rel".into(),
                format!("b{}", i),
            ))
            .await
            .unwrap();
    }

    let results = store.search("a", 3).await.unwrap();
    assert!(results.len() <= 3);
}

#[tokio::test]
async fn graph_query_triples_by_subject() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new(
            "alice".into(),
            "knows".into(),
            "bob".into(),
        ))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "carol".into(),
            "knows".into(),
            "dave".into(),
        ))
        .await
        .unwrap();

    let results = store.query_triples("alice", "", "").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].subject, "alice");
}

#[tokio::test]
async fn graph_query_triples_by_predicate() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "hates".into(), "d".into()))
        .await
        .unwrap();

    let results = store.query_triples("", "knows", "").await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn graph_query_triples_by_object() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "target".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("b".into(), "rel".into(), "other".into()))
        .await
        .unwrap();

    let results = store.query_triples("", "", "target").await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].object, "target");
}

#[tokio::test]
async fn graph_query_triples_wildcard() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel1".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "rel2".into(), "d".into()))
        .await
        .unwrap();

    let results = store.query_triples("", "", "").await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn graph_get_related_depth_one() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("b".into(), "rel".into(), "c".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("d".into(), "rel".into(), "e".into()))
        .await
        .unwrap();

    let related = store.get_related("a", 1).await.unwrap();
    assert_eq!(related.len(), 1); // Only a->b
}

#[tokio::test]
async fn graph_get_related_depth_zero_defaults_to_one() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();

    let related = store.get_related("a", 0).await.unwrap();
    assert_eq!(related.len(), 1); // depth=0 defaults to 1
}

#[tokio::test]
async fn graph_get_related_bidirectional() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "back".into(), "a".into()))
        .await
        .unwrap();

    let related = store.get_related("a", 1).await.unwrap();
    // Should find both a->b and c->a
    assert_eq!(related.len(), 2);
}

#[tokio::test]
async fn graph_delete_entity_cascades_triples() {
    let store = InMemoryGraphStore::new();
    store
        .upsert_entity(GraphEntity::new("target".into(), "thing".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "a".into(),
            "refers".into(),
            "target".into(),
        ))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "target".into(),
            "knows".into(),
            "b".into(),
        ))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("c".into(), "unrelated".into(), "d".into()))
        .await
        .unwrap();

    store.delete_entity("target").await.unwrap();

    // Entity gone
    assert!(store.get_entity("target").await.unwrap().is_none());

    // All triples involving "target" should be gone
    let remaining = store.list_triples("target").await.unwrap();
    assert!(remaining.is_empty());

    // Unrelated triples should remain
    let unrelated = store.list_triples("c").await.unwrap();
    assert_eq!(unrelated.len(), 1);
}

#[tokio::test]
async fn graph_delete_nonexistent_entity() {
    let store = InMemoryGraphStore::new();
    // Should not panic
    store.delete_entity("ghost").await.unwrap();
    assert_eq!(store.entity_count().await.unwrap(), 0);
}

#[tokio::test]
async fn graph_remove_triple_nonexistent() {
    let store = InMemoryGraphStore::new();
    let removed = store.remove_triple("x", "y", "z").await.unwrap();
    assert!(!removed);
}

#[tokio::test]
async fn graph_entity_count_multiple() {
    let store = InMemoryGraphStore::new();
    for i in 0..5 {
        store
            .upsert_entity(GraphEntity::new(format!("e{}", i), "thing".into()))
            .await
            .unwrap();
    }
    assert_eq!(store.entity_count().await.unwrap(), 5);
}

#[tokio::test]
async fn graph_triple_count_multiple() {
    let store = InMemoryGraphStore::new();
    for i in 0..5 {
        store
            .add_triple(GraphTriple::new(
                format!("s{}", i),
                "rel".into(),
                format!("o{}", i),
            ))
            .await
            .unwrap();
    }
    assert_eq!(store.triple_count().await.unwrap(), 5);
}

#[tokio::test]
async fn graph_upsert_entity_overwrites() {
    let store = InMemoryGraphStore::new();
    store
        .upsert_entity(GraphEntity::new("x".into(), "original".into()))
        .await
        .unwrap();
    store
        .upsert_entity(GraphEntity::new("x".into(), "updated".into()))
        .await
        .unwrap();

    let entity = store.get_entity("x").await.unwrap().unwrap();
    assert_eq!(entity.typ, "updated");
    assert_eq!(store.entity_count().await.unwrap(), 1);
}

#[tokio::test]
async fn graph_list_triples_no_match() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "b".into(), "c".into()))
        .await
        .unwrap();

    let triples = store.list_triples("nonexistent").await.unwrap();
    assert!(triples.is_empty());
}

#[tokio::test]
async fn graph_persist_and_reload_full_cycle() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
        store
            .upsert_entity(GraphEntity::new("alice".into(), "person".into()))
            .await
            .unwrap();
        store
            .upsert_entity(GraphEntity::new("bob".into(), "person".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new(
                "alice".into(),
                "knows".into(),
                "bob".into(),
            ))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new(
                "bob".into(),
                "works_with".into(),
                "alice".into(),
            ))
            .await
            .unwrap();

        // Remove one triple
        store
            .remove_triple("bob", "works_with", "alice")
            .await
            .unwrap();
    }

    // Reload
    let store2 = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    assert_eq!(store2.entity_count().await.unwrap(), 2);
    assert_eq!(store2.triple_count().await.unwrap(), 1);

    let triple = store2.list_triples("alice").await.unwrap();
    assert_eq!(triple.len(), 1);
    assert_eq!(triple[0].predicate, "knows");
}

#[tokio::test]
async fn graph_multiple_triples_same_subject() {
    let store = InMemoryGraphStore::new();
    store
        .add_triple(GraphTriple::new("a".into(), "rel1".into(), "b".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("a".into(), "rel2".into(), "c".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("a".into(), "rel3".into(), "d".into()))
        .await
        .unwrap();

    let triples = store.list_triples("a").await.unwrap();
    assert_eq!(triples.len(), 3);
}

#[tokio::test]
async fn graph_path_hop_fields() {
    let store = InMemoryGraphStore::new();
    let triple = GraphTriple::new("x".into(), "connects".into(), "y".into()).with_confidence(0.9);
    store.add_triple(triple).await.unwrap();

    let result = store.query_bfs("x", 1).await.unwrap();
    assert_eq!(result.paths.len(), 1);
    assert_eq!(result.paths[0].len(), 1);
    let hop = &result.paths[0][0];
    assert_eq!(hop.subject, "x");
    assert_eq!(hop.predicate, "connects");
    assert_eq!(hop.object, "y");
    assert!((hop.confidence - 0.9).abs() < f64::EPSILON);
}

#[tokio::test]
async fn graph_default_store() {
    let store = InMemoryGraphStore::default();
    assert_eq!(store.entity_count().await.unwrap(), 0);
    assert_eq!(store.triple_count().await.unwrap(), 0);
}

// ---- coverage: disk load with blank/malformed lines + triple persistence ----

#[tokio::test]
async fn load_from_disk_skips_blank_and_malformed_lines() {
    let dir = tempfile::tempdir().unwrap();

    // entities.jsonl: valid line + blank lines + malformed line.
    let entity = GraphEntity::new("disk-ent".to_string(), "tool".to_string());
    let entity_line = serde_json::to_string(&entity).unwrap();
    std::fs::write(
        dir.path().join("entities.jsonl"),
        format!("\n\n{entity_line}\n\nnot-json\n"),
    )
    .unwrap();

    // triples.jsonl: valid line + blank lines + malformed line.
    let triple = GraphTriple::new("a".to_string(), "rel".to_string(), "b".to_string());
    let triple_line = serde_json::to_string(&triple).unwrap();
    std::fs::write(
        dir.path().join("triples.jsonl"),
        format!("\n\n{triple_line}\n\nnot-json\n"),
    )
    .unwrap();

    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    // Trigger lazy load via a read op.
    let loaded = store.get_entity("disk-ent").await.unwrap();
    assert!(loaded.is_some(), "valid entity line must load");

    let triples = store.list_triples("a").await.unwrap();
    assert_eq!(triples.len(), 1, "valid triple line must load");
    assert_eq!(triples[0].object, "b");
}

#[tokio::test]
async fn persist_and_reload_triples_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    store
        .add_triple(GraphTriple::new(
            "s1".to_string(),
            "p1".to_string(),
            "o1".to_string(),
        ))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "s1".to_string(),
            "p2".to_string(),
            "o2".to_string(),
        ))
        .await
        .unwrap();

    let path = dir.path().join("triples.jsonl");
    assert!(path.exists(), "triples must be persisted on mutation");
    let raw = std::fs::read_to_string(&path).unwrap();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(lines.len(), 2, "two distinct triples → two lines");

    // Reload into a fresh store and verify both survive.
    let reloaded = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    let all = reloaded.list_triples("s1").await.unwrap();
    assert_eq!(all.len(), 2);
}

// ---- coverage: load-from-disk failure swallow + save-side dedup ----

#[tokio::test]
async fn persistence_dir_pointing_at_file_starts_empty() {
    // The persistence dir itself is a REGULAR FILE → entities.jsonl can
    // never be read → load_from_disk errs → ensure_loaded swallows the
    // error (warn-only) and the store works from an empty slate.
    let dir = tempfile::tempdir().unwrap();
    let bogus = dir.path().join("not_a_dir");
    std::fs::write(&bogus, b"x").unwrap();

    let store = InMemoryGraphStore::new().with_persistence(bogus);
    let all = store.list_triples("anything").await.unwrap();
    assert!(all.is_empty(), "load errors are swallowed; got: {all:?}");
}

#[tokio::test]
async fn persist_triples_dedupes_identical_keys_on_save() {
    let dir = tempfile::tempdir().unwrap();
    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());

    let t = GraphTriple::new("s".into(), "p".into(), "o".into());
    store.add_triple(t.clone()).await.unwrap();
    store.add_triple(t.clone()).await.unwrap(); // same key → save-side dedup

    // In-memory keeps every push; the JSONL snapshot dedupes by key.
    let all = store.list_triples("s").await.unwrap();
    let raw = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap_or_default();
    let on_disk = raw.lines().filter(|l| !l.trim().is_empty()).count();
    assert_eq!(
        on_disk,
        std::cmp::min(all.len(), 1),
        "dup keys collapse on disk"
    );
}

// persist 走唯一临时名权威 helper。旧固定
// `entities.jsonl.tmp` / `triples.jsonl.tmp` 被残留垃圾（目录形态最顽固）
// 占用时 File::create 失败 → 落盘被静默吞掉（变更永不落盘）。种目录在旧
// 固定名上：旧实现必红，新实现经唯一临时名照常落盘。
#[tokio::test]
async fn bug59_persist_survives_residue_at_fixed_tmp_paths() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir(dir.path().join("entities.jsonl.tmp")).unwrap();
    std::fs::create_dir(dir.path().join("triples.jsonl.tmp")).unwrap();

    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    store
        .upsert_entity(GraphEntity::new("alice".into(), "person".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new("a".into(), "knows".into(), "b".into()))
        .await
        .unwrap();

    let ent = std::fs::read_to_string(dir.path().join("entities.jsonl")).unwrap_or_default();
    assert!(
        ent.contains("alice"),
        "entities persist must be applied (old fixed-tmp shape silently defeated): {ent:?}"
    );
    let tri = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap_or_default();
    assert!(
        tri.contains("knows"),
        "triples persist must be applied (old fixed-tmp shape silently defeated): {tri:?}"
    );
}

// -- 回归锁：键归一大小写一致性 --------------------------
// 存储键统一小写归一（entities 键 + triples 双索引键），结构体字段保留
// 原始大小写供展示；任何大小写形态的查询必须命中同一份数据。

#[tokio::test]
async fn graph_triple_lookup_case_insensitive_same_session() {
    let store = InMemoryGraphStore::new();
    store
        .upsert_entity(GraphEntity::new("Alice".into(), "person".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "Alice".into(),
            "knows".into(),
            "Bob".into(),
        ))
        .await
        .unwrap();

    // 旧缺陷：add_triple 以原始大小写建索引，get_related 以小写查 →
    // 同会话漏查返回空。
    let related = store.get_related("Alice", 1).await.unwrap();
    assert_eq!(
        related.len(),
        1,
        "get_related must find the stored triple regardless of case"
    );
    assert_eq!(related[0].object, "Bob");

    let listed = store.list_triples("alice").await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "list_triples must match case-insensitively"
    );
}

#[tokio::test]
async fn graph_delete_entity_removes_entity_and_triples_case_insensitive() {
    let store = InMemoryGraphStore::new();
    store
        .upsert_entity(GraphEntity::new("Alice".into(), "person".into()))
        .await
        .unwrap();
    store
        .add_triple(GraphTriple::new(
            "Alice".into(),
            "knows".into(),
            "Bob".into(),
        ))
        .await
        .unwrap();

    // 旧缺陷：delete_entity 以小写键查，upsert/add_triple 以原始键存 →
    // 删除假成功（返回 Ok 但数据全残留）。
    store.delete_entity("alice").await.unwrap();

    assert!(
        store.get_entity("Alice").await.unwrap().is_none(),
        "entity must be gone after delete_entity"
    );
    assert_eq!(store.entity_count().await.unwrap(), 0);
    assert_eq!(store.triple_count().await.unwrap(), 0);
    assert!(
        store.list_triples("Alice").await.unwrap().is_empty(),
        "triples referencing the deleted entity must be gone"
    );
}

#[tokio::test]
async fn graph_reload_matches_mixed_case_queries() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
        store
            .upsert_entity(GraphEntity::new("Alice".into(), "person".into()))
            .await
            .unwrap();
        store
            .add_triple(GraphTriple::new(
                "Alice".into(),
                "knows".into(),
                "Bob".into(),
            ))
            .await
            .unwrap();
    }

    // 旧缺陷：load_from_disk 以小写建键，get_entity/query_bfs 以原始大小写
    // 查 → 重启后同名查询漏。
    let store2 = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    let entity = store2.get_entity("Alice").await.unwrap();
    assert!(entity.is_some(), "get_entity must hit after reload");
    assert_eq!(entity.unwrap().name, "Alice");

    let related = store2.get_related("alice", 1).await.unwrap();
    assert_eq!(related.len(), 1, "get_related must hit after reload");
    assert_eq!(related[0].object, "Bob");

    let bfs = store2.query_bfs("Alice", 1).await.unwrap();
    assert_eq!(
        bfs.paths.len(),
        1,
        "query_bfs must find the reloaded triple"
    );
    assert!(bfs.entities.iter().any(|e| e.name == "Alice"));
}

// ---- BUG-20261112-087 回归锁：加载失败/坏行留档的数据保全 ----

// M1（graph 版，per-file 粒度）：entities.jsonl 整体不可读 → 该文件覆写
// 冻结（磁盘字节不变），triples 文件不受牵连照常持久化。旧实现：读失败
// 整文件静默空库，第一次无关变更全量覆写 = 磁盘历史整体覆没。
#[tokio::test]
async fn bug87_graph_load_failure_freezes_entities_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let entities_path = dir.path().join("entities.jsonl");
    let poisoned: &[u8] = &[0xFF, 0xFE, 0x80, 0x00, 0xC3];
    std::fs::write(&entities_path, poisoned).unwrap();

    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    // 触发懒加载（entities 读失败置旗标；triples 不存在不置）。
    let _ = store.get_entity("anything").await.unwrap();

    // 内存变更照常 + 触发 persist_entities（被旗标拦）。
    store
        .upsert_entity(crate::graph::GraphEntity::new(
            "mem-only".to_string(),
            "tool".to_string(),
        ))
        .await
        .unwrap();

    let now = std::fs::read(&entities_path).unwrap();
    assert_eq!(
        now, poisoned,
        "entities rewrite must be frozen after failed load (old shape destroyed on-disk history)"
    );

    // per-file 粒度：triples 侧变更照常持久化（降级不外溢）。
    store
        .add_triple(crate::graph::GraphTriple::new(
            "a".to_string(),
            "rel".to_string(),
            "b".to_string(),
        ))
        .await
        .unwrap();
    assert!(
        dir.path().join("triples.jsonl").exists(),
        "triples persistence must stay unaffected by entities-side freeze"
    );
}

// M2（graph 版）：加载时解析失败行留档；无关变更的覆写把留档行写回
// （坏行不静默消失——持久层不因无关改写丢历史）。
#[tokio::test]
async fn bug87_graph_malformed_lines_survive_rewrite() {
    let dir = tempfile::tempdir().unwrap();
    let entity = crate::graph::GraphEntity::new("disk-ent".to_string(), "tool".to_string());
    let triple =
        crate::graph::GraphTriple::new("a".to_string(), "rel".to_string(), "b".to_string());
    std::fs::write(
        dir.path().join("entities.jsonl"),
        format!(
            "{}\nnot-json-entity\n",
            serde_json::to_string(&entity).unwrap()
        ),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("triples.jsonl"),
        format!(
            "{}\nnot-json-triple\n",
            serde_json::to_string(&triple).unwrap()
        ),
    )
    .unwrap();

    let store = InMemoryGraphStore::new().with_persistence(dir.path().to_path_buf());
    let loaded = store.get_entity("disk-ent").await.unwrap();
    assert!(loaded.is_some(), "valid entity must load");

    // 无关变更触发两侧覆写。
    store
        .upsert_entity(crate::graph::GraphEntity::new(
            "new-ent".to_string(),
            "tool".to_string(),
        ))
        .await
        .unwrap();
    store
        .add_triple(crate::graph::GraphTriple::new(
            "x".to_string(),
            "rel".to_string(),
            "y".to_string(),
        ))
        .await
        .unwrap();

    let raw_ent = std::fs::read_to_string(dir.path().join("entities.jsonl")).unwrap();
    assert!(
        raw_ent.contains("not-json-entity"),
        "malformed entity line must survive rewrites (old shape dropped it): {raw_ent:?}"
    );
    assert!(raw_ent.contains("disk-ent") && raw_ent.contains("new-ent"));

    let raw_tri = std::fs::read_to_string(dir.path().join("triples.jsonl")).unwrap();
    assert!(
        raw_tri.contains("not-json-triple"),
        "malformed triple line must survive rewrites (old shape dropped it): {raw_tri:?}"
    );
    assert!(raw_tri.contains("\"x\""));
}
