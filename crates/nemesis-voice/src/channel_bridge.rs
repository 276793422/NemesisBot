//! Channel bridge — connects local STT engine to VoiceTranscriber trait
//!
//! Provides LocalVoiceTranscriber that uses sherpa-onnx STT + punctuation
//! restoration for transcription, implementing the VoiceTranscriber trait
//! from nemesis-channels for injection into channel instances.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::config::AppConfig;
use crate::{PunctEngine, SttEngine, model, sherpa};

/// A local voice transcriber that uses sherpa-onnx STT engine.
/// Implements VoiceTranscriber trait for injection into channels.
pub struct LocalVoiceTranscriber {
    stt_engine: Arc<SttEngine>,
    punct_engine: Option<Arc<PunctEngine>>,
    sample_rate: u32,
}

impl LocalVoiceTranscriber {
    pub fn new(voice_dir: &Path) -> Result<Self> {
        let config_path = voice_dir.join("config.toml");
        let cfg = AppConfig::load_or_default(&config_path);

        let stt_dir = model::ensure_stt_model(&cfg)?;
        let stt_engine = SttEngine::new(
            &stt_dir,
            &cfg.stt.language,
            cfg.stt.use_itn,
            cfg.stt.num_threads,
        )?;

        let punct_engine = match model::ensure_punct_model(&cfg) {
            Ok(dir) => {
                let model_path = dir.join("model.onnx");
                match PunctEngine::new(&model_path, cfg.punct.num_threads) {
                    Ok(engine) => Some(Arc::new(engine)),
                    Err(_) => None,
                }
            }
            Err(_) => None,
        };

        Ok(Self {
            stt_engine: Arc::new(stt_engine),
            punct_engine,
            sample_rate: 16000,
        })
    }
}

impl nemesis_channels::base::VoiceTranscriber for LocalVoiceTranscriber {
    fn is_available(&self) -> bool {
        true
    }

    fn transcribe(
        &self,
        file_path: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = std::result::Result<String, String>> + Send + '_>> {
        let stt = self.stt_engine.clone();
        let punct = self.punct_engine.clone();
        let sr = self.sample_rate;
        let path = file_path.to_string();

        Box::pin(async move {
            let path_c = sherpa::to_cstr(&path);
            let wave = unsafe { sherpa::SherpaOnnxReadWave(path_c.as_ptr()) };
            if wave.is_null() {
                return Err(format!("Failed to read WAV file: {}", path));
            }

            let wave_ref = unsafe { &*wave };
            let n = wave_ref.num_samples as usize;
            let file_sr = wave_ref.sample_rate as u32;

            let samples = if !wave_ref.samples.is_null() && n > 0 {
                unsafe { std::slice::from_raw_parts(wave_ref.samples, n) }.to_vec()
            } else {
                unsafe { sherpa::SherpaOnnxFreeWave(wave) };
                return Err("WAV file contains no samples".to_string());
            };

            unsafe { sherpa::SherpaOnnxFreeWave(wave) };

            let use_sr = if file_sr > 0 { file_sr } else { sr };

            let text = stt.recognize(&samples, use_sr)
                .map_err(|e| e.to_string())?;

            if text.is_empty() {
                return Ok(String::new());
            }

            let result = match punct {
                Some(p) => p.add_punctuation(&text).unwrap_or(text),
                None => text,
            };

            Ok(result)
        })
    }
}
