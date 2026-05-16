//! plugin-onnx — NemesisBot ONNX embedding plugin DLL
//!
//! Provides C ABI exports for computing text embeddings using a local ONNX model.
//! Designed for BERT-style sentence embedding models (e.g. all-MiniLM-L6-v2).
//!
//! C ABI contract (unified interface):
//! - `plugin_init(config_dir, host)` → i32 (0 = success)
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
use std::sync::LazyLock;
use std::sync::atomic::{AtomicI32, AtomicBool, AtomicPtr, Ordering};

#[cfg(test)]
use ndarray::{Array1, Array2};
use host_services::HostServices;
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
/// Download failed.
const E_DOWNLOAD: i32 = -7;
/// Configuration error.
const E_CONFIG: i32 = -8;

// ---------------------------------------------------------------------------
// Plugin configuration
// ---------------------------------------------------------------------------

/// Compile-time embedded default configuration.
const DEFAULT_CONFIG: &str = include_str!("../config/plugin.toml");

/// Top-level plugin configuration with three model tiers.
#[derive(serde::Deserialize, serde::Serialize, Clone)]
struct PluginConfig {
    #[serde(default)]
    plugin: PluginInfo,
    #[serde(default = "default_active")]
    active: String,
    #[serde(default)]
    models: ModelsConfig,
}

fn default_active() -> String { "medium".to_string() }

impl Default for PluginConfig {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG).unwrap_or_else(|_| PluginConfig {
            plugin: PluginInfo::default(),
            active: default_active(),
            models: ModelsConfig::default(),
        })
    }
}

#[derive(serde::Deserialize, serde::Serialize, Default, Clone)]
struct PluginInfo {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
}

/// Container for the three model tiers.
#[derive(serde::Deserialize, serde::Serialize, Default, Clone)]
struct ModelsConfig {
    #[serde(default)]
    large: ModelConfig,
    #[serde(default)]
    medium: ModelConfig,
    #[serde(default)]
    small: ModelConfig,
}

impl ModelsConfig {
    fn get(&self, key: &str) -> Option<&ModelConfig> {
        match key {
            "large" => Some(&self.large),
            "medium" => Some(&self.medium),
            "small" => Some(&self.small),
            _ => None,
        }
    }

    fn get_mut(&mut self, key: &str) -> Option<&mut ModelConfig> {
        match key {
            "large" => Some(&mut self.large),
            "medium" => Some(&mut self.medium),
            "small" => Some(&mut self.small),
            _ => None,
        }
    }
}

/// Per-tier model configuration.
#[derive(serde::Deserialize, serde::Serialize, Default, Clone)]
struct ModelConfig {
    #[serde(default)]
    name: String,
    #[serde(default)]
    dimension: i32,
    #[serde(default)]
    model_url: String,
    #[serde(default)]
    model_size: u64,
    #[serde(default)]
    tokenizer_url: String,
    #[serde(default)]
    tokenizer_size: u64,
    /// Absolute local path after download. Empty = not yet downloaded.
    #[serde(default)]
    local_model_path: String,
    #[serde(default)]
    local_tokenizer_path: String,
}

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Internal state holding the ONNX session, tokenizer, and dimension.
struct EmbedState {
    session: Option<Session>,
    tokenizer: Option<Tokenizer>,
    dim: i32,
}

/// Mutex serialises all access.  ONNX Runtime's `Session::run()` requires
/// `&mut self` in ort 2.0.0-rc.12, so we cannot use an RwLock for
/// concurrent inference.  Concurrency is instead handled at the consumer
/// level (the `Mutex<NativePlugin>` in `embedding.rs`).
static STATE: LazyLock<std::sync::Mutex<EmbedState>> = LazyLock::new(|| {
    std::sync::Mutex::new(EmbedState {
        session: None,
        tokenizer: None,
        dim: 0,
    })
});

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
// Configuration loading & saving
// ---------------------------------------------------------------------------

fn config_path(config_dir: &str) -> std::path::PathBuf {
    Path::new(config_dir).join("plugin-onnx.toml")
}

/// Load plugin configuration.
///
/// 1. If `{config_dir}/plugin-onnx.toml` exists → load it.
/// 2. If not → save embedded default config to that path, then load from disk.
fn load_config(config_dir: &str) -> PluginConfig {
    let path = config_path(config_dir);

    if !path.exists() {
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            log_msg(3, &format!("Failed to create config dir '{}': {}", config_dir, e));
        } else {
            match std::fs::write(&path, DEFAULT_CONFIG) {
                Ok(()) => {
                    log_msg(2, &format!("Default config saved to {}", path.display()));
                }
                Err(e) => {
                    log_msg(3, &format!("Failed to save default config: {}", e));
                }
            }
        }
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<PluginConfig>(&content) {
            Ok(config) => {
                log_msg(2, &format!("Config loaded from {}", path.display()));
                config
            }
            Err(e) => {
                log_msg(4, &format!("Failed to parse config '{}': {}", path.display(), e));
                PluginConfig::default()
            }
        },
        Err(e) => {
            log_msg(3, &format!("Failed to read config '{}': {}, using defaults", path.display(), e));
            PluginConfig::default()
        }
    }
}

/// Save plugin configuration back to disk.
fn save_config(config: &PluginConfig, config_dir: &str) {
    let path = config_path(config_dir);
    match toml::to_string_pretty(config) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                log_msg(3, &format!("Failed to save config to {}: {}", path.display(), e));
            } else {
                log_msg(1, &format!("Config saved to {}", path.display()));
            }
        }
        Err(e) => {
            log_msg(3, &format!("Failed to serialize config: {}", e));
        }
    }
}

// ---------------------------------------------------------------------------
// File resolution & download
// ---------------------------------------------------------------------------

/// Ensure a model/tokenizer file is locally available.
///
/// Returns `(resolved_path, was_updated)`.
///
/// - `local_path` non-empty and file exists → use it directly
/// - `local_path` non-empty but file missing → re-download
/// - `local_path` empty → download
/// - No host services → search `data_dir` for the file as fallback
fn ensure_file(
    url: &str,
    local_path: &str,
    data_dir: &str,
    model_name: &str,
    filename: &str,
    host: Option<&HostServices>,
) -> Result<(String, bool), i32> {
    // Case 1: local_path is set and file exists
    if !local_path.is_empty() && Path::new(local_path).exists() {
        log_msg(1, &format!("{} found at {}", filename, local_path));
        return Ok((local_path.to_string(), false));
    }

    // Case 2: local_path is set but file is missing — fall through to download
    if !local_path.is_empty() {
        log_msg(2, &format!("{} was at {} but file missing, re-downloading", filename, local_path));
    }

    // Need to download — compute destination path
    let dest_dir = Path::new(data_dir).join(model_name);
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        log_msg(3, &format!("Failed to create dir {}: {}", dest_dir.display(), e));
    }
    let dest = dest_dir.join(filename);
    let dest_str = dest.to_string_lossy().to_string();

    if let Some(host) = host {
        if url.is_empty() {
            log_msg(4, &format!("{} not found and no URL configured", filename));
            return Err(E_CONFIG);
        }
        log_msg(2, &format!("Downloading {} from {}...", filename, url));
        download_via_host(host, url, &dest_str)?;
        log_msg(2, &format!("{} downloaded to {}", filename, dest_str));
        Ok((dest_str, true))
    } else {
        // No host services — search known locations as fallback (standalone test)
        let candidates = [
            dest.clone(),
            Path::new(data_dir).join(filename),
            Path::new(".").join(filename),
        ];
        for candidate in &candidates {
            if candidate.exists() {
                let found = candidate.to_string_lossy().to_string();
                log_msg(1, &format!("Found {} at {}", filename, found));
                return Ok((found, true));
            }
        }
        log_msg(4, &format!("No host services and {} not found in known locations", filename));
        Err(E_DOWNLOAD)
    }
}

/// Resolve the plugin data directory via host services, or fall back to config_dir.
fn resolve_data_dir(host: Option<&HostServices>, fallback: &str) -> String {
    match host {
        Some(host) => match host.get_plugin_data_dir {
            Some(get_data_dir) => {
                let plugin_name = std::ffi::CString::new("plugin-onnx").unwrap();
                let mut buf = vec![0u8; 4096];
                let len = get_data_dir(
                    plugin_name.as_ptr(),
                    buf.as_mut_ptr() as *mut c_char,
                    buf.len(),
                );
                if len < 0 {
                    log_msg(3, &format!("get_plugin_data_dir failed: {}, using fallback", len));
                    return fallback.to_string();
                }
                CStr::from_bytes_with_nul(&buf[..len as usize + 1])
                    .map(|c| c.to_string_lossy().to_string())
                    .unwrap_or_else(|_| fallback.to_string())
            }
            None => {
                log_msg(1, "host.get_plugin_data_dir not available, using config_dir");
                fallback.to_string()
            }
        },
        None => fallback.to_string(),
    }
}

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

/// Download a file using host services if available.
fn download_via_host(host: &HostServices, url: &str, dest: &str) -> Result<(), i32> {
    let download_fn = host.download_file.ok_or(E_DOWNLOAD)?;
    let c_url = std::ffi::CString::new(url).map_err(|_| E_DOWNLOAD)?;
    let c_dest = std::ffi::CString::new(dest).map_err(|_| E_DOWNLOAD)?;
    let result = download_fn(c_url.as_ptr(), c_dest.as_ptr());
    if result == 0 {
        Ok(())
    } else {
        Err(E_DOWNLOAD)
    }
}

// ---------------------------------------------------------------------------
// C ABI exports — Unified Plugin Interface
// ---------------------------------------------------------------------------

/// Initialize the plugin.
///
/// `config_dir`: Directory path where plugin configuration files are located.
/// `host`: Pointer to HostServices vtable (may be NULL for standalone testing).
///
/// Returns: 0 on success, negative error code on failure.
///
/// Flow:
/// 1. Load config (save embedded default if file missing)
/// 2. Read `active` model tier
/// 3. Check `local_model_path`:
///    - non-empty + file exists → use it
///    - non-empty + file missing → download, update config
///    - empty → download, update config
/// 4. Same for tokenizer
/// 5. Load ONNX session + tokenizer
#[no_mangle]
pub extern "C" fn plugin_init(config_dir: *const c_char, host: *const HostServices) -> i32 {
    // Store host pointer (even if null)
    HOST_PTR.store(host as *mut HostServices, Ordering::SeqCst);

    let host_ref = if host.is_null() {
        None
    } else {
        Some(unsafe { &*host })
    };

    // --- Gate 1: ONNX Runtime already torn down? ---
    if PERMANENTLY_FREED.load(Ordering::SeqCst) {
        log_msg(4, "plugin_init: cannot re-init after free (ONNX Runtime limitation)");
        return E_INIT;
    }

    // Parse config_dir
    let config_dir_str = if config_dir.is_null() {
        log_msg(3, "plugin_init: config_dir is null, using current directory");
        ".".to_string()
    } else {
        match unsafe { CStr::from_ptr(config_dir) }.to_str() {
            Ok(s) => s.to_string(),
            Err(e) => {
                log_msg(4, &format!("plugin_init: config_dir is not valid UTF-8: {}", e));
                return E_CONFIG;
            }
        }
    };

    // --- Gate 2: Fast path — already initialised, just bump ref count ---
    if INIT_COUNT.load(Ordering::SeqCst) > 0 {
        INIT_COUNT.fetch_add(1, Ordering::SeqCst);
        log_msg(1, &format!(
            "plugin_init: already initialised, ref_count={}",
            INIT_COUNT.load(Ordering::SeqCst)
        ));
        return E_OK;
    }

    // --- Slow path: load config and create session ---
    let mut config = load_config(&config_dir_str);
    let active = config.active.clone();

    // Resolve data directory
    let data_dir = resolve_data_dir(host_ref, &config_dir_str);

    // Get active model config
    let model_conf = match config.models.get(&active) {
        Some(m) => m.clone(),
        None => {
            log_msg(4, &format!("plugin_init: unknown active model '{}'", active));
            return E_CONFIG;
        }
    };

    let dim = model_conf.dimension;
    if dim <= 0 {
        log_msg(4, &format!("plugin_init: invalid dim={} for model '{}'", dim, active));
        return E_DIM;
    }

    if model_conf.name.is_empty() {
        log_msg(4, &format!("plugin_init: model name is empty for tier '{}'", active));
        return E_CONFIG;
    }

    // Ensure model file is available
    let (model_path, model_updated) = match ensure_file(
        &model_conf.model_url,
        &model_conf.local_model_path,
        &data_dir,
        &model_conf.name,
        "model.onnx",
        host_ref,
    ) {
        Ok(result) => result,
        Err(e) => {
            log_msg(4, &format!("plugin_init: failed to obtain model file: {}", e));
            return e;
        }
    };

    // Ensure tokenizer file is available
    let (tokenizer_path, tokenizer_updated) = match ensure_file(
        &model_conf.tokenizer_url,
        &model_conf.local_tokenizer_path,
        &data_dir,
        &model_conf.name,
        "tokenizer.json",
        host_ref,
    ) {
        Ok(result) => result,
        Err(_) => {
            // Tokenizer failure is not fatal — derive from model path as fallback
            log_msg(3, "plugin_init: tokenizer not available, trying derived path");
            (derive_tokenizer_path(&model_path), false)
        }
    };

    // Write updated paths back to config and save
    if model_updated || tokenizer_updated {
        if let Some(mc) = config.models.get_mut(&active) {
            if model_updated {
                mc.local_model_path = model_path.clone();
            }
            if tokenizer_updated {
                mc.local_tokenizer_path = tokenizer_path.clone();
            }
        }
        save_config(&config, &config_dir_str);
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
        log_msg(1, &format!(
            "plugin_init: initialised by another thread, ref_count={}",
            INIT_COUNT.load(Ordering::SeqCst)
        ));
        return E_OK;
    }

    // Load tokenizer
    let tokenizer = match Tokenizer::from_file(&tokenizer_path) {
        Ok(t) => {
            log_msg(1, &format!("Tokenizer loaded from {}", tokenizer_path));
            Some(t)
        }
        Err(e) => {
            log_msg(3, &format!("Tokenizer not found at {} ({}), continuing without", tokenizer_path, e));
            None
        }
    };

    // Create ONNX session
    let session = match Session::builder()
        .and_then(|mut b| b.commit_from_file(&model_path))
    {
        Ok(s) => s,
        Err(e) => {
            log_msg(4, &format!("plugin_init: failed to create ONNX session: {}", e));
            return E_INIT;
        }
    };

    state.session = Some(session);
    state.tokenizer = tokenizer;
    state.dim = dim;
    drop(state);

    INIT_COUNT.fetch_add(1, Ordering::SeqCst);
    log_msg(2, &format!(
        "Initialized: tier={}, model={}, dim={}, ref_count=1",
        active, model_path, dim
    ));
    E_OK
}

/// Compute embedding for the given text.
///
/// `text`: Null-terminated UTF-8 string.
/// `out`: Pointer to a buffer of `dim` floats (caller-allocated).
/// `dim`: Expected embedding dimension (must match plugin_init's dim).
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

    if dim != state.dim {
        log_msg(4, &format!(
            "plugin_embed: dimension mismatch (expected={}, got={})",
            state.dim, dim
        ));
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
            log_msg(4, &format!("plugin_embed: failed to create input_ids tensor: {}", e));
            return E_INFER;
        }
    };

    let attention_mask_data: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let attention_mask_shape = vec![1i64, seq_len as i64];
    let attention_mask_tensor = match Tensor::from_array((attention_mask_shape, attention_mask_data)) {
        Ok(t) => t,
        Err(e) => {
            log_msg(4, &format!("plugin_embed: failed to create attention_mask tensor: {}", e));
            return E_INFER;
        }
    };

    let token_type_ids_data = vec![0i64; seq_len];
    let token_type_ids_shape = vec![1i64, seq_len as i64];
    let token_type_ids_tensor = match Tensor::from_array((token_type_ids_shape, token_type_ids_data)) {
        Ok(t) => t,
        Err(e) => {
            log_msg(4, &format!("plugin_embed: failed to create token_type_ids tensor: {}", e));
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
            log_msg(4, &format!("plugin_embed: failed to extract output tensor: {}", e));
            return E_INFER;
        }
    };

    let expected_elements = seq_len * (dim as usize);
    if output_data.len() < expected_elements {
        log_msg(4, &format!(
            "plugin_embed: output too small (got {}, expected {})",
            output_data.len(),
            expected_elements
        ));
        return E_INFER;
    }

    // Mean pooling
    let mask_i64: Vec<i64> = attention_mask.iter().map(|&m| m as i64).collect();
    let mut pooled = mean_pool(output_data, &mask_i64, seq_len, dim as usize);

    // L2 normalization
    l2_normalize(&mut pooled);

    // Write result
    let out_slice = unsafe { std::slice::from_raw_parts_mut(out, dim as usize) };
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
            state.dim = 0;
        }
        INIT_COUNT.store(0, Ordering::SeqCst);
        PERMANENTLY_FREED.store(true, Ordering::SeqCst);
        HOST_PTR.store(std::ptr::null_mut(), Ordering::SeqCst);
        log_msg(2, "Resources released (last reference freed)");
    } else {
        log_msg(1, &format!(
            "plugin_free: ref_count {} → {} (session kept alive)",
            prev,
            prev - 1
        ));
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
        assert_eq!(E_DOWNLOAD, -7);
        assert_eq!(E_CONFIG, -8);
    }

    #[test]
    fn test_global_state_default() {
        let state = STATE.lock().unwrap();
        assert!(state.session.is_none());
        assert!(state.tokenizer.is_none());
        assert_eq!(state.dim, 0);
        assert_eq!(INIT_COUNT.load(Ordering::SeqCst), 0);
        assert!(!PERMANENTLY_FREED.load(Ordering::SeqCst));
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
    fn test_init_null_config_dir_with_null_host() {
        // With no host, plugin looks for model in current dir / test-data/.
        // If model exists (test-data/model.onnx), init succeeds; otherwise fails.
        // Either way, the function should not crash.
        let _result = plugin_init(std::ptr::null(), std::ptr::null());
        // Cleanup
        plugin_free();
    }

    #[test]
    fn test_init_nonexistent_config_dir_no_host() {
        // With nonexistent config_dir and no host, plugin looks in ./test-data/.
        // If model exists there, init succeeds; if not, fails.
        let config_dir = std::ffi::CString::new("/nonexistent/path").unwrap();
        let _result = plugin_init(config_dir.as_ptr(), std::ptr::null());
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
            let config_dir = std::ffi::CString::new("/nonexistent").unwrap();
            let result = plugin_init(config_dir.as_ptr(), std::ptr::null());
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

    #[test]
    fn test_load_config_default() {
        // Use a unique temp dir to avoid stale config from previous runs
        let temp_dir = format!("/tmp/plugin-onnx-test-{}", std::process::id());
        let config = load_config(&temp_dir);
        assert_eq!(config.active, "medium");
        assert_eq!(config.models.medium.dimension, 384);
        assert_eq!(config.models.medium.name, "all-MiniLM-L6-v2");
        assert!(!config.models.medium.model_url.is_empty());
        assert_eq!(config.models.large.name, "bge-base-en-v1.5");
        assert_eq!(config.models.large.dimension, 768);
        assert_eq!(config.models.small.name, "all-MiniLM-L4-v2");
        assert_eq!(config.models.small.dimension, 256);
        // Cleanup
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_config_embedded() {
        let config: PluginConfig = toml::from_str(DEFAULT_CONFIG).unwrap();
        assert_eq!(config.active, "medium");
        assert_eq!(config.models.medium.dimension, 384);
        assert_eq!(config.models.large.dimension, 768);
        assert_eq!(config.models.small.dimension, 256);
    }

    // ===================================================================
    // Model-required tests (run with `cargo test -- --ignored`)
    // ===================================================================

    fn test_model_dir() -> String {
        std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
            .unwrap_or_else(|_| {
                let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("test-data");
                dir.to_str().expect("valid path").to_string()
            })
    }

    /// All model tests in a single lifecycle to avoid ONNX Runtime re-init crash.
    #[test]
    #[ignore]
    fn test_all_model_scenarios() {
        let model_dir = test_model_dir();

        // ---- Init via plugin_init with config_dir pointing to test-data ----
        let config_dir = std::ffi::CString::new(model_dir.as_str()).unwrap();
        let init_result = plugin_init(config_dir.as_ptr(), std::ptr::null());
        assert_eq!(init_result, E_OK, "plugin_init should succeed");

        // ---- Scenario: Idempotent init (ref count) ----
        let init2 = plugin_init(config_dir.as_ptr(), std::ptr::null());
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
            let result = plugin_init(config_dir.as_ptr(), std::ptr::null());
            assert_eq!(result, E_INIT, "re-init after free should fail");
            println!("[model-test] re-init after free fails — PASS");
        }

        println!("[model-test] All scenarios PASSED");
    }
}
