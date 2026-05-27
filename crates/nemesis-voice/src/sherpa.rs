//! sherpa-onnx FFI bindings — struct layouts matching v1.13.2
//!
//! Uses runtime dynamic loading (libloading) so the program can start
//! without the DLL, download it, then load it — full auto-setup.
//!
//! IMPORTANT: Struct layouts are matched to sherpa-onnx v1.13.2.
//! When upgrading sherpa-onnx, verify each struct against the new c-api.h.

use std::ffi::CString;
use std::path::Path;

// =============================================================================
// Opaque types
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxOfflineRecognizer {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SherpaOnnxOfflineStream {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SherpaOnnxVoiceActivityDetector {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SherpaOnnxOfflineTts {
    _private: [u8; 0],
}

#[repr(C)]
pub struct SherpaOnnxLinearResampler {
    _private: [u8; 0],
}

// =============================================================================
// Offline Recognizer structs (STT) — v1.13.2 exact match
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxOfflineTransducerModelConfig {
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub joiner: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineParaformerModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineNemoEncDecCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineWhisperModelConfig {
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub language: *const libc::c_char,
    pub task: *const libc::c_char,
    pub tail_paddings: libc::c_int,
    pub enable_token_timestamps: libc::c_int,
    pub enable_segment_timestamps: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTdnnModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineSenseVoiceModelConfig {
    pub model: *const libc::c_char,
    pub language: *const libc::c_char,
    pub use_itn: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxOfflineMoonshineModelConfig {
    pub preprocessor: *const libc::c_char,
    pub encoder: *const libc::c_char,
    pub uncached_decoder: *const libc::c_char,
    pub cached_decoder: *const libc::c_char,
    pub merged_decoder: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineFireRedAsrModelConfig {
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineDolphinModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineZipformerCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineCanaryModelConfig {
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub src_lang: *const libc::c_char,
    pub tgt_lang: *const libc::c_char,
    pub use_pnc: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxOfflineWenetCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineOmnilingualAsrCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineMedAsrCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineFunASRNanoModelConfig {
    pub encoder_adaptor: *const libc::c_char,
    pub llm: *const libc::c_char,
    pub embedding: *const libc::c_char,
    pub tokenizer: *const libc::c_char,
    pub system_prompt: *const libc::c_char,
    pub user_prompt: *const libc::c_char,
    pub max_new_tokens: libc::c_int,
    pub temperature: f32,
    pub top_p: f32,
    pub seed: libc::c_int,
    pub language: *const libc::c_char,
    pub itn: libc::c_int,
    pub hotwords: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineFireRedAsrCtcModelConfig {
    pub model: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineQwen3ASRModelConfig {
    pub conv_frontend: *const libc::c_char,
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub tokenizer: *const libc::c_char,
    pub max_total_len: libc::c_int,
    pub max_new_tokens: libc::c_int,
    pub temperature: f32,
    pub top_p: f32,
    pub seed: libc::c_int,
    pub hotwords: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineCohereTranscribeModelConfig {
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub language: *const libc::c_char,
    pub use_punct: libc::c_int,
    pub use_itn: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxHomophoneReplacerConfig {
    pub dict_dir: *const libc::c_char,
    pub lexicon: *const libc::c_char,
    pub rule_fsts: *const libc::c_char,
}

// v1.13.2: OfflineModelConfig — full field list
#[repr(C)]
pub struct SherpaOnnxOfflineModelConfig {
    pub transducer: SherpaOnnxOfflineTransducerModelConfig,
    pub paraformer: SherpaOnnxOfflineParaformerModelConfig,
    pub nemo_ctc: SherpaOnnxOfflineNemoEncDecCtcModelConfig,
    pub whisper: SherpaOnnxOfflineWhisperModelConfig,
    pub tdnn: SherpaOnnxOfflineTdnnModelConfig,
    pub tokens: *const libc::c_char,
    pub num_threads: libc::c_int,
    pub debug: libc::c_int,
    pub provider: *const libc::c_char,
    pub model_type: *const libc::c_char,
    pub modeling_unit: *const libc::c_char,
    pub bpe_vocab: *const libc::c_char,
    pub telespeech_ctc: *const libc::c_char,
    pub sense_voice: SherpaOnnxOfflineSenseVoiceModelConfig,
    pub moonshine: SherpaOnnxOfflineMoonshineModelConfig,
    pub fire_red_asr: SherpaOnnxOfflineFireRedAsrModelConfig,
    pub dolphin: SherpaOnnxOfflineDolphinModelConfig,
    pub zipformer_ctc: SherpaOnnxOfflineZipformerCtcModelConfig,
    pub canary: SherpaOnnxOfflineCanaryModelConfig,
    pub wenet_ctc: SherpaOnnxOfflineWenetCtcModelConfig,
    pub omnilingual: SherpaOnnxOfflineOmnilingualAsrCtcModelConfig,
    pub medasr: SherpaOnnxOfflineMedAsrCtcModelConfig,
    pub funasr_nano: SherpaOnnxOfflineFunASRNanoModelConfig,
    pub fire_red_asr_ctc: SherpaOnnxOfflineFireRedAsrCtcModelConfig,
    pub qwen3_asr: SherpaOnnxOfflineQwen3ASRModelConfig,
    pub cohere_transcribe: SherpaOnnxOfflineCohereTranscribeModelConfig,
}

#[repr(C)]
pub struct SherpaOnnxFeatureConfig {
    pub sample_rate: libc::c_int,
    pub feature_dim: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxOfflineLMConfig {
    pub model: *const libc::c_char,
    pub scale: f32,
}

// v1.13.2: RecognizerConfig
#[repr(C)]
pub struct SherpaOnnxOfflineRecognizerConfig {
    pub feat_config: SherpaOnnxFeatureConfig,
    pub model_config: SherpaOnnxOfflineModelConfig,
    pub lm_config: SherpaOnnxOfflineLMConfig,
    pub decoding_method: *const libc::c_char,
    pub max_active_paths: libc::c_int,
    pub hotwords_file: *const libc::c_char,
    pub hotwords_score: f32,
    pub rule_fsts: *const libc::c_char,
    pub rule_fars: *const libc::c_char,
    pub blank_penalty: f32,
    pub hr: SherpaOnnxHomophoneReplacerConfig,
}

// v1.13.2: Result — extended with lang/emotion/event/etc.
#[repr(C)]
pub struct SherpaOnnxOfflineRecognizerResult {
    pub text: *const libc::c_char,
    pub timestamps: *mut f32,
    pub count: libc::c_int,
    pub tokens: *const libc::c_char,
    pub tokens_arr: *const *const libc::c_char,
    pub json: *const libc::c_char,
    pub lang: *const libc::c_char,
    pub emotion: *const libc::c_char,
    pub event: *const libc::c_char,
    pub durations: *mut f32,
    pub ys_log_probs: *mut f32,
    pub segment_timestamps: *const f32,
    pub segment_durations: *const f32,
    pub segment_texts: *const libc::c_char,
    pub segment_texts_arr: *const *const libc::c_char,
    pub segment_count: libc::c_int,
}

// =============================================================================
// VAD structs — v1.13.2
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxSileroVadModelConfig {
    pub model: *const libc::c_char,
    pub threshold: f32,
    pub min_silence_duration: f32,
    pub min_speech_duration: f32,
    pub window_size: libc::c_int,
    pub max_speech_duration: f32,
}

#[repr(C)]
pub struct SherpaOnnxTenVadModelConfig {
    pub model: *const libc::c_char,
    pub threshold: f32,
    pub min_silence_duration: f32,
    pub min_speech_duration: f32,
    pub window_size: libc::c_int,
    pub max_speech_duration: f32,
}

#[repr(C)]
pub struct SherpaOnnxVadModelConfig {
    pub silero_vad: SherpaOnnxSileroVadModelConfig,
    pub sample_rate: libc::c_int,
    pub num_threads: libc::c_int,
    pub provider: *const libc::c_char,
    pub debug: libc::c_int,
    pub ten_vad: SherpaOnnxTenVadModelConfig,
}

#[repr(C)]
pub struct SherpaOnnxSpeechSegment {
    pub start: libc::c_int,
    pub samples: *mut f32,
    pub n: libc::c_int,
}

// =============================================================================
// Offline Punctuation structs
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxOfflinePunctuationModelConfig {
    pub ct_transformer: *const libc::c_char,
    pub num_threads: libc::c_int,
    pub debug: libc::c_int,
    pub provider: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflinePunctuationConfig {
    pub model: SherpaOnnxOfflinePunctuationModelConfig,
}

#[repr(C)]
pub struct SherpaOnnxOfflinePunctuation {
    _private: [u8; 0],
}

// =============================================================================
// TTS structs — v1.13.2
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxOfflineTtsVitsModelConfig {
    pub model: *const libc::c_char,
    pub lexicon: *const libc::c_char,
    pub tokens: *const libc::c_char,
    pub data_dir: *const libc::c_char,
    pub noise_scale: f32,
    pub noise_scale_w: f32,
    pub length_scale: f32,
    pub dict_dir: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsMatchaModelConfig {
    pub acoustic_model: *const libc::c_char,
    pub vocoder: *const libc::c_char,
    pub lexicon: *const libc::c_char,
    pub tokens: *const libc::c_char,
    pub data_dir: *const libc::c_char,
    pub noise_scale: f32,
    pub length_scale: f32,
    pub dict_dir: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsKokoroModelConfig {
    pub model: *const libc::c_char,
    pub voices: *const libc::c_char,
    pub tokens: *const libc::c_char,
    pub data_dir: *const libc::c_char,
    pub length_scale: f32,
    pub dict_dir: *const libc::c_char,
    pub lexicon: *const libc::c_char,
    pub lang: *const libc::c_char,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsKittenModelConfig {
    pub model: *const libc::c_char,
    pub voices: *const libc::c_char,
    pub tokens: *const libc::c_char,
    pub data_dir: *const libc::c_char,
    pub length_scale: f32,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsZipvoiceModelConfig {
    pub tokens: *const libc::c_char,
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub vocoder: *const libc::c_char,
    pub data_dir: *const libc::c_char,
    pub lexicon: *const libc::c_char,
    pub feat_scale: f32,
    pub t_shift: f32,
    pub target_rms: f32,
    pub guidance_scale: f32,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsPocketModelConfig {
    pub lm_flow: *const libc::c_char,
    pub lm_main: *const libc::c_char,
    pub encoder: *const libc::c_char,
    pub decoder: *const libc::c_char,
    pub text_conditioner: *const libc::c_char,
    pub vocab_json: *const libc::c_char,
    pub token_scores_json: *const libc::c_char,
    pub voice_embedding_cache_capacity: libc::c_int,
}

#[repr(C)]
pub struct SherpaOnnxOfflineTtsSupertonicModelConfig {
    pub duration_predictor: *const libc::c_char,
    pub text_encoder: *const libc::c_char,
    pub vector_estimator: *const libc::c_char,
    pub vocoder: *const libc::c_char,
    pub tts_json: *const libc::c_char,
    pub unicode_indexer: *const libc::c_char,
    pub voice_style: *const libc::c_char,
}

// v1.13.2: TtsModelConfig — full field list
#[repr(C)]
pub struct SherpaOnnxOfflineTtsModelConfig {
    pub vits: SherpaOnnxOfflineTtsVitsModelConfig,
    pub num_threads: libc::c_int,
    pub debug: libc::c_int,
    pub provider: *const libc::c_char,
    pub matcha: SherpaOnnxOfflineTtsMatchaModelConfig,
    pub kokoro: SherpaOnnxOfflineTtsKokoroModelConfig,
    pub kitten: SherpaOnnxOfflineTtsKittenModelConfig,
    pub zipvoice: SherpaOnnxOfflineTtsZipvoiceModelConfig,
    pub pocket: SherpaOnnxOfflineTtsPocketModelConfig,
    pub supertonic: SherpaOnnxOfflineTtsSupertonicModelConfig,
}

// v1.13.2: TtsConfig
#[repr(C)]
pub struct SherpaOnnxOfflineTtsConfig {
    pub model: SherpaOnnxOfflineTtsModelConfig,
    pub rule_fsts: *const libc::c_char,
    pub max_num_sentences: libc::c_int,
    pub rule_fars: *const libc::c_char,
    pub silence_scale: f32,
}

#[repr(C)]
pub struct SherpaOnnxGeneratedAudio {
    pub samples: *const f32,
    pub n: libc::c_int,
    pub sample_rate: libc::c_int,
}

// =============================================================================
// Wave I/O
// =============================================================================

#[repr(C)]
pub struct SherpaOnnxWave {
    pub samples: *mut f32,
    pub sample_rate: libc::c_int,
    pub num_samples: libc::c_int,
}

// =============================================================================
// Runtime dynamic loading via libloading
// =============================================================================

use libloading::Library;
#[cfg(target_os = "windows")]
use libloading::os::windows as win_lib;
use std::sync::OnceLock;

static SHERPA_LIB: OnceLock<Library> = OnceLock::new();

/// Load the sherpa-onnx DLL from the given path. Must be called before any sherpa function.
///
/// On Windows, uses LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR so that transitive
/// dependencies (onnxruntime.dll etc.) are resolved from the DLL's own
/// directory, not from the system PATH.  Without this flag Windows would
/// load whatever onnxruntime.dll it finds first (e.g. from Python, Edge,
/// Office …), which is often too old for the Kokoro TTS model.
pub fn init(dll_path: &Path) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    {
        // 0x100 = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
        // 0x1000 = LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
        const FLAGS: u32 = 0x100 | 0x1000;
        let lib = unsafe {
            win_lib::Library::load_with_flags(dll_path, FLAGS)
        }
        .map_err(|e| anyhow::anyhow!("Failed to load {}: {}", dll_path.display(), e))?;
        SHERPA_LIB
            .set(lib.into())
            .map_err(|_| anyhow::anyhow!("sherpa-onnx already initialized"))?;
    }
    #[cfg(not(target_os = "windows"))]
    {
        let lib = unsafe { Library::new(dll_path) }
            .map_err(|e| anyhow::anyhow!("Failed to load {}: {}", dll_path.display(), e))?;
        SHERPA_LIB
            .set(lib)
            .map_err(|_| anyhow::anyhow!("sherpa-onnx already initialized"))?;
    }
    Ok(())
}

/// Check if sherpa-onnx has been initialized.
pub fn is_initialized() -> bool {
    SHERPA_LIB.get().is_some()
}

/// Get a function pointer from the loaded library.
/// Panics if the library is not loaded or the symbol is not found.
unsafe fn get_fn<F>(name: &[u8]) -> libloading::Symbol<'static, F> {
    let lib = SHERPA_LIB
        .get()
        .unwrap_or_else(|| panic!("sherpa-onnx not initialized (looking for {})", String::from_utf8_lossy(name)));
    // We transmute the symbol lifetime to 'static because the Library is never dropped.
    // This is safe because SHERPA_LIB is a OnceLock and lives for the entire program.
    let sym: libloading::Symbol<'_, F> = unsafe {
        lib.get(name)
            .unwrap_or_else(|e| panic!("Symbol {} not found: {}", String::from_utf8_lossy(name), e))
    };
    unsafe { std::mem::transmute(sym) }
}

/// Get a raw function pointer from the loaded library (extern "C" ABI).
unsafe fn get_raw_fn<F>(name: &[u8]) -> libloading::Symbol<'static, F> {
    let lib = SHERPA_LIB
        .get()
        .unwrap_or_else(|| panic!("sherpa-onnx not initialized (looking for {})", String::from_utf8_lossy(name)));
    let sym: libloading::Symbol<'_, F> = unsafe {
        lib.get(name)
            .unwrap_or_else(|e| panic!("Symbol {} not found: {}", String::from_utf8_lossy(name), e))
    };
    unsafe { std::mem::transmute(sym) }
}

// =============================================================================
// Runtime-loaded function wrappers
// Naming follows sherpa-onnx C API convention (PascalCase) for consistency.
// =============================================================================

macro_rules! sherpa_fn {
    // Generates a wrapper function with the C API name (PascalCase)
    ($(#[$meta:meta])* $name:ident($($arg:ident : $arg_ty:ty),*) -> $ret:ty) => {
        #[allow(non_snake_case)]
        $(#[$meta])*
        pub unsafe fn $name($($arg : $arg_ty),*) -> $ret {
            let f = unsafe { get_fn::<unsafe fn($($arg_ty),*) -> $ret>(
                concat!(stringify!($name), "\0").as_bytes(),
            ) };
            unsafe { f($($arg),*) }
        }
    };
    // void return
    ($(#[$meta:meta])* $name:ident($($arg:ident : $arg_ty:ty),*)) => {
        #[allow(non_snake_case)]
        $(#[$meta])*
        pub unsafe fn $name($($arg : $arg_ty),*) {
            let f = unsafe { get_fn::<unsafe fn($($arg_ty),*)>(
                concat!(stringify!($name), "\0").as_bytes(),
            ) };
            unsafe { f($($arg),*) }
        }
    };
}

// ---- Offline Recognizer (STT) ----

sherpa_fn!(SherpaOnnxCreateOfflineRecognizer(config: *const SherpaOnnxOfflineRecognizerConfig) -> *const SherpaOnnxOfflineRecognizer);
sherpa_fn!(SherpaOnnxDestroyOfflineRecognizer(recognizer: *const SherpaOnnxOfflineRecognizer));
sherpa_fn!(SherpaOnnxCreateOfflineStream(recognizer: *const SherpaOnnxOfflineRecognizer) -> *const SherpaOnnxOfflineStream);
sherpa_fn!(SherpaOnnxDestroyOfflineStream(stream: *const SherpaOnnxOfflineStream));
sherpa_fn!(SherpaOnnxAcceptWaveformOffline(stream: *const SherpaOnnxOfflineStream, sample_rate: libc::c_int, samples: *const f32, n: libc::c_int));
sherpa_fn!(SherpaOnnxDecodeOfflineStream(recognizer: *const SherpaOnnxOfflineRecognizer, stream: *const SherpaOnnxOfflineStream));
sherpa_fn!(SherpaOnnxGetOfflineStreamResult(stream: *const SherpaOnnxOfflineStream) -> *const SherpaOnnxOfflineRecognizerResult);
sherpa_fn!(SherpaOnnxDestroyOfflineRecognizerResult(result: *const SherpaOnnxOfflineRecognizerResult));

// ---- VAD ----

sherpa_fn!(SherpaOnnxCreateVoiceActivityDetector(config: *const SherpaOnnxVadModelConfig, buffer_size_in_seconds: f32) -> *const SherpaOnnxVoiceActivityDetector);
sherpa_fn!(SherpaOnnxDestroyVoiceActivityDetector(vad: *const SherpaOnnxVoiceActivityDetector));
sherpa_fn!(SherpaOnnxVoiceActivityDetectorAcceptWaveform(vad: *const SherpaOnnxVoiceActivityDetector, samples: *const f32, n: libc::c_int));
sherpa_fn!(SherpaOnnxVoiceActivityDetectorEmpty(vad: *const SherpaOnnxVoiceActivityDetector) -> libc::c_int);
sherpa_fn!(SherpaOnnxVoiceActivityDetectorDetected(vad: *const SherpaOnnxVoiceActivityDetector) -> libc::c_int);
sherpa_fn!(SherpaOnnxVoiceActivityDetectorFront(vad: *const SherpaOnnxVoiceActivityDetector) -> *const SherpaOnnxSpeechSegment);
sherpa_fn!(SherpaOnnxDestroySpeechSegment(segment: *const SherpaOnnxSpeechSegment));
sherpa_fn!(SherpaOnnxVoiceActivityDetectorPop(vad: *const SherpaOnnxVoiceActivityDetector));
sherpa_fn!(SherpaOnnxVoiceActivityDetectorFlush(vad: *const SherpaOnnxVoiceActivityDetector));
sherpa_fn!(SherpaOnnxVoiceActivityDetectorReset(vad: *const SherpaOnnxVoiceActivityDetector));

// ---- TTS ----
// Create and Generate are wrapped by C++ try/catch (cpp/safe_ffi.cpp)
// to prevent C++ exceptions from crossing the FFI boundary and aborting.

sherpa_fn!(SherpaOnnxDestroyOfflineTts(tts: *const SherpaOnnxOfflineTts));
sherpa_fn!(SherpaOnnxDestroyOfflineTtsGeneratedAudio(audio: *const SherpaOnnxGeneratedAudio));

#[cfg(target_os = "windows")]
unsafe extern "C" {
    fn safe_tts_create(
        create_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTtsConfig) -> *const SherpaOnnxOfflineTts,
        config: *const SherpaOnnxOfflineTtsConfig,
        out: *mut *const SherpaOnnxOfflineTts,
    ) -> libc::c_int;

    fn safe_tts_generate(
        generate_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTts, *const libc::c_char, libc::c_int, f32) -> *const SherpaOnnxGeneratedAudio,
        tts: *const SherpaOnnxOfflineTts,
        text: *const libc::c_char,
        sid: libc::c_int,
        speed: f32,
        out: *mut *const SherpaOnnxGeneratedAudio,
    ) -> libc::c_int;
}

/// Safe TTS create — catches C++ exceptions on Windows.
pub fn safe_create_offline_tts(config: *const SherpaOnnxOfflineTtsConfig) -> Result<*const SherpaOnnxOfflineTts, String> {
    #[cfg(target_os = "windows")]
    {
        let create_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTtsConfig) -> *const SherpaOnnxOfflineTts =
            unsafe { *get_raw_fn(b"SherpaOnnxCreateOfflineTts\0") };
        let mut out: *const SherpaOnnxOfflineTts = std::ptr::null();
        let rc = unsafe { safe_tts_create(create_fn, config, &mut out) };
        if rc != 0 { Err("sherpa-onnx TTS create threw C++ exception".into()) }
        else if out.is_null() { Err("sherpa-onnx TTS create returned null".into()) }
        else { Ok(out) }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let create_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTtsConfig) -> *const SherpaOnnxOfflineTts =
            unsafe { *get_raw_fn(b"SherpaOnnxCreateOfflineTts\0") };
        let out = unsafe { create_fn(config) };
        if out.is_null() { Err("sherpa-onnx TTS create returned null".into()) }
        else { Ok(out) }
    }
}

/// Safe TTS generate — catches C++ exceptions on Windows.
pub fn safe_tts_generate_audio(
    tts: *const SherpaOnnxOfflineTts,
    text: *const libc::c_char,
    sid: libc::c_int,
    speed: f32,
) -> Result<*const SherpaOnnxGeneratedAudio, String> {
    #[cfg(target_os = "windows")]
    {
        let generate_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTts, *const libc::c_char, libc::c_int, f32) -> *const SherpaOnnxGeneratedAudio =
            unsafe { *get_raw_fn(b"SherpaOnnxOfflineTtsGenerate\0") };
        let mut out: *const SherpaOnnxGeneratedAudio = std::ptr::null();
        let rc = unsafe { safe_tts_generate(generate_fn, tts, text, sid, speed, &mut out) };
        if rc != 0 { Err("sherpa-onnx TTS generate threw C++ exception".into()) }
        else if out.is_null() { Err("sherpa-onnx TTS generate returned null".into()) }
        else { Ok(out) }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let generate_fn: unsafe extern "C" fn(*const SherpaOnnxOfflineTts, *const libc::c_char, libc::c_int, f32) -> *const SherpaOnnxGeneratedAudio =
            unsafe { *get_raw_fn(b"SherpaOnnxOfflineTtsGenerate\0") };
        let out = unsafe { generate_fn(tts, text, sid, speed) };
        if out.is_null() { Err("sherpa-onnx TTS generate returned null".into()) }
        else { Ok(out) }
    }
}

// ---- Linear Resampler ----

sherpa_fn!(SherpaOnnxCreateLinearResampler(samp_rate_in: f32, samp_rate_out: f32, filter_cutoff_freq: f32, num_zeros: libc::c_int) -> *const SherpaOnnxLinearResampler);
sherpa_fn!(SherpaOnnxDestroyLinearResampler(resampler: *const SherpaOnnxLinearResampler));
sherpa_fn!(SherpaOnnxLinearResamplerResample(resampler: *const SherpaOnnxLinearResampler, input: *const f32, input_dim: libc::c_int, n: libc::c_int, flush: libc::c_int, output: *mut *mut f32, output_dim: *mut libc::c_int, output_n: *mut libc::c_int) -> libc::c_int);
sherpa_fn!(SherpaOnnxLinearResamplerReset(resampler: *const SherpaOnnxLinearResampler));

// ---- Wave I/O ----

sherpa_fn!(SherpaOnnxReadWave(filename: *const libc::c_char) -> *const SherpaOnnxWave);
sherpa_fn!(SherpaOnnxFreeWave(wave: *const SherpaOnnxWave));
sherpa_fn!(SherpaOnnxWriteWave(samples: *const f32, n: libc::c_int, sample_rate: libc::c_int, filename: *const libc::c_char) -> libc::c_int);

// ---- Offline Punctuation ----

sherpa_fn!(SherpaOnnxCreateOfflinePunctuation(config: *const SherpaOnnxOfflinePunctuationConfig) -> *const SherpaOnnxOfflinePunctuation);
sherpa_fn!(SherpaOnnxDestroyOfflinePunctuation(punct: *const SherpaOnnxOfflinePunctuation));
sherpa_fn!(SherpaOfflinePunctuationAddPunct(punct: *const SherpaOnnxOfflinePunctuation, text: *const libc::c_char) -> *const libc::c_char);
sherpa_fn!(SherpaOfflinePunctuationFreeText(text: *const libc::c_char));

// =============================================================================
// Helper
// =============================================================================

pub fn null_cstr() -> *const libc::c_char {
    static EMPTY: &[u8; 1] = b"\0";
    EMPTY.as_ptr() as *const libc::c_char
}

pub fn to_cstr(s: &str) -> CString {
    CString::new(s).unwrap_or_else(|_| CString::new("").unwrap())
}
