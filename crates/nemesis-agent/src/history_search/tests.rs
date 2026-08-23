//! history_search unit tests (U20 sixth batch) — PURE helpers only.
//!
//! The STATEFUL family (reindex/search against real session logs: the
//! fts_chinese / english / ghost-purge / idempotent / incremental tests)
//! moved to `tests/history_search_fts.rs`, a DEDICATED integration binary.
//! Reason: `default_path_manager()` is a process-global OnceLock — in this
//! lib test binary ~1400 sibling tests bake the home to the shared
//! `~/.nemesisbot` before any test here could redirect it, which is exactly
//! the cross-binary sharing that made the fts family flaky. The dedicated
//! binary sets `NEMESISBOT_HOME` to a per-process tempdir before the
//! singleton's first resolution. Do not move them back (see that file's
//! header). What remains here touches no global state.

use super::*;

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
