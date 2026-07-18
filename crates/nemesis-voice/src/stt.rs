//! STT safe wrapper — SenseVoice via sherpa-onnx offline recognizer
//!
//! 本模块只管"建识别器 + 解码"。语言补救（auto 误判时用强制语言重解）是独立策略，
//! 见 `crate::lang_restriction`。`SttEngine` 持有 `Option<LangRestriction>`：构造时
//! 问模型要不要补救（`default_remedy_for_model`），要就建 fallback 识别器并交给
//! `LangRestriction`；解码后委托它决定是否重解。换不在声明表里的模型 → `None` → 不补救。

use anyhow::Result;
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::lang_restriction::{self, LangRestriction};
use crate::sherpa;

pub struct SttEngine {
    recognizer: *const sherpa::SherpaOnnxOfflineRecognizer,
    /// 语言补救策略 + fallback 识别器。模型未声明补救、或总开关关闭时为 `None`。
    restriction: Option<LangRestriction>,
}

unsafe impl Send for SttEngine {}
unsafe impl Sync for SttEngine {}

impl SttEngine {
    /// Create an STT engine.
    ///
    /// `lang_remedy`（默认 true）= 是否启用"模型自声明的语言补救"。仅 `language="auto"` 下生效：
    /// 若 `default_remedy_for_model(model_name)` 声明了补救方式，就额外加载一个 fallback
    /// 识别器；auto 检测到不在白名单的语言时，把同一句重解。设 `lang_remedy=false` 可强制
    /// 关闭（回退到纯 auto，不做任何补救）。
    pub fn new(
        model_dir: &Path,
        model_name: &str,
        language: &str,
        lang_remedy: bool,
        use_itn: bool,
        num_threads: u32,
    ) -> Result<Self> {
        let recognizer = Self::build_recognizer(model_dir, language, use_itn, num_threads)?;

        let restriction = if lang_remedy && language == "auto" {
            // 底层判断：模型自声明要不要补救。
            match lang_restriction::default_remedy_for_model(model_name) {
                Some(remedy) => {
                    // 拿到补救方式 → 建 fallback 识别器。
                    match Self::build_recognizer(model_dir, &remedy.fallback, use_itn, num_threads) {
                        Ok(fb) => {
                            tracing::info!(
                                model = model_name,
                                allowed = ?remedy.allowed,
                                fallback = %remedy.fallback,
                                "[STT] 语言补救启用：模型声明需要补救，已加载 {} 回退识别器",
                                remedy.fallback
                            );
                            Some(LangRestriction::new(remedy, fb))
                        }
                        Err(e) => {
                            tracing::warn!(
                                "[STT] 补救回退识别器（{}）构建失败（{}），跳过补救，走纯 auto。",
                                remedy.fallback,
                                e
                            );
                            None
                        }
                    }
                }
                None => {
                    // 模型未声明补救 → 补丁不介入。
                    tracing::debug!(
                        "[STT] 模型 {} 未声明语言补救，auto 结果原样返回。",
                        model_name
                    );
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            recognizer,
            restriction,
        })
    }

    /// 底层 FFI：构建一个 offline recognizer（指定 language）。供主识别器与补救 fallback 复用。
    fn build_recognizer(
        model_dir: &Path,
        language: &str,
        use_itn: bool,
        num_threads: u32,
    ) -> Result<*const sherpa::SherpaOnnxOfflineRecognizer> {
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

        macro_rules! e {
            () => {
                empty_c.as_ptr()
            };
        }

        let sense_voice = sherpa::SherpaOnnxOfflineSenseVoiceModelConfig {
            model: model_c.as_ptr(),
            language: lang_c.as_ptr(),
            use_itn: if use_itn { 1 } else { 0 },
        };

        // v1.13.2: OfflineModelConfig — all fields including new model types
        let model_config = sherpa::SherpaOnnxOfflineModelConfig {
            transducer: sherpa::SherpaOnnxOfflineTransducerModelConfig {
                encoder: e!(),
                decoder: e!(),
                joiner: e!(),
            },
            paraformer: sherpa::SherpaOnnxOfflineParaformerModelConfig { model: e!() },
            nemo_ctc: sherpa::SherpaOnnxOfflineNemoEncDecCtcModelConfig { model: e!() },
            whisper: sherpa::SherpaOnnxOfflineWhisperModelConfig {
                encoder: e!(),
                decoder: e!(),
                language: e!(),
                task: e!(),
                tail_paddings: 0,
                enable_token_timestamps: 0,
                enable_segment_timestamps: 0,
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
                preprocessor: e!(),
                encoder: e!(),
                uncached_decoder: e!(),
                cached_decoder: e!(),
                merged_decoder: e!(),
            },
            fire_red_asr: sherpa::SherpaOnnxOfflineFireRedAsrModelConfig {
                encoder: e!(),
                decoder: e!(),
            },
            dolphin: sherpa::SherpaOnnxOfflineDolphinModelConfig { model: e!() },
            zipformer_ctc: sherpa::SherpaOnnxOfflineZipformerCtcModelConfig { model: e!() },
            canary: sherpa::SherpaOnnxOfflineCanaryModelConfig {
                encoder: e!(),
                decoder: e!(),
                src_lang: e!(),
                tgt_lang: e!(),
                use_pnc: 0,
            },
            wenet_ctc: sherpa::SherpaOnnxOfflineWenetCtcModelConfig { model: e!() },
            omnilingual: sherpa::SherpaOnnxOfflineOmnilingualAsrCtcModelConfig { model: e!() },
            medasr: sherpa::SherpaOnnxOfflineMedAsrCtcModelConfig { model: e!() },
            funasr_nano: sherpa::SherpaOnnxOfflineFunASRNanoModelConfig {
                encoder_adaptor: e!(),
                llm: e!(),
                embedding: e!(),
                tokenizer: e!(),
                system_prompt: e!(),
                user_prompt: e!(),
                max_new_tokens: 0,
                temperature: 0.0,
                top_p: 0.0,
                seed: 0,
                language: e!(),
                itn: 0,
                hotwords: e!(),
            },
            fire_red_asr_ctc: sherpa::SherpaOnnxOfflineFireRedAsrCtcModelConfig { model: e!() },
            qwen3_asr: sherpa::SherpaOnnxOfflineQwen3ASRModelConfig {
                conv_frontend: e!(),
                encoder: e!(),
                decoder: e!(),
                tokenizer: e!(),
                max_total_len: 0,
                max_new_tokens: 0,
                temperature: 0.0,
                top_p: 0.0,
                seed: 0,
                hotwords: e!(),
            },
            cohere_transcribe: sherpa::SherpaOnnxOfflineCohereTranscribeModelConfig {
                encoder: e!(),
                decoder: e!(),
                language: e!(),
                use_punct: 0,
                use_itn: 0,
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
                dict_dir: e!(),
                lexicon: e!(),
                rule_fsts: e!(),
            },
        };

        let recognizer = unsafe { sherpa::SherpaOnnxCreateOfflineRecognizer(&config) };

        if recognizer.is_null() {
            anyhow::bail!("Failed to create STT recognizer");
        }

        Ok(recognizer)
    }

    pub fn recognize(&self, samples: &[f32], sample_rate: u32) -> Result<String> {
        let (text, _lang) = self.recognize_detail(samples, sample_rate)?;
        Ok(text)
    }

    /// Like `recognize` but also returns the model-detected language tag (normalized, e.g. `zh`/`en`).
    ///
    /// 当补救触发重解时，返回的文本是 fallback 引擎结果，`lang` 返回 fallback 语言
    /// （即"返回值始终描述最终文本的语言"）；原始 auto 检测结果记到 debug 日志。
    pub fn recognize_detail(&self, samples: &[f32], sample_rate: u32) -> Result<(String, Option<String>)> {
        let (text, lang) = decode_recognizer(self.recognizer, samples, sample_rate)?;

        if let Some(r) = &self.restriction {
            if let Some((fb_text, fb_lang)) = r.apply(lang.as_deref(), samples, sample_rate)? {
                tracing::debug!(
                    detected = ?lang,
                    fallback = %fb_lang,
                    "STT 语言补救：检测语言不在白名单，已用 {} 重解",
                    fb_lang
                );
                return Ok((fb_text, Some(fb_lang)));
            }
        }

        Ok((text, lang))
    }
}

/// 在指定 recognizer 上跑一次离线解码，返回 (text, normalized_lang)。
/// 主识别器与补救 fallback 识别器共用此函数（pub(crate) 供 `lang_restriction` 调用）。
pub(crate) fn decode_recognizer(
    recognizer: *const sherpa::SherpaOnnxOfflineRecognizer,
    samples: &[f32],
    sample_rate: u32,
) -> Result<(String, Option<String>)> {
    let stream = unsafe { sherpa::SherpaOnnxCreateOfflineStream(recognizer) };
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
        sherpa::SherpaOnnxDecodeOfflineStream(recognizer, stream);
    }

    let result_ptr = unsafe { sherpa::SherpaOnnxGetOfflineStreamResult(stream) };
    let mut text = String::new();
    let mut lang: Option<String> = None;

    if !result_ptr.is_null() {
        let result = unsafe { &*result_ptr };
        if !result.text.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(result.text) }.to_str() {
                text = s.to_string();
            }
        }
        if !result.lang.is_null() {
            if let Ok(s) = unsafe { CStr::from_ptr(result.lang) }.to_str() {
                let normalized = normalize_lang_token(s);
                if !normalized.is_empty() {
                    lang = Some(normalized);
                }
            }
        }
        unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizerResult(result_ptr) };
    }

    unsafe { sherpa::SherpaOnnxDestroyOfflineStream(stream) };

    Ok((text, lang))
}

/// 去掉 SenseVoice 语言 token 的包装：`<|zh|>` → `zh`、`<|en|>` → `en`。
/// 对返回裸语言码的模型是 no-op，模型无关。
fn normalize_lang_token(raw: &str) -> String {
    raw.trim()
        .trim_start_matches("<|")
        .trim_end_matches("|>")
        .to_string()
}

impl Drop for SttEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyOfflineRecognizer(self.recognizer) };
        // self.restriction（Option<LangRestriction>）自动 drop → 销毁 fallback 识别器
    }
}

#[cfg(test)]
mod tests;
