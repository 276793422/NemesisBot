//! TTS safe wrapper — supports VITS and Kokoro models via sherpa-onnx
//!
//! Auto-detects model type by checking for `voices.bin` (Kokoro) vs `model.onnx` only (VITS).
//!
//! # sherpa-onnx kokoro TTS C++ 异常问题（2026-05 记录）
//!
//! ## 问题现象
//!
//! sherpa-onnx 的 kokoro TTS 模型在遇到某些输入时会抛出 C++ 异常，
//! 导致 Rust 进程直接 abort（`fatal runtime error: Rust cannot catch
//! foreign exceptions, aborting`），没有任何恢复机会。
//!
//! 触发条件（已确认）：
//! - 连续重复标点：`？？`、`！！`、`。。。`
//! - 相邻的不同标点：`)?`、`。？`
//! - 中文特殊符号：`【】`、`（）`、`《》`、`〈〉`、`「」`
//! - 英文特殊符号：`<>`、`[]`、`{}`
//! - 根本原因：kokoro 内部的 espeak-ng tokenizer / lexicon 无法处理
//!   这些字符，sherpa-onnx 的 C++ 代码直接 throw 而非返回错误
//!
//! 相关 issue：
//! - https://github.com/k2-fsa/sherpa-onnx/issues/2223 (TTS crashes on unknown token)
//! - https://github.com/k2-fsa/sherpa-onnx/issues/2528 (onnxruntime version conflict)
//!
//! ## 三层防护方案
//!
//! 第一层 — 文本白名单（`normalize_tts_text`）：
//!   只保留 CJK 汉字、ASCII 字母数字、已验证安全的中英文标点，
//!   禁止任意两个标点相邻。所有不在白名单的字符直接过滤。
//!   这是最有效的防线，正常情况下到此为止。
//!
//! 第二层 — C++ try/catch shim（`cpp/safe_ffi.cpp`）：
//!   如果第一层遗漏了某个字符，C++ 异常会被 `catch(...)` 捕获，
//!   返回 null 指针和错误码给 Rust，而不是让进程 abort。
//!   Rust 侧（`safe_tts_generate_audio`）将错误码转为 `Err(String)`。
//!
//! 第三层 — 引擎自愈（`recreate_engine`）：
//!   C++ 异常可能导致引擎内部状态损坏，即使捕获了异常，引擎也
//!   不可再用。因此 `generate()` 在收到 C++ 异常错误后，自动
//!   销毁旧引擎、重新创建新引擎，下次调用恢复正常。
//!   `TtsEngine` 保存 `model_dir`、`num_threads`、`is_kokoro`
//!   等参数，使用 `Mutex<TtsInner>` 包裹原始指针以支持重建。
//!
//! ## 如果以后遇到新的崩溃
//!
//! 1. 先确认是文本输入问题还是其他问题（用 `voice tts "文字"` 复现）
//! 2. 如果是新的字符导致，把该字符加入 `is_tts_safe` 的过滤逻辑
//! 3. 如果第二层也没拦住（C++ 异常未被 catch），检查 `cpp/safe_ffi.cpp`
//!    是否正确链接（`cc::Build` 编译）
//! 4. 考虑升级 sherpa-onnx 版本（未来版本可能修复 C++ 异常处理）
//!
//! ## 注意事项
//!
//! - `SherpaOnnxDestroyOfflineTts` 和 `SherpaOnnxDestroyOfflineTtsGeneratedAudio`
//!   没有用 C++ shim 包裹，因为它们只做内存释放，不应该抛异常
//! - 其他 sherpa-onnx 调用（STT、VAD、Punctuation）暂未发现类似问题，
//!   如果将来出现，可以用同样的 C++ shim 方案包裹

use anyhow::Result;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::sherpa;

pub struct TtsEngine {
    inner: Mutex<TtsInner>,
    model_dir: PathBuf,
    num_threads: u32,
    is_kokoro: bool,
    pub sample_rate: u32,
}

struct TtsInner {
    tts: *const sherpa::SherpaOnnxOfflineTts,
}

unsafe impl Send for TtsEngine {}
unsafe impl Sync for TtsEngine {}

impl TtsEngine {
    pub fn new(model_dir: &Path, num_threads: u32) -> Result<Self> {
        let is_kokoro = model_dir.join("voices.bin").exists();
        let sample_rate = if is_kokoro { 24000 } else { 44100 };

        let tts = Self::create_engine(model_dir, num_threads, is_kokoro)?;

        Ok(Self {
            inner: Mutex::new(TtsInner { tts }),
            model_dir: model_dir.to_path_buf(),
            num_threads,
            is_kokoro,
            sample_rate,
        })
    }

    /// Build a fresh TTS engine from stored params.
    fn create_engine(
        model_dir: &Path,
        num_threads: u32,
        is_kokoro: bool,
    ) -> Result<*const sherpa::SherpaOnnxOfflineTts> {
        if is_kokoro {
            Self::build_kokoro_config(model_dir, num_threads)
        } else {
            Self::build_vits_config(model_dir, num_threads)
        }
    }

    fn build_vits_config(
        model_dir: &Path,
        num_threads: u32,
    ) -> Result<*const sherpa::SherpaOnnxOfflineTts> {
        let model_path = model_dir.join("model.onnx");
        let lexicon_path = model_dir.join("lexicon.txt");
        let tokens_path = model_dir.join("tokens.txt");

        if !model_path.exists() {
            anyhow::bail!("TTS model not found: {}", model_path.display());
        }
        if !tokens_path.exists() {
            anyhow::bail!("TTS tokens not found: {}", tokens_path.display());
        }

        let model_c = sherpa::to_cstr(model_dir.join("model.onnx").to_str().unwrap_or(""));
        let tokens_c = sherpa::to_cstr(tokens_path.to_str().unwrap_or(""));
        let provider_c = sherpa::to_cstr("cpu");
        let empty = sherpa::null_cstr();

        let lexicon_c = if lexicon_path.exists() {
            sherpa::to_cstr(lexicon_path.to_str().unwrap_or(""))
        } else {
            return Err(anyhow::anyhow!(
                "TTS lexicon not found: {}",
                lexicon_path.display()
            ));
        };

        let data_dir = model_dir.join("espeak-ng-data");
        let data_dir_c = sherpa::to_cstr(if data_dir.exists() {
            data_dir.to_str().unwrap_or("")
        } else {
            ""
        });

        let dict_dir = model_dir.join("dict");
        let dict_dir_c = sherpa::to_cstr(if dict_dir.exists() {
            dict_dir.to_str().unwrap_or("")
        } else {
            ""
        });

        let vits = sherpa::SherpaOnnxOfflineTtsVitsModelConfig {
            model: model_c.as_ptr(),
            lexicon: lexicon_c.as_ptr(),
            tokens: tokens_c.as_ptr(),
            data_dir: data_dir_c.as_ptr(),
            noise_scale: 0.667,
            noise_scale_w: 0.8,
            length_scale: 1.0,
            dict_dir: dict_dir_c.as_ptr(),
        };

        let model_config = build_model_config(vits, num_threads, provider_c.as_ptr(), empty);
        let config = sherpa::SherpaOnnxOfflineTtsConfig {
            model: model_config,
            rule_fsts: empty,
            max_num_sentences: 0,
            rule_fars: empty,
            silence_scale: 0.0,
        };

        let tts = sherpa::safe_create_offline_tts(&config)
            .map_err(|e| anyhow::anyhow!("Failed to create TTS engine: {}", e))?;

        tracing::info!("[TTS] Using VITS model");
        Ok(tts)
    }

    fn build_kokoro_config(
        model_dir: &Path,
        num_threads: u32,
    ) -> Result<*const sherpa::SherpaOnnxOfflineTts> {
        let model_path = model_dir.join("model.onnx");
        let voices_path = model_dir.join("voices.bin");
        let tokens_path = model_dir.join("tokens.txt");

        if !model_path.exists() {
            anyhow::bail!("Kokoro TTS model not found: {}", model_path.display());
        }
        if !voices_path.exists() {
            anyhow::bail!("Kokoro voices.bin not found: {}", voices_path.display());
        }
        if !tokens_path.exists() {
            anyhow::bail!("Kokoro tokens not found: {}", tokens_path.display());
        }

        let model_c = sherpa::to_cstr(model_path.to_str().unwrap_or(""));
        let voices_c = sherpa::to_cstr(voices_path.to_str().unwrap_or(""));
        let tokens_c = sherpa::to_cstr(tokens_path.to_str().unwrap_or(""));
        let provider_c = sherpa::to_cstr("cpu");
        let empty = sherpa::null_cstr();

        let data_dir = model_dir.join("espeak-ng-data");
        let data_dir_c = sherpa::to_cstr(if data_dir.exists() {
            data_dir.to_str().unwrap_or("")
        } else {
            ""
        });

        let dict_dir = model_dir.join("dict");
        let dict_dir_c = sherpa::to_cstr(if dict_dir.exists() {
            dict_dir.to_str().unwrap_or("")
        } else {
            ""
        });

        let lexicon_en = model_dir.join("lexicon-us-en.txt");
        let lexicon_zh = model_dir.join("lexicon-zh.txt");
        let lexicon_str = if lexicon_en.exists() && lexicon_zh.exists() {
            format!(
                "{},{}",
                lexicon_en.to_str().unwrap_or(""),
                lexicon_zh.to_str().unwrap_or("")
            )
        } else if lexicon_en.exists() {
            lexicon_en.to_str().unwrap_or("").to_string()
        } else if lexicon_zh.exists() {
            lexicon_zh.to_str().unwrap_or("").to_string()
        } else {
            String::new()
        };
        let lexicon_c = sherpa::to_cstr(&lexicon_str);

        let kokoro = sherpa::SherpaOnnxOfflineTtsKokoroModelConfig {
            model: model_c.as_ptr(),
            voices: voices_c.as_ptr(),
            tokens: tokens_c.as_ptr(),
            data_dir: data_dir_c.as_ptr(),
            length_scale: 1.0,
            dict_dir: dict_dir_c.as_ptr(),
            lexicon: lexicon_c.as_ptr(),
            lang: empty,
        };

        let vits = sherpa::SherpaOnnxOfflineTtsVitsModelConfig {
            model: empty,
            lexicon: empty,
            tokens: empty,
            data_dir: empty,
            noise_scale: 0.0,
            noise_scale_w: 0.0,
            length_scale: 1.0,
            dict_dir: empty,
        };
        let model_config =
            build_model_config_with_kokoro(vits, kokoro, num_threads, provider_c.as_ptr(), empty);

        let rule_fsts = {
            let mut fsts = Vec::new();
            for name in &["date-zh.fst", "number-zh.fst", "phone-zh.fst"] {
                let p = model_dir.join(name);
                if p.exists() {
                    fsts.push(p.to_str().unwrap_or("").to_string());
                }
            }
            sherpa::to_cstr(&fsts.join(","))
        };

        let config = sherpa::SherpaOnnxOfflineTtsConfig {
            model: model_config,
            rule_fsts: rule_fsts.as_ptr(),
            max_num_sentences: 0,
            rule_fars: empty,
            silence_scale: 0.0,
        };

        let tts = sherpa::safe_create_offline_tts(&config)
            .map_err(|e| anyhow::anyhow!("Failed to create Kokoro TTS engine: {}", e))?;

        tracing::info!("[TTS] Using Kokoro model");
        Ok(tts)
    }

    /// Destroy the current engine and recreate it from stored params.
    fn recreate_engine(&self) -> Result<()> {
        let new_tts = Self::create_engine(&self.model_dir, self.num_threads, self.is_kokoro)?;
        let mut guard = self.inner.lock().unwrap();
        unsafe { sherpa::SherpaOnnxDestroyOfflineTts(guard.tts) };
        guard.tts = new_tts;
        tracing::warn!("[TTS] Engine recreated after C++ exception");
        Ok(())
    }

    pub fn generate(&self, text: &str, speaker_id: u32, speed: f32) -> Result<(Vec<f32>, u32)> {
        let normalized = normalize_tts_text(text);
        let text_c = sherpa::to_cstr(&normalized);

        let tts_ptr = {
            let guard = self.inner.lock().unwrap();
            guard.tts
        };

        let audio_result = sherpa::safe_tts_generate_audio(
            tts_ptr,
            text_c.as_ptr(),
            speaker_id as libc::c_int,
            speed,
        );

        match audio_result {
            Ok(audio) => {
                let audio_ref = unsafe { &*audio };
                let n = audio_ref.n as usize;
                let sr = audio_ref.sample_rate as u32;

                let samples = if !audio_ref.samples.is_null() && n > 0 {
                    unsafe { std::slice::from_raw_parts(audio_ref.samples, n) }.to_vec()
                } else {
                    Vec::new()
                };

                unsafe { sherpa::SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio) };
                Ok((samples, sr))
            }
            Err(e) => {
                tracing::error!(
                    "[TTS] Generate failed (normalized_text={:?}, codepoints=[{}]): {} — recreating engine",
                    normalized,
                    normalized
                        .chars()
                        .map(|c| format!("U+{:04X}", c as u32))
                        .collect::<Vec<_>>()
                        .join(" "),
                    e
                );
                self.recreate_engine()?;
                Err(anyhow::anyhow!(
                    "TTS generation failed (engine recreated): {}",
                    e
                ))
            }
        }
    }
}

/// Build TtsModelConfig with VITS active, all others empty.
fn build_model_config(
    vits: sherpa::SherpaOnnxOfflineTtsVitsModelConfig,
    num_threads: u32,
    provider: *const libc::c_char,
    empty: *const libc::c_char,
) -> sherpa::SherpaOnnxOfflineTtsModelConfig {
    sherpa::SherpaOnnxOfflineTtsModelConfig {
        vits,
        num_threads: num_threads as libc::c_int,
        debug: 0,
        provider,
        matcha: sherpa::SherpaOnnxOfflineTtsMatchaModelConfig {
            acoustic_model: empty,
            vocoder: empty,
            lexicon: empty,
            tokens: empty,
            data_dir: empty,
            noise_scale: 0.0,
            length_scale: 1.0,
            dict_dir: empty,
        },
        kokoro: sherpa::SherpaOnnxOfflineTtsKokoroModelConfig {
            model: empty,
            voices: empty,
            tokens: empty,
            data_dir: empty,
            length_scale: 1.0,
            dict_dir: empty,
            lexicon: empty,
            lang: empty,
        },
        kitten: sherpa::SherpaOnnxOfflineTtsKittenModelConfig {
            model: empty,
            voices: empty,
            tokens: empty,
            data_dir: empty,
            length_scale: 1.0,
        },
        zipvoice: sherpa::SherpaOnnxOfflineTtsZipvoiceModelConfig {
            tokens: empty,
            encoder: empty,
            decoder: empty,
            vocoder: empty,
            data_dir: empty,
            lexicon: empty,
            feat_scale: 0.0,
            t_shift: 0.0,
            target_rms: 0.0,
            guidance_scale: 0.0,
        },
        pocket: sherpa::SherpaOnnxOfflineTtsPocketModelConfig {
            lm_flow: empty,
            lm_main: empty,
            encoder: empty,
            decoder: empty,
            text_conditioner: empty,
            vocab_json: empty,
            token_scores_json: empty,
            voice_embedding_cache_capacity: 0,
        },
        supertonic: sherpa::SherpaOnnxOfflineTtsSupertonicModelConfig {
            duration_predictor: empty,
            text_encoder: empty,
            vector_estimator: empty,
            vocoder: empty,
            tts_json: empty,
            unicode_indexer: empty,
            voice_style: empty,
        },
    }
}

/// Build TtsModelConfig with Kokoro active, VITS empty.
fn build_model_config_with_kokoro(
    vits: sherpa::SherpaOnnxOfflineTtsVitsModelConfig,
    kokoro: sherpa::SherpaOnnxOfflineTtsKokoroModelConfig,
    num_threads: u32,
    provider: *const libc::c_char,
    empty: *const libc::c_char,
) -> sherpa::SherpaOnnxOfflineTtsModelConfig {
    sherpa::SherpaOnnxOfflineTtsModelConfig {
        vits,
        num_threads: num_threads as libc::c_int,
        debug: 0,
        provider,
        matcha: sherpa::SherpaOnnxOfflineTtsMatchaModelConfig {
            acoustic_model: empty,
            vocoder: empty,
            lexicon: empty,
            tokens: empty,
            data_dir: empty,
            noise_scale: 0.0,
            length_scale: 1.0,
            dict_dir: empty,
        },
        kokoro,
        kitten: sherpa::SherpaOnnxOfflineTtsKittenModelConfig {
            model: empty,
            voices: empty,
            tokens: empty,
            data_dir: empty,
            length_scale: 1.0,
        },
        zipvoice: sherpa::SherpaOnnxOfflineTtsZipvoiceModelConfig {
            tokens: empty,
            encoder: empty,
            decoder: empty,
            vocoder: empty,
            data_dir: empty,
            lexicon: empty,
            feat_scale: 0.0,
            t_shift: 0.0,
            target_rms: 0.0,
            guidance_scale: 0.0,
        },
        pocket: sherpa::SherpaOnnxOfflineTtsPocketModelConfig {
            lm_flow: empty,
            lm_main: empty,
            encoder: empty,
            decoder: empty,
            text_conditioner: empty,
            vocab_json: empty,
            token_scores_json: empty,
            voice_embedding_cache_capacity: 0,
        },
        supertonic: sherpa::SherpaOnnxOfflineTtsSupertonicModelConfig {
            duration_predictor: empty,
            text_encoder: empty,
            vector_estimator: empty,
            vocoder: empty,
            tts_json: empty,
            unicode_indexer: empty,
            voice_style: empty,
        },
    }
}

impl Drop for TtsEngine {
    fn drop(&mut self) {
        let guard = self.inner.lock().unwrap();
        unsafe { sherpa::SherpaOnnxDestroyOfflineTts(guard.tts) };
    }
}

/// Pre-process text for kokoro TTS: strip characters the model can't handle
/// and ensure no two punctuation marks are adjacent (causes C++ exception).
fn normalize_tts_text(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last_was_punct = false;
    let mut prev_stripped = false; // 上一字符是否被剥（如 emoji）
    let mut prev_emitted_cjk = false; // 上一个**保留**的字符是否 CJK
    let mut iter = text.chars().peekable();
    while let Some(raw) = iter.next() {
        // 全角标点 → ASCII：Kokoro 词表只有 ASCII 标点，全角（？ ！ 等）会触发
        // sherpa "Unknown token" C++ 异常。先转成 ASCII 再走下面的安全过滤。
        let ch = map_fullwidth_punct(raw);
        if !is_tts_safe(ch) {
            prev_stripped = true;
            continue;
        }
        // 空格处理：Kokoro 中文路径下，CJK 之间的 ASCII 空格会触发 "Unknown token"
        // （见文件顶部 issue 2223）。原先只删"被剥字符后的孤儿空格"，漏掉了用户/LLM
        // 在中文之间手打的空格——"3日 周五"这类整句都会播放失败。三类空格都去掉：
        //   (a) 紧跟被剥字符后的"孤儿空格"；
        //   (b) 前一个保留字符是 CJK（"3日 周五" → "3日周五"）；
        //   (c) 后一个字符是 CJK（"周五 下午" 从后看命中）。
        // 英文单词之间的空格（前后都不是 CJK）保留——Kokoro 英文路径需要。
        if ch == ' ' {
            let next_is_cjk = iter.peek().map_or(false, |&n| is_cjk(n));
            if prev_stripped || prev_emitted_cjk || next_is_cjk {
                prev_stripped = false;
                continue;
            }
        }
        prev_stripped = false;
        if is_tts_punct(ch) {
            if last_was_punct {
                continue;
            }
            last_was_punct = true;
        } else {
            last_was_punct = false;
        }
        prev_emitted_cjk = is_cjk(ch);
        result.push(ch);
    }
    result
}

/// Characters known to be safe for kokoro TTS.
fn is_tts_safe(ch: char) -> bool {
    // CJK ideographs
    if is_cjk(ch) {
        return true;
    }
    // ASCII alphanumeric + space
    if ch.is_ascii_alphanumeric() || ch == ' ' {
        return true;
    }
    // Known-safe punctuation only
    is_tts_punct(ch)
}

/// 是否为 CJK 表意文字（Kokoro 中文路径安全）。
fn is_cjk(ch: char) -> bool {
    matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}')
}

/// 全角标点 → ASCII：Kokoro 词表只有 ASCII 标点，全角会触发 "Unknown token"。
/// 非标点（含 CJK 汉字、ASCII 字母数字）原样返回。
fn map_fullwidth_punct(ch: char) -> char {
    match ch {
        '，' | '、' => ',',
        '。' => '.',
        '？' => '?',
        '！' => '!',
        '；' => ';',
        '：' => ':',
        c => c,
    }
}

/// Punctuation the kokoro model handles correctly.
fn is_tts_punct(ch: char) -> bool {
    matches!(
        ch,
        // ASCII
        ',' | '.' | '?' | '!' | ';' | ':' | '-' | '\'' | '"' | '(' | ')'
        // Chinese
        | '，' | '。' | '？' | '！' | '、' | '；' | '：'
    )
}

#[cfg(test)]
mod tests;
