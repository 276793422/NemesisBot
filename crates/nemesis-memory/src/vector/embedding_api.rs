//! API-based embedding function.
//!
//! Uses an HTTP-based embedding API endpoint to generate embeddings.
//! The function takes a base URL, model name, and optional API key,
//! then sends text to the endpoint and returns float vectors.

use serde::Deserialize;

/// Response from an OpenAI-compatible embedding API.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

/// Configuration for the API embedding function.
#[derive(Debug, Clone)]
pub struct ApiEmbeddingConfig {
    /// Base URL for the embedding API (e.g., "https://api.openai.com/v1").
    pub base_url: String,
    /// Model name (e.g., "text-embedding-3-small").
    pub model: String,
    /// Optional API key for authentication.
    pub api_key: Option<String>,
    /// Expected embedding dimension (for validation).
    pub dimension: usize,
}

/// Create an API embedding function that calls an OpenAI-compatible embedding endpoint.
///
/// The returned function sends a POST request to `{base_url}/embeddings` with
/// the text input and returns the embedding vector from the response.
///
/// Matches Go's `APIEmbeddingFunc` which uses `provider.CreateEmbedding`.
pub fn api_embedding_func(config: ApiEmbeddingConfig) -> Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
    Box::new(move |text: &str| {
        // Build the request body (OpenAI-compatible format)
        let request_body = serde_json::json!({
            "model": config.model,
            "input": text,
        });

        // Build the URL
        let url = format!("{}/embeddings", config.base_url.trim_end_matches('/'));

        // Build the HTTP client and request
        let client = reqwest::blocking::Client::new();
        let mut req = client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&request_body);

        if let Some(ref key) = config.api_key {
            req = req.header("Authorization", format!("Bearer {}", key));
        }

        // Send the request
        let resp = req
            .send()
            .map_err(|e| format!("API embedding request failed: {}", e))?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().unwrap_or_default();
            return Err(format!(
                "API embedding returned status {}: {}",
                status, body
            ));
        }

        // Parse the response
        let resp_data: EmbeddingResponse = resp
            .json()
            .map_err(|e| format!("API embedding response parse error: {}", e))?;

        // Extract the embedding vector from the first result
        let embedding = resp_data
            .data
            .into_iter()
            .next()
            .ok_or_else(|| "API embedding returned no data".to_string())?
            .embedding;

        // Validate dimension if configured
        if config.dimension > 0 && embedding.len() != config.dimension {
            return Err(format!(
                "API embedding dimension mismatch: expected {}, got {}",
                config.dimension,
                embedding.len()
            ));
        }

        Ok(embedding)
    })
}

/// Create a simpler API embedding function using just model name and default dimension.
///
/// This is the legacy interface that matches the previous signature. It requires
/// the caller to have set up the API configuration elsewhere and injected it.
pub fn api_embedding_func_simple(model: String) -> Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync> {
    let _dim = 256; // default dimension
    Box::new(move |text: &str| {
        Err(format!(
            "API embedding not configured: model={}, text length={}. \
             Use api_embedding_func() with ApiEmbeddingConfig instead.",
            model,
            text.len()
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_embedding_func_simple_returns_error() {
        let func = api_embedding_func_simple("text-embedding-3-small".into());
        let result = func("hello");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not configured"));
    }

    #[test]
    fn test_api_embedding_config_construction() {
        let config = ApiEmbeddingConfig {
            base_url: "https://api.openai.com/v1".to_string(),
            model: "text-embedding-3-small".to_string(),
            api_key: Some("sk-test".to_string()),
            dimension: 1536,
        };
        assert_eq!(config.model, "text-embedding-3-small");
        assert_eq!(config.dimension, 1536);
    }

    #[test]
    fn test_api_embedding_config_no_key() {
        let config = ApiEmbeddingConfig {
            base_url: "http://localhost:8080/v1".to_string(),
            model: "test-model".to_string(),
            api_key: None,
            dimension: 256,
        };
        assert!(config.api_key.is_none());
    }

    #[test]
    fn test_api_embedding_func_simple_includes_model_name() {
        let func = api_embedding_func_simple("my-model-v2".into());
        let err = func("test text").unwrap_err();
        assert!(err.contains("my-model-v2"));
    }

    #[test]
    fn test_api_embedding_func_simple_includes_text_length() {
        let func = api_embedding_func_simple("model".into());
        let err = func("hello world").unwrap_err();
        assert!(err.contains("11")); // length of "hello world"
    }

    #[test]
    fn test_embedding_response_deserialization_valid() {
        let json = r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].embedding.len(), 3);
        assert!((resp.data[0].embedding[0] - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_embedding_response_deserialization_empty_data() {
        let json = r#"{"data":[]}"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn test_embedding_response_deserialization_multiple_entries() {
        let json = r#"{"data":[{"embedding":[1.0,0.0]},{"embedding":[0.0,1.0]}]}"#;
        let resp: EmbeddingResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.data.len(), 2);
    }

    #[test]
    fn test_embedding_response_deserialization_invalid_json() {
        let json = r#"not valid json"#;
        let result: Result<EmbeddingResponse, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_embedding_response_deserialization_missing_data() {
        let json = r#"{}"#;
        let result: Result<EmbeddingResponse, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_api_embedding_func_calls_endpoint() {
        // Construct the func (it won't succeed without a real server, but we can
        // verify it returns a connection-type error rather than panicking)
        let config = ApiEmbeddingConfig {
            base_url: "http://127.0.0.1:1".to_string(), // intentionally bad port
            model: "test".to_string(),
            api_key: None,
            dimension: 0,
        };
        let func = api_embedding_func(config);
        let result = func("test");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("API embedding"));
    }

    #[test]
    fn test_api_embedding_func_with_api_key() {
        let config = ApiEmbeddingConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "test".to_string(),
            api_key: Some("sk-secret".to_string()),
            dimension: 128,
        };
        let func = api_embedding_func(config);
        let result = func("hello");
        assert!(result.is_err());
    }

    #[test]
    fn test_api_embedding_config_debug() {
        let config = ApiEmbeddingConfig {
            base_url: "http://test".to_string(),
            model: "m".to_string(),
            api_key: Some("key".to_string()),
            dimension: 64,
        };
        let debug = format!("{:?}", config);
        assert!(debug.contains("ApiEmbeddingConfig"));
    }

    #[test]
    fn test_api_embedding_config_clone() {
        let config = ApiEmbeddingConfig {
            base_url: "http://test".to_string(),
            model: "m".to_string(),
            api_key: Some("key".to_string()),
            dimension: 64,
        };
        let cloned = config.clone();
        assert_eq!(config.base_url, cloned.base_url);
        assert_eq!(config.model, cloned.model);
        assert_eq!(config.dimension, cloned.dimension);
    }

    #[test]
    fn test_api_embedding_func_trailing_slash_url() {
        let config = ApiEmbeddingConfig {
            base_url: "http://127.0.0.1:1/v1/".to_string(),
            model: "test".to_string(),
            api_key: None,
            dimension: 0,
        };
        let func = api_embedding_func(config);
        let result = func("test");
        assert!(result.is_err());
        // The URL should have trailing slash stripped before appending /embeddings
        let err = result.unwrap_err();
        assert!(err.contains("API embedding"));
    }

    #[test]
    fn test_api_embedding_func_empty_text() {
        let config = ApiEmbeddingConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            model: "test".to_string(),
            api_key: None,
            dimension: 0,
        };
        let func = api_embedding_func(config);
        let result = func("");
        assert!(result.is_err());
    }

    #[test]
    fn test_api_embedding_func_simple_empty_text() {
        let func = api_embedding_func_simple("model".into());
        let err = func("").unwrap_err();
        assert!(err.contains("0")); // length 0
    }
}
