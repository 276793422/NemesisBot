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
// download_file 超时——reqwest::blocking::get 内部
// Client::new() 零超时，模型 CDN 半死（TCP 已建但不发数据）时 Dashboard
// 安装按钮永久挂死。reqwest 0.12.28 的 blocking::ClientBuilder **没有
// read_timeout**（只有总 timeout/connect_timeout），对齐同族先例
// nemesis-voice model.rs用总超时 600s 兜底：嵌入模型
// 实际只有几十 MB（hf-mirror 实测 ~3.6MB/s，合法下载远低于 600s），
// write_file_atomic 保证超时中途失败不落半截残骸（重装幂等重下）。
#[cfg(not(test))]
const DOWNLOAD_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(600);
#[cfg(test)]
// 测试态 400ms：stall 回归测试经真实 download_file 跑，等待必须秒级以内。
const DOWNLOAD_TOTAL_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(400);
const DOWNLOAD_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
    /// .1 (sixth batch): auto-inject memory hits into the conversation as
    /// a `# Memory Context` snapshot section. Grey-release default OFF —
    /// with false the agent's message stream is byte-identical to before
    /// this feature existed (same promise as steer mode).
    #[serde(default)]
    pub auto_inject: bool,
    /// .1: how many top-K memory hits to inject at most (default 3).
    #[serde(default = "default_auto_inject_top_k")]
    pub auto_inject_top_k: usize,
}

fn default_active() -> String {
    "medium".to_string()
}

fn default_auto_inject_top_k() -> usize {
    3
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            active: default_active(),
            models: ModelsConfig::default(),
            auto_inject: false,
            auto_inject_top_k: default_auto_inject_top_k(),
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
        auto_inject: false,
        auto_inject_top_k: default_auto_inject_top_k(),
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

    match nemesis_utils::fsutil::read_config_string(&path) {
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
            // 落盘走 write_file_atomic（原子
            // 写家族）——裸 std::fs:write 直写终路径，写一半（进程被杀/磁盘
            // 满）残留半截 JSON；load 解析失败回落 default（enabled=false），
            // 损坏文件滞留盘上，增强记忆静默自禁且需手工清理。
            if let Err(e) = nemesis_utils::file::write_file_atomic(
                &path.to_string_lossy(),
                content.as_bytes(),
                0o644,
            ) {
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

    // Check local_model_path first. 存在性判定一律走 is_non_empty_file：
    // 零字节残骸不能报 Model found——否则这里放行、
    // 插件 init 以晦涩错误失败，用户重装也永远跳不过残骸。
    let data_dir = embedding_data_dir(config_dir).join(&model_conf.name);

    let model_path = if !model_conf.local_model_path.is_empty()
        && is_non_empty_file(Path::new(&model_conf.local_model_path))
    {
        info!(
            "[EmbeddingConfig] Model found at {}",
            model_conf.local_model_path
        );
        Path::new(&model_conf.local_model_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.clone())
    } else if is_non_empty_file(&data_dir.join("model.onnx")) {
        info!(
            "[EmbeddingConfig] Model found at {}",
            data_dir.join("model.onnx").display()
        );
        data_dir.clone()
    } else if is_non_empty_file(&config_dir.join("model.onnx")) {
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

/// 零字节文件 = 中断下载的残骸：存在但为空不能当
/// 「已下载/已安装」放行，调用方应走重下路径。metadata 读失败视为不存在。
/// 目录形态同样不算（ext4 上目录 metadata len>0 会
/// 误判「非空」，且目录本就不是可加载的模型/分词器资产）。
fn is_non_empty_file(p: &Path) -> bool {
    std::fs::metadata(p)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
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
        && is_non_empty_file(Path::new(&model_conf.local_model_path))
    {
        info!(
            "[EmbeddingConfig] Model found at {}",
            model_conf.local_model_path
        );
        Path::new(&model_conf.local_model_path)
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| data_dir.clone())
    } else if is_non_empty_file(&model_dest) {
        info!("[EmbeddingConfig] Model found at {}", model_dest.display());
        data_dir.clone()
    } else if is_non_empty_file(&config_dir.join("model.onnx")) {
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
    if !is_non_empty_file(&tokenizer_path) {
        // 拷贝源判定与目标/模型臂同语义（is_non_empty_file =
        // 零字节/目录形态残骸 = 未安装）。旧代码裸 exists() 把残骸当有效源，
        // fs::copy 拷出 0 字节 tokenizer 后照常报成功——残骸被合法化、下方 URL
        // 下载臂被遮蔽，重装永远跳不过（在 model 臂修掉的
        // 病理在本地拷贝臂重演）。fs::copy 非原子（中断残留半截文件）→ 改读
        // 全量 + write_file_atomic（原子写家族）；拷贝失败
        // 不再静默——落 URL 臂兜底，两臂皆不成由下方最终校验诚实报错。
        let local_src = (!model_conf.local_tokenizer_path.is_empty())
            .then(|| Path::new(&model_conf.local_tokenizer_path))
            .filter(|p| is_non_empty_file(p));
        let mut copied = false;
        if let Some(src) = local_src {
            let copied_res = std::fs::read(src)
                .map_err(|e| format!("read {}: {}", src.display(), e))
                .and_then(|bytes| {
                    nemesis_utils::file::write_file_atomic(
                        &tokenizer_path.to_string_lossy(),
                        &bytes,
                        0o644,
                    )
                    .map_err(|e| format!("write {}: {}", tokenizer_path.display(), e))
                });
            match copied_res {
                Ok(()) => copied = true,
                Err(e) => warn!(
                    "[EmbeddingConfig] Failed to copy tokenizer from {}: {}",
                    src.display(),
                    e
                ),
            }
        }
        if !copied && !model_conf.tokenizer_url.is_empty() {
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

    // 安装最终校验——无论本地拷贝/下载哪条臂，tokenizer
    // 必须真实非空才算安装成功。旧形态两臂皆不成时静默 Ok（tokenizer 缺失
    // /零字节），插件 init 以晦涩错误失败 → with_config_dir 写 enabled:false
    // 永久自禁增强记忆（定义的失败链）。
    if !is_non_empty_file(&tokenizer_path) {
        return Err(format!(
            "tokenizer 文件未安装或为空: {}。请检查 local_tokenizer_path / tokenizer_url 配置后重试安装",
            tokenizer_path.display()
        ));
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
///
/// 三重加固（对齐 nemesis-voice model.rs 同族已修先例
///
/// - 已存在文件只在**非空**时跳过——零字节是中断下载的残骸，重下；
/// - 响应体为空 → 诚实报错，不落 0 字节终文件；
/// - 落盘走 write_file_atomic——写一半（进程被杀/磁盘满）不会以终文件名
///   残留半截文件。旧 `std::fs:write` 直写终路径是「一次中断 → 半截
///   model.onnx → 重装永远跳过 → 插件 init 失败 → with_config_dir 写
///   enabled:false 永久自禁增强记忆」失败链的源头。
fn download_file(url: &str, dest: &Path) -> Result<(), String> {
    if is_non_empty_file(dest) {
        return Ok(());
    }

    // blocking::get 内部 Client::new() 零超时 → 换带
    // connect/total timeout 的显式客户端，半死 CDN 上诚实报错不挂死。
    let client = reqwest::blocking::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TOTAL_TIMEOUT)
        .build()
        .map_err(|e| format!("download client build failed for {}: {}", url, e))?;
    let response = client
        .get(url)
        .send()
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

    if bytes.is_empty() {
        return Err(format!("downloaded file is empty: {}", url));
    }

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create parent dir: {}", e))?;
    }

    nemesis_utils::file::write_file_atomic(&dest.to_string_lossy(), &bytes, 0o644)
        .map_err(|e| format!("failed to write file to {}: {}", dest.display(), e))?;

    Ok(())
}

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod w2c_tests;

#[cfg(test)]
mod cov_tests;

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
