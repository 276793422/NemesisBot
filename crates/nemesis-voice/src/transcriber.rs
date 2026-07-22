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
    pub fn new(api_key: &str) -> Self {
        Self {
            api_url: "https://api.groq.com/openai/v1".to_string(),
            api_key: if api_key.is_empty() {
                None
            } else {
                Some(api_key.to_string())
            },
            model: "whisper-large-v3".to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Create with custom API URL.
    pub fn with_url(api_url: &str, api_key: Option<&str>) -> Self {
        Self {
            api_url: api_url.to_string(),
            api_key: api_key.map(|s| s.to_string()),
            model: "whisper-large-v3".to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap_or_default(),
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
            let body = response.text().await.unwrap_or_default();
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
            let body = response.text().await.unwrap_or_default();
            return Err(anyhow::anyhow!("API error (status {}): {}", status, body));
        }

        let result: TranscriptionResponse = response.json().await?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests;
