//! history_search tests (U20 sixth batch).
//!
//! Isolation note: `default_path_manager()` is a process-global singleton
//! (home resolved once), and the FTS index state is a static. Tests
//! therefore run against the SAME session_logs dir as chat_log's tests —
//! they use unique session keys and a static lock to serialize, mirroring
//! the chat_log tests' approach. The index db lives beside session_logs
//! (logs/history_index.db) and is content-idempotent (mtime reindex +
//! delete-before-insert per file).

use super::*;

/// Serialize tests touching the global index (chat_log's tests also append
/// rows through index_append — they don't query, so only these serialize).
static IDX_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

fn fresh_session(prefix: &str) -> String {
    // Unique per run: nanos timestamp suffix.
    let key = format!(
        "test:hs:{}:{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    // Family hygiene: a FAILED run panics at its assert before the trailing
    // delete_chat_log, leaving its same-prefix file behind. Those leftovers
    // share identical literal content, so ~20 of them fill the limit-20
    // search window and push the current session out — the failure then
    // self-perpetuates. Purge the family so every run starts clean.
    purge_family(prefix);
    crate::chat_log::delete_chat_log(&key);
    key
}

/// Delete leftover `test:hs:<prefix>:*` session files from prior (failed)
/// runs — this test family's own artifacts only, never foreign keys.
fn purge_family(prefix: &str) {
    let dir = nemesis_path::default_path_manager().sessions_log_dir();
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return;
    };
    let stem_prefix = format!("test_hs_{prefix}_");
    for ent in rd.flatten() {
        let path = ent.path();
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if stem.starts_with(&stem_prefix) {
            let _ = std::fs::remove_file(&path);
        }
    }
}

#[test]
fn test_cjk_bigrams() {
    // Pure CJK run → overlapping bigrams (trailing space trimmed).
    assert_eq!(cjk_bigrams("部署文档"), "部署 署文 文档");
    // Single CJK char passes through as itself.
    assert_eq!(cjk_bigrams("好"), "好");
    // Latin words stay whole; CJK bigrammed separately.
    assert_eq!(cjk_bigrams("deploy 部署 now"), "deploy 部署 now");
    // Mixed CJK/Latin boundaries split runs.
    assert_eq!(cjk_bigrams("中文abc"), "中文 abc");
    // Punctuation separates words (dropped, not indexed).
    assert_eq!(cjk_bigrams("hello, 世界!"), "hello 世界");
    // Empty stays empty.
    assert_eq!(cjk_bigrams(""), "");
}

/// 2026-08-23 regression (found by the strict-completion final regression):
/// deleting a session's chat log must purge its FTS rows on the next
/// reindex — pre-fix, deleted files' rows lingered forever (ghost hits
/// accumulated across runs until they pushed live sessions out of the
/// search window; in production a deleted session stayed searchable).
#[test]
fn test_reindex_purges_deleted_session_rows() {
    let _lock = IDX_LOCK.lock();
    let k = fresh_session("ghost");
    crate::chat_log::append_chat_log(&k, "user", "ghostsessionmarker unique phrase zq7");
    reindex_session_logs();
    assert!(
        !search("ghostsessionmarker", 10).is_empty(),
        "must hit before delete"
    );

    crate::chat_log::delete_chat_log(&k);
    reindex_session_logs();
    let hits = search("ghostsessionmarker", 10);
    assert!(
        hits.iter().all(|h| h.session_key != k.replace(':', "_")),
        "deleted session's rows must be purged, got {:?}",
        hits.iter().map(|h| &h.session_key).collect::<Vec<_>>()
    );
}

#[test]
fn test_fts_chinese_cross_session() {
    let _lock = IDX_LOCK.lock();
    let k1 = fresh_session("zh1");
    let k2 = fresh_session("zh2");
    crate::chat_log::append_chat_log(&k1, "user", "请帮我部署文档系统到测试环境");
    crate::chat_log::append_chat_log(&k2, "user", "今天天气不错");
    crate::chat_log::append_chat_log(&k2, "assistant", "是的，适合出门散步");

    // Lazy full index then search a SHORT Chinese phrase that only the
    // bigram column can match (unicode61 would treat 部署文档... as one
    // token).
    reindex_session_logs();
    let hits = search("部署 文档", 20);
    assert!(
        hits.iter().any(|h| h.session_key == k1.replace(':', "_")),
        "must hit the deployment session: {:?}",
        hits.iter().map(|h| &h.session_key).collect::<Vec<_>>()
    );
    assert!(!hits.iter().any(|h| h.session_key == k2.replace(':', "_")));

    crate::chat_log::delete_chat_log(&k1);
    crate::chat_log::delete_chat_log(&k2);
}

#[test]
fn test_fts_english_and_snippet() {
    let _lock = IDX_LOCK.lock();
    let k = fresh_session("en");
    crate::chat_log::append_chat_log(&k, "user", "the quick brown fox jumps over the lazy dog");
    crate::chat_log::append_chat_log(&k, "assistant", "a plain reply about something else");
    reindex_session_logs();

    let hits = search("brown fox", 10);
    assert!(!hits.is_empty(), "english phrase hits");
    let h = &hits[0];
    assert_eq!(h.role, "user");
    // Same ghost-row lesson as the incremental test: pin the session so a
    // stale row from a deleted prior-run file can never satisfy the assert.
    assert_eq!(h.session_key, k.replace(':', "_"));
    assert!(h.snippet.contains("brown"), "snippet: {}", h.snippet);

    // No-hit query returns empty (not an error).
    assert!(search("zebraunicorn", 10).is_empty());

    crate::chat_log::delete_chat_log(&k);
}

#[test]
fn test_reindex_idempotent() {
    let _lock = IDX_LOCK.lock();
    let k = fresh_session("idem");
    let stem = k.replace(':', "_");
    crate::chat_log::append_chat_log(&k, "user", "idempotency probe content xyzzy");
    // First index.
    reindex_session_logs();
    // A second full pass must not duplicate this session's rows (the
    // DELETE-before-INSERT per-file contract), even if a concurrent test
    // touched OTHER files (the global "changed" count is racy across tests;
    // per-stem row count is the real correctness invariant).
    reindex_session_logs();
    let hits: Vec<_> = search("xyzzy", 50)
        .into_iter()
        .filter(|h| h.session_key == stem)
        .collect();
    assert_eq!(hits.len(), 1, "no duplicates for this session: {hits:?}");

    crate::chat_log::delete_chat_log(&k);
}

#[test]
fn test_index_append_incremental() {
    let _lock = IDX_LOCK.lock();
    let k = fresh_session("incr");
    // Full-index first (marks the file known).
    crate::chat_log::append_chat_log(&k, "user", "before marker plugh");
    reindex_session_logs();
    // Append AFTER indexing — index_append should pick it up without a
    // full reindex.
    crate::chat_log::append_chat_log(&k, "assistant", "after marker wabbajack");
    let hits = search("wabbajack", 10);
    assert_eq!(hits.len(), 1, "appended row indexed incrementally: {hits:?}");
    assert_eq!(hits[0].role, "assistant");
    // Key assertion: without it a STALE row from a deleted prior-run file
    // could satisfy this test (which is exactly how the raw-key/stem lookup
    // mismatch in index_append stayed masked — ghost rows answered the
    // query while the real append was never indexed).
    assert_eq!(hits[0].session_key, k.replace(':', "_"));

    crate::chat_log::delete_chat_log(&k);
}

#[test]
fn test_render_hits_shapes() {
    let hits = vec![HistoryHit {
        session_key: "web_chat1".into(),
        seq: 3,
        role: "user".into(),
        timestamp: "2026-08-23T00:00:00+08:00".into(),
        snippet: "…matched text…".into(),
    }];
    let s = render_hits(&hits);
    assert!(s.contains("1 条匹配"));
    assert!(s.contains("web_chat1"));
    assert!(s.contains("matched text"));
    assert_eq!(render_hits(&[]), "没有找到匹配的历史消息。");
}

#[test]
fn test_search_empty_query() {
    assert!(search("   ", 10).is_empty());
    assert!(search("", 10).is_empty());
}
