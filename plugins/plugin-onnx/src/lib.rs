//! plugin-onnx — NemesisBot ONNX embedding plugin DLL
//!
//! Provides C ABI exports for computing text embeddings using a local ONNX model.
//! Designed for BERT-style sentence embedding models (e.g. all-MiniLM-L6-v2).
//!
//! C ABI contract (unified interface):
//! - `plugin_init(model_dir, host)` → i32 (0 = success)
//! - `plugin_embed(text, out, dim)` → i32 (0 = success)
//! - `plugin_free()` → release resources
//!
//! Pipeline: tokenize → ONNX inference → mean pooling → L2 normalize
//!
//! Build: `cargo build --release` → plugin_onnx.dll

mod host_services;

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicPtr, Ordering};
use std::sync::LazyLock;

use host_services::HostServices;
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
/// Plugin not initialized (call plugin_init first).
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

/// Internal state holding the ONNX session and tokenizer.
struct EmbedState {
    session: Option<Session>,
    tokenizer: Option<Tokenizer>,
}

/// Mutex serialises all access.  ONNX Runtime's `Session::run()` requires
/// `&mut self` in ort 2.0.0-rc.12, so we cannot use an RwLock for
/// concurrent inference.  Concurrency is instead handled at the consumer
/// level (the `Mutex<NativePlugin>` in `embedding.rs`).
static STATE: LazyLock<std::sync::Mutex<EmbedState>> = LazyLock::new(|| {
    std::sync::Mutex::new(EmbedState {
        session: None,
        tokenizer: None,
    })
});

/// Inferred model dimension (set on first plugin_embed call).
static MODEL_DIM: AtomicI32 = AtomicI32::new(0);

/// Reference count: how many callers have called `plugin_init` without a
/// matching `plugin_free`.  The ONNX session is only destroyed when this
/// drops to zero.
static INIT_COUNT: AtomicI32 = AtomicI32::new(0);

/// Once the ONNX session has been destroyed, ONNX Runtime's internal global
/// state cannot safely be re-initialised.  Setting this flag blocks all
/// future `plugin_init` calls, preventing the segfault.
static PERMANENTLY_FREED: AtomicBool = AtomicBool::new(false);

/// Stored host services pointer (set during plugin_init).
static HOST_PTR: AtomicPtr<HostServices> = AtomicPtr::new(std::ptr::null_mut());

// ---------------------------------------------------------------------------
// Logging helpers
// ---------------------------------------------------------------------------

/// Log a message via host services if available, otherwise eprintln.
fn log_msg(level: i32, msg: &str) {
    let host_ptr = HOST_PTR.load(Ordering::SeqCst);
    if !host_ptr.is_null() {
        let host = unsafe { &*host_ptr };
        host_services::host_log(Some(host), level, "plugin-onnx", msg);
    } else {
        eprintln!("[plugin-onnx] {}", msg);
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Perform mean pooling over the sequence dimension, weighted by attention mask.
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
// C ABI exports — Unified Plugin Interface
// ---------------------------------------------------------------------------

/// Initialize the plugin.
///
/// `model_dir`: Directory path containing `model.onnx` and `tokenizer.json`.
/// `host`: Pointer to HostServices vtable (may be NULL for standalone testing).
///
/// Returns: 0 on success, negative error code on failure.
///
/// Flow:
/// 1. Parse model_dir path (null → ".")
/// 2. Check PERMANENTLY_FREED gate
/// 3. Fast path: already initialised → bump ref count
/// 4. Slow path:
///    a. Resolve model.onnx and tokenizer.json paths from model_dir
///    b. Load tokenizer
///    c. Create ONNX session
///    d. Set dim=0 (inferred from first plugin_embed call)
/// 5. Return E_OK
#[no_mangle]
pub extern "C" fn plugin_init(model_dir: *const c_char, host: *const HostServices) -> i32 {
    // Store host pointer (even if null)
    HOST_PTR.store(host as *mut HostServices, Ordering::SeqCst);

    // --- Gate 1: ONNX Runtime already torn down? ---
    if PERMANENTLY_FREED.load(Ordering::SeqCst) {
        log_msg(
            4,
            "plugin_init: cannot re-init after free (ONNX Runtime limitation)",
        );
        return E_INIT;
    }

    // Parse model_dir
    let model_dir_str = if model_dir.is_null() {
        log_msg(3, "plugin_init: model_dir is null, using current directory");
        ".".to_string()
    } else {
        match unsafe { CStr::from_ptr(model_dir) }.to_str() {
            Ok(s) => s.to_string(),
            Err(e) => {
                log_msg(
                    4,
                    &format!("plugin_init: model_dir is not valid UTF-8: {}", e),
                );
                return E_INIT;
            }
        }
    };

    // --- Gate 2: Fast path — already initialised, just bump ref count ---
    if INIT_COUNT.load(Ordering::SeqCst) > 0 {
        INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        log_msg(
            1,
            &format!(
                "plugin_init: already initialised, ref_count={}",
                INIT_COUNT.load(Ordering::SeqCst)
            ),
        );
        return E_OK;
    }

    // --- Slow path: resolve model files and create session ---
    let model_path = Path::new(&model_dir_str).join("model.onnx");
    let tokenizer_path = Path::new(&model_dir_str).join("tokenizer.json");

    if !model_path.exists() {
        log_msg(
            4,
            &format!(
                "plugin_init: model file not found: {}",
                model_path.display()
            ),
        );
        return E_INIT;
    }

    if !tokenizer_path.exists() {
        log_msg(
            4,
            &format!(
                "plugin_init: tokenizer file not found: {}",
                tokenizer_path.display()
            ),
        );
        return E_INIT;
    }

    // --- Create session ---
    let mut state = match STATE.lock() {
        Ok(s) => s,
        Err(e) => {
            log_msg(4, &format!("plugin_init: failed to lock state: {}", e));
            return E_INIT;
        }
    };

    // Double-check inside write lock
    if state.session.is_some() {
        drop(state);
        INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        log_msg(
            1,
            &format!(
                "plugin_init: initialised by another thread, ref_count={}",
                INIT_COUNT.load(Ordering::SeqCst)
            ),
        );
        return E_OK;
    }

    // Load tokenizer
    let tokenizer = match Tokenizer::from_file(&tokenizer_path) {
        Ok(t) => {
            log_msg(
                1,
                &format!("Tokenizer loaded from {}", tokenizer_path.display()),
            );
            Some(t)
        }
        Err(e) => {
            log_msg(
                4,
                &format!(
                    "Failed to load tokenizer from {}: {}",
                    tokenizer_path.display(),
                    e
                ),
            );
            return E_INIT;
        }
    };

    // Create ONNX session
    let session = match Session::builder().and_then(|mut b| b.commit_from_file(&model_path)) {
        Ok(s) => s,
        Err(e) => {
            log_msg(
                4,
                &format!("plugin_init: failed to create ONNX session: {}", e),
            );
            return E_INIT;
        }
    };

    state.session = Some(session);
    state.tokenizer = tokenizer;
    // dim is inferred from ONNX output on first plugin_embed call.
    drop(state);

    INIT_COUNT.fetch_add(1, Ordering::SeqCst);
    log_msg(
        2,
        &format!("Initialized: model_dir={}, ref_count=1", model_dir_str),
    );
    E_OK
}

/// Compute embedding for the given text.
///
/// `text`: Null-terminated UTF-8 string.
/// `out`: Pointer to a buffer of `dim` floats (caller-allocated).
/// `dim`: Expected embedding dimension (must match model output).
///
/// Returns: 0 on success, negative error code on failure.
#[no_mangle]
pub extern "C" fn plugin_embed(text: *const c_char, out: *mut f32, dim: i32) -> i32 {
    if text.is_null() || out.is_null() {
        log_msg(4, "plugin_embed: null pointer argument");
        return E_NULL_PTR;
    }

    let mut state = match STATE.lock() {
        Ok(s) => s,
        Err(e) => {
            log_msg(4, &format!("plugin_embed: failed to lock state: {}", e));
            return E_NOT_INIT;
        }
    };

    if state.session.is_none() {
        log_msg(4, "plugin_embed: not initialized");
        return E_NOT_INIT;
    }

    // Validate dim: if state.dim is 0 (first call), infer from output.
    // Otherwise check that caller's dim matches.
    let dim_usize = dim as usize;
    if dim_usize == 0 {
        log_msg(4, "plugin_embed: dim must be > 0");
        return E_DIM;
    }

    let text_str = unsafe { CStr::from_ptr(text) };
    let text_str = match text_str.to_str() {
        Ok(s) => s,
        Err(e) => {
            log_msg(4, &format!("plugin_embed: text is not valid UTF-8: {}", e));
            return E_TOKENIZE;
        }
    };

    if text_str.is_empty() {
        log_msg(4, "plugin_embed: empty text");
        return E_TOKENIZE;
    }

    // Tokenize
    let encoding = match &state.tokenizer {
        Some(tokenizer) => match tokenizer.encode(text_str, true) {
            Ok(enc) => enc,
            Err(e) => {
                log_msg(4, &format!("plugin_embed: tokenization failed: {}", e));
                return E_TOKENIZE;
            }
        },
        None => {
            log_msg(4, "plugin_embed: no tokenizer available");
            return E_TOKENIZE;
        }
    };

    let input_ids = encoding.get_ids();
    let attention_mask = encoding.get_attention_mask();
    let seq_len = input_ids.len();

    if seq_len == 0 {
        log_msg(4, "plugin_embed: tokenization produced empty sequence");
        return E_TOKENIZE;
    }

    // Create input tensors
    let input_ids_data: Vec<i64> = input_ids.iter().map(|&id| id as i64).collect();
    let input_ids_shape = vec![1i64, seq_len as i64];
    let input_ids_tensor = match Tensor::from_array((input_ids_shape, input_ids_data)) {
        Ok(t) => t,
        Err(e) => {
            log_msg(
                4,
                &format!("plugin_embed: failed to create input_ids tensor: {}", e),
            );
            return E_INFER;
        }
    };

    let attention_mask_data: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let attention_mask_shape = vec![1i64, seq_len as i64];
    let attention_mask_tensor =
        match Tensor::from_array((attention_mask_shape, attention_mask_data)) {
            Ok(t) => t,
            Err(e) => {
                log_msg(
                    4,
                    &format!(
                        "plugin_embed: failed to create attention_mask tensor: {}",
                        e
                    ),
                );
                return E_INFER;
            }
        };

    let token_type_ids_data = vec![0i64; seq_len];
    let token_type_ids_shape = vec![1i64, seq_len as i64];
    let token_type_ids_tensor =
        match Tensor::from_array((token_type_ids_shape, token_type_ids_data)) {
            Ok(t) => t,
            Err(e) => {
                log_msg(
                    4,
                    &format!(
                        "plugin_embed: failed to create token_type_ids tensor: {}",
                        e
                    ),
                );
                return E_INFER;
            }
        };

    // Run inference
    let session = state.session.as_mut().expect("session checked above");
    let outputs = match session.run(ort::inputs! {
        "input_ids" => input_ids_tensor,
        "attention_mask" => attention_mask_tensor,
        "token_type_ids" => token_type_ids_tensor,
    }) {
        Ok(o) => o,
        Err(e) => {
            log_msg(4, &format!("plugin_embed: inference failed: {}", e));
            return E_INFER;
        }
    };

    // Extract output tensor
    let output_value = match outputs.get("last_hidden_state") {
        Some(v) => v,
        None => {
            let first_name = outputs.keys().next();
            match first_name {
                Some(name) => match outputs.get(name) {
                    Some(v) => v,
                    None => {
                        log_msg(4, "plugin_embed: no output tensor found");
                        return E_INFER;
                    }
                },
                None => {
                    log_msg(4, "plugin_embed: no output tensor found");
                    return E_INFER;
                }
            }
        }
    };

    let (_shape, output_data) = match output_value.try_extract_tensor::<f32>() {
        Ok(s) => s,
        Err(e) => {
            log_msg(
                4,
                &format!("plugin_embed: failed to extract output tensor: {}", e),
            );
            return E_INFER;
        }
    };

    // Infer model dim from output shape if not yet set
    // Output shape is [1, seq_len, model_dim], so model_dim = output_data.len() / seq_len
    let model_dim = output_data.len() / seq_len;
    let cached_dim = MODEL_DIM.load(Ordering::SeqCst);
    if cached_dim == 0 {
        MODEL_DIM.store(model_dim as i32, Ordering::SeqCst);
    }

    // Validate caller's dim matches model's actual dim
    let expected_dim = if cached_dim != 0 {
        cached_dim
    } else {
        model_dim as i32
    };
    if dim != expected_dim {
        log_msg(
            4,
            &format!(
                "plugin_embed: dimension mismatch (model={}, got={})",
                expected_dim, dim
            ),
        );
        return E_DIM;
    }

    let expected_elements = seq_len * dim_usize;
    if output_data.len() < expected_elements {
        log_msg(
            4,
            &format!(
                "plugin_embed: output too small (got {}, expected {})",
                output_data.len(),
                expected_elements
            ),
        );
        return E_INFER;
    }

    // Mean pooling
    let mask_i64: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let mut pooled = mean_pool(output_data, &mask_i64, seq_len, dim_usize);

    // L2 normalization
    l2_normalize(&mut pooled);

    // Write result
    let out_slice = unsafe { std::slice::from_raw_parts_mut(out, dim_usize) };
    out_slice.copy_from_slice(&pooled);

    E_OK
}

/// Release one reference to the plugin.
///
/// Reference-counted: only destroys the ONNX session when the last
/// caller calls `plugin_free()`. Intermediate calls simply decrement the
/// count and return.
///
/// One-shot: once the session is actually destroyed, ONNX Runtime's
/// global state cannot be re-initialised.
///
/// Safe to call multiple times (idempotent when count is already zero).
#[no_mangle]
pub extern "C" fn plugin_free() {
    let count = INIT_COUNT.load(Ordering::SeqCst);
    if count <= 0 {
        return;
    }

    let prev = INIT_COUNT.fetch_sub(1, Ordering::SeqCst);

    if prev <= 1 {
        if let Ok(mut state) = STATE.lock() {
            state.session = None;
            state.tokenizer = None;
        }
        INIT_COUNT.store(0, Ordering::SeqCst);
        MODEL_DIM.store(0, Ordering::SeqCst);
        PERMANENTLY_FREED.store(true, Ordering::SeqCst);
        HOST_PTR.store(std::ptr::null_mut(), Ordering::SeqCst);
        log_msg(2, "Resources released (last reference freed)");
    } else {
        log_msg(
            1,
            &format!(
                "plugin_free: ref_count {} → {} (session kept alive)",
                prev,
                prev - 1
            ),
        );
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
