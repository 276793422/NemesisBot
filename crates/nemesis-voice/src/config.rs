//! Configuration management

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    pub stt: SttConfig,
    pub vad: VadConfig,
    pub tts: TtsConfig,
    #[serde(default)]
    pub punct: PunctConfig,
    #[serde(default)]
    pub speaker: SpeakerConfig,
    pub audio: AudioConfig,
    pub models: ModelsConfig,
    /// Directory where config.toml resides. Set on load, not deserialized.
    #[serde(skip)]
    pub base_dir: PathBuf,
}

#[derive(Debug, Deserialize, Clone)]
pub struct SttConfig {
    pub model_name: String,
    pub language: String,
    pub use_itn: bool,
    pub num_threads: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct VadConfig {
    pub model_name: String,
    pub threshold: f32,
    pub min_silence_duration: f32,
    pub min_speech_duration: f32,
    pub max_speech_duration: f32,
    pub window_size: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TtsConfig {
    pub model_name: String,
    pub speaker_id: u32,
    pub speed: f32,
    pub num_threads: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct PunctConfig {
    pub model_name: String,
    pub num_threads: u32,
}

fn default_punct_model() -> String { "ct-transformer-zh-en".into() }
fn default_punct_threads() -> u32 { 1 }

impl Default for PunctConfig {
    fn default() -> Self {
        Self {
            model_name: default_punct_model(),
            num_threads: default_punct_threads(),
        }
    }
}

fn default_speaker_model() -> String { "3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced".into() }

#[derive(Debug, Deserialize, Clone, Default)]
pub struct SpeakerConfig {
    #[serde(default = "default_speaker_model")]
    pub model_name: String,
    #[serde(default)]
    pub num_threads: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct AudioConfig {
    pub capture_device: String,
    pub playback_device: String,
    pub target_sample_rate: u32,
    #[serde(default = "default_gain")]
    pub gain: f32,
    #[serde(default = "default_energy_threshold")]
    pub energy_threshold: f32,
}

fn default_gain() -> f32 { 3.0 }
fn default_energy_threshold() -> f32 { 0.015 }

#[derive(Debug, Deserialize, Clone)]
pub struct ModelsConfig {
    pub dir: String,
    pub auto_download: bool,
    pub mirror: MirrorConfig,
    #[serde(default)]
    pub sources: Vec<ModelSource>,
    #[serde(default)]
    pub proxy: ProxyConfig,
}

#[derive(Debug, Deserialize, Clone)]
pub struct MirrorConfig {
    pub base: String,
}

#[derive(Debug, Deserialize, Clone, Default)]
pub struct ProxyConfig {
    #[serde(default)]
    pub url: String,
}

impl ProxyConfig {
    pub fn is_set(&self) -> bool {
        !self.url.trim().is_empty()
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModelSource {
    pub name: String,
    pub category: String,
    pub repo: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ModelFile {
    pub local: String,
    #[serde(default)]
    pub remote: String,
    /// Direct download URL (bypasses mirror+repo URL construction)
    #[serde(default)]
    pub url: String,
}

impl AppConfig {
    pub fn load(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("Failed to read config: {}", path.display()))?;
        let mut config: AppConfig = toml::from_str(&content)
            .with_context(|| "Failed to parse config.toml")?;
        config.base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> Self {
        let base_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        Self::load(path).unwrap_or_else(|e| {
            tracing::warn!("Warning: {}. Using defaults.", e);
            let mut cfg = Self::default();
            cfg.base_dir = base_dir;
            cfg
        })
    }

    /// Base model directory, resolved relative to config file location.
    pub fn model_dir(&self) -> PathBuf {
        let dir = PathBuf::from(&self.models.dir);
        if dir.is_absolute() {
            dir
        } else {
            self.base_dir.join(dir)
        }
    }

    /// Find model source config by name
    pub fn find_model_source(&self, name: &str) -> Option<&ModelSource> {
        self.models.sources.iter().find(|s| s.name == name)
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            stt: SttConfig {
                model_name: "sensevoice-small".into(),
                language: "zh".into(),
                use_itn: false,
                num_threads: 1,
            },
            vad: VadConfig {
                model_name: "silero_vad".into(),
                threshold: 0.5,
                min_silence_duration: 0.3,
                min_speech_duration: 0.25,
                max_speech_duration: 30.0,
                window_size: 512,
            },
            tts: TtsConfig {
                // v1_0：v1_1 在当前 sherpa-onnx 正式版念英文会崩；v1_0 中英文都支持。
                model_name: "kokoro-multi-lang-v1_0".into(),
                speaker_id: 45,
                speed: 1.0,
                num_threads: 4,
            },
            punct: PunctConfig::default(),
            speaker: SpeakerConfig::default(),
            audio: AudioConfig {
                capture_device: String::new(),
                playback_device: String::new(),
                target_sample_rate: 16000,
                gain: 3.0,
                energy_threshold: 0.015,
            },
            models: ModelsConfig {
                dir: "./data".into(),
                auto_download: true,
                mirror: MirrorConfig {
                    base: "https://hf-mirror.com".into(),
                },
                sources: vec![],
                proxy: ProxyConfig::default(),
            },
            base_dir: PathBuf::from("."),
        }
    }
}

#[cfg(test)]
mod tests;
