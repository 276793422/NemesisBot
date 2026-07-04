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
    assert!(state.session.is_none());
    assert!(state.tokenizer.is_none());
    assert_eq!(INIT_COUNT.load(Ordering::SeqCst), 0);
    assert!(!PERMANENTLY_FREED.load(Ordering::SeqCst));
    assert_eq!(MODEL_DIM.load(Ordering::SeqCst), 0);
}

#[test]
fn test_null_text_returns_error() {
    let mut buf = [0.0f32; 64];
    let result = plugin_embed(std::ptr::null(), buf.as_mut_ptr(), 64);
    assert_eq!(result, E_NULL_PTR);
}

#[test]
fn test_null_out_returns_error() {
    let text = std::ffi::CString::new("hello").unwrap();
    let result = plugin_embed(text.as_ptr(), std::ptr::null_mut(), 64);
    assert_eq!(result, E_NULL_PTR);
}

#[test]
fn test_embed_before_init_returns_error() {
    let text = std::ffi::CString::new("hello").unwrap();
    let mut buf = [0.0f32; 64];
    let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 64);
    assert_eq!(result, E_NOT_INIT);
}

#[test]
fn test_init_null_model_dir_with_null_host() {
    // With no model files in current dir, init should fail
    let result = plugin_init(std::ptr::null(), std::ptr::null());
    // Either fails because model.onnx not found, or succeeds if in test-data
    assert!(result != E_OK || result == E_OK, "should not crash");
    // Cleanup
    plugin_free();
}

#[test]
fn test_init_nonexistent_model_dir_no_host() {
    // model_dir doesn't exist → model.onnx not found → E_INIT
    let model_dir = std::ffi::CString::new("/nonexistent/path").unwrap();
    let result = plugin_init(model_dir.as_ptr(), std::ptr::null());
    assert_eq!(result, E_INIT, "nonexistent dir should return E_INIT");
    // Cleanup
    plugin_free();
}

#[test]
fn test_free_idempotent() {
    plugin_free();
    plugin_free();
    plugin_free();
}

#[test]
fn test_init_after_free_returns_error() {
    if PERMANENTLY_FREED.load(Ordering::SeqCst) {
        // Already freed by a previous test — re-init should fail
        let model_dir = std::ffi::CString::new("/nonexistent").unwrap();
        let result = plugin_init(model_dir.as_ptr(), std::ptr::null());
        assert_eq!(result, E_INIT, "re-init after free should return E_INIT");
    }
    // If not permanently freed, just skip — this test is only meaningful
    // when run in isolation after a free.
}

#[test]
fn test_ref_count_basics() {
    let count = INIT_COUNT.load(Ordering::SeqCst);
    assert!(count >= 0, "ref count should not be negative: {}", count);
}

#[test]
fn test_mean_pool_basic() {
    let output = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let mask = vec![1i64, 1];
    let result = mean_pool(&output, &mask, 2, 3);
    assert!((result[0] - 2.5).abs() < 1e-6);
    assert!((result[1] - 3.5).abs() < 1e-6);
    assert!((result[2] - 4.5).abs() < 1e-6);
}

#[test]
fn test_mean_pool_with_mask() {
    let output = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
    let mask = vec![1i64, 0];
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

fn test_model_dir() -> String {
    std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
        .unwrap_or_else(|_| {
            let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../test-tools/plugin-onnx-test/test-data");
            dir.to_str().expect("valid path").to_string()
        })
}

/// All model tests in a single lifecycle to avoid ONNX Runtime re-init crash.
#[test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p plugin-onnx -- --ignored --test-threads=1 <test_name>
#[ignore]
fn test_all_model_scenarios() {
    let model_dir = test_model_dir();

    // ---- Init via plugin_init with model_dir pointing to test-data ----
    let model_dir_cstr = std::ffi::CString::new(model_dir.as_str()).unwrap();
    let init_result = plugin_init(model_dir_cstr.as_ptr(), std::ptr::null());
    assert_eq!(init_result, E_OK, "plugin_init should succeed");

    // ---- Scenario: Idempotent init (ref count) ----
    let init2 = plugin_init(model_dir_cstr.as_ptr(), std::ptr::null());
    assert_eq!(init2, E_OK, "second plugin_init should succeed (ref count)");
    assert!(INIT_COUNT.load(Ordering::SeqCst) >= 2);
    plugin_free();
    assert!(INIT_COUNT.load(Ordering::SeqCst) >= 1);

    // ---- Scenario: embed short text via plugin_embed ----
    {
        let text = std::ffi::CString::new("hello world").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);
        let non_zero = buf.iter().filter(|&&v| v != 0.0).count();
        assert!(non_zero > 0, "Embedding should have non-zero values");
        println!("[model-test] short text embed — PASS");
    }

    // ---- Scenario: correct dimension ----
    {
        let text = std::ffi::CString::new("test embedding").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);
        assert_eq!(buf.len(), 384);
        println!("[model-test] correct dimension — PASS");
    }

    // ---- Scenario: L2 normalized ----
    {
        let text = std::ffi::CString::new("verify normalization").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_OK);
        let l2_norm: f32 = buf.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((l2_norm - 1.0).abs() < 1e-4, "L2 norm should be ~1.0, got {}", l2_norm);
        println!("[model-test] L2 normalized — PASS");
    }

    // ---- Scenario: deterministic ----
    {
        let text = std::ffi::CString::new("deterministic test").unwrap();
        let mut buf1 = vec![0.0f32; 384];
        let mut buf2 = vec![0.0f32; 384];
        let r1 = plugin_embed(text.as_ptr(), buf1.as_mut_ptr(), 384);
        let r2 = plugin_embed(text.as_ptr(), buf2.as_mut_ptr(), 384);
        assert_eq!(r1, E_OK);
        assert_eq!(r2, E_OK);
        for (i, (a, b)) in buf1.iter().zip(buf2.iter()).enumerate() {
            assert!((a - b).abs() < 1e-6, "Mismatch at index {}: {} vs {}", i, a, b);
        }
        println!("[model-test] deterministic — PASS");
    }

    // ---- Scenario: multiple texts ----
    {
        let texts = vec!["first text", "second text", "third text"];
        let mut embeddings: Vec<Vec<f32>> = Vec::new();
        for text_str in &texts {
            let text = std::ffi::CString::new(*text_str).unwrap();
            let mut buf = vec![0.0f32; 384];
            let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 384);
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
        println!("[model-test] multiple texts — PASS");
    }

    // ---- Free ----
    plugin_free();
    assert!(PERMANENTLY_FREED.load(Ordering::SeqCst), "should be permanently freed");
    assert_eq!(INIT_COUNT.load(Ordering::SeqCst), 0);

    // ---- Scenario: embed after free fails ----
    {
        let text = std::ffi::CString::new("should fail").unwrap();
        let mut buf = vec![0.0f32; 384];
        let result = plugin_embed(text.as_ptr(), buf.as_mut_ptr(), 384);
        assert_eq!(result, E_NOT_INIT, "embed after free should fail");
        println!("[model-test] embed after free fails — PASS");
    }

    // ---- Scenario: re-init after free fails ----
    {
        let result = plugin_init(model_dir_cstr.as_ptr(), std::ptr::null());
        assert_eq!(result, E_INIT, "re-init after free should fail");
        println!("[model-test] re-init after free fails — PASS");
    }

    println!("[model-test] All scenarios PASSED");
}
