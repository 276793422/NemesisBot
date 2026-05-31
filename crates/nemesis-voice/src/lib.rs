//! Voice transcription and local voice processing.
//!
//! Provides both cloud-based transcription (Groq Whisper) and local
//! sherpa-onnx-based voice processing (STT, TTS, VAD, punctuation).
//! Local voice pipeline is only supported on Windows.

// --- Cloud transcription (cross-platform) ---
pub mod transcriber;

// --- Config (cross-platform, pure data types) ---
pub mod config;

// --- Local voice pipeline (Windows only) ---
#[cfg(target_os = "windows")]
pub mod audio;
#[cfg(target_os = "windows")]
pub mod bootstrap;
#[cfg(target_os = "windows")]
pub mod channel_bridge;
#[cfg(target_os = "windows")]
pub mod model;
#[cfg(target_os = "windows")]
pub mod punct;
#[cfg(target_os = "windows")]
pub mod speaker;
#[cfg(target_os = "windows")]
pub mod sherpa;
#[cfg(target_os = "windows")]
pub mod stt;
#[cfg(target_os = "windows")]
pub mod tts;
#[cfg(target_os = "windows")]
pub mod vad;
#[cfg(target_os = "windows")]
pub mod voice_detect;

// --- Cloud re-exports (cross-platform) ---
pub use transcriber::{AudioFormat, Transcriber, TranscriptionResponse};
pub use config::AppConfig;

// --- Local pipeline re-exports (Windows only) ---
#[cfg(target_os = "windows")]
pub use audio::{AudioCapture, AudioPlayback, Resampler};
#[cfg(target_os = "windows")]
pub use bootstrap::{init_sherpa, run_in_dir as bootstrap_run_in_dir};
#[cfg(target_os = "windows")]
pub use sherpa::is_initialized as sherpa_is_initialized;
#[cfg(target_os = "windows")]
pub use punct::PunctEngine;
#[cfg(target_os = "windows")]
pub use speaker::{SpeakerEngine, SpeakerManager};
#[cfg(target_os = "windows")]
pub use speaker::cosine_similarity;
#[cfg(target_os = "windows")]
pub use stt::SttEngine;
#[cfg(target_os = "windows")]
pub use tts::TtsEngine;
#[cfg(target_os = "windows")]
pub use vad::{SpeechSegment, VadEngine};
#[cfg(target_os = "windows")]
pub use voice_detect::{RmsVoiceDetector, SileroVoiceDetector, VoiceDetector, create_detector};

// --- Progress (Windows only) ---
#[cfg(target_os = "windows")]
pub use model::set_progress;
