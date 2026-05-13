//! Local character n-gram hash embedding function.
//!
//! Zero token cost, zero external dependencies, fully offline.
//! Algorithm: tokenize -> character trigram extraction -> FNV dual-hash mapping -> L2 normalization

/// Produce a fixed-dimension embedding vector from text using character trigram hashing.
pub fn ngram_hash_embed(text: &str, dim: usize) -> Vec<f32> {
    let dim = if dim == 0 { 256 } else { dim };
    let mut vec = vec![0.0f32; dim];

    if text.is_empty() {
        return vec;
    }

    let words = tokenize_for_embedding(text);
    for word in words {
        let trigrams = extract_trigrams(&word);
        for tg in trigrams {
            // First hash: determine dimension index
            let h1 = fnv_hash32(&tg);
            let idx = (h1 as usize) % dim;
            // Second hash: determine sign (+1 or -1)
            let h2 = fnv_hash32(&format!("{}_sign", tg));
            if h2 % 2 == 0 {
                vec[idx] += 1.0;
            } else {
                vec[idx] -= 1.0;
            }
        }
    }

    l2_normalize(&mut vec);
    vec
}

/// The standard local embedding function (for use as a closure).
pub fn local_embedding_func(dim: usize) -> impl Fn(&str) -> Result<Vec<f32>, String> + Send + Sync {
    move |text: &str| Ok(ngram_hash_embed(text, dim))
}

/// Tokenize text into lowercase words suitable for embedding.
fn tokenize_for_embedding(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_punctuation() {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// Extract character trigrams from a word with boundary markers.
///
/// Example: "hello" -> ["#he", "hel", "ell", "llo", "lo#"]
fn extract_trigrams(word: &str) -> Vec<String> {
    if word.is_empty() {
        return Vec::new();
    }

    let padded = format!("#{}#", word);
    if padded.len() < 3 {
        return vec![padded];
    }

    let mut trigrams = Vec::new();
    let bytes = padded.as_bytes();
    for i in 0..=bytes.len().saturating_sub(3) {
        // Use chars for proper Unicode handling
        if let Some(slice) = padded.get(i..i + 3) {
            trigrams.push(slice.to_string());
        }
    }
    trigrams
}

/// Compute FNV-1a 32-bit hash.
fn fnv_hash32(s: &str) -> u32 {
    const PRIME32: u32 = 16777619;
    const OFFSET32: u32 = 2166136261;

    let mut h = OFFSET32;
    for byte in s.bytes() {
        h ^= byte as u32;
        h = h.wrapping_mul(PRIME32);
    }
    h
}

/// L2-normalize a vector in place. If the vector is zero, it is left unchanged.
fn l2_normalize(vec: &mut [f32]) {
    let sum: f64 = vec.iter().map(|v| (*v as f64) * (*v as f64)).sum();
    let norm = sum.sqrt() as f32;
    if norm > 0.0 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for i in 0..a.len() {
        dot += a[i] as f64 * b[i] as f64;
        norm_a += a[i] as f64 * a[i] as f64;
        norm_b += b[i] as f64 * b[i] as f64;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a.sqrt() * norm_b.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ngram_hash_embed_non_empty() {
        let vec = ngram_hash_embed("hello world", 256);
        assert_eq!(vec.len(), 256);
        // Should be L2-normalized
        let norm: f64 = vec.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_ngram_hash_embed_empty() {
        let vec = ngram_hash_embed("", 256);
        assert!(vec.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn test_similar_texts_have_high_similarity() {
        let vec1 = ngram_hash_embed("the cat sat on the mat", 256);
        let vec2 = ngram_hash_embed("the cat is on the mat", 256);
        let vec3 = ngram_hash_embed("quantum physics equations", 256);

        let sim12 = cosine_similarity(&vec1, &vec2);
        let sim13 = cosine_similarity(&vec1, &vec3);

        assert!(sim12 > sim13, "Similar texts should have higher similarity");
    }

    #[test]
    fn test_extract_trigrams() {
        let trigrams = extract_trigrams("hello");
        assert!(trigrams.contains(&"#he".to_string()));
        assert!(trigrams.contains(&"lo#".to_string()));
    }

    #[test]
    fn test_fnv_hash32_deterministic() {
        let h1 = fnv_hash32("test");
        let h2 = fnv_hash32("test");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_cosine_similarity_same_vector() {
        let vec = ngram_hash_embed("hello", 128);
        let sim = cosine_similarity(&vec, &vec);
        assert!((sim - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_local_embedding_func() {
        let func = local_embedding_func(256);
        let result = func("test text").unwrap();
        assert_eq!(result.len(), 256);
    }

    #[test]
    fn test_different_dimensions() {
        let vec128 = ngram_hash_embed("hello", 128);
        let vec512 = ngram_hash_embed("hello", 512);
        assert_eq!(vec128.len(), 128);
        assert_eq!(vec512.len(), 512);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_ngram_hash_embed_zero_dim_defaults() {
        let vec = ngram_hash_embed("hello", 0);
        assert_eq!(vec.len(), 256); // defaults to 256
    }

    #[test]
    fn test_ngram_hash_embed_single_char() {
        let vec = ngram_hash_embed("a", 64);
        assert_eq!(vec.len(), 64);
        // Should have some non-zero values
        assert!(vec.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn test_ngram_hash_embed_deterministic() {
        let v1 = ngram_hash_embed("deterministic test", 256);
        let v2 = ngram_hash_embed("deterministic test", 256);
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_cosine_similarity_opposite_vectors() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![-1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 0.01);
    }

    #[test]
    fn test_cosine_similarity_orthogonal_vectors() {
        let a = vec![1.0f32, 0.0, 0.0];
        let b = vec![0.0f32, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_cosine_similarity_different_lengths() {
        let a = vec![1.0f32, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_cosine_similarity_empty_vectors() {
        let a: Vec<f32> = vec![];
        let b: Vec<f32> = vec![];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_cosine_similarity_zero_vectors() {
        let a = vec![0.0f32, 0.0, 0.0];
        let b = vec![1.0f32, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_cosine_similarity_range() {
        let vec1 = ngram_hash_embed("first text", 256);
        let vec2 = ngram_hash_embed("second text", 256);
        let sim = cosine_similarity(&vec1, &vec2);
        assert!(sim >= -1.0 && sim <= 1.0);
    }

    #[test]
    fn test_tokenize_for_embedding_lowercase() {
        let tokens = tokenize_for_embedding("Hello World");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_tokenize_for_embedding_strips_punctuation() {
        let tokens = tokenize_for_embedding("hello, world! (test)");
        assert_eq!(tokens, vec!["hello", "world", "test"]);
    }

    #[test]
    fn test_tokenize_for_embedding_whitespace_handling() {
        let tokens = tokenize_for_embedding("  multiple   spaces  ");
        assert_eq!(tokens, vec!["multiple", "spaces"]);
    }

    #[test]
    fn test_tokenize_for_embedding_empty() {
        let tokens = tokenize_for_embedding("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_extract_trigrams_empty() {
        let trigrams = extract_trigrams("");
        assert!(trigrams.is_empty());
    }

    #[test]
    fn test_extract_trigrams_single_char() {
        let trigrams = extract_trigrams("a");
        // "#a#" -> single trigram since len is 3
        assert!(!trigrams.is_empty());
    }

    #[test]
    fn test_extract_trigrams_two_chars() {
        let trigrams = extract_trigrams("ab");
        // "#ab#" -> "#ab", "ab#" = 2 trigrams
        assert_eq!(trigrams.len(), 2);
    }

    #[test]
    fn test_extract_trigrams_boundary_markers() {
        let trigrams = extract_trigrams("cat");
        assert!(trigrams.contains(&"#ca".to_string()));
        assert!(trigrams.contains(&"at#".to_string()));
        assert!(trigrams.contains(&"cat".to_string()));
    }

    #[test]
    fn test_fnv_hash32_different_inputs() {
        let h1 = fnv_hash32("hello");
        let h2 = fnv_hash32("world");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_fnv_hash32_empty() {
        let h = fnv_hash32("");
        // FNV offset basis
        assert_eq!(h, 2166136261);
    }

    #[test]
    fn test_l2_normalize_unit_vector() {
        let mut vec = vec![3.0f32, 4.0f32];
        l2_normalize(&mut vec);
        let norm: f64 = vec.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_l2_normalize_zero_vector() {
        let mut vec = vec![0.0f32, 0.0f32, 0.0f32];
        l2_normalize(&mut vec);
        assert!(vec.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn test_embedding_with_unicode() {
        let vec = ngram_hash_embed("日本語テスト", 256);
        assert_eq!(vec.len(), 256);
    }

    #[test]
    fn test_embedding_with_numbers() {
        let vec = ngram_hash_embed("test123 number456", 256);
        assert_eq!(vec.len(), 256);
        assert!(vec.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn test_local_embedding_func_closure() {
        let func = local_embedding_func(128);
        let r1 = func("test").unwrap();
        let r2 = func("test").unwrap();
        assert_eq!(r1, r2); // deterministic
    }

    #[test]
    fn test_ngram_hash_embed_large_dim() {
        let vec = ngram_hash_embed("hello world", 1024);
        assert_eq!(vec.len(), 1024);
    }

    #[test]
    fn test_ngram_hash_embed_very_long_text() {
        let text = "word ".repeat(10000);
        let vec = ngram_hash_embed(&text, 256);
        assert_eq!(vec.len(), 256);
        let norm: f64 = vec.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }
}
