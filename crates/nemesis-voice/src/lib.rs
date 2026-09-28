//! Voice transcription and local voice processing.
//!
//! Provides both cloud-based transcription (Groq Whisper) and local
//! sherpa-onnx-based voice processing (STT, TTS, VAD, punctuation).
//! Local voice pipeline is only supported on Windows.

// --- Cloud transcription (cross-platform) ---
pub mod transcriber;

// --- Config (cross-platform, pure data types) ---
pub mod config;

// --- 语音化输出纯函数层（cross-platform；realtime P1 W4 L1 / W2 句切分） ---
pub mod sentence;
pub mod spoken_form;

// --- realtime P1 跨平台纯件：barge-in 决策状态机 + WAV 编解码 ---
pub mod barge_in;
pub mod wav;

// --- Local voice pipeline (Windows only) ---
// 设备 IO 三件套（audio/aec/loopback）随 `voice-capture` feature 进出：开启 =
// cpal/wasapi 真实现；关闭 = 同名 stub 模块（`#[path]` 重映射保持消费方路径
// `nemesis_voice::audio::…` 不变），运行期对设备 IO 诚实报「未编译」。
#[cfg(all(target_os = "windows", feature = "voice-capture"))]
pub mod aec;
#[cfg(not(all(target_os = "windows", feature = "voice-capture")))]
#[path = "aec_stub.rs"]
pub mod aec;
#[cfg(all(target_os = "windows", feature = "voice-capture"))]
pub mod audio;
#[cfg(not(all(target_os = "windows", feature = "voice-capture")))]
#[path = "audio_stub.rs"]
pub mod audio;
#[cfg(target_os = "windows")]
pub mod bootstrap;
#[cfg(target_os = "windows")]
pub mod channel_bridge;
// 音频采集源抽象（realtime P1 W7）：生产 = 麦克风，测试 = WavSource 注入。
// 随 Windows 进出；无 voice-capture 构建下 MicSource::new / WavSource 读文件
// 经 audio_stub 诚实报「未编译」（trait 本体可用）。
#[cfg(target_os = "windows")]
pub mod chunk_source;
#[cfg(target_os = "windows")]
pub mod lang_restriction;
#[cfg(all(target_os = "windows", feature = "voice-capture"))]
pub mod loopback;
#[cfg(not(all(target_os = "windows", feature = "voice-capture")))]
#[path = "loopback_stub.rs"]
pub mod loopback;
#[cfg(target_os = "windows")]
pub mod model;
#[cfg(target_os = "windows")]
pub mod punct;
#[cfg(target_os = "windows")]
pub mod sherpa;
#[cfg(target_os = "windows")]
pub mod speaker;
#[cfg(target_os = "windows")]
pub mod stt;
#[cfg(target_os = "windows")]
pub mod stt_stream;
#[cfg(target_os = "windows")]
pub mod tts;
#[cfg(target_os = "windows")]
pub mod vad;
#[cfg(target_os = "windows")]
pub mod voice_detect;

// --- 测试辅助（仅测试编译；见 test_util.rs 头注） ---
#[cfg(all(test, target_os = "windows"))]
mod test_util;

// --- Cloud transcription (cross-platform) ---
pub use config::AppConfig;
pub use spoken_form::spoken_pieces;
pub use transcriber::{AudioFormat, Transcriber, TranscriptionResponse};

// --- Local pipeline re-exports (Windows only) ---
#[cfg(target_os = "windows")]
pub use aec::{
    AEC_SAMPLE_RATE, DEFAULT_FILTER_LENGTH, DEFAULT_FRAME_SIZE, EchoCanceller, SpeexAec,
};
#[cfg(target_os = "windows")]
pub use audio::{
    AudioCapture, AudioPlayback, Resampler, StopHandle, far_end_buffer, far_end_sample_rate,
};
#[cfg(target_os = "windows")]
pub use bootstrap::{download_aec_lib, init_sherpa, run_in_dir as bootstrap_run_in_dir};
#[cfg(target_os = "windows")]
pub use chunk_source::{AudioChunkSource, MicSource, WavSource};
#[cfg(target_os = "windows")]
pub use loopback::{start_loopback, stop_loopback};
#[cfg(target_os = "windows")]
pub use punct::PunctEngine;
#[cfg(target_os = "windows")]
pub use sherpa::is_initialized as sherpa_is_initialized;
#[cfg(target_os = "windows")]
pub use speaker::cosine_similarity;
#[cfg(target_os = "windows")]
pub use speaker::{SpeakerEngine, SpeakerManager};
#[cfg(target_os = "windows")]
pub use stt::SttEngine;
#[cfg(target_os = "windows")]
pub use stt_stream::StreamSttEngine;
#[cfg(target_os = "windows")]
pub use tts::TtsEngine;
#[cfg(target_os = "windows")]
pub use vad::{SpeechSegment, VadEngine};
#[cfg(target_os = "windows")]
pub use voice_detect::{RmsVoiceDetector, SileroVoiceDetector, VoiceDetector, create_detector};

// --- Progress (Windows only) ---
#[cfg(target_os = "windows")]
pub use model::set_progress;
