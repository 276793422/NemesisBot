//! Embedding model configuration for nemesis-memory.
//!
//! Manages the embedding model configuration file (`embedding.toml`),
//! including loading, saving, and ensuring model files are available
//! (downloading if necessary).

use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{info, warn, error};

/// Compile-time embedded default configuration.
const DEFAULT_CONFIG: &str = include_str!("../../config/embedding.toml");

// ---------------------------------------------------------------------------
// Configuration types
// ---------------------------------------------------------------------------

/// Top-level embedding configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_active")]
    pub active: String,
    #[serde(default)]
    pub models: ModelsConfig,
}

fn default_active() -> String { "medium".to_string() }

impl Default for EmbeddingConfig {
    fn default() -> Self {
        toml::from_str(DEFAULT_CONFIG).expect("embedded config is valid")
    }
}

/// Container for the three model tiers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModelsConfig {
    #[serde(default)]
    pub large: ModelConfig,
    #[serde(default)]
    pub medium: ModelConfig,
    #[serde(default)]
    pub small: ModelConfig,
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
    config_dir.join("embedding.toml")
}

// ---------------------------------------------------------------------------
// Load / Save
// ---------------------------------------------------------------------------

/// Load embedding config from file, or save default and load it.
///
/// 1. If `{config_dir}/embedding.toml` exists → load it.
/// 2. If not → save embedded default config to that path, then load from disk.
pub fn load_embedding_config(config_dir: &Path) -> EmbeddingConfig {
    let path = config_path(config_dir);

    if !path.exists() {
        if let Err(e) = std::fs::create_dir_all(config_dir) {
            warn!("[EmbeddingConfig] Failed to create config dir '{}': {}", config_dir.display(), e);
        } else {
            match std::fs::write(&path, DEFAULT_CONFIG) {
                Ok(()) => {
                    info!("[EmbeddingConfig] Default embedding config saved to {}", path.display());
                }
                Err(e) => {
                    warn!("[EmbeddingConfig] Failed to save default embedding config: {}", e);
                }
            }
        }
    }

    match std::fs::read_to_string(&path) {
        Ok(content) => match toml::from_str::<EmbeddingConfig>(&content) {
            Ok(config) => {
                info!("[EmbeddingConfig] Embedding config loaded from {}", path.display());
                config
            }
            Err(e) => {
                error!("[EmbeddingConfig] Failed to parse embedding config '{}': {}", path.display(), e);
                EmbeddingConfig::default()
            }
        },
        Err(e) => {
            warn!("[EmbeddingConfig] Failed to read embedding config '{}': {}, using defaults", path.display(), e);
            EmbeddingConfig::default()
        }
    }
}

/// Save embedding configuration back to disk.
pub fn save_embedding_config(config: &EmbeddingConfig, config_dir: &Path) {
    let path = config_path(config_dir);
    match toml::to_string_pretty(config) {
        Ok(content) => {
            if let Err(e) = std::fs::write(&path, content) {
                warn!("[EmbeddingConfig] Failed to save embedding config to {}: {}", path.display(), e);
            } else {
                info!("[EmbeddingConfig] Embedding config saved to {}", path.display());
            }
        }
        Err(e) => {
            warn!("[EmbeddingConfig] Failed to serialize embedding config: {}", e);
        }
    }
}

// ---------------------------------------------------------------------------
// Model file management
// ---------------------------------------------------------------------------

/// Ensure model and tokenizer files are available for the active tier.
///
/// Returns `(model_dir, dim)` where `model_dir` is the directory containing
/// both model.onnx and tokenizer.json.
///
/// Flow:
/// 1. Get active model config
/// 2. Check local_model_path → if exists, use its parent dir
/// 3. If missing → download from model_url → update config
/// 4. Same for tokenizer
/// 5. Return the model directory + dimension
pub fn ensure_model_files(
    config: &mut EmbeddingConfig,
    config_dir: &Path,
) -> Result<(String, i32), String> {
    let active = config.active.clone();
    let model_conf = config.models.get(&active).cloned()
        .ok_or_else(|| format!("unknown active model tier: '{}'", active))?;

    let dim = model_conf.dimension;
    if dim <= 0 {
        return Err(format!("invalid dimension={} for model '{}'", dim, active));
    }
    if model_conf.name.is_empty() {
        return Err(format!("model name is empty for tier '{}'", active));
    }

    // Determine model data directory: prefer config_dir/models/{model_name}
    let data_dir = config_dir.join("models").join(&model_conf.name);
    std::fs::create_dir_all(&data_dir)
        .map_err(|e| format!("failed to create model dir: {}", e))?;

    let model_dest = data_dir.join("model.onnx");
    let _tokenizer_dest = data_dir.join("tokenizer.json");

    let mut model_updated = false;
    let mut tokenizer_updated = false;

    // Ensure model file is available
    let model_path = if !model_conf.local_model_path.is_empty()
        && Path::new(&model_conf.local_model_path).exists()
    {
        info!("[EmbeddingConfig] Model found at {}", model_conf.local_model_path);
        // Use the directory of the existing model file
        Path::new(&model_conf.local_model_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.clone())
    } else {
        // Try the default data_dir location first
        if model_dest.exists() {
            info!("[EmbeddingConfig] Model found at {}", model_dest.display());
            data_dir.clone()
        } else if config_dir.join("model.onnx").exists() {
            // Fallback: check config_dir itself (useful for test-data dirs)
            info!("[EmbeddingConfig] Model found at {}", config_dir.join("model.onnx").display());
            config_dir.to_path_buf()
        } else if model_conf.model_url.is_empty() {
            return Err(format!("model file not found and no URL configured for tier '{}'", active));
        } else {
            info!("[EmbeddingConfig] Downloading model from {}...", model_conf.model_url);
            download_file(&model_conf.model_url, &model_dest)?;
            model_updated = true;
            info!("[EmbeddingConfig] Model downloaded to {}", model_dest.display());
            data_dir.clone()
        }
    };

    // Ensure tokenizer file is available (must be in same directory as model)
    let tokenizer_path = model_path.join("tokenizer.json");
    if !tokenizer_path.exists() {
        // Check if local_tokenizer_path is set and exists
        if !model_conf.local_tokenizer_path.is_empty()
            && Path::new(&model_conf.local_tokenizer_path).exists()
        {
            // Copy to model directory
            let src = Path::new(&model_conf.local_tokenizer_path);
            if let Err(e) = std::fs::copy(src, &tokenizer_path) {
                warn!("[EmbeddingConfig] Failed to copy tokenizer to model dir: {}", e);
            }
        } else if !model_conf.tokenizer_url.is_empty() {
            info!("[EmbeddingConfig] Downloading tokenizer from {}...", model_conf.tokenizer_url);
            download_file(&model_conf.tokenizer_url, &tokenizer_path)?;
            tokenizer_updated = true;
            info!("[EmbeddingConfig] Tokenizer downloaded to {}", tokenizer_path.display());
        }
        // If tokenizer still not available, the plugin will handle the error
    }

    // Write updated paths back to config
    if model_updated || tokenizer_updated {
        if let Some(mc) = config.models.get_mut(&active) {
            if model_updated {
                mc.local_model_path = model_path.join("model.onnx").to_string_lossy().to_string();
            }
            if tokenizer_updated {
                mc.local_tokenizer_path = model_path.join("tokenizer.json").to_string_lossy().to_string();
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
        return Err(format!("download failed for {}: HTTP {}", url, response.status()));
    }

    let bytes = response.bytes()
        .map_err(|e| format!("failed to read download response: {}", e))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent dir: {}", e))?;
    }

    std::fs::write(dest, &bytes)
        .map_err(|e| format!("failed to write file to {}: {}", dest.display(), e))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
