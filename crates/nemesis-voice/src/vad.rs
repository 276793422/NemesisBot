//! VAD safe wrapper — Silero via sherpa-onnx

use anyhow::Result;
use std::path::Path;

use crate::sherpa;

pub struct VadEngine {
    vad: *const sherpa::SherpaOnnxVoiceActivityDetector,
}

unsafe impl Send for VadEngine {}
unsafe impl Sync for VadEngine {}

impl VadEngine {
    pub fn new(
        model_path: &Path,
        threshold: f32,
        min_silence_duration: f32,
        min_speech_duration: f32,
        max_speech_duration: f32,
        window_size: u32,
        sample_rate: u32,
    ) -> Result<Self> {
        if !model_path.exists() {
            anyhow::bail!("VAD model not found: {}", model_path.display());
        }

        let model_c = sherpa::to_cstr(model_path.to_str().unwrap_or(""));
        let provider_c = sherpa::to_cstr("cpu");

        // v1.13.2: SileroVad field order: window_size before max_speech_duration
        let silero = sherpa::SherpaOnnxSileroVadModelConfig {
            model: model_c.as_ptr(),
            threshold,
            min_silence_duration,
            min_speech_duration,
            window_size: window_size as libc::c_int,
            max_speech_duration,
        };

        let empty = sherpa::null_cstr();

        // v1.13.2: VadModelConfig includes ten_vad
        let ten_vad = sherpa::SherpaOnnxTenVadModelConfig {
            model: empty,
            threshold: 0.0,
            min_silence_duration: 0.0,
            min_speech_duration: 0.0,
            window_size: 0,
            max_speech_duration: 0.0,
        };

        let config = sherpa::SherpaOnnxVadModelConfig {
            silero_vad: silero,
            sample_rate: sample_rate as libc::c_int,
            num_threads: 1,
            provider: provider_c.as_ptr(),
            debug: 0,
            ten_vad,
        };

        let vad = unsafe { sherpa::SherpaOnnxCreateVoiceActivityDetector(&config, 30.0) };
        if vad.is_null() {
            anyhow::bail!("Failed to create VAD engine");
        }

        Ok(Self { vad })
    }

    pub fn accept_waveform(&self, samples: &[f32]) {
        unsafe {
            sherpa::SherpaOnnxVoiceActivityDetectorAcceptWaveform(
                self.vad,
                samples.as_ptr(),
                samples.len() as libc::c_int,
            );
        }
    }

    pub fn is_speech_detected(&self) -> bool {
        unsafe { sherpa::SherpaOnnxVoiceActivityDetectorDetected(self.vad) != 0 }
    }

    pub fn is_empty(&self) -> bool {
        unsafe { sherpa::SherpaOnnxVoiceActivityDetectorEmpty(self.vad) != 0 }
    }

    pub fn front(&self) -> Option<SpeechSegment> {
        let seg = unsafe { sherpa::SherpaOnnxVoiceActivityDetectorFront(self.vad) };
        if seg.is_null() {
            return None;
        }
        let segment = unsafe { &*seg };
        let start = segment.start as usize;
        let n = segment.n as usize;
        let samples = if !segment.samples.is_null() && n > 0 {
            unsafe { std::slice::from_raw_parts(segment.samples, n) }.to_vec()
        } else {
            Vec::new()
        };
        unsafe { sherpa::SherpaOnnxDestroySpeechSegment(seg) };
        Some(SpeechSegment { start, samples })
    }

    pub fn pop(&self) {
        unsafe { sherpa::SherpaOnnxVoiceActivityDetectorPop(self.vad) };
    }

    pub fn flush(&self) {
        unsafe { sherpa::SherpaOnnxVoiceActivityDetectorFlush(self.vad) };
    }

    pub fn reset(&self) {
        unsafe { sherpa::SherpaOnnxVoiceActivityDetectorReset(self.vad) };
    }
}

pub struct SpeechSegment {
    pub start: usize,
    pub samples: Vec<f32>,
}

impl Drop for VadEngine {
    fn drop(&mut self) {
        unsafe { sherpa::SherpaOnnxDestroyVoiceActivityDetector(self.vad) };
    }
}
