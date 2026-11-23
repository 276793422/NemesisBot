//! Voice transcription with Groq Whisper API, model management.

use anyhow::Result;

use serde::Deserialize;
use std::path::Path;

/// Audio format.
#[derive(Debug, Clone)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Ogg,
    Webm,
    Flac,
}

/// Transcription response from the API.
#[derive(Debug, Clone, Deserialize)]
pub struct TranscriptionResponse {
    pub text: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
}

/// Voice transcriber using Groq Whisper API.
pub struct Transcriber {
    api_url: String,
    api_key: Option<String>,
    model: String,
    http_client: reqwest::Client,
}

impl Transcriber {
    /// Create a new transcriber with the Groq API.
    ///
    /// 构建失败 fail-closed（BUG-20261123-028 横向扫）：不再
    /// `unwrap_or_default()` 静默回落默认 client——那会同时丢掉 60s
    /// 超时与凭据随重定向防线（本请求带 `Authorization: Bearer` +
    /// multipart 音频正文，reqwest 默认策略对跨主机 307/308 保方法转发
    /// body——用户语音内容会流向重定向目标主机）。构建失败不是纯理论
    /// 形态（rustls CryptoProvider 歧义，BUG-20261114-037/040 同族实录）。
    pub fn new(api_key: &str) -> Result<Self, String> {
        Self::with_url("https://api.groq.com/openai/v1", Self::opt_key(api_key))
    }

    /// Create with custom API URL.
    ///
    /// 凭据随重定向泄漏防线（单一真相源 nemesis_utils::http_redirect
    /// ——同源跟随/跨源终停）。构建失败语义同 [`Self::new`]：fail-closed
    /// 诚实报错（BUG-20261123-028 横向扫）。
    pub fn with_url(api_url: &str, api_key: Option<&str>) -> Result<Self, String> {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
            .build()
            .map_err(|e| {
                format!(
                    "transcriber HTTP client build failed (refusing default client \
                     without timeout + cross-host redirect guard): {e}"
                )
            })?;
        Ok(Self {
            api_url: api_url.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: "whisper-large-v3".to_string(),
            http_client,
        })
    }

    fn opt_key(api_key: &str) -> Option<&str> {
        if api_key.is_empty() {
            None
        } else {
            Some(api_key)
        }
    }

    /// Check if the transcriber is available (has API key).
    pub fn is_available(&self) -> bool {
        self.api_key.is_some()
    }

    /// Get the model name.
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Set the model name.
    pub fn set_model(&mut self, model: &str) {
        self.model = model.to_string();
    }

    /// Transcribe an audio file.
    /// Uses multipart/form-data to match the Go implementation and the Groq Whisper API spec.
    pub async fn transcribe_file(&self, file_path: &Path) -> Result<TranscriptionResponse> {
        if !file_path.exists() {
            return Err(anyhow::anyhow!(
                "audio file not found: {}",
                file_path.display()
            ));
        }

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no API key configured"))?;

        let file_data = std::fs::read(file_path)?;
        let file_name = file_path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "audio.wav".to_string());

        // Use multipart/form-data — matching Go's multipart.NewWriter approach
        let file_part = reqwest::multipart::Part::bytes(file_data)
            .file_name(file_name)
            .mime_str("application/octet-stream")?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            .text("response_format", "json".to_string());

        let url = format!("{}/audio/transcriptions", self.api_url);
        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = nemesis_utils::http_cap::read_error_body(response).await;
            return Err(anyhow::anyhow!("API error (status {}): {}", status, body));
        }

        let result: TranscriptionResponse = response.json().await?;
        Ok(result)
    }

    /// Transcribe audio bytes.
    /// Uses multipart/form-data to match the Go implementation and the Groq Whisper API spec.
    pub async fn transcribe_bytes(
        &self,
        data: &[u8],
        format: AudioFormat,
    ) -> Result<TranscriptionResponse> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("empty audio data"));
        }

        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no API key configured"))?;

        let (ext, mime) = match format {
            AudioFormat::Wav => ("wav", "audio/wav"),
            AudioFormat::Mp3 => ("mp3", "audio/mpeg"),
            AudioFormat::Ogg => ("ogg", "audio/ogg"),
            AudioFormat::Webm => ("webm", "audio/webm"),
            AudioFormat::Flac => ("flac", "audio/flac"),
        };

        let file_name = format!("audio.{}", ext);

        // Use multipart/form-data — matching Go's multipart.NewWriter approach
        let file_part = reqwest::multipart::Part::bytes(data.to_vec())
            .file_name(file_name)
            .mime_str(mime)?;

        let form = reqwest::multipart::Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            .text("response_format", "json".to_string());

        let url = format!("{}/audio/transcriptions", self.api_url);
        let response = self
            .http_client
            .post(&url)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = nemesis_utils::http_cap::read_error_body(response).await;
            return Err(anyhow::anyhow!("API error (status {}): {}", status, body));
        }

        let result: TranscriptionResponse = response.json().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
