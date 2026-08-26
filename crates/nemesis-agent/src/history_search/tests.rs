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

/// M3 补测（quality-hardening goal 2026-08-25）：MATCH 表达式构造的直接断言。
/// 此前只经 FTS 集成测试间接走到（命中≠表达式形状正确），这里钉住三件事：
/// 短语臂 / CJK bigram 臂的形状、拉丁词不进 bigram 臂、FTS5 语法注入转义。
#[test]
fn test_match_expr_shapes_and_injection_escape() {
    // 纯英文 → 只有短语臂（无 bigram 臂）
    assert_eq!(match_expr("brown fox"), r#"content : "brown fox""#);
    // 中文 → 短语臂 + bigram 臂（cjk_bigrams 展开后逐个 OR）
    assert_eq!(
        match_expr("部署"),
        r#"content : "部署" OR content_bigram : "部署""#
    );
    assert_eq!(
        match_expr("部署文档"),
        r#"content : "部署文档" OR content_bigram : "部署" OR content_bigram : "署文" OR content_bigram : "文档""#
    );
    // 混合：拉丁词被过滤出 bigram 臂（短语臂已覆盖），CJK bigram 保留
    assert_eq!(
        match_expr("deploy 部署 now"),
        r#"content : "deploy 部署 now" OR content_bigram : "部署""#
    );
    // FTS5 注入转义：查询里的双引号翻倍，破坏短语定界
    assert_eq!(
        match_expr("he said \"hi\""),
        "content : \"he said \"\"hi\"\"\""
    );
}

// --- W3a: 纯 helper 补测 —— CJK ext-B 字符、snippet_around 椭圆臂与
// 字符边界回路。有状态族（reindex/search_linear）按本文件头约定留在
// tests/history_search_fts.rs 专属集成二进制，不回迁。 ---

#[test]
fn cjk_bigrams_covers_extension_b_plane() {
    // U+20000..=U+2A6DF（CJK ext B）也算 CJK：单字 1-gram、双字 bigram。
    assert_eq!(cjk_bigrams("𠀀"), "𠀀");
    assert_eq!(cjk_bigrams("𠀀𠀁"), "𠀀𠀁");
    // 与拉丁词相邻时分属两段。注意：单字 CJK flush 不补尾随空格（那个
    // 补空格的 if 在 len>1 的 else 分支里），所以后跟的拉丁词直接粘连。
    assert_eq!(cjk_bigrams("x𠀀y"), "x 𠀀y");
}

#[test]
fn snippet_around_ellipsis_and_boundary_arms() {
    // 内容远长于窗口：needle 居中 → 前后都加 "…"，且切窗不 panic。
    let pad = "前".repeat(40);
    let tail = "后".repeat(40);
    let content = format!("{pad}needle{tail}");
    let s = snippet_around(&content, "needle", 40);
    assert!(s.starts_with('…'), "leading ellipsis: {s}");
    assert!(s.ends_with('…'), "trailing ellipsis: {s}");
    assert!(s.contains("needle"));
    // start = pos - 40/3 落在多字节字符中间 → floor_char_boundary 回路。
    assert!(s.chars().all(|c| !c.is_ascii_control()));

    // needle 靠开头：start=0 无前缀省略号，只有尾部 "…"。
    let head = format!("needle{}", tail);
    let s2 = snippet_around(&head, "needle", 32);
    assert!(!s2.starts_with('…'), "no leading ellipsis at pos 0: {s2}");
    assert!(s2.ends_with('…'));

    // 查询词不存在 → pos=0，同样无前缀省略号。
    let s3 = snippet_around("完全无关的短内容", "absent", 32);
    assert!(!s3.starts_with('…'), "miss query → pos 0: {s3}");

    // 空内容安全。
    assert_eq!(snippet_around("", "x", 32), "");
}

#[test]
fn char_boundary_helpers_floor_and_ceil() {
    // 直接钉住边界回路：中文字符 3 字节，切在中间必须被 floor/ceil 归位。
    let s = "中文边界检查";
    // floor：从中间字节往回退到边界（"中"=0..3，"文"=3..6 …）
    assert_eq!(floor_char_boundary(s, 1), 0);
    assert_eq!(floor_char_boundary(s, 4), 3);
    assert_eq!(floor_char_boundary(s, 100), s.len());
    // ceil：从中间字节前进到边界。
    assert_eq!(ceil_char_boundary(s, 1), 3);
    assert_eq!(ceil_char_boundary(s, 4), 6);
    assert_eq!(ceil_char_boundary(s, 100), s.len());
}
