//! STT safe wrapper — SenseVoice via sherpa-onnx offline recognizer

use anyhow::Result;
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::sherpa;

pub struct SttEngine {
    recognizer: *const sherpa::SherpaOnnxOfflineRecognizer,
}

unsafe impl Send for SttEngine {}
unsafe impl Sync for SttEngine {}

impl SttEngine {
    pub fn new(model_dir: &Path, language: &str, use_itn: bool, num_threads: u32) -> Result<Self> {
        let model_path = model_dir.join("model_sherpa.onnx");
        let model_path = if model_path.exists() {
            model_path
        } else {
            model_dir.join("model.onnx")
        };

        let tokens_path = model_dir.join("tokens.txt");

        if !model_path.exists() {
            anyhow::bail!("STT model not found: {}", model_path.display());
        }
        if !tokens_path.exists() {
            anyhow::bail!("STT tokens not found: {}", tokens_path.display());
        }

        let model_c = CString::new(model_path.to_str().unwrap_or("")).unwrap();
        let tokens_c = CString::new(tokens_path.to_str().unwrap_or("")).unwrap();
        let lang_c = CString::new(language).unwrap();
        let provider_c = CString::new("cpu").unwrap();
        let empty_c = CString::new("").unwrap();

        macro_rules! e { () => { empty_c.as_ptr() } }

        let sense_voice = sherpa::SherpaOnnxOfflineSenseVoiceModelConfig {
            model: model_c.as_ptr(),
            language: lang_c.as_ptr(),
            use_itn: if use_itn { 1 } else { 0 },
        };

        // v1.13.2: OfflineModelConfig — all fields including new model types
        let model_config = sherpa::SherpaOnnxOfflineModelConfig {
            transducer: sherpa::SherpaOnnxOfflineTransducerModelConfig {
                encoder: e!(), decoder: e!(), joiner: e!(),
            },
            paraformer: sherpa::SherpaOnnxOfflineParaformerModelConfig { model: e!() },
            nemo_ctc: sherpa::SherpaOnnxOfflineNemoEncDecCtcModelConfig { model: e!() },
            whisper: sherpa::SherpaOnnxOfflineWhisperModelConfig {
                encoder: e!(), decoder: e!(), language: e!(), task: e!(), tail_paddings: 0,
                enable_token_timestamps: 0, enable_segment_timestamps: 0,
            },
            tdnn: sherpa::SherpaOnnxOfflineTdnnModelConfig { model: e!() },
            tokens: tokens_c.as_ptr(),
            num_threads: num_threads as libc::c_int,
            debug: 0,
            provider: provider_c.as_ptr(),
            model_type: e!(),
            modeling_unit: e!(),
            bpe_vocab: e!(),
            telespeech_ctc: e!(),
            sense_voice,
            moonshine: sherpa::SherpaOnnxOfflineMoonshineModelConfig {
                preprocessor: e!(), encoder: e!(), uncached_decoder: e!(),
                cached_decoder: e!(), merged_decoder: e!(),
            },
            fire_red_asr: sherpa::SherpaOnnxOfflineFireRedAsrModelConfig {
                encoder: e!(), decoder: e!(),
            },
            dolphin: sherpa::SherpaOnnxOfflineDolphinModelConfig { model: e!() },
            zipformer_ctc: sherpa::SherpaOnnxOfflineZipformerCtcModelConfig { model: e!() },
            canary: sherpa::SherpaOnnxOfflineCanaryModelConfig {
                encoder: e!(), decoder: e!(), src_lang: e!(), tgt_lang: e!(), use_pnc: 0,
            },
            wenet_ctc: sherpa::SherpaOnnxOfflineWenetCtcModelConfig { model: e!() },
            omnilingual: sherpa::SherpaOnnxOfflineOmnilingualAsrCtcModelConfig { model: e!() },
            medasr: sherpa::SherpaOnnxOfflineMedAsrCtcModelConfig { model: e!() },
            funasr_nano: sherpa::SherpaOnnxOfflineFunASRNanoModelConfig {
                encoder_adaptor: e!(), llm: e!(), embedding: e!(), tokenizer: e!(),
                system_prompt: e!(), user_prompt: e!(), max_new_tokens: 0,
                temperature: 0.0, top_p: 0.0, seed: 0,
                language: e!(), itn: 0, hotwords: e!(),
            },
            fire_red_asr_ctc: sherpa::SherpaOnnxOfflineFireRedAsrCtcModelConfig { model: e!() },
            qwen3_asr: sherpa::SherpaOnnxOfflineQwen3ASRModelConfig {
                conv_frontend: e!(), encoder: e!(), decoder: e!(), tokenizer: e!(),
                max_total_len: 0, max_new_tokens: 0, temperature: 0.0, top_p: 0.0,
                seed: 0, hotwords: e!(),
            },
            cohere_transcribe: sherpa::SherpaOnnxOfflineCohereTranscribeModelConfig {
                encoder: e!(), decoder: e!(), language: e!(), use_punct: 0, use_itn: 0,
            },
        };

        let feat_config = sherpa::SherpaOnnxFeatureConfig {
            sample_rate: 16000,
            feature_dim: 80,
        };

        let lm_config = sherpa::SherpaOnnxOfflineLMConfig {
            model: e!(),
            scale: 0.0,
        };

        let config = sherpa::SherpaOnnxOfflineRecognizerConfig {
            feat_config,
            model_config,
            lm_config,
            decoding_method: e!(),
            max_active_paths: 0,
            hotwords_file: e!(),
            hotwords_score: 0.0,
            rule_fsts: e!(),
            rule_fars: e!(),
            blank_penalty: 0.0,
            hr: sherpa::SherpaOnnxHomophoneReplacerConfig {
                dict_dir: e!(), lexicon: e!(), rule_fsts: e!(),
            },
        };

        let recognizer = unsafe {
            sherpa::SherpaOnnxCreateOfflineRecognizer(&config)
        };

        if recognizer.is_null() {
            anyhow::bail!("Failed to create STT recognizer");
        }

        Ok(Self { recognizer })
    }

    pub fn recognize(&self, samples: &[f32], sample_rate: u32) -> Result<String> {
        let stream = unsafe { sherpa::SherpaOnnxCreateOfflineStream(self.recognizer) };
        if stream.is_null() {
            anyhow::bail!("Failed to create STT stream");
        }

        unsafe {
            sherpa::SherpaOnnxAcceptWaveformOffline(
                stream,
                sample_rate as libc::c_int,
                samples.as_ptr(),
                samples.len() as libc::c_int,
            );
            sherpa::SherpaOnnxDecodeOfflineStream(self.recognizer, stream);
        }

        let result_ptr = unsafe { sherpa::SherpaOnnxGetOfflineStreamResult(stream) };
        let mut text = String::new();

        if !result_ptr.is_null() {
            let result = unsafe { &*result_ptr };
            if !result.text.is_null() {
                if let Ok(s) = unsafe { CStr::from_ptr(result.text) }.to_str() {
                    text = s.to_string();
                }
            }
            unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizerResult(result_ptr) };
        }

        unsafe { sherpa::SherpaOnnxDestroyOfflineStream(stream) };

        Ok(text)
    }
}

impl Drop for SttEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizer(self.recognizer) };
    }
}
