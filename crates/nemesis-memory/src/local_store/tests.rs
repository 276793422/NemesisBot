use super::*;
use crate::types::Entry;

fn make_entry(typ: MemoryType, content: &str) -> Entry {
    Entry::new(typ, content.to_string())
}

#[test]
fn test_tokenize_basic() {
    let tokens = tokenize("Hello, World! This is a test.");
    assert_eq!(tokens, vec!["hello", "world", "this", "is", "a", "test"]);
}

#[test]
fn test_tokenize_empty() {
    let tokens = tokenize("");
    assert!(tokens.is_empty());
}

#[test]
fn test_tokenize_punctuation_only() {
    let tokens = tokenize("!!! ??? ...");
    assert!(tokens.is_empty());
}

#[test]
fn test_tokenize_unicode() {
    let tokens = tokenize("Hello world");
    assert!(tokens.contains(&"hello".to_string()));
    assert!(tokens.contains(&"world".to_string()));
}

#[tokio::test]
async fn test_local_store_new_creates_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();
    // File doesn't exist yet -- no error.
    assert!(!path.exists());
    let entries = store.list(None, 100, 0).await.unwrap();
    assert!(entries.is_empty());
}

#[tokio::test]
async fn test_store_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    let entry = make_entry(MemoryType::LongTerm, "Paris is the capital of France");
    let id = store.store(entry).await.unwrap();

    let retrieved = store.get(&id).await.unwrap().unwrap();
    assert_eq!(retrieved.content, "Paris is the capital of France");
    assert_eq!(retrieved.typ, MemoryType::LongTerm);
}

#[tokio::test]
async fn test_store_persists_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");

    let id = {
        let store = TfIdfLocalStore::new(&path).await.unwrap();
        let entry = make_entry(MemoryType::LongTerm, "persisted content");
        let id = store.store(entry).await.unwrap();
        assert!(path.exists());
        id
    };

    // Load again from disk.
    let store2 = TfIdfLocalStore::new(&path).await.unwrap();
    let retrieved = store2.get(&id).await.unwrap().unwrap();
    assert_eq!(retrieved.content, "persisted content");
}

#[tokio::test]
async fn test_delete_removes_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    let entry = make_entry(MemoryType::ShortTerm, "temporary");
    let id = store.store(entry).await.unwrap();

    let deleted = store.delete(&id).await.unwrap();
    assert!(deleted);

    let gone = store.get(&id).await.unwrap();
    assert!(gone.is_none());

    // Deleting again returns false.
    let deleted_again = store.delete(&id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn test_delete_archives_to_sidecar() {
    // P2 archive-on-forget: delete must NOT hard-delete — it appends the entry
    // to <stem>.archive.jsonl so a forgotten memory stays inspectable/recoverable.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    let id = store
        .store(make_entry(MemoryType::LongTerm, "important durable fact"))
        .await
        .unwrap();
    assert!(store.delete(&id).await.unwrap());

    // Archive sidecar must exist and contain the deleted entry.
    let archive_path = dir.path().join("store.archive.jsonl");
    assert!(archive_path.exists(), "archive sidecar must exist after delete");
    let archived = tokio::fs::read_to_string(&archive_path).await.unwrap();
    assert!(
        archived.contains(&id),
        "archive must contain the deleted entry's id"
    );

    // Reloading must NOT bring the archived entry back into the active set.
    let store2 = TfIdfLocalStore::new(&path).await.unwrap();
    assert!(
        store2.get(&id).await.unwrap().is_none(),
        "archived entry must stay inactive on reload"
    );
}

#[tokio::test]
async fn test_delete_nonexistent_does_not_create_archive() {
    // Boundary: deleting an id that was never stored must not create an archive
    // file (and must return false, not panic).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    let deleted = store.delete("ghost-id").await.unwrap();
    assert!(!deleted);
    assert!(
        !dir.path().join("store.archive.jsonl").exists(),
        "no archive must be created when nothing was deleted"
    );
}

#[tokio::test]
async fn test_query_finds_relevant() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    store.store(make_entry(MemoryType::LongTerm, "The cat sat on the mat")).await.unwrap();
    store.store(make_entry(MemoryType::LongTerm, "Dogs love to play fetch")).await.unwrap();
    store.store(make_entry(MemoryType::ShortTerm, "Cat food is expensive")).await.unwrap();

    let result = store.query("cat", None, 10).await.unwrap();
    assert_eq!(result.total, 2);
    // Both cat entries should be present.
    assert!(result
        .entries
        .iter()
        .all(|e| e.entry.content.contains("Cat") || e.entry.content.contains("cat")));
}

#[tokio::test]
async fn test_query_with_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    store.store(make_entry(MemoryType::LongTerm, "cat info long")).await.unwrap();
    store.store(make_entry(MemoryType::ShortTerm, "cat info short")).await.unwrap();

    let result = store.query("cat", Some(MemoryType::ShortTerm), 10).await.unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.entries[0].entry.typ, MemoryType::ShortTerm);
}

#[tokio::test]
async fn test_query_empty_tokens_returns_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    store.store(make_entry(MemoryType::LongTerm, "some content")).await.unwrap();

    let result = store.query("!!!", None, 10).await.unwrap();
    assert_eq!(result.total, 0);
}

#[tokio::test]
async fn test_list_with_pagination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    for i in 0..5 {
        store.store(make_entry(MemoryType::LongTerm, &format!("entry {i}"))).await.unwrap();
    }

    let page1 = store.list(None, 2, 0).await.unwrap();
    assert_eq!(page1.len(), 2);

    let page2 = store.list(None, 2, 2).await.unwrap();
    assert_eq!(page2.len(), 2);

    let page3 = store.list(None, 2, 4).await.unwrap();
    assert_eq!(page3.len(), 1);

    let page4 = store.list(None, 2, 10).await.unwrap();
    assert!(page4.is_empty());
}

#[tokio::test]
async fn test_list_with_type_filter() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    store.store(make_entry(MemoryType::LongTerm, "long term entry")).await.unwrap();
    store.store(make_entry(MemoryType::ShortTerm, "short term entry")).await.unwrap();
    store.store(make_entry(MemoryType::LongTerm, "another long term")).await.unwrap();

    let result = store.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
    assert_eq!(result.len(), 2);
    assert!(result.iter().all(|e| e.typ == MemoryType::LongTerm));
}

#[tokio::test]
async fn test_query_uses_tags_and_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();

    // Entry with matching tag but no matching content.
    let entry = Entry::new(MemoryType::LongTerm, "programming language".to_string())
        .with_tags(vec!["rust".to_string()]);
    store.store(entry).await.unwrap();

    let result = store.query("rust", None, 10).await.unwrap();
    assert_eq!(result.total, 1);
}

#[tokio::test]
async fn test_close_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("store.jsonl");
    let store = TfIdfLocalStore::new(&path).await.unwrap();
    assert!(store.close().await.is_ok());
}

#[test]
fn test_tfidf_score_identical_docs() {
    let tokens = vec!["hello".to_string(), "world".to_string()];
    let total_docs = 5;
    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    doc_freq.insert("hello".to_string(), 3);
    doc_freq.insert("world".to_string(), 2);

    let score = tfidf_score(&tokens, &tokens, total_docs, &doc_freq);
    // Identical documents should score ~1.0.
    assert!((score - 1.0).abs() < 0.01, "Expected ~1.0, got {score}");
}

#[test]
fn test_tfidf_score_no_overlap() {
    let query = vec!["cat".to_string()];
    let doc = vec!["dog".to_string()];
    let total_docs = 5;
    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    doc_freq.insert("cat".to_string(), 2);
    doc_freq.insert("dog".to_string(), 3);

    let score = tfidf_score(&query, &doc, total_docs, &doc_freq);
    assert_eq!(score, 0.0);
}

#[test]
fn test_tfidf_score_partial_overlap() {
    let query = vec!["cat".to_string(), "mat".to_string()];
    let doc = vec!["cat".to_string(), "sat".to_string()];
    let total_docs = 10;
    let mut doc_freq: HashMap<String, usize> = HashMap::new();
    doc_freq.insert("cat".to_string(), 5);
    doc_freq.insert("mat".to_string(), 3);
    doc_freq.insert("sat".to_string(), 4);

    let score = tfidf_score(&query, &doc, total_docs, &doc_freq);
    assert!(score > 0.0 && score < 1.0, "Expected (0, 1), got {score}");
}
