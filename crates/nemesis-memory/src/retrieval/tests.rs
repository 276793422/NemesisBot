use super::*;

#[test]
fn tokens_latin_and_cjk() {
    let t = tokens("Hello 世界 foo_bar");
    assert!(t.contains(&"hello".to_string()));
    assert!(t.contains(&"世".to_string()));
    assert!(t.contains(&"界".to_string()));
    assert!(t.contains(&"foo_bar".to_string()));
}

#[test]
fn bm25_ranks_relevant_doc_higher() {
    let d1 = term_counts(&tokens("rust async runtime tokio"));
    let d2 = term_counts(&tokens("python web framework django"));
    let docs = vec![d1.clone(), d2.clone()];
    let df = document_frequency(&docs);
    let q = query_terms("rust tokio");
    let s1 = bm25_score(&d1, 4, &q, &df, 2, 4.0);
    let s2 = bm25_score(&d2, 4, &q, &df, 2, 4.0);
    assert!(s1 > s2, "doc with terms should score higher: {s1} vs {s2}");
    assert!(s2 == 0.0, "doc without terms should score 0, got {s2}");
}

/// S22 计分可复现回归锁（2026-11-19；同日修正断言前提）：
/// **确定性 = 同输入恒同产出**，而非「跨输入序同产出」——query_terms 保
/// 首现序（HashSet 只去重不改序），同集合不同词序产出的序列本就不同
/// （首版断言 q1==q2 与该设计直接矛盾、恒假，属测试前提错误非实现缺陷；
/// 首版「已跑绿」记录作废）。真实不变量三层：
/// ① 同输入两次调用 → 同去重序列 → bm25 分值逐位相等；
/// ② 不同输入序 → 序列不同（首现序由输入完全决定）但去重后**词集相同**
///    （filter-only 消费语义只依赖词集：词命中与否与序无关）；
/// ③ 跨输入序的分值**不承诺逐位相等**（bm25_score 沿词序累加，序不同
///    f64 尾数可差）——消费面（logs.rs）BM25 是 filter-only + recency
///    重排覆盖（BUG-20261116-011），分值不进最终排序，词序差异零可见
///    影响。avg/len 均为整数和（HashMap 迭代序不影响精确和）。
#[test]
fn bm25_scoring_is_bit_deterministic() {
    let d1 = term_counts(&tokens("rust async runtime tokio runtime"));
    let d2 = term_counts(&tokens("python web framework django"));
    let docs = vec![d1.clone(), d2.clone()];
    let df = document_frequency(&docs);
    // ① 同输入：重跑序列与分值逐位稳定。
    let q1 = query_terms("tokio rust runtime async");
    let q1b = query_terms("tokio rust runtime async");
    assert_eq!(q1, q1b, "same input must yield identical dedup sequence");
    let s1 = bm25_score(&d1, 5, &q1, &df, 2, 5.0);
    let s1_again = bm25_score(&d1, 5, &q1, &df, 2, 5.0);
    assert_eq!(s1, s1_again, "repeated scoring must be bit-stable");
    assert!(s1 > 0.0, "all query terms hit d1, score must be positive");
    // ② 不同输入序：首现序不同（由输入完全决定），去重后词集相同。
    let q2 = query_terms("async tokio runtime rust");
    assert_ne!(
        q1, q2,
        "first-occurrence order must follow input order (by design)"
    );
    let mut set1 = q1.clone();
    set1.sort();
    let mut set2 = q2.clone();
    set2.sort();
    assert_eq!(set1, set2, "same term set regardless of input order");
    let s2 = bm25_score(&d1, 5, &q2, &df, 2, 5.0);
    let s2_again = bm25_score(&d1, 5, &q2, &df, 2, 5.0);
    assert_eq!(s2, s2_again, "q2 repeated scoring must be bit-stable too");
}

#[test]
fn keep_top_trims_weak() {
    let items = vec![1.0_f64, 0.9, 0.1];
    let kept = keep_top_relative_score(items, 0.5, |x: &f64| *x);
    assert_eq!(kept.len(), 2); // 1.0 and 0.9 kept, 0.1 dropped
}

#[test]
fn snippet_truncates_with_ellipsis() {
    let s = make_snippet("a b c d e f g h i j k", &["f".to_string()], 5);
    assert!(s.contains("..."), "snippet should be elided: {s}");
}

#[test]
fn query_terms_empty_on_noise() {
    assert!(query_terms("   !!! @@@   ").is_empty());
    assert!(!query_terms("rust 状态").is_empty());
}

#[test]
fn bm25_empty_corpus_or_query_returns_zero() {
    // Boundary: no docs / no total → score is 0, never panics or divides by zero.
    let df = document_frequency(&[]);
    let q = query_terms("rust");
    assert_eq!(bm25_score(&HashMap::new(), 0, &q, &df, 0, 0.0), 0.0);
    // avg_len <= 0 is clamped internally.
    let d1 = term_counts(&tokens("rust"));
    let df2 = document_frequency(std::slice::from_ref(&d1));
    assert!(bm25_score(&d1, 1, &query_terms("rust"), &df2, 1, 0.0) > 0.0);
}

#[test]
fn keep_top_relative_score_empty_and_zero_top() {
    // Boundary: empty input returned as-is; top score 0 → returned unchanged.
    let empty: Vec<f64> = vec![];
    assert!(keep_top_relative_score(empty, 0.5, |x: &f64| *x).is_empty());
    let zeros = vec![0.0_f64, 0.0];
    assert_eq!(
        keep_top_relative_score(zeros, 0.5, |x: &f64| *x).len(),
        2,
        "zero top score → no trimming"
    );
}

#[test]
fn bm25_term_missing_from_df_is_skipped() {
    // counts has the term but df does not → df-miss continue (score stays 0).
    let mut counts = HashMap::new();
    counts.insert("ghost".to_string(), 3usize);
    let df = document_frequency(&[]); // empty df: no term has a df entry
    let q = vec!["ghost".to_string()];
    let score = bm25_score(&counts, 3, &q, &df, 5, 3.0);
    assert_eq!(score, 0.0, "term absent from df must contribute nothing");
    // df entry of 0 is equally skipped.
    let mut df0 = HashMap::new();
    df0.insert("ghost".to_string(), 0usize);
    assert_eq!(bm25_score(&counts, 3, &q, &df0, 5, 3.0), 0.0);
    // tf of 0 is skipped too (counts value 0).
    let mut counts0 = HashMap::new();
    counts0.insert("ghost".to_string(), 0usize);
    let mut df1 = HashMap::new();
    df1.insert("ghost".to_string(), 1usize);
    assert_eq!(bm25_score(&counts0, 3, &q, &df1, 5, 3.0), 0.0);
}

#[test]
fn make_snippet_zero_max_and_short_text_return_whole() {
    // max_chars == 0 → early return of whitespace-compacted text.
    assert_eq!(
        make_snippet("  hello   world  ", &["zz".to_string()], 0),
        "hello world"
    );
    // text shorter than max_chars → early return too.
    assert_eq!(make_snippet("tiny", &["zz".to_string()], 10), "tiny");
}

#[test]
fn make_snippet_centers_on_multichar_hit() {
    let words: Vec<String> = (0..20).map(|i| format!("w{i:02}")).collect();
    let text = words.join(" ");
    let snippet = make_snippet(&text, &["w10".to_string()], 11);
    assert!(
        snippet.contains("w10"),
        "snippet must contain the hit: {snippet}"
    );
    assert!(
        snippet.starts_with("..."),
        "start>0 → prefix ellipsis: {snippet}"
    );
    assert!(
        snippet.ends_with("..."),
        "end<total → suffix ellipsis: {snippet}"
    );
}

#[test]
fn make_snippet_cjk_single_char_is_a_valid_term() {
    // Single CJK char must NOT be skipped by the latin-only guard.
    let words: Vec<String> = (0..15).map(|i| format!("t{i:02}")).collect();
    let text = format!("{} 你好吗 {}", words[..3].join(" "), words[3..].join(" "));
    let snippet = make_snippet(&text, &["你".to_string()], 9);
    assert!(
        snippet.contains('你'),
        "CJK single-char term must hit: {snippet}"
    );
}

#[test]
fn make_snippet_hit_near_end_recomputes_start() {
    // Hit at the tail: start+max > total → end=total, start recomputed from end.
    let words: Vec<String> = (0..10).map(|i| format!("p{i:02}")).collect();
    let text = words.join(" ");
    let snippet = make_snippet(&text, &["p09".to_string()], 7);
    assert!(
        snippet.contains("p09"),
        "snippet must contain tail hit: {snippet}"
    );
    assert!(
        snippet.starts_with("..."),
        "recomputed start>0 → ellipsis: {snippet}"
    );
    assert!(
        !snippet.ends_with("..."),
        "end==total → no suffix: {snippet}"
    );
}

#[test]
fn find_subslice_boundaries() {
    let hay: Vec<char> = "abcabd".chars().collect();
    // Empty needle → None.
    assert_eq!(find_subslice(&hay, &[]), None);
    // Needle longer than haystack → None.
    let long: Vec<char> = "abcdefg".chars().collect();
    assert_eq!(find_subslice(&hay, &long), None);
    // Match at the last possible offset.
    let needle: Vec<char> = "bd".chars().collect();
    assert_eq!(find_subslice(&hay, &needle), Some(4));
    // No match.
    let miss: Vec<char> = "xyz".chars().collect();
    assert_eq!(find_subslice(&hay, &miss), None);
}

// ---- coverage: CJK-boundary buffer flush + snippet hit path ----

#[test]
fn tokens_flushes_latin_run_before_cjk() {
    // The pending latin buffer must flush when the first CJK rune arrives,
    // then each CJK rune becomes its own term; trailing latin run flushes
    // at the end of input.
    let t = tokens("abc中文x9");
    assert_eq!(t, vec!["abc", "中", "文", "x9"]);
}

#[test]
fn make_snippet_centers_on_first_hit() {
    let text = "alpha beta gamma delta epsilon";
    let out = make_snippet(text, &["beta".to_string()], 12);
    assert!(
        out.contains("beta"),
        "snippet must be built around the first term hit, got: {out}"
    );
}

// ---------------------------------------------------------------------------
// AGT 覆盖率批次：make_snippet 无命中臂——terms 都不在文本里
// → hit 保持 None → 从文本头部开窗（unwrap_or(0) 臂）。
// ---------------------------------------------------------------------------

#[test]
fn agt_make_snippet_no_hit_starts_at_beginning() {
    let words: Vec<String> = (0..40).map(|i| format!("w{i:02}")).collect();
    let text = words.join(" ");
    let snippet = make_snippet(&text, &["absent-term".to_string()], 30);
    // 无命中 → 窗口从开头起
    assert!(snippet.starts_with("w00"), "got: {snippet}");
    assert!(snippet.chars().count() <= 30 + "...".len());
}

#[test]
fn agt_make_snippet_single_char_latin_terms_skipped() {
    // 单字符非 CJK term 被跳过（continue 臂）——不会把窗口拉到任意单字母处
    let text = "a big whale swims";
    let snippet = make_snippet(text, &["a".to_string(), "big".to_string()], 100);
    assert_eq!(snippet, text);
}
