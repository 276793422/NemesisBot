//! Y1 (Phase4-a): semantic tool-documentation folding — PURE helpers.
//!
//! VCP-inspired: with a large toolset (Big tier = 40+ tools) most tool
//! descriptions are irrelevant to the current conversation. When enabled
//! (`agents.tool_doc_folding`), the loop ranks tools by cosine similarity
//! between the latest user message and each tool description (P3.1 embed
//! backend), keeps the top-N fully expanded, and collapses the rest to a
//! one-line summary. Folding is orthogonal to tier supply: it rewrites the
//! description TEXT only — every tool stays callable with its full
//! parameter schema.
//!
//! Everything here is deterministic and side-effect free: same inputs →
//! byte-identical outputs (no clocks, no map-iteration-order leakage —
//! rank ties break alphabetically). The loop-side gating (config, tier,
//! embed availability) lives in `AgentLoop::apply_tool_doc_folding`.

use crate::types::ToolDefinition;
use std::collections::HashMap;

/// Default `expand_top_n` when the config section omits it.
pub const DEFAULT_EXPAND_TOP_N: usize = 8;

/// Cap for a folded one-line summary, in CHARS (never bytes — str byte
/// slicing panics mid-codepoint on CJK text).
pub const SUMMARY_CHAR_CAP: usize = 100;

/// Cosine similarity between two vectors. Zero-length or zero-norm inputs
/// yield 0.0 (an embedding that carries no direction cannot be similar to
/// anything — deterministic, no NaN).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let n = a.len().min(b.len());
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for i in 0..n {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na <= 0.0 || nb <= 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Extract the first sentence of a tool description as the folded
/// one-liner. Cuts at the first sentence terminator — CJK `。！？；`, a
/// newline, an ASCII semicolon, or an ASCII period that ENDS a sentence:
/// followed by end-of-text, or by whitespace whose next non-whitespace
/// char is uppercase/digit (sentence-start heuristic — so `e.g. ls` or
/// `vs. code`, where the next word is lowercase, does NOT cut). Capped at
/// [`SUMMARY_CHAR_CAP`] chars on a char boundary with a `…` suffix.
pub fn one_line_summary(desc: &str) -> String {
    let trimmed = desc.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let mut cut: Option<usize> = None;
    for (i, ch) in trimmed.char_indices() {
        match ch {
            // CJK terminators: keep the punctuation (conventional in a
            // Chinese one-liner).
            '。' | '！' | '？' | '；' => {
                cut = Some(i + ch.len_utf8());
                break;
            }
            // ASCII semicolon / newline: drop the terminator itself.
            ';' | '\n' => {
                cut = Some(i);
                break;
            }
            '.' => {
                let rest = &trimmed[i + 1..];
                let next = rest.chars().next();
                let ends_sentence = match next {
                    None => true,
                    Some(_) => {
                        let word_start = rest.trim_start().chars().next();
                        match word_start {
                            None => false,
                            Some(c) => c.is_uppercase() || c.is_ascii_digit(),
                        }
                    }
                };
                if ends_sentence {
                    cut = Some(i + 1);
                    break;
                }
            }
            _ => {}
        }
    }
    let sentence: &str = match cut {
        Some(i) => &trimmed[..i],
        None => trimmed,
    };
    if sentence.chars().count() > SUMMARY_CHAR_CAP {
        let end = sentence
            .char_indices()
            .nth(SUMMARY_CHAR_CAP)
            .map(|(i, _)| i)
            .unwrap_or(sentence.len());
        format!("{}…", &sentence[..end])
    } else {
        sentence.to_string()
    }
}

/// Fold tool definitions: the `expand_top_n` tools ranked highest by
/// similarity keep their description BYTES untouched; every other tool's
/// description is replaced by [`one_line_summary`]. Order, names, and
/// parameter schemas are never modified. `expand_top_n >= defs.len()` (or
/// an empty set) is a byte-identical passthrough — nothing to fold.
///
/// Ranking: similarity desc, then name asc for deterministic ties; a tool
/// missing from `similarities` ranks 0.0 (folds unless top-N is large
/// enough to swallow it).
pub fn fold_tool_defs(
    defs: Vec<ToolDefinition>,
    similarities: &HashMap<String, f32>,
    expand_top_n: usize,
) -> Vec<ToolDefinition> {
    if defs.is_empty() || expand_top_n >= defs.len() {
        return defs;
    }
    let mut ranked: Vec<&ToolDefinition> = defs.iter().collect();
    ranked.sort_by(|a, b| {
        let sa = similarities.get(&a.function.name).copied().unwrap_or(0.0);
        let sb = similarities.get(&b.function.name).copied().unwrap_or(0.0);
        sb.partial_cmp(&sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.function.name.cmp(&b.function.name))
    });
    let expand: std::collections::HashSet<String> = ranked
        .into_iter()
        .take(expand_top_n)
        .map(|d| d.function.name.clone())
        .collect();
    let mut defs = defs;
    for d in defs.iter_mut() {
        if !expand.contains(&d.function.name) {
            d.function.description = one_line_summary(&d.function.description);
        }
    }
    defs
}

#[cfg(test)]
mod tests;
