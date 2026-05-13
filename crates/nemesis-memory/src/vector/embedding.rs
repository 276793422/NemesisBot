//! Embedding function factory - selects the appropriate embedding tier.
//!
//! Priority:
//!   1. Plugin (ONNX) - best quality, fully offline
//!   2. API (Provider) - good quality, costs tokens
//!   3. Local hash - zero cost, fully offline

use std::sync::Mutex;

use crate::types::VectorConfig;
use crate::vector::plugin_loader::{NativePlugin, EmbeddingPlugin};

/// An embedding function that produces a fixed-dimension vector from text.
pub type EmbeddingFunc = Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync>;

/// Create an embedding function based on configuration.
///
/// The returned function is never nil.
pub fn new_embedding_func(config: &VectorConfig) -> EmbeddingFunc {
    let tier = config.embedding_tier.as_str();

    // Tier 1: Plugin (if configured)
    if (tier == "plugin" || tier == "auto" || tier == "") && config.plugin_path.is_some() {
        let plugin_path = config.plugin_path.as_ref().unwrap();
        if std::path::Path::new(plugin_path).exists() {
            match try_load_plugin(plugin_path, config) {
                Ok(func) => {
                    tracing::info!(path = %plugin_path, "Plugin embedding loaded successfully");
                    return func;
                }
                Err(e) => {
                    tracing::warn!(path = %plugin_path, error = %e, "Plugin embedding failed, falling back");
                }
            }
        }
    }

    // Tier 2: API (if configured)
    if (tier == "api" || tier == "auto" || tier == "") && config.api_model.is_some() {
        // API embedding would go here - requires provider
        tracing::info!("API embedding configured but no provider available, falling back");
    }

    // Tier 3: Local hash (always available)
    let dim = config.local_dim;
    Box::new(move |text: &str| {
        Ok(crate::vector::embedding_local::ngram_hash_embed(text, dim))
    })
}

/// Attempt to load a native embedding plugin and wrap it as an EmbeddingFunc.
fn try_load_plugin(
    plugin_path: &str,
    config: &VectorConfig,
) -> Result<EmbeddingFunc, crate::vector::plugin_loader::PluginError> {
    let mut plugin = NativePlugin::load(plugin_path)?;

    let model_path = config
        .plugin_model_path
        .as_deref()
        .unwrap_or("");
    let dim = config.local_dim as i32;

    plugin.init(model_path, dim)?;

    // Wrap in Mutex for thread-safe access inside the closure.
    let plugin = Mutex::new(plugin);

    Ok(Box::new(move |text: &str| {
        let guard = plugin.lock().map_err(|e| e.to_string())?;
        guard.embed(text).map_err(|e| e.to_string())
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::VectorConfig;

    #[test]
    fn test_new_embedding_func_auto_no_plugin_no_api_returns_local() {
        let config = VectorConfig {
            embedding_tier: "auto".to_string(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("hello world").unwrap();
        assert_eq!(result.len(), 64);
        assert!(result.iter().any(|v| *v != 0.0));
    }

    #[test]
    fn test_new_embedding_func_empty_tier_returns_local() {
        let config = VectorConfig {
            embedding_tier: String::new(),
            local_dim: 128,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 128);
    }

    #[test]
    fn test_new_embedding_func_local_tier_returns_local() {
        let config = VectorConfig {
            embedding_tier: "local".to_string(),
            local_dim: 256,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("test text").unwrap();
        assert_eq!(result.len(), 256);
    }

    #[test]
    fn test_new_embedding_func_plugin_tier_nonexistent_falls_back() {
        let config = VectorConfig {
            embedding_tier: "plugin".to_string(),
            local_dim: 64,
            plugin_path: Some("/nonexistent/plugin.so".to_string()),
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_new_embedding_func_auto_plugin_nonexistent_falls_back() {
        let config = VectorConfig {
            embedding_tier: "auto".to_string(),
            local_dim: 32,
            plugin_path: Some("/does/not/exist.dll".to_string()),
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("hello").unwrap();
        assert_eq!(result.len(), 32);
    }

    #[test]
    fn test_local_hash_produces_different_vectors_for_different_text() {
        let config = VectorConfig::default();
        let func = new_embedding_func(&config);
        let v1 = func("cat").unwrap();
        let v2 = func("dog").unwrap();
        assert_ne!(v1, v2);
    }

    #[test]
    fn test_local_hash_produces_same_vectors_for_same_text() {
        let config = VectorConfig::default();
        let func = new_embedding_func(&config);
        let v1 = func("same text").unwrap();
        let v2 = func("same text").unwrap();
        assert_eq!(v1, v2);
    }

    #[test]
    fn test_local_hash_correct_dimension() {
        for dim in &[64, 128, 256, 512] {
            let config = VectorConfig {
                embedding_tier: "local".to_string(),
                local_dim: *dim,
                plugin_path: None,
                plugin_model_path: None,
                api_model: None,
            };
            let func = new_embedding_func(&config);
            let result = func("test").unwrap();
            assert_eq!(result.len(), *dim);
        }
    }

    #[test]
    fn test_local_hash_is_l2_normalized() {
        let config = VectorConfig {
            embedding_tier: "local".to_string(),
            local_dim: 128,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let v = func("normalization test").unwrap();
        let norm: f64 = v.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
        assert!((norm - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_new_embedding_func_api_tier_no_provider_falls_back() {
        let config = VectorConfig {
            embedding_tier: "api".to_string(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: Some("text-embedding-3-small".to_string()),
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_embedding_func_empty_text() {
        let config = VectorConfig {
            embedding_tier: "local".to_string(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("").unwrap();
        assert_eq!(result.len(), 64);
        assert!(result.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn test_new_embedding_func_auto_with_api_model_falls_back() {
        let config = VectorConfig {
            embedding_tier: "auto".into(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: Some("text-embedding-3-small".into()),
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_new_embedding_func_empty_tier_with_api() {
        let config = VectorConfig {
            embedding_tier: String::new(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: Some("model".into()),
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 64);
    }

    #[test]
    fn test_try_load_plugin_nonexistent() {
        let config = VectorConfig {
            embedding_tier: "plugin".into(),
            local_dim: 64,
            plugin_path: Some("/nonexistent/plugin.so".into()),
            plugin_model_path: None,
            api_model: None,
        };
        let result = try_load_plugin("/nonexistent/plugin.so", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_new_embedding_func_plugin_tier_no_path_falls_back() {
        let config = VectorConfig {
            embedding_tier: "plugin".into(),
            local_dim: 64,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
        };
        let func = new_embedding_func(&config);
        let result = func("test").unwrap();
        assert_eq!(result.len(), 64);
    }
}
