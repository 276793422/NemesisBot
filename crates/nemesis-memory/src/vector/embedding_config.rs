//! Embedding model configuration for nemesis-memory.
//!
//! Manages the embedding model configuration file (`config.enhanced_memory.json`),
//! including loading, saving, and resolving model file paths.
//!
//! Model downloading is strictly user-initiated (`download_model_files`):
//! only the Dashboard install button and CLI commands may download models.
//! All other paths use `resolve_model_files` (check-only, never downloads).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Top-level embedding configuration.
///
/// Merged from the former `config.enhanced_memory.json` (enabled switch)
/// and `embedding.toml` (model definitions) into a single JSON file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    /// Whether enhanced memory (vector search) is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Active model tier: "large", "medium", or "small".
    #[serde(default = "default_active")]
    pub active: String,
    /// Model definitions for each tier.
    #[serde(default)]
    pub models: ModelsConfig,
}

fn default_active() -> String {
    "medium".to_string()
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active: default_active(),
            models: ModelsConfig::default(),
        }
    }
}

/// Container for the three model tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    #[serde(default = "default_large")]
    pub large: ModelConfig,
    #[serde(default = "default_medium")]
    pub medium: ModelConfig,
    #[serde(default = "default_small")]
    pub small: ModelConfig,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            large: default_large(),
            medium: default_medium(),
            small: default_small(),
        }
    }
}

fn default_large() -> ModelConfig {
    ModelConfig {
        name: "bge-base-en-v1.5".into(),
        dimension: 768,
        model_url: "https://hf-mirror.com/BAAI/bge-base-en-v1.5/resolve/main/onnx/model.onnx"
            .into(),
        model_size: 430000000,
        tokenizer_url: "https://hf-mirror.com/BAAI/bge-base-en-v1.5/resolve/main/tokenizer.json"
            .into(),
        tokenizer_size: 700000,
        local_model_path: String::new(),
        local_tokenizer_path: String::new(),
    }
}

fn default_medium() -> ModelConfig {
    ModelConfig {
        name: "all-MiniLM-L6-v2".into(),
        dimension: 384,
        model_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx".into(),
        model_size: 90405214,
        tokenizer_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json".into(),
        tokenizer_size: 466247,
        local_model_path: String::new(),
        local_tokenizer_path: String::new(),
    }
}

fn default_small() -> ModelConfig {
    ModelConfig {
        name: "all-MiniLM-L4-v2".into(),
        dimension: 256,
        model_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L4-v2/resolve/main/onnx/model.onnx".into(),
        model_size: 60000000,
        tokenizer_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L4-v2/resolve/main/tokenizer.json".into(),
        tokenizer_size: 466000,
        local_model_path: String::new(),
        local_tokenizer_path: String::new(),
    }
}

impl ModelsConfig {
    pub fn get(&self, key: &str) -> Option<&ModelConfig> {
        match key {
            "large" => Some(&self.large),
            "medium" => Some(&self.medium),
            "small" => Some(&self.small),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut ModelConfig> {
        match key {
            "large" => Some(&mut self.large),
            "medium" => Some(&mut self.medium),
            "small" => Some(&mut self.small),
            _ => None,
        }
    }
}

/// Per-tier model configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub dimension: i32,
    #[serde(default)]
    pub model_url: String,
    #[serde(default)]
    pub model_size: u64,
    #[serde(default)]
    pub tokenizer_url: String,
    #[serde(default)]
    pub tokenizer_size: u64,
    /// Absolute local path after download. Empty = not yet downloaded.
    #[serde(default)]
    pub local_model_path: String,
    #[serde(default)]
    pub local_tokenizer_path: String,
}

// ---------------------------------------------------------------------------
// Config file path
// ---------------------------------------------------------------------------

/// Return the path to the embedding config file within the given config directory.
fn config_path(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join("config.enhanced_memory.json")
}

// ---------------------------------------------------------------------------
// Default config content (for creating new files)
// ---------------------------------------------------------------------------

/// Return the default config as a JSON string for writing to disk.
fn default_config_json() -> String {
    let config = EmbeddingConfig {
        enabled: false,
        active: default_active(),
        models: ModelsConfig {
            large: ModelConfig {
                name: "bge-base-en-v1.5".into(),
                dimension: 768,
                model_url: "https://hf-mirror.com/BAAI/bge-base-en-v1.5/resolve/main/onnx/model.onnx".into(),
                model_size: 430000000,
                tokenizer_url: "https://hf-mirror.com/BAAI/bge-base-en-v1.5/resolve/main/tokenizer.json".into(),
                tokenizer_size: 700000,
                local_model_path: String::new(),
                local_tokenizer_path: String::new(),
            },
            medium: ModelConfig {
                name: "all-MiniLM-L6-v2".into(),
                dimension: 384,
                model_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/onnx/model.onnx".into(),
                model_size: 90405214,
                tokenizer_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L6-v2/resolve/main/tokenizer.json".into(),
                tokenizer_size: 466247,
                local_model_path: String::new(),
                local_tokenizer_path: String::new(),
            },
            small: ModelConfig {
                name: "all-MiniLM-L4-v2".into(),
                dimension: 256,
                model_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L4-v2/resolve/main/onnx/model.onnx".into(),
                model_size: 60000000,
                tokenizer_url: "https://hf-mirror.com/sentence-transformers/all-MiniLM-L4-v2/resolve/main/tokenizer.json".into(),
                tokenizer_size: 466000,
                local_model_path: String::new(),
                local_tokenizer_path: String::new(),
            },
        },
    };
    serde_json::to_string_pretty(&config).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

/// Load embedding config from file, or save default and load it.
///
/// 1. If `{config_dir}/config.enhanced_memory.json` exists → load it.
/// 2. If not → save default config to that path, then load from disk.
pub fn load_embedding_config(config_dir: &Path) -> EmbeddingConfig {
    let path = config_path(config_dir);

    if !path.exists() {
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            warn!(
                "[EmbeddingConfig] Failed to create config dir '{}': {}",
                config_dir.display(),
                e
            );
        } else {
            match std::fs::write(&path, default_config_json()) {
                Ok(()) => {
                    info!(
                        "[EmbeddingConfig] Default embedding config saved to {}",
                        path.display()
                    );
                }
                Err(e) => {
                    warn!(
                        "[EmbeddingConfig] Failed to save default embedding config: {}",
                        e
                    );
                }
            }
        }
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str::<EmbeddingConfig>(&content) {
            Ok(config) => {
                info!(
                    "[EmbeddingConfig] Embedding config loaded from {}",
                    path.display()
                );
                config
            }
            Err(e) => {
                error!(
                    "[EmbeddingConfig] Failed to parse embedding config '{}': {}",
                    path.display(),
                    e
                );
                EmbeddingConfig::default()
            }
        },
        Err(e) => {
            warn!(
                "[EmbeddingConfig] Failed to read embedding config '{}': {}, using defaults",
                path.display(),
                e
            );
            EmbeddingConfig::default()
        }
    }
}

/// Save embedding configuration back to disk.
pub fn save_embedding_config(config: &EmbeddingConfig, config_dir: &Path) {
    let path = config_path(config_dir);
    match serde_json::to_string_pretty(config) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!(
                    "[EmbeddingConfig] Failed to save embedding config to {}: {}",
                    path.display(),
                    e
                );
            } else {
                info!(
                    "[EmbeddingConfig] Embedding config saved to {}",
                    path.display()
                );
            }
        }
        Err(e) => {
            warn!(
                "[EmbeddingConfig] Failed to serialize embedding config: {}",
                e
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Model data directory
// ---------------------------------------------------------------------------

/// Return the directory where embedding model files are stored.
///
/// `config_dir` is `{workspace}/config`, so the data directory is
/// `{workspace}/tools/memory/data/embedding`.
pub fn embedding_data_dir(config_dir: &Path) -> std::path::PathBuf {
    config_dir
        .parent()
        .unwrap_or(config_dir)
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
}

// ---------------------------------------------------------------------------
// Model file management
// ---------------------------------------------------------------------------

/// Check if model files for the active tier exist locally (no download).
///
/// Returns `(model_dir, dim)` where `model_dir` is the directory containing
/// both model.onnx and tokenizer.json.
///
/// Returns `Err` if the model files are not found — the caller should
/// not attempt to proceed with vector store initialization.
pub fn resolve_model_files(
    config: &EmbeddingConfig,
    config_dir: &Path,
) -> Result<(String, i32), String> {
    let active = &config.active;
    let model_conf = config
        .models
        .get(active)
        .cloned()
        .ok_or_else(|| format!("unknown active model tier: '{}'", active))?;

    let dim = model_conf.dimension;
    if dim <= 0 {
        return Err(format!("invalid dimension={} for model '{}'", dim, active));
    }
    if model_conf.name.is_empty() {
        return Err(format!("model name is empty for tier '{}'", active));
    }

    // Check local_model_path first
    let data_dir = embedding_data_dir(config_dir).join(&model_conf.name);

    let model_path = if !model_conf.local_model_path.is_empty()
        && Path::new(&model_conf.local_model_path).exists()
    {
        info!(
            "[EmbeddingConfig] Model found at {}",
            model_conf.local_model_path
        );
        Path::new(&model_conf.local_model_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.clone())
    } else if data_dir.join("model.onnx").exists() {
        info!(
            "[EmbeddingConfig] Model found at {}",
            data_dir.join("model.onnx").display()
        );
        data_dir.clone()
    } else if config_dir.join("model.onnx").exists() {
        info!(
            "[EmbeddingConfig] Model found at {}",
            config_dir.join("model.onnx").display()
        );
        config_dir.to_path_buf()
    } else {
        return Err(format!(
            "模型文件未安装 (tier '{}'). 请先通过 Dashboard 或 CLI 安装模型",
            active
        ));
    };

    Ok((model_path.to_string_lossy().to_string(), dim))
}

/// Download model and tokenizer files for the active tier.
///
/// This is the ONLY function that downloads model files. It should only be
/// called from user-initiated actions (Dashboard install button, CLI command).
///
/// Returns `(model_dir, dim)` and updates `local_model_path` / `local_tokenizer_path`
/// in the config, then saves the config to disk.
pub fn download_model_files(
    config: &mut EmbeddingConfig,
    config_dir: &Path,
) -> Result<(String, i32), String> {
    let active = config.active.clone();
    let model_conf = config
        .models
        .get(&active)
        .cloned()
        .ok_or_else(|| format!("unknown active model tier: '{}'", active))?;

    let dim = model_conf.dimension;
    if dim <= 0 {
        return Err(format!("invalid dimension={} for model '{}'", dim, active));
    }
    if model_conf.name.is_empty() {
        return Err(format!("model name is empty for tier '{}'", active));
    }

    // Determine model data directory: {workspace}/tools/memory/data/embedding/{model_name}
    let data_dir = embedding_data_dir(config_dir).join(&model_conf.name);
    std::fs::create_dir_all(&data_dir).map_err(|e| format!("failed to create model dir: {}", e))?;

    let model_dest = data_dir.join("model.onnx");
    let mut model_updated = false;
    let mut tokenizer_updated = false;

    // Resolve or download model file
    let model_path = if !model_conf.local_model_path.is_empty()
        && Path::new(&model_conf.local_model_path).exists()
    {
        info!(
            "[EmbeddingConfig] Model found at {}",
            model_conf.local_model_path
        );
        Path::new(&model_conf.local_model_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.clone())
    } else if model_dest.exists() {
        info!("[EmbeddingConfig] Model found at {}", model_dest.display());
        data_dir.clone()
    } else if config_dir.join("model.onnx").exists() {
        info!(
            "[EmbeddingConfig] Model found at {}",
            config_dir.join("model.onnx").display()
        );
        config_dir.to_path_buf()
    } else if model_conf.model_url.is_empty() {
        return Err(format!(
            "model file not found and no URL configured for tier '{}'",
            active
        ));
    } else {
        info!(
            "[EmbeddingConfig] Downloading model from {}...",
            model_conf.model_url
        );
        download_file(&model_conf.model_url, &model_dest)?;
        model_updated = true;
        info!(
            "[EmbeddingConfig] Model downloaded to {}",
            model_dest.display()
        );
        data_dir.clone()
    };

    // Resolve or download tokenizer file
    let tokenizer_path = model_path.join("tokenizer.json");
    if !tokenizer_path.exists() {
        if !model_conf.local_tokenizer_path.is_empty()
            && Path::new(&model_conf.local_tokenizer_path).exists()
        {
            let src = Path::new(&model_conf.local_tokenizer_path);
            if let Err(e) = std::fs::copy(src, &tokenizer_path) {
                warn!(
                    "[EmbeddingConfig] Failed to copy tokenizer to model dir: {}",
                    e
                );
            }
        } else if !model_conf.tokenizer_url.is_empty() {
            info!(
                "[EmbeddingConfig] Downloading tokenizer from {}...",
                model_conf.tokenizer_url
            );
            download_file(&model_conf.tokenizer_url, &tokenizer_path)?;
            tokenizer_updated = true;
            info!(
                "[EmbeddingConfig] Tokenizer downloaded to {}",
                tokenizer_path.display()
            );
        }
    }

    // Write updated paths back to config
    if model_updated || tokenizer_updated {
        if let Some(mc) = config.models.get_mut(&active) {
            if model_updated {
                mc.local_model_path = model_path.join("model.onnx").to_string_lossy().to_string();
            }
            if tokenizer_updated {
                mc.local_tokenizer_path = model_path
                    .join("tokenizer.json")
                    .to_string_lossy()
                    .to_string();
            }
        }
        save_embedding_config(config, config_dir);
    }

    Ok((model_path.to_string_lossy().to_string(), dim))
}

/// Download a file using reqwest (blocking).
fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Ok(());
    }

    let response = reqwest::blocking::get(url)
        .map_err(|e| format!("download request failed for {}: {}", url, e))?;

    if !response.status().is_success() {
        return Err(format!(
            "download failed for {}: HTTP {}",
            url,
            response.status()
        ));
    }

    let bytes = response
        .bytes()
        .map_err(|e| format!("failed to read download response: {}", e))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent dir: {}", e))?;
    }

    std::fs::write(dest, &bytes)
        .map_err(|e| format!("failed to write file to {}: {}", dest.display(), e))?;

    Ok(())
}

#[cfg(test)]
mod extra_tests;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
