//! Lightweight BM25 lexical retrieval (no external dependency).
//!
//! A local, dependency-free approximation of FTS matching for memory/session
//! text search. Lexical only — not semantic. Used as a fallback when the vector
//! store is unavailable, and for session log search. Standard BM25 with k1=1.2,
//! b=0.75, +1-smoothed IDF, plus CJK single-rune tokenization so
//! Chinese/Japanese/Korean text is searchable.

use std::collections::HashMap;

/// Tokenize: lowercase Latin/digit/underscore runs, split CJK into single-rune terms.
pub fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    for r in s.chars() {
        if is_cjk(r) {
            if !buf.is_empty() {
                out.push(std::mem::take(&mut buf));
            }
            out.push(r.to_string());
        } else if r.is_alphanumeric() || r == '_' {
            buf.push(r.to_ascii_lowercase());
        } else if !buf.is_empty() {
            out.push(std::mem::take(&mut buf));
        }
    }
    if !buf.is_empty() {
        out.push(buf);
    }
    out
}

/// Is this a CJK rune (Han / Hiragana / Katakana / Hangul common ranges)?
pub fn is_cjk(r: char) -> bool {
    let c = r as u32;
    (0x4E00..=0x9FFF).contains(&c) // CJK Unified Ideographs
        || (0x3400..=0x4DBF).contains(&c) // CJK Extension A
        || (0x3040..=0x30FF).contains(&c) // Hiragana + Katakana
        || (0xAC00..=0xD7AF).contains(&c) // Hangul Syllables
}

/// Term-frequency map from a token slice.
pub fn term_counts(terms: &[String]) -> HashMap<String, usize> {
    let mut m = HashMap::new();
    for t in terms {
        *m.entry(t.clone()).or_insert(0) += 1;
    }
    m
}

/// Document frequency: how many docs contain each term.
pub fn document_frequency(docs: &[HashMap<String, usize>]) -> HashMap<String, usize> {
    let mut df = HashMap::new();
    for counts in docs {
        for term in counts.keys() {
            *df.entry(term.clone()).or_insert(0) += 1;
        }
    }
    df
}

/// BM25 score for one document against query terms.
pub fn bm25_score(
    counts: &HashMap<String, usize>,
    doc_len: usize,
    query_terms: &[String],
    df: &HashMap<String, usize>,
    total_docs: usize,
    avg_len: f64,
) -> f64 {
    const K1: f64 = 1.2;
    const B: f64 = 0.75;
    if doc_len == 0 || total_docs == 0 {
        return 0.0;
    }
    let avg = if avg_len > 0.0 { avg_len } else { 1.0 };
    let doc_len_f = doc_len as f64;
    let mut score = 0.0;
    for term in query_terms {
        let tf = match counts.get(term) {
            Some(&v) if v > 0 => v as f64,
            _ => continue,
        };
        let term_df = match df.get(term) {
            Some(&v) if v > 0 => v,
            _ => continue,
        };
        let idf = (((total_docs - term_df) as f64 + 0.5) / (term_df as f64 + 0.5) + 1.0).ln();
        score += idf * (tf * (K1 + 1.0)) / (tf + K1 * (1.0 - B + B * doc_len_f / avg));
    }
    score
}

/// Keep items whose score is >= `ratio * top_score` (top always kept). Input must
/// be sorted best-first. Trims common-word-only noise without an absolute cutoff.
pub fn keep_top_relative_score<T, F>(items: Vec<T>, ratio: f64, score: F) -> Vec<T>
where
    F: Fn(&T) -> f64,
{
    if items.is_empty() || ratio <= 0.0 {
        return items;
    }
    let top = score(&items[0]);
    if top <= 0.0 {
        return items;
    }
    let cutoff = top * ratio;
    items
        .into_iter()
        .enumerate()
        .filter(|(i, it)| *i == 0 || score(it) >= cutoff)
        .map(|(_, it)| it)
        .collect()
}

/// Normalize a search string to unique query terms. Empty if nothing searchable.
pub fn query_terms(query: &str) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for t in tokens(query.trim()) {
        if !t.is_empty() && seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// Build a whitespace-compacted snippet around the first query-term hit, with
/// `...` elision when truncated. `max_chars` is in chars (rune-aware).
pub fn make_snippet(text: &str, terms: &[String], max_chars: usize) -> String {
    let text: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if max_chars == 0 || text.chars().count() <= max_chars {
        return text;
    }
    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = text.chars().map(|c| c.to_ascii_lowercase()).collect();

    let mut hit = None;
    for term in terms {
        let tc: Vec<char> = term.chars().collect();
        if tc.len() == 1 && !is_cjk(tc[0]) {
            continue;
        }
        if let Some(pos) = find_subslice(&lower, &tc) {
            hit = Some(pos);
            break;
        }
    }
    let pos = hit.unwrap_or(0);
    let total = chars.len();
    let mut start = pos.saturating_sub(max_chars / 2);
    let mut end = start + max_chars;
    if end > total {
        end = total;
        start = end.saturating_sub(max_chars);
    }
    let prefix = if start > 0 { "..." } else { "" };
    let suffix = if end < total { "..." } else { "" };
    let body: String = chars[start..end].iter().collect();
    format!("{prefix}{body}{suffix}")
}

fn find_subslice(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

#[cfg(test)]
mod tests;
