use super::*;

#[test]
fn episode_new_has_valid_id() {
    let ep = Episode::new(
        "sess-1".to_string(),
        "user".to_string(),
        "hello".to_string(),
    );
    assert_eq!(ep.id.len(), 36);
    assert_eq!(ep.session_key, "sess-1");
    assert_eq!(ep.role, "user");
    assert_eq!(ep.content, "hello");
    assert!(ep.metadata.is_empty());
    assert!(ep.tags.is_empty());
}

#[tokio::test]
async fn file_store_append_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    let ep1 = Episode::new("s1".into(), "user".into(), "hi there".into());
    let ep2 = Episode::new("s1".into(), "assistant".into(), "hello!".into());

    store.append(ep1.clone()).await.unwrap();
    store.append(ep2.clone()).await.unwrap();

    let episodes = store.get_session("s1").await.unwrap();
    assert_eq!(episodes.len(), 2);
    assert_eq!(episodes[0].content, "hi there");
    assert_eq!(episodes[1].content, "hello!");
}

#[tokio::test]
async fn file_store_list_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("alpha".into(), "user".into(), "a".into()))
        .await
        .unwrap();
    store
        .append(Episode::new("beta".into(), "user".into(), "b".into()))
        .await
        .unwrap();

    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 2);
    // Sorted alphabetically.
    assert_eq!(sessions[0], "alpha");
    assert_eq!(sessions[1], "beta");
}

#[tokio::test]
async fn file_store_delete_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("doom".into(), "user".into(), "bye".into()))
        .await
        .unwrap();

    let count = store.delete_session("doom").await.unwrap();
    assert_eq!(count, 1);

    let episodes = store.get_session("doom").await.unwrap();
    assert!(episodes.is_empty());

    // Deleting non-existent session returns 0.
    let count2 = store.delete_session("doom").await.unwrap();
    assert_eq!(count2, 0);
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[tokio::test]
async fn episode_serialization_roundtrip() {
    let ep = Episode::new("sess-1".into(), "user".into(), "hello world".into());
    let json = serde_json::to_string(&ep).unwrap();
    let deserialized: Episode = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, ep.id);
    assert_eq!(deserialized.session_key, "sess-1");
    assert_eq!(deserialized.role, "user");
    assert_eq!(deserialized.content, "hello world");
}

#[tokio::test]
async fn file_store_get_recent() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    for i in 0..10 {
        store
            .append(Episode::new(
                "sess-recent".into(),
                "user".into(),
                format!("message {}", i),
            ))
            .await
            .unwrap();
    }

    let recent = store.get_recent("sess-recent", 3).await.unwrap();
    assert_eq!(recent.len(), 3);
    // Should be the last 3 messages
    assert!(recent[0].content.contains("message 7"));
    assert!(recent[1].content.contains("message 8"));
    assert!(recent[2].content.contains("message 9"));
}

#[tokio::test]
async fn file_store_get_recent_zero_defaults_to_ten() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    for i in 0..15 {
        store
            .append(Episode::new("sess-zerolim".into(), "user".into(), format!("m{}", i)))
            .await
            .unwrap();
    }

    let recent = store.get_recent("sess-zerolim", 0).await.unwrap();
    assert_eq!(recent.len(), 10); // defaults to 10
}

#[tokio::test]
async fn file_store_get_recent_more_than_total() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("sess-small".into(), "user".into(), "only one".into()))
        .await
        .unwrap();

    let recent = store.get_recent("sess-small", 10).await.unwrap();
    assert_eq!(recent.len(), 1);
}

#[tokio::test]
async fn file_store_search_by_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("s1".into(), "user".into(), "The Eiffel Tower is in Paris".into()))
        .await
        .unwrap();
    store
        .append(Episode::new("s2".into(), "user".into(), "The Colosseum is in Rome".into()))
        .await
        .unwrap();
    store
        .append(Episode::new("s3".into(), "user".into(), "Big Ben is in London".into()))
        .await
        .unwrap();

    let results = store.search("Eiffel", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].content.contains("Eiffel"));
}

#[tokio::test]
async fn file_store_search_case_insensitive() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("s1".into(), "user".into(), "RUST programming".into()))
        .await
        .unwrap();

    let results = store.search("rust", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn file_store_search_limit() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    for i in 0..10 {
        store
            .append(Episode::new(
                format!("s{}", i),
                "user".into(),
                "common keyword here".into(),
            ))
            .await
            .unwrap();
    }

    let results = store.search("keyword", 3).await.unwrap();
    assert!(results.len() <= 3);
}

#[tokio::test]
async fn file_store_search_by_tags() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    let mut ep = Episode::new("s1".into(), "user".into(), "some content".into());
    ep.tags.push("important-tag".into());
    store.append(ep).await.unwrap();

    let results = store.search("important-tag", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn file_store_session_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store.append(Episode::new("a".into(), "user".into(), "a".into())).await.unwrap();
    store.append(Episode::new("b".into(), "user".into(), "b".into())).await.unwrap();
    store.append(Episode::new("c".into(), "user".into(), "c".into())).await.unwrap();

    let count = store.session_count().await.unwrap();
    assert_eq!(count, 3);
}

#[tokio::test]
async fn file_store_episode_count() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    for i in 0..5 {
        store.append(Episode::new("s1".into(), "user".into(), format!("msg {}", i))).await.unwrap();
    }
    store.append(Episode::new("s2".into(), "user".into(), "other".into())).await.unwrap();

    let count = store.episode_count().await.unwrap();
    assert_eq!(count, 6);
}

#[tokio::test]
async fn file_store_cleanup_old_episodes() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    // Old episode
    let mut old = Episode::new("s-old".into(), "user".into(), "old content".into());
    old.timestamp = chrono::Local::now() - chrono::Duration::days(10);
    store.append(old).await.unwrap();

    // Recent episode
    store
        .append(Episode::new("s-recent".into(), "user".into(), "recent content".into()))
        .await
        .unwrap();

    let removed = store.cleanup(5).await.unwrap();
    assert_eq!(removed, 1);

    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert!(sessions.contains(&"s-recent".to_string()));
}

#[tokio::test]
async fn file_store_cleanup_nothing_old() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("s1".into(), "user".into(), "fresh".into()))
        .await
        .unwrap();

    let removed = store.cleanup(365).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn file_store_get_session_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    let episodes = store.get_session("nonexistent").await.unwrap();
    assert!(episodes.is_empty());
}

#[tokio::test]
async fn file_store_list_sessions_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    let sessions = store.list_sessions().await.unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn file_store_session_file_sanitization() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    // Session key with special characters
    store
        .append(Episode::new("sess/with:chars*?".into(), "user".into(), "sanitized".into()))
        .await
        .unwrap();

    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    // The file should exist with sanitized name
    assert!(!sessions[0].contains('/'));
}

#[tokio::test]
async fn file_store_episodes_ordered_by_timestamp() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store
        .append(Episode::new("s1".into(), "user".into(), "first".into()))
        .await
        .unwrap();
    store
        .append(Episode::new("s1".into(), "assistant".into(), "second".into()))
        .await
        .unwrap();
    store
        .append(Episode::new("s1".into(), "user".into(), "third".into()))
        .await
        .unwrap();

    let episodes = store.get_session("s1").await.unwrap();
    assert_eq!(episodes.len(), 3);
    assert_eq!(episodes[0].content, "first");
    assert_eq!(episodes[1].content, "second");
    assert_eq!(episodes[2].content, "third");
}

#[tokio::test]
async fn file_store_multiple_roles() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    store.append(Episode::new("s1".into(), "system".into(), "system prompt".into())).await.unwrap();
    store.append(Episode::new("s1".into(), "user".into(), "user query".into())).await.unwrap();
    store.append(Episode::new("s1".into(), "assistant".into(), "response".into())).await.unwrap();

    let episodes = store.get_session("s1").await.unwrap();
    assert_eq!(episodes[0].role, "system");
    assert_eq!(episodes[1].role, "user");
    assert_eq!(episodes[2].role, "assistant");
}

#[tokio::test]
async fn file_store_episode_with_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let store = FileEpisodicStore::new(dir.path());

    let mut ep = Episode::new("s1".into(), "user".into(), "with meta".into());
    ep.metadata.insert("source".into(), "api".into());
    ep.metadata.insert("version".into(), "1.0".into());
    store.append(ep).await.unwrap();

    let episodes = store.get_session("s1").await.unwrap();
    assert_eq!(episodes[0].metadata.get("source").unwrap(), "api");
}
