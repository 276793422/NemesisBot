//! plugin-onnx — NemesisBot ONNX embedding plugin DLL
//!
//! Provides C ABI exports for computing text embeddings using a local ONNX model.
//! Designed for BERT-style sentence embedding models (e.g. all-MiniLM-L6-v2).
//!
//! C ABI contract:
//! - `embed_init(model_path, dim)` → i32 (0 = success)
//! - `embed(text, out, dim)` → i32 (0 = success)
//! - `embed_free()` → release resources
//!
//! Pipeline: tokenize → ONNX inference → mean pooling → L2 normalize
//!
//! Build: `cargo build --release` → plugin_onnx.dll

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::LazyLock;

#[cfg(test)]
use ndarray::{Array1, Array2};
use ort::session::Session;
#[allow(unused_imports)]
use ort::value::Tensor;
use tokenizers::Tokenizer;

// ---------------------------------------------------------------------------
// Error codes
// ---------------------------------------------------------------------------

/// Success.
const E_OK: i32 = 0;
/// Null pointer argument.
const E_NULL_PTR: i32 = -1;
/// Plugin not initialized (call embed_init first).
const E_NOT_INIT: i32 = -2;
/// Tokenization failed.
const E_TOKENIZE: i32 = -3;
/// ONNX inference failed.
const E_INFER: i32 = -4;
/// Dimension mismatch.
const E_DIM: i32 = -5;
/// Initialization failed (model load, session creation, etc.).
const E_INIT: i32 = -6;

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Internal state holding the ONNX session, tokenizer, and dimension.
struct EmbedState {
    session: Option<Session>,
    tokenizer: Option<Tokenizer>,
    dim: i32,
    initialized: bool,
}

static STATE: LazyLock<std::sync::Mutex<EmbedState>> = LazyLock::new(|| {
    std::sync::Mutex::new(EmbedState {
        session: None,
        tokenizer: None,
        dim: 0,
        initialized: false,
    })
});

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Derive the tokenizer path from the model path.
///
/// Given `/path/to/model.onnx`, returns `/path/to/tokenizer.json`.
fn derive_tokenizer_path(model_path: &str) -> String {
    let path = Path::new(model_path);
    let dir = path.parent().unwrap_or(Path::new("."));
    dir.join("tokenizer.json").to_string_lossy().to_string()
}

/// Perform mean pooling over the sequence dimension, weighted by attention mask.
///
/// `output`: flat data [1, seq_len, dim] from ONNX output
/// `mask`: attention mask values (1.0 for real tokens, 0.0 for padding)
/// `seq_len`: sequence length
/// `dim`: embedding dimension
fn mean_pool(output: &[f32], mask: &[i64], seq_len: usize, dim: usize) -> Vec<f32> {
    let mut result = vec![0.0f32; dim];
    let mut count = 0usize;

    for i in 0..seq_len {
        if mask[i] == 1 {
            count += 1;
            let offset = i * dim;
            for j in 0..dim {
                result[j] += output[offset + j];
            }
        }
    }

    if count > 0 {
        let inv = 1.0f32 / count as f32;
        for v in result.iter_mut() {
            *v *= inv;
        }
    }

    result
}

/// L2 normalize a vector in place.
fn l2_normalize(vec: &mut [f32]) {
    let sum_sq: f32 = vec.iter().map(|v| v * v).sum();
    if sum_sq > 0.0 {
        let norm = (sum_sq as f64).sqrt() as f32;
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}

// ---------------------------------------------------------------------------
// C ABI exports
// ---------------------------------------------------------------------------

/// Initialize the embedding plugin with an ONNX model and output dimension.
///
/// `model_path`: Null-terminated UTF-8 string pointing to the `.onnx` model file.
/// `dim`: Expected output embedding dimension (e.g. 384 for all-MiniLM-L6-v2).
///
/// Returns: 0 on success, negative error code on failure.
#[no_mangle]
pub extern "C" fn embed_init(model_path: *const c_char, dim: i32) -> i32 {
    if model_path.is_null() {
        eprintln!("[plugin-onnx] embed_init: model_path is null");
        return E_NULL_PTR;
    }

    let model_str = unsafe { CStr::from_ptr(model_path) };
    let model_str = match model_str.to_str() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed_init: model_path is not valid UTF-8: {}", e);
            return E_INIT;
        }
    };

    if model_str.is_empty() {
        eprintln!("[plugin-onnx] embed_init: model_path is empty");
        return E_INIT;
    }

    if dim <= 0 {
        eprintln!("[plugin-onnx] embed_init: invalid dim={}", dim);
        return E_DIM;
    }

    // Load tokenizer from same directory as model
    let tokenizer_path = derive_tokenizer_path(model_str);
    let tokenizer = match Tokenizer::from_file(&tokenizer_path) {
        Ok(t) => {
            eprintln!("[plugin-onnx] Tokenizer loaded from {}", tokenizer_path);
            Some(t)
        }
        Err(e) => {
            eprintln!(
                "[plugin-onnx] WARN: tokenizer not found at {} ({}), using fallback",
                tokenizer_path, e
            );
            None
        }
    };

    // Create ONNX session
    let session = match Session::builder()
        .and_then(|mut b| b.commit_from_file(model_str))
    {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed_init: failed to create ONNX session: {}", e);
            return E_INIT;
        }
    };

    // Store in global state
    let mut state = match STATE.lock() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed_init: failed to lock state: {}", e);
            return E_INIT;
        }
    };

    state.session = Some(session);
    state.tokenizer = tokenizer;
    state.dim = dim;
    state.initialized = true;

    eprintln!("[plugin-onnx] Initialized: dim={}, model={}", dim, model_str);
    E_OK
}

/// Compute embedding for the given text.
///
/// `text`: Null-terminated UTF-8 string.
/// `out`: Pointer to a buffer of `dim` floats (caller-allocated).
/// `dim`: Expected embedding dimension (must match embed_init's dim).
///
/// Returns: 0 on success, negative error code on failure.
#[no_mangle]
pub extern "C" fn embed(text: *const c_char, out: *mut f32, dim: i32) -> i32 {
    if text.is_null() || out.is_null() {
        eprintln!("[plugin-onnx] embed: null pointer argument");
        return E_NULL_PTR;
    }

    let mut state = match STATE.lock() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: failed to lock state: {}", e);
            return E_NOT_INIT;
        }
    };

    if !state.initialized || state.session.is_none() {
        eprintln!("[plugin-onnx] embed: not initialized");
        return E_NOT_INIT;
    }

    if dim != state.dim {
        eprintln!(
            "[plugin-onnx] embed: dimension mismatch (expected={}, got={})",
            state.dim, dim
        );
        return E_DIM;
    }

    let text_str = unsafe { CStr::from_ptr(text) };
    let text_str = match text_str.to_str() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: text is not valid UTF-8: {}", e);
            return E_TOKENIZE;
        }
    };

    if text_str.is_empty() {
        eprintln!("[plugin-onnx] embed: empty text");
        return E_TOKENIZE;
    }

    // Tokenize
    let encoding = match &state.tokenizer {
        Some(tokenizer) => match tokenizer.encode(text_str, true) {
            Ok(enc) => enc,
            Err(e) => {
                eprintln!("[plugin-onnx] embed: tokenization failed: {}", e);
                return E_TOKENIZE;
            }
        },
        None => {
            eprintln!("[plugin-onnx] embed: no tokenizer available");
            return E_TOKENIZE;
        }
    };

    let input_ids = encoding.get_ids();
    let attention_mask = encoding.get_attention_mask();
    let seq_len = input_ids.len();

    if seq_len == 0 {
        eprintln!("[plugin-onnx] embed: tokenization produced empty sequence");
        return E_TOKENIZE;
    }

    // Create input tensors using Tensor::from_array
    let input_ids_data: Vec<i64> = input_ids.iter().map(|&id| id as i64).collect();
    let input_ids_shape = vec![1i64, seq_len as i64];
    let input_ids_tensor = match Tensor::from_array((input_ids_shape, input_ids_data)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: failed to create input_ids tensor: {}", e);
            return E_INFER;
        }
    };

    let attention_mask_data: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let attention_mask_shape = vec![1i64, seq_len as i64];
    let attention_mask_tensor = match Tensor::from_array((attention_mask_shape, attention_mask_data)) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: failed to create attention_mask tensor: {}", e);
            return E_INFER;
        }
    };

    // Run inference
    let session = state.session.as_mut().expect("session exists");
    let outputs = match session.run(ort::inputs! {
        "input_ids" => input_ids_tensor,
        "attention_mask" => attention_mask_tensor,
    }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: inference failed: {}", e);
            return E_INFER;
        }
    };

    // Extract output tensor: last_hidden_state [1, seq_len, dim]
    let output_value = match outputs.get("last_hidden_state") {
        Some(v) => v,
        None => {
            // Fallback: use first output by name
            let first_name = outputs.keys().next();
            match first_name {
                Some(name) => match outputs.get(name) {
                    Some(v) => v,
                    None => {
                        eprintln!("[plugin-onnx] embed: no output tensor found");
                        return E_INFER;
                    }
                },
                None => {
                    eprintln!("[plugin-onnx] embed: no output tensor found");
                    return E_INFER;
                }
            }
        }
    };

    let (_shape, output_data) = match output_value.try_extract_tensor::<f32>() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[plugin-onnx] embed: failed to extract output tensor: {}", e);
            return E_INFER;
        }
    };

    // Verify shape: should be [1, seq_len, dim]
    let expected_elements = seq_len * (dim as usize);
    if output_data.len() < expected_elements {
        eprintln!(
            "[plugin-onnx] embed: output too small (got {}, expected {})",
            output_data.len(),
            expected_elements
        );
        return E_INFER;
    }

    // Mean pooling
    let mask_i64: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let mut pooled = mean_pool(output_data, &mask_i64, seq_len, dim as usize);

    // L2 normalization
    l2_normalize(&mut pooled);

    // Write result to output buffer
    let out_slice = unsafe { std::slice::from_raw_parts_mut(out, dim as usize) };
    out_slice.copy_from_slice(&pooled);

    E_OK
}

/// Release all resources held by the plugin.
///
/// Safe to call multiple times (idempotent).
#[no_mangle]
pub extern "C" fn embed_free() {
    if let Ok(mut state) = STATE.lock() {
        state.session = None;
        state.tokenizer = None;
        state.dim = 0;
        state.initialized = false;
        eprintln!("[plugin-onnx] Resources released");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ===================================================================
    // No-model tests (always run)
    // ===================================================================

    #[test]
    fn test_error_code_constants() {
        assert_eq!(E_OK, 0);
        assert_eq!(E_NULL_PTR, -1);
        assert_eq!(E_NOT_INIT, -2);
        assert_eq!(E_TOKENIZE, -3);
        assert_eq!(E_INFER, -4);
        assert_eq!(E_DIM, -5);
        assert_eq!(E_INIT, -6);
    }

    #[test]
    fn test_global_state_default() {
        let state = STATE.lock().unwrap();
        assert!(!state.initialized);
        assert!(state.session.is_none());
        assert!(state.tokenizer.is_none());
        assert_eq!(state.dim, 0);
    }

    #[test]
    fn test_null_text_returns_error() {
        let mut buf = [0.0f32; 64];
        let result = embed(std::ptr::null(), buf.as_mut_ptr(), 64);
        assert_eq!(result, E_NULL_PTR);
    }

    #[test]
    fn test_null_out_returns_error() {
        let text = std::ffi::CString::new("hello").unwrap();
        let result = embed(text.as_ptr(), std::ptr::null_mut(), 64);
        assert_eq!(result, E_NULL_PTR);
    }

    #[test]
    fn test_embed_before_init_returns_error() {
        let text = std::ffi::CString::new("hello").unwrap();
        let mut buf = [0.0f32; 64];
        let result = embed(text.as_ptr(), buf.as_mut_ptr(), 64);
        assert_eq!(result, E_NOT_INIT);
    }

    #[test]
    fn test_init_null_path_returns_error() {
        let result = embed_init(std::ptr::null(), 384);
        assert_eq!(result, E_NULL_PTR);
    }

    #[test]
    fn test_init_empty_path_returns_error() {
        let path = std::ffi::CString::new("").unwrap();
        let result = embed_init(path.as_ptr(), 384);
        assert_eq!(result, E_INIT);
    }

    #[test]
    fn test_init_nonexistent_model_returns_error() {
        let path = std::ffi::CString::new("/nonexistent/model.onnx").unwrap();
        let result = embed_init(path.as_ptr(), 384);
        assert_eq!(result, E_INIT);
    }

    #[test]
    fn test_init_invalid_dim_returns_error() {
        let path = std::ffi::CString::new("/some/model.onnx").unwrap();
        assert_eq!(embed_init(path.as_ptr(), 0), E_DIM);
        assert_eq!(embed_init(path.as_ptr(), -1), E_DIM);
    }

    #[test]
    fn test_free_idempotent() {
        embed_free();
        embed_free();
        embed_free();
    }

    #[test]
    fn test_derive_tokenizer_path() {
        let path = derive_tokenizer_path("/path/to/model.onnx");
        assert!(path.ends_with("tokenizer.json"), "got: {}", path);
        assert!(path.contains("to"), "got: {}", path);

        let path = derive_tokenizer_path("model.onnx");
        assert!(path.ends_with("tokenizer.json"), "got: {}", path);

        let path = derive_tokenizer_path("/a/b/c/model.onnx");
        assert!(path.contains("c"), "got: {}", path);
        assert!(path.ends_with("tokenizer.json"), "got: {}", path);
    }

    #[test]
    fn test_mean_pool_basic() {
        // 2 tokens, 3 dims, flat data: [1,2,3,4,5,6]
        let output = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mask = vec![1i64, 1];
        let result = mean_pool(&output, &mask, 2, 3);
        // Mean of [1,4], [2,5], [3,6] = [2.5, 3.5, 4.5]
        assert!((result[0] - 2.5).abs() < 1e-6);
        assert!((result[1] - 3.5).abs() < 1e-6);
        assert!((result[2] - 4.5).abs() < 1e-6);
    }

    #[test]
    fn test_mean_pool_with_mask() {
        let output = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mask = vec![1i64, 0]; // second token masked out
        let result = mean_pool(&output, &mask, 2, 3);
        assert!((result[0] - 1.0).abs() < 1e-6);
        assert!((result[1] - 2.0).abs() < 1e-6);
        assert!((result[2] - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_mean_pool_all_masked() {
        let output = vec![1.0f32, 2.0, 3.0, 4.0];
        let mask = vec![0i64, 0];
        let result = mean_pool(&output, &mask, 2, 2);
        assert!((result[0] - 0.0).abs() < 1e-6);
        assert!((result[1] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_l2_normalize_basic() {
        let mut vec = vec![3.0f32, 4.0];
        l2_normalize(&mut vec);
        assert!((vec[0] - 0.6).abs() < 1e-6);
        assert!((vec[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_l2_normalize_unit_vector() {
        let mut vec = vec![1.0f32, 0.0, 0.0];
        l2_normalize(&mut vec);
        assert!((vec[0] - 1.0).abs() < 1e-6);
        assert!((vec[1] - 0.0).abs() < 1e-6);
        assert!((vec[2] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_l2_normalize_zero_vector() {
        let mut vec = vec![0.0f32, 0.0, 0.0];
        l2_normalize(&mut vec);
        assert!((vec[0] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_l2_normalize_result_is_unit() {
        let mut vec = vec![1.0f32, 2.0, 3.0];
        l2_normalize(&mut vec);
        let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_ndarray_mean_pool_matches() {
        // Verify our mean_pool matches ndarray-based implementation
        let output = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        let mask = vec![1i64, 1];
        let result = mean_pool(&output, &mask, 2, 3);

        let output_arr = Array2::from_shape_vec((2, 3), vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
        let mask_arr = Array1::from_vec(vec![1.0f32, 1.0]);
        let mut nd_result = Array1::zeros(3);
        for i in 0..2 {
            let weight = mask_arr[i];
            if weight > 0.0 {
                nd_result.scaled_add(weight, &output_arr.row(i));
            }
        }
        nd_result /= mask_arr.sum();

        for j in 0..3 {
            assert!((result[j] - nd_result[j]).abs() < 1e-6);
        }
    }

    // ===================================================================
    // Model-required tests (run with `cargo test -- --ignored`)
    // ===================================================================

    #[test]
    #[ignore]
    fn test_init_with_valid_model() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let result = embed_init(path.as_ptr(), 384);
        assert_eq!(result, E_OK);
        embed_free();
    }

    #[test]
    #[ignore]
    fn test_embed_short_text() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        let text = std::ffi::CString::new("hello world").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);

        let non_zero = buf.iter().filter(|&&v| v != 0.0).count();
        assert!(non_zero > 0, "Embedding should have non-zero values");

        embed_free();
    }

    #[test]
    #[ignore]
    fn test_embed_returns_correct_dim() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        let text = std::ffi::CString::new("test embedding").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);
        assert_eq!(buf.len(), 384);

        embed_free();
    }

    #[test]
    #[ignore]
    fn test_embed_l2_normalized() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        let text = std::ffi::CString::new("verify normalization").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);

        let l2_norm: f32 = buf.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!(
            (l2_norm - 1.0).abs() < 1e-4,
            "L2 norm should be ~1.0, got {}",
            l2_norm
        );

        embed_free();
    }

    #[test]
    #[ignore]
    fn test_embed_deterministic() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        let text = std::ffi::CString::new("deterministic test").unwrap();
        let mut buf1 = vec![0.0f32; 384];
        let mut buf2 = vec![0.0f32; 384];

        let r1 = embed(text.as_ptr(), buf1.as_mut_ptr(), 384);
        let r2 = embed(text.as_ptr(), buf2.as_mut_ptr(), 384);
        assert_eq!(r1, E_OK);
        assert_eq!(r2, E_OK);

        for (i, (a, b)) in buf1.iter().zip(buf2.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "Mismatch at index {}: {} vs {}",
                i, a, b
            );
        }

        embed_free();
    }

    #[test]
    #[ignore]
    fn test_init_then_free_then_embed_fails() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        embed_free();

        let text = std::ffi::CString::new("should fail").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_NOT_INIT);
    }

    #[test]
    #[ignore]
    fn test_embed_multiple_texts() {
        let model_dir = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| "test-model".to_string());
        let model_path = format!("{}/model.onnx", model_dir);
        let path = std::ffi::CString::new(model_path).unwrap();
        let init_result = embed_init(path.as_ptr(), 384);
        assert_eq!(init_result, E_OK);

        let texts = vec!["first text", "second text", "third text"];
        let mut embeddings: Vec<Vec<f32>> = Vec::new();

        for text_str in &texts {
            let text = std::ffi::CString::new(*text_str).unwrap();
            let mut buf = vec![0.0f32; 384];
            let result = embed(text.as_ptr(), buf.as_mut_ptr(), 384);
            assert_eq!(result, E_OK);
            embeddings.push(buf);
        }

        assert_eq!(embeddings.len(), 3);

        for (i, emb) in embeddings.iter().enumerate() {
            let non_zero = emb.iter().filter(|&&v| v != 0.0).count();
            assert!(non_zero > 0, "Embedding {} should have non-zero values", i);
        }

        let diff: f32 = embeddings[0]
            .iter()
            .zip(embeddings[1].iter())
            .map(|(a, b)| (a - b).powi(2))
            .sum();
        assert!(diff > 0.0, "Different texts should produce different embeddings");

        embed_free();
    }
}
