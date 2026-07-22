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
