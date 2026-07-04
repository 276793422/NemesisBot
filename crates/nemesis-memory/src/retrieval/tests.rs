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
    let df2 = document_frequency(&[d1.clone()]);
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
