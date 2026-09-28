//! Streaming STT safe wrapper — streaming Zipformer/Paraformer via sherpa-onnx
//!
//! 语音 realtime P1（W1）：边说边出 partial。与 `stt.rs`（SenseVoice 整段精识）
//! 互补：partial 只管实时显示与端点判定，final 默认仍走离线整段精识（two_pass，
//! 见 handlers/voice.rs 管线）。
//!
//! 生命周期：一个 [`StreamSttEngine`] 建一个 recognizer；每段话语 `reset()` 后
//! 复用同一 stream（`OnlineStreamReset`），避免反复建流的分配抖动。

use anyhow::Result;
use std::ffi::{CStr, CString};
use std::path::Path;

use crate::sherpa;

/// 模型目录内固定文件名约定（config.toml sources 条目的 local 字段落地同名；
/// 换模型 = 换 sources 条目 + 保持这四个 local 名，零代码改动）。
pub const STREAM_ENCODER_FILE: &str = "encoder.onnx";
pub const STREAM_DECODER_FILE: &str = "decoder.onnx";
pub const STREAM_JOINER_FILE: &str = "joiner.onnx";
pub const STREAM_TOKENS_FILE: &str = "tokens.txt";

pub struct StreamSttEngine {
    recognizer: *const sherpa::SherpaOnnxOnlineRecognizer,
    stream: *const sherpa::SherpaOnnxOnlineStream,
    // CString 保活：FFI 指针指向这些缓冲，必须活到 recognizer 销毁之后。
    _keep: Vec<CString>,
}

// recognizer/stream 是 sherpa-onnx 内部自带锁的独立对象，跨线程移动安全；
// 调用方负责同一实例不并发调用（run_stt_pipeline 单线程消费）。
unsafe impl Send for StreamSttEngine {}
unsafe impl Sync for StreamSttEngine {}

impl StreamSttEngine {
    /// 约定文件名构造（生产路径；管线直接传 cfg.stt_stream 的端点参数）。
    pub fn new_default(
        model_dir: &Path,
        num_threads: u32,
        rule1_min_trailing_silence: f32,
        rule2_min_trailing_silence: f32,
    ) -> Result<Self> {
        Self::new(
            model_dir,
            STREAM_ENCODER_FILE,
            STREAM_DECODER_FILE,
            STREAM_JOINER_FILE,
            STREAM_TOKENS_FILE,
            num_threads,
            rule1_min_trailing_silence,
            rule2_min_trailing_silence,
        )
    }

    /// Create a streaming recognizer from a model directory.
    ///
    /// `encoder`/`decoder`/`joiner` 是流式 transducer 三件套文件名（目录内相对
    /// 路径）；`tokens` 同理。目录缺少任一文件 → Err。
    pub fn new(
        model_dir: &Path,
        encoder: &str,
        decoder: &str,
        joiner: &str,
        tokens: &str,
        num_threads: u32,
        // 端点检测三参数（sherpa 内建端点，与 VAD 双信号互补）
        rule1_min_trailing_silence: f32,
        rule2_min_trailing_silence: f32,
    ) -> Result<Self> {
        let enc_path = model_dir.join(encoder);
        let dec_path = model_dir.join(decoder);
        let joi_path = model_dir.join(joiner);
        let tok_path = model_dir.join(tokens);

        for p in [&enc_path, &dec_path, &joi_path, &tok_path] {
            if !p.exists() {
                anyhow::bail!("Streaming STT model file not found: {}", p.display());
            }
        }

        // CString 全部先物化，再取指针组装 config（借用必须活过 create 调用）。
        let keep: Vec<CString> = vec![
            sherpa::to_cstr(enc_path.to_str().unwrap_or("")),
            sherpa::to_cstr(dec_path.to_str().unwrap_or("")),
            sherpa::to_cstr(joi_path.to_str().unwrap_or("")),
            sherpa::to_cstr(tok_path.to_str().unwrap_or("")),
            sherpa::to_cstr("greedy_search"),
            sherpa::to_cstr("cpu"),
        ];
        let e = |i: usize| keep[i].as_ptr();

        let config = sherpa::SherpaOnnxOnlineRecognizerConfig {
            feat_config: sherpa::SherpaOnnxFeatureConfig {
                sample_rate: 16000,
                feature_dim: 80,
            },
            model_config: sherpa::SherpaOnnxOnlineModelConfig {
                transducer: sherpa::SherpaOnnxOnlineTransducerModelConfig {
                    encoder: e(0),
                    decoder: e(1),
                    joiner: e(2),
                },
                paraformer: sherpa::SherpaOnnxOnlineParaformerModelConfig {
                    encoder: sherpa::null_cstr(),
                    decoder: sherpa::null_cstr(),
                },
                zipformer2_ctc: sherpa::SherpaOnnxOnlineZipformer2CtcModelConfig {
                    model: sherpa::null_cstr(),
                },
                tokens: e(3),
                num_threads: num_threads as libc::c_int,
                provider: e(5),
                debug: 0,
                model_type: sherpa::null_cstr(),
                modeling_unit: sherpa::null_cstr(),
                bpe_vocab: sherpa::null_cstr(),
                tokens_buf: sherpa::null_cstr(),
                tokens_buf_size: 0,
                nemo_ctc: sherpa::SherpaOnnxOnlineNemoCtcModelConfig {
                    model: sherpa::null_cstr(),
                },
                t_one_ctc: sherpa::SherpaOnnxOnlineToneCtcModelConfig {
                    model: sherpa::null_cstr(),
                },
            },
            decoding_method: e(4),
            max_active_paths: 4,
            enable_endpoint: 1,
            rule1_min_trailing_silence,
            rule2_min_trailing_silence,
            rule3_min_utterance_length: 20.0,
            hotwords_file: sherpa::null_cstr(),
            hotwords_score: 0.0,
            ctc_fst_decoder_config: sherpa::SherpaOnnxOnlineCtcFstDecoderConfig {
                graph: sherpa::null_cstr(),
                max_active: 0,
            },
            rule_fsts: sherpa::null_cstr(),
            rule_fars: sherpa::null_cstr(),
            blank_penalty: 0.0,
            hotwords_buf: sherpa::null_cstr(),
            hotwords_buf_size: 0,
            hr: sherpa::SherpaOnnxHomophoneReplacerConfig {
                dict_dir: sherpa::null_cstr(),
                lexicon: sherpa::null_cstr(),
                rule_fsts: sherpa::null_cstr(),
            },
        };

        let recognizer = unsafe { sherpa::SherpaOnnxCreateOnlineRecognizer(&config) };
        if recognizer.is_null() {
            anyhow::bail!("Failed to create streaming STT recognizer");
        }

        let stream = unsafe { sherpa::SherpaOnnxCreateOnlineStream(recognizer) };
        if stream.is_null() {
            unsafe { sherpa::SherpaOnnxDestroyOnlineRecognizer(recognizer) };
            anyhow::bail!("Failed to create streaming STT stream");
        }

        Ok(Self {
            recognizer,
            stream,
            _keep: keep,
        })
    }

    /// Feed one chunk of samples (f32, target sample rate) into the stream.
    pub fn accept_waveform(&self, samples: &[f32], sample_rate: u32) {
        if samples.is_empty() {
            return;
        }
        unsafe {
            sherpa::SherpaOnnxOnlineStreamAcceptWaveform(
                self.stream,
                sample_rate as libc::c_int,
                samples.as_ptr(),
                samples.len() as libc::c_int,
            );
        }
    }

    /// Drain internal decoding: decode while ready. Call after each feed.
    pub fn decode(&self) {
        while unsafe { sherpa::SherpaOnnxIsOnlineStreamReady(self.recognizer, self.stream) } == 1 {
            unsafe { sherpa::SherpaOnnxDecodeOnlineStream(self.recognizer, self.stream) };
        }
    }

    /// Current partial hypothesis (accumulated text so far). Empty = nothing yet.
    pub fn partial(&self) -> String {
        let result_ptr =
            unsafe { sherpa::SherpaOnnxGetOnlineStreamResult(self.recognizer, self.stream) };
        if result_ptr.is_null() {
            return String::new();
        }
        let text = unsafe { &*result_ptr };
        let s = if text.text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(text.text) }
                .to_str()
                .unwrap_or("")
                .to_string()
        };
        unsafe { sherpa::SherpaOnnxDestroyOnlineRecognizerResult(result_ptr) };
        s
    }

    /// Whether the built-in endpoint detector fired (trailing silence budget spent).
    pub fn is_endpoint(&self) -> bool {
        unsafe { sherpa::SherpaOnnxOnlineStreamIsEndpoint(self.recognizer, self.stream) == 1 }
    }

    /// Reset the stream for a new utterance (drops internal decoder state).
    pub fn reset(&self) {
        unsafe { sherpa::SherpaOnnxOnlineStreamReset(self.recognizer, self.stream) };
    }

    /// Signal end-of-input (flushes decoder). Not used by the continuous
    /// pipeline (streams are reused), kept for tests.
    pub fn input_finished(&self) {
        unsafe { sherpa::SherpaOnnxOnlineStreamInputFinished(self.stream) };
    }
}

impl Drop for StreamSttEngine {
    fn drop(&mut self) {
        unsafe {
            sherpa::SherpaOnnxDestroyOnlineStream(self.stream);
            sherpa::SherpaOnnxDestroyOnlineRecognizer(self.recognizer);
        }
    }
}

#[cfg(all(test, target_os = "windows"))]
mod stt_stream_tests;
