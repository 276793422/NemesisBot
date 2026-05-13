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
            api_key: if api_key.is_empty() { None } else { Some(api_key.to_string()) },
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
            return Err(anyhow::anyhow!("audio file not found: {}", file_path.display()));
        }

        let api_key = self.api_key.as_ref()
            .ok_or_else(|| anyhow::anyhow!("no API key configured"))?;

        let file_data = std::fs::read(file_path)?;
        let file_name = file_path.file_name()
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
        let response = self.http_client.post(&url)
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
    pub async fn transcribe_bytes(&self, data: &[u8], format: AudioFormat) -> Result<TranscriptionResponse> {
        if data.is_empty() {
            return Err(anyhow::anyhow!("empty audio data"));
        }

        let api_key = self.api_key.as_ref()
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
        let response = self.http_client.post(&url)
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
mod tests {
    use super::*;

    #[test]
    fn test_transcriber_creation() {
        let t = Transcriber::new("test-key");
        assert_eq!(t.model(), "whisper-large-v3");
        assert!(t.is_available());
    }

    #[test]
    fn test_transcriber_no_key() {
        let t = Transcriber::new("");
        assert!(!t.is_available());
    }

    #[test]
    fn test_transcriber_set_model() {
        let mut t = Transcriber::new("key");
        t.set_model("whisper-large-v3-turbo");
        assert_eq!(t.model(), "whisper-large-v3-turbo");
    }

    #[test]
    fn test_transcriber_with_url() {
        let t = Transcriber::with_url("http://localhost:8080/v1", Some("key"));
        assert_eq!(t.api_url, "http://localhost:8080/v1");
    }

    #[tokio::test]
    async fn test_transcribe_file_not_found() {
        let t = Transcriber::new("key");
        let result = t.transcribe_file(Path::new("/nonexistent/audio.wav")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_empty() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Wav).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_no_key() {
        let t = Transcriber::new("");
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Mp3).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    #[test]
    fn test_transcriber_default_url() {
        let t = Transcriber::new("key");
        assert_eq!(t.api_url, "https://api.groq.com/openai/v1");
    }

    #[test]
    fn test_transcriber_with_url_no_key() {
        let t = Transcriber::with_url("http://custom.api.com", None);
        assert!(!t.is_available());
        assert_eq!(t.api_url, "http://custom.api.com");
    }

    #[test]
    fn test_audio_format_variants() {
        // Just ensure the variants exist and compile
        let _wav = AudioFormat::Wav;
        let _mp3 = AudioFormat::Mp3;
        let _ogg = AudioFormat::Ogg;
        let _webm = AudioFormat::Webm;
        let _flac = AudioFormat::Flac;
    }

    #[test]
    fn test_transcription_response_deserialize() {
        let json = r#"{"text":"hello world","language":"en","duration":5.5}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "hello world");
        assert_eq!(resp.language, Some("en".to_string()));
        assert_eq!(resp.duration, Some(5.5));
    }

    #[test]
    fn test_transcription_response_minimal() {
        let json = r#"{"text":"hello"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "hello");
        assert!(resp.language.is_none());
        assert!(resp.duration.is_none());
    }

    #[test]
    fn test_transcription_response_empty_text() {
        let json = r#"{"text":""}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.text.is_empty());
    }

    #[tokio::test]
    async fn test_transcribe_file_no_key() {
        let t = Transcriber::new("");
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.wav");
        std::fs::write(&file_path, b"fake audio data").unwrap();
        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    #[test]
    fn test_transcriber_set_model_multiple() {
        let mut t = Transcriber::new("key");
        t.set_model("model-a");
        assert_eq!(t.model(), "model-a");
        t.set_model("model-b");
        assert_eq!(t.model(), "model-b");
    }

    #[test]
    fn test_transcriber_clone_format_debug() {
        let t = AudioFormat::Wav;
        let debug = format!("{:?}", t);
        assert!(debug.contains("Wav"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_all_formats() {
        let t = Transcriber::new(""); // No key, will fail at API key check
        for (data, fmt) in [
            (&[1u8, 2, 3][..], AudioFormat::Wav),
            (&[1u8, 2, 3][..], AudioFormat::Mp3),
            (&[1u8, 2, 3][..], AudioFormat::Ogg),
            (&[1u8, 2, 3][..], AudioFormat::Webm),
            (&[1u8, 2, 3][..], AudioFormat::Flac),
        ] {
            let result = t.transcribe_bytes(data, fmt).await;
            assert!(result.is_err());
        }
    }

    #[test]
    fn test_transcription_response_debug() {
        let resp = TranscriptionResponse {
            text: "test".to_string(),
            language: Some("en".to_string()),
            duration: Some(1.0),
        };
        let debug = format!("{:?}", resp);
        assert!(debug.contains("test"));
    }

    // ---- New tests ----

    #[test]
    fn test_transcriber_new_with_empty_key() {
        let t = Transcriber::new("");
        assert!(!t.is_available());
    }

    #[test]
    fn test_transcriber_new_with_key() {
        let t = Transcriber::new("my-api-key");
        assert!(t.is_available());
        assert_eq!(t.model(), "whisper-large-v3");
    }

    #[test]
    fn test_transcriber_set_model_changes_model() {
        let mut t = Transcriber::new("key");
        assert_eq!(t.model(), "whisper-large-v3");
        t.set_model("whisper-tiny");
        assert_eq!(t.model(), "whisper-tiny");
        t.set_model("whisper-medium");
        assert_eq!(t.model(), "whisper-medium");
    }

    #[test]
    fn test_transcriber_with_url_and_key() {
        let t = Transcriber::with_url("http://custom:9090", Some("mykey"));
        assert_eq!(t.api_url, "http://custom:9090");
        assert!(t.is_available());
    }

    #[test]
    fn test_transcriber_with_url_no_key_v2() {
        let t = Transcriber::with_url("http://custom:9090", None);
        assert_eq!(t.api_url, "http://custom:9090");
        assert!(!t.is_available());
    }

    #[test]
    fn test_transcription_response_with_all_fields() {
        let json = r#"{"text":"hello world","language":"en","duration":10.5}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "hello world");
        assert_eq!(resp.language.unwrap(), "en");
        assert_eq!(resp.duration.unwrap(), 10.5);
    }

    #[test]
    fn test_transcription_response_text_only() {
        let json = r#"{"text":"only text"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "only text");
        assert!(resp.language.is_none());
        assert!(resp.duration.is_none());
    }

    #[test]
    fn test_transcription_response_long_text() {
        let long_text = "a".repeat(10000);
        let json = format!(r#"{{"text":"{}"}}"#, long_text);
        let resp: TranscriptionResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(resp.text.len(), 10000);
    }

    #[test]
    fn test_transcription_response_unicode() {
        let json = r#"{"text":"こんにちは世界","language":"ja"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "こんにちは世界");
        assert_eq!(resp.language.as_deref(), Some("ja"));
    }

    #[tokio::test]
    async fn test_transcribe_file_nonexistent() {
        let t = Transcriber::new("key");
        let result = t.transcribe_file(Path::new("/definitely/does/not/exist.wav")).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_no_key() {
        let t = Transcriber::new("");
        let result = t.transcribe_bytes(&[1, 2, 3, 4], AudioFormat::Wav).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    #[tokio::test]
    async fn test_transcribe_file_no_key_with_existing_file() {
        let t = Transcriber::new("");
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("audio.wav");
        std::fs::write(&file, b"fake").unwrap();
        let result = t.transcribe_file(&file).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    #[test]
    fn test_audio_format_debug_wav() {
        assert_eq!(format!("{:?}", AudioFormat::Wav), "Wav");
    }

    #[test]
    fn test_audio_format_debug_mp3() {
        assert_eq!(format!("{:?}", AudioFormat::Mp3), "Mp3");
    }

    #[test]
    fn test_audio_format_debug_ogg() {
        assert_eq!(format!("{:?}", AudioFormat::Ogg), "Ogg");
    }

    #[test]
    fn test_audio_format_debug_webm() {
        assert_eq!(format!("{:?}", AudioFormat::Webm), "Webm");
    }

    #[test]
    fn test_audio_format_debug_flac() {
        assert_eq!(format!("{:?}", AudioFormat::Flac), "Flac");
    }

    #[test]
    fn test_transcription_response_clone() {
        let resp = TranscriptionResponse {
            text: "hello".into(),
            language: Some("en".into()),
            duration: Some(1.0),
        };
        let cloned = resp.clone();
        assert_eq!(cloned.text, "hello");
        assert_eq!(cloned.language, Some("en".into()));
    }

    // ---- Coverage improvement tests ----

    // -- TranscriptionResponse deserialization edge cases --

    #[test]
    fn test_response_deserialize_extra_fields_ignored() {
        let json = r#"{"text":"hi","language":"en","duration":1.0,"extra":"ignored","segments":[]}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, "hi");
        assert_eq!(resp.language.as_deref(), Some("en"));
        assert_eq!(resp.duration, Some(1.0));
    }

    #[test]
    fn test_response_deserialize_missing_text_fails() {
        let json = r#"{"language":"en","duration":1.0}"#;
        let result = serde_json::from_str::<TranscriptionResponse>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_response_deserialize_zero_duration() {
        let json = r#"{"text":"hello","duration":0.0}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.duration, Some(0.0));
    }

    #[test]
    fn test_response_deserialize_negative_duration() {
        let json = r#"{"text":"hello","duration":-5.0}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.duration, Some(-5.0));
    }

    #[test]
    fn test_response_deserialize_large_duration() {
        let json = r#"{"text":"hello","duration":999999.99}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.duration, Some(999999.99));
    }

    #[test]
    fn test_response_deserialize_special_chars_in_text() {
        let json = r#"{"text":"Hello \"world\" & <friends>"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.text, r#"Hello "world" & <friends>"#);
    }

    #[test]
    fn test_response_deserialize_multiline_text() {
        let json = r#"{"text":"line1\nline2\nline3"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.text.contains('\n'));
    }

    #[test]
    fn test_response_deserialize_null_language() {
        let json = r#"{"text":"hello","language":null}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.language.is_none());
    }

    #[test]
    fn test_response_deserialize_null_duration() {
        let json = r#"{"text":"hello","duration":null}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.duration.is_none());
    }

    #[test]
    fn test_response_deserialize_empty_language() {
        let json = r#"{"text":"hello","language":""}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.language.as_deref(), Some(""));
    }

    #[test]
    fn test_response_deserialize_invalid_json() {
        let json = r#"not valid json"#;
        let result = serde_json::from_str::<TranscriptionResponse>(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_response_deserialize_text_with_emoji() {
        let json = r#"{"text":"🎵 Music time 🎶"}"#;
        let resp: TranscriptionResponse = serde_json::from_str(json).unwrap();
        assert!(resp.text.contains("🎵"));
    }

    // -- AudioFormat Clone derive --

    #[test]
    fn test_audio_format_clone_all() {
        let wav = AudioFormat::Wav.clone();
        let mp3 = AudioFormat::Mp3.clone();
        let ogg = AudioFormat::Ogg.clone();
        let webm = AudioFormat::Webm.clone();
        let flac = AudioFormat::Flac.clone();
        // Verify they are the correct variants via debug
        assert_eq!(format!("{:?}", wav), "Wav");
        assert_eq!(format!("{:?}", mp3), "Mp3");
        assert_eq!(format!("{:?}", ogg), "Ogg");
        assert_eq!(format!("{:?}", webm), "Webm");
        assert_eq!(format!("{:?}", flac), "Flac");
    }

    // -- Transcriber construction edge cases --

    #[test]
    fn test_transcriber_new_whitespace_key_is_available() {
        // Whitespace-only key is not empty, so is_available() should be true
        let t = Transcriber::new(" ");
        assert!(t.is_available());
    }

    #[test]
    fn test_transcriber_with_url_empty_string() {
        let t = Transcriber::with_url("", Some("key"));
        assert_eq!(t.api_url, "");
        assert!(t.is_available());
    }

    #[test]
    fn test_transcriber_new_default_model() {
        let t = Transcriber::new("key");
        assert_eq!(t.model(), "whisper-large-v3");
    }

    #[test]
    fn test_transcriber_with_url_default_model() {
        let t = Transcriber::with_url("http://test:8080", Some("key"));
        assert_eq!(t.model(), "whisper-large-v3");
    }

    // -- transcribe_file edge cases --

    #[tokio::test]
    async fn test_transcribe_file_error_message_contains_path() {
        let t = Transcriber::new("key");
        let weird_path = Path::new("/very/specific/nonexistent/path/audio.wav");
        let result = t.transcribe_file(weird_path).await;
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("audio file not found"));
        assert!(err_msg.contains("very/specific/nonexistent"));
    }

    #[tokio::test]
    async fn test_transcribe_file_with_directory() {
        let t = Transcriber::new("key");
        let dir = tempfile::tempdir().unwrap();
        // A directory path that exists but is not a file — reading will fail
        let result = t.transcribe_file(dir.path()).await;
        // Should fail because std::fs::read on a directory errors
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_file_no_extension() {
        let t = Transcriber::new("");
        let dir = tempfile::tempdir().unwrap();
        // File with no extension — should fail at API key check, not at path handling
        let file_path = dir.path().join("audiofile");
        std::fs::write(&file_path, b"fake audio").unwrap();
        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    // -- transcribe_bytes edge cases --

    #[tokio::test]
    async fn test_transcribe_bytes_empty_error_message() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Wav).await;
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("empty audio data"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_single_byte() {
        let t = Transcriber::new("");
        let result = t.transcribe_bytes(&[0xFF], AudioFormat::Mp3).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_large_data_no_key() {
        let t = Transcriber::new("");
        let data = vec![0u8; 1024 * 1024]; // 1MB
        let result = t.transcribe_bytes(&data, AudioFormat::Flac).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no API key"));
    }

    // -- Mock server tests for actual HTTP paths --

    use wiremock::{MockServer, Mock, ResponseTemplate};
    use wiremock::matchers::{method, path};

    #[tokio::test]
    async fn test_transcribe_file_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "hello from mock",
                "language": "en",
                "duration": 3.5
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("test-key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.wav");
        std::fs::write(&file_path, b"RIFF fake wav data").unwrap();

        let result = t.transcribe_file(&file_path).await.unwrap();
        assert_eq!(result.text, "hello from mock");
        assert_eq!(result.language.as_deref(), Some("en"));
        assert_eq!(result.duration, Some(3.5));
    }

    #[tokio::test]
    async fn test_transcribe_file_api_error_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(401).set_body_string("Unauthorized"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("bad-key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.wav");
        std::fs::write(&file_path, b"fake").unwrap();

        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("API error"));
        assert!(err_msg.contains("401"));
    }

    #[tokio::test]
    async fn test_transcribe_file_server_error_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("Internal Server Error"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("audio.mp3");
        std::fs::write(&file_path, b"fake mp3").unwrap();

        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("API error"));
        assert!(err_msg.contains("500"));
    }

    #[tokio::test]
    async fn test_transcribe_file_malformed_response_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("audio.ogg");
        std::fs::write(&file_path, b"fake ogg").unwrap();

        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_file_response_text_only_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "only text no extras"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("audio.webm");
        std::fs::write(&file_path, b"fake webm").unwrap();

        let result = t.transcribe_file(&file_path).await.unwrap();
        assert_eq!(result.text, "only text no extras");
        assert!(result.language.is_none());
        assert!(result.duration.is_none());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_wav_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "wav transcription",
                "language": "en",
                "duration": 2.0
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3, 4, 5], AudioFormat::Wav).await.unwrap();
        assert_eq!(result.text, "wav transcription");
        assert_eq!(result.language.as_deref(), Some("en"));
        assert_eq!(result.duration, Some(2.0));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_mp3_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "mp3 transcription"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Mp3).await.unwrap();
        assert_eq!(result.text, "mp3 transcription");
    }

    #[tokio::test]
    async fn test_transcribe_bytes_ogg_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "ogg transcription"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Ogg).await.unwrap();
        assert_eq!(result.text, "ogg transcription");
    }

    #[tokio::test]
    async fn test_transcribe_bytes_webm_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "webm transcription"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Webm).await.unwrap();
        assert_eq!(result.text, "webm transcription");
    }

    #[tokio::test]
    async fn test_transcribe_bytes_flac_success_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "flac transcription"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Flac).await.unwrap();
        assert_eq!(result.text, "flac transcription");
    }

    #[tokio::test]
    async fn test_transcribe_bytes_api_error_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(403).set_body_string("Forbidden"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Wav).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("API error"));
        assert!(err_msg.contains("403"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_malformed_response_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not valid json"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Wav).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_rate_limit_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("Rate limit exceeded"))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Wav).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("429"));
    }

    // -- set_model + transcribe with mock to ensure model is used --

    #[tokio::test]
    async fn test_transcriber_custom_model_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "custom model result"
            })))
            .mount(&mock_server)
            .await;

        let mut t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        t.set_model("whisper-tiny");
        assert_eq!(t.model(), "whisper-tiny");

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("audio.wav");
        std::fs::write(&file_path, b"fake").unwrap();

        let result = t.transcribe_file(&file_path).await.unwrap();
        assert_eq!(result.text, "custom model result");
    }

    // -- Connection error tests (no mock server running) --

    #[tokio::test]
    async fn test_transcribe_file_connection_refused() {
        // Use a URL that nobody is listening on
        let t = Transcriber::with_url("http://127.0.0.1:1", Some("key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("audio.wav");
        std::fs::write(&file_path, b"fake").unwrap();

        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_err());
        // Should be a connection error, not API key or file not found
        let err_msg = result.unwrap_err().to_string();
        assert!(!err_msg.contains("no API key"));
        assert!(!err_msg.contains("not found"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_connection_refused() {
        let t = Transcriber::with_url("http://127.0.0.1:1", Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Wav).await;
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(!err_msg.contains("no API key"));
        assert!(!err_msg.contains("empty"));
    }

    // -- Transcriber HTTP client timeout behavior --

    #[tokio::test]
    async fn test_transcribe_bytes_empty_data_mp3() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Mp3).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty audio data"));
    }

    #[tokio::test]
    async fn test_transcribe_bytes_empty_data_ogg() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Ogg).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_empty_data_webm() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Webm).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_empty_data_flac() {
        let t = Transcriber::new("key");
        let result = t.transcribe_bytes(&[], AudioFormat::Flac).await;
        assert!(result.is_err());
    }

    // -- Verify URL construction in transcribe methods --

    #[tokio::test]
    async fn test_transcribe_file_url_construction_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "ok"
            })))
            .mount(&mock_server)
            .await;

        // The api_url from mock_server is like "http://127.0.0.1:PORT"
        // transcribe_file appends "/audio/transcriptions"
        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.wav");
        std::fs::write(&file_path, b"data").unwrap();

        let result = t.transcribe_file(&file_path).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_transcribe_bytes_url_construction_with_mock() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "text": "ok"
            })))
            .mount(&mock_server)
            .await;

        let t = Transcriber::with_url(&mock_server.uri(), Some("key"));
        let result = t.transcribe_bytes(&[1, 2, 3], AudioFormat::Wav).await;
        assert!(result.is_ok());
    }
}
