use super::*;

fn make_entry(typ: MemoryType, content: &str) -> Entry {
    Entry::new(typ, content.to_string())
}

#[tokio::test]
async fn store_and_get() {
    let store = LocalStore::new();
    let entry = make_entry(MemoryType::LongTerm, "Rust is a systems language");
    let id = store.store(entry.clone()).await.unwrap();
    let retrieved = store.get(&id).await.unwrap().unwrap();
    assert_eq!(retrieved.content, "Rust is a systems language");
    assert_eq!(retrieved.typ, MemoryType::LongTerm);
}

#[tokio::test]
async fn query_finds_relevant_entries() {
    let store = LocalStore::new();
    store
        .store(make_entry(MemoryType::LongTerm, "The cat sat on the mat"))
        .await
        .unwrap();
    store
        .store(make_entry(MemoryType::LongTerm, "Dogs love to play fetch"))
        .await
        .unwrap();
    store
        .store(make_entry(MemoryType::ShortTerm, "Cat food is expensive"))
        .await
        .unwrap();

    let result = store.query("cat", None, 10).await.unwrap();
    assert_eq!(result.total, 2);
    // Both "cat" entries should appear; the dog entry should not.
    assert!(result
        .entries
        .iter()
        .all(|e| e.entry.content.contains("Cat") || e.entry.content.contains("cat")));
}

#[tokio::test]
async fn query_with_type_filter() {
    let store = LocalStore::new();
    store
        .store(make_entry(MemoryType::LongTerm, "cat info"))
        .await
        .unwrap();
    store
        .store(make_entry(MemoryType::ShortTerm, "cat info short"))
        .await
        .unwrap();

    let result = store
        .query("cat", Some(MemoryType::ShortTerm), 10)
        .await
        .unwrap();
    assert_eq!(result.total, 1);
    assert_eq!(result.entries[0].entry.typ, MemoryType::ShortTerm);
}

#[tokio::test]
async fn delete_removes_entry() {
    let store = LocalStore::new();
    let id = store
        .store(make_entry(MemoryType::Daily, "temp"))
        .await
        .unwrap();
    let deleted = store.delete(&id).await.unwrap();
    assert!(deleted);
    let gone = store.get(&id).await.unwrap();
    assert!(gone.is_none());
    // Deleting again returns false.
    let deleted_again = store.delete(&id).await.unwrap();
    assert!(!deleted_again);
}

#[tokio::test]
async fn list_with_pagination() {
    let store = LocalStore::new();
    for i in 0..5 {
        store
            .store(make_entry(MemoryType::LongTerm, &format!("entry {i}")))
            .await
            .unwrap();
    }
    let page = store.list(None, 2, 0).await.unwrap();
    assert_eq!(page.len(), 2);
    let page2 = store.list(None, 2, 2).await.unwrap();
    assert_eq!(page2.len(), 2);
    let page3 = store.list(None, 2, 4).await.unwrap();
    assert_eq!(page3.len(), 1);
}

// ---- New tests ----

#[test]
fn tokenize_splits_on_punctuation() {
    let tokens = tokenize("Hello, World! This is a test.");
    assert_eq!(tokens, vec!["hello", "world", "this", "is", "a", "test"]);
}

#[test]
fn tokenize_handles_empty_string() {
    let tokens = tokenize("");
    assert!(tokens.is_empty());
}

#[test]
fn tokenize_handles_special_chars() {
    let tokens = tokenize("foo@bar.com #hashtag $100");
    assert!(tokens.contains(&"foo".to_string()));
    assert!(tokens.contains(&"bar".to_string()));
    assert!(tokens.contains(&"com".to_string()));
    assert!(tokens.contains(&"hashtag".to_string()));
    assert!(tokens.contains(&"100".to_string()));
}

#[test]
fn tokenize_handles_unicode() {
    let tokens = tokenize("hello 世界");
    assert!(tokens.contains(&"hello".to_string()));
    assert!(tokens.contains(&"世界".to_string()));
}

#[test]
fn tokenize_lowercases() {
    let tokens = tokenize("HELLO World");
    assert!(tokens.contains(&"hello".to_string()));
    assert!(tokens.contains(&"world".to_string()));
}

#[test]
fn compute_score_identical() {
    let q = tokenize("hello world");
    let d = tokenize("hello world");
    assert_eq!(compute_score(&q, &d), 1.0);
}

#[test]
fn compute_score_no_overlap() {
    let q = tokenize("hello");
    let d = tokenize("world");
    assert_eq!(compute_score(&q, &d), 0.0);
}

#[test]
fn compute_score_partial_overlap() {
    let q = tokenize("hello world foo");
    let d = tokenize("hello bar baz");
    assert!(compute_score(&q, &d) > 0.0);
    assert!(compute_score(&q, &d) < 1.0);
}

#[test]
fn compute_score_empty_query() {
    let q = vec![];
    let d = tokenize("hello");
    assert_eq!(compute_score(&q, &d), 0.0);
}

#[test]
fn compute_score_empty_doc() {
    let q = tokenize("hello");
    let d = vec![];
    assert_eq!(compute_score(&q, &d), 0.0);
}

#[tokio::test]
async fn store_returns_entry_id() {
    let store = LocalStore::new();
    let entry = make_entry(MemoryType::LongTerm, "test content");
    let id_before = entry.id.clone();
    let id = store.store(entry).await.unwrap();
    assert_eq!(id, id_before);
}

#[tokio::test]
async fn get_nonexistent_returns_none() {
    let store = LocalStore::new();
    let result = store.get("nonexistent-id").await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
async fn delete_nonexistent_returns_false() {
    let store = LocalStore::new();
    let result = store.delete("nonexistent-id").await.unwrap();
    assert!(!result);
}

#[tokio::test]
async fn list_empty_store() {
    let store = LocalStore::new();
    let result = store.list(None, 10, 0).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn list_with_type_filter() {
    let store = LocalStore::new();
    store.store(make_entry(MemoryType::LongTerm, "long term")).await.unwrap();
    store.store(make_entry(MemoryType::ShortTerm, "short term")).await.unwrap();
    store.store(make_entry(MemoryType::Daily, "daily")).await.unwrap();

    let long_only = store.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
    assert_eq!(long_only.len(), 1);
    assert_eq!(long_only[0].typ, MemoryType::LongTerm);

    let short_only = store.list(Some(MemoryType::ShortTerm), 10, 0).await.unwrap();
    assert_eq!(short_only.len(), 1);
    assert_eq!(short_only[0].typ, MemoryType::ShortTerm);
}

#[tokio::test]
async fn query_no_results() {
    let store = LocalStore::new();
    store.store(make_entry(MemoryType::LongTerm, "hello world")).await.unwrap();
    let result = store.query("nonexistent", None, 10).await.unwrap();
    assert_eq!(result.total, 0);
    assert!(result.entries.is_empty());
}

#[tokio::test]
async fn query_with_tags() {
    let store = LocalStore::new();
    let mut entry = make_entry(MemoryType::LongTerm, "some content");
    entry.tags = vec!["rust".to_string(), "programming".to_string()];
    store.store(entry).await.unwrap();

    let result = store.query("rust", None, 10).await.unwrap();
    assert_eq!(result.total, 1);
}

#[tokio::test]
async fn query_respects_limit() {
    let store = LocalStore::new();
    for i in 0..10 {
        store.store(make_entry(MemoryType::LongTerm, &format!("cat entry number {}", i))).await.unwrap();
    }
    let result = store.query("cat", None, 3).await.unwrap();
    assert_eq!(result.entries.len(), 3);
    assert_eq!(result.total, 10);
}

#[tokio::test]
async fn close_is_ok() {
    let store = LocalStore::new();
    store.close().await.unwrap();
}

#[tokio::test]
async fn store_multiple_and_list() {
    let store = LocalStore::new();
    for i in 0..20 {
        store.store(make_entry(MemoryType::LongTerm, &format!("entry {}", i))).await.unwrap();
    }
    let all = store.list(None, 100, 0).await.unwrap();
    assert_eq!(all.len(), 20);
}

#[tokio::test]
async fn query_scoring_order() {
    let store = LocalStore::new();
    store.store(make_entry(MemoryType::LongTerm, "cat cat cat")).await.unwrap();
    store.store(make_entry(MemoryType::LongTerm, "cat dog")).await.unwrap();
    store.store(make_entry(MemoryType::LongTerm, "dog dog dog")).await.unwrap();

    let result = store.query("cat", None, 10).await.unwrap();
    assert_eq!(result.total, 2);
    // First result should be the entry with more "cat" occurrences
    assert!(result.entries[0].entry.content.contains("cat cat cat"));
}

#[tokio::test]
async fn list_with_offset_beyond_entries() {
    let store = LocalStore::new();
    store.store(make_entry(MemoryType::LongTerm, "only entry")).await.unwrap();
    let result = store.list(None, 10, 100).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn store_default() {
    let store = LocalStore::default();
    let result = store.list(None, 10, 0).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn store_update_by_id() {
    let store = LocalStore::new();
    let entry = make_entry(MemoryType::LongTerm, "original");
    let id = store.store(entry.clone()).await.unwrap();

    // Store again with same ID
    let mut updated = make_entry(MemoryType::ShortTerm, "updated");
    updated.id = id.clone();
    store.store(updated).await.unwrap();

    // Old entry still exists (append-only)
    let all = store.list(None, 10, 0).await.unwrap();
    assert_eq!(all.len(), 2);
}

// ---- Additional coverage tests for 95%+ ----

#[tokio::test]
async fn test_local_store_close() {
    let store = LocalStore::new();
    let result = store.close().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_local_store_delete_nonexistent() {
    let store = LocalStore::new();
    let deleted = store.delete("nonexistent").await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_local_store_list_with_offset() {
    let store = LocalStore::new();
    for i in 0..5 {
        let entry = Entry::new(MemoryType::LongTerm, format!("content {}", i));
        store.store(entry).await.unwrap();
    }
    let result = store.list(None, 10, 3).await.unwrap();
    assert_eq!(result.len(), 2); // 5 total, offset 3 = 2 remaining
}

#[tokio::test]
async fn test_local_store_query_empty() {
    let store = LocalStore::new();
    let result = store.query("", None, 10).await.unwrap();
    assert!(result.entries.is_empty());
}

#[tokio::test]
async fn test_local_store_list_by_type() {
    let store = LocalStore::new();
    store.store(Entry::new(MemoryType::LongTerm, "long term".to_string())).await.unwrap();
    store.store(Entry::new(MemoryType::ShortTerm, "short term".to_string())).await.unwrap();

    let long_only = store.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
    assert_eq!(long_only.len(), 1);

    let short_only = store.list(Some(MemoryType::ShortTerm), 10, 0).await.unwrap();
    assert_eq!(short_only.len(), 1);
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
fn test_tokenize_special_chars() {
    let tokens = tokenize("foo@bar.com #hashtag 123");
    assert_eq!(tokens, vec!["foo", "bar", "com", "hashtag", "123"]);
}

#[test]
fn test_compute_score_no_overlap() {
    let q = tokenize("alpha beta");
    let d = tokenize("gamma delta");
    let score = compute_score(&q, &d);
    assert_eq!(score, 0.0);
}

#[test]
fn test_compute_score_full_overlap() {
    let q = tokenize("hello world");
    let d = tokenize("hello world test");
    let score = compute_score(&q, &d);
    assert!(score > 0.99);
}

#[test]
fn test_compute_score_empty_query() {
    let d = tokenize("document text");
    let score = compute_score(&[], &d);
    assert_eq!(score, 0.0);
}

#[test]
fn test_local_store_default() {
    let store = LocalStore::default();
    assert!(store.entries.read().is_empty());
}
