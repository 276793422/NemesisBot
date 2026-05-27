//! Voice activity detection — pluggable interface
//!
//! Two implementations:
//! - `RmsVoiceDetector`: energy-based, simple and reliable
//! - `SileroVoiceDetector`: neural network-based (sherpa-onnx Silero VAD)
//!
//! Use `create_detector()` to get the best available.
//! Every 3 seconds, Silero prints diagnostics to stderr for debugging.

use std::time::Instant;
use anyhow::Result;

// =============================================================================
// Trait
// =============================================================================

/// Common interface for voice activity detection.
pub trait VoiceDetector {
    /// Feed an audio chunk. Returns `Some(audio)` when a complete utterance is ready.
    fn process(&mut self, chunk: &[f32], sample_rate: u32) -> Option<Vec<f32>>;
    /// Flush any remaining buffered audio (for timeout / exit).
    fn flush(&mut self) -> Option<Vec<f32>>;
    /// Whether speech is currently being detected.
    fn is_speaking(&self) -> bool;
    /// Detector name for logging.
    fn name(&self) -> &str;
}

// =============================================================================
// RMS Energy detector
// =============================================================================

pub struct RmsVoiceDetector {
    energy_threshold: f32,
    silence_ms: u64,
    max_speech_ms: u64,
    is_speaking: bool,
    silence_start: Option<Instant>,
    speech_start: Option<Instant>,
    buffer: Vec<f32>,
}

impl RmsVoiceDetector {
    pub fn new(energy_threshold: f32, silence_ms: u64, max_speech_ms: u64) -> Self {
        Self {
            energy_threshold,
            silence_ms,
            max_speech_ms,
            is_speaking: false,
            silence_start: None,
            speech_start: None,
            buffer: Vec::new(),
        }
    }
}

impl VoiceDetector for RmsVoiceDetector {
    fn process(&mut self, chunk: &[f32], sample_rate: u32) -> Option<Vec<f32>> {
        let rms = if chunk.is_empty() {
            0.0
        } else {
            (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt()
        };

        if rms >= self.energy_threshold {
            self.buffer.extend_from_slice(chunk);
            if !self.is_speaking {
                self.is_speaking = true;
                self.speech_start = Some(Instant::now());
            }
            self.silence_start = None;
        } else if self.is_speaking {
            self.buffer.extend_from_slice(chunk);
            if self.silence_start.is_none() {
                self.silence_start = Some(Instant::now());
            }
        }

        let min_samples = (sample_rate as f64 * 0.3) as usize;
        let buffer_ms = self.buffer.len() as f64 / sample_rate as f64 * 1000.0;
        let silence_elapsed = self.silence_start.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
        let speech_elapsed = self.speech_start.map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);

        let ready = self.is_speaking
            && ((silence_elapsed >= self.silence_ms && buffer_ms >= 300.0)
                || speech_elapsed >= self.max_speech_ms);

        if ready && self.buffer.len() >= min_samples {
            let audio = std::mem::take(&mut self.buffer);
            self.is_speaking = false;
            self.silence_start = None;
            self.speech_start = None;
            Some(audio)
        } else {
            None
        }
    }

    fn flush(&mut self) -> Option<Vec<f32>> {
        if !self.buffer.is_empty() {
            let audio = std::mem::take(&mut self.buffer);
            self.is_speaking = false;
            self.silence_start = None;
            self.speech_start = None;
            Some(audio)
        } else {
            None
        }
    }

    fn is_speaking(&self) -> bool {
        self.is_speaking
    }

    fn name(&self) -> &str {
        "RMS"
    }
}

// =============================================================================
// Silero VAD detector
// =============================================================================

pub struct SileroVoiceDetector {
    engine: crate::vad::VadEngine,
    window_size: usize,
    /// Internal buffer to accumulate until we have exactly window_size samples.
    chunk_buffer: Vec<f32>,
    /// Completed speech segments waiting to be returned.
    pending_segments: Vec<Vec<f32>>,
    is_speaking: bool,
    // Diagnostics
    feed_count: usize,
    window_count: usize,
    detect_count: usize,
    last_diag: Instant,
}

/// Parameters for creating a Silero VAD detector.
pub struct SileroVadParams {
    pub model_path: std::path::PathBuf,
    pub threshold: f32,
    pub min_silence_duration: f32,
    pub min_speech_duration: f32,
    pub max_speech_duration: f32,
    pub window_size: u32,
    pub sample_rate: u32,
}

impl SileroVoiceDetector {
    pub fn new(params: &SileroVadParams) -> Result<Self> {
        let engine = crate::vad::VadEngine::new(
            &params.model_path,
            params.threshold,
            params.min_silence_duration,
            params.min_speech_duration,
            params.max_speech_duration,
            params.window_size,
            params.sample_rate,
        )?;

        Ok(Self {
            engine,
            window_size: params.window_size as usize,
            chunk_buffer: Vec::with_capacity(params.window_size as usize * 2),
            pending_segments: Vec::new(),
            is_speaking: false,
            feed_count: 0,
            window_count: 0,
            detect_count: 0,
            last_diag: Instant::now(),
        })
    }
}

impl VoiceDetector for SileroVoiceDetector {
    fn process(&mut self, chunk: &[f32], sample_rate: u32) -> Option<Vec<f32>> {
        // Return any previously buffered segment first
        if let Some(audio) = self.next_pending(sample_rate) {
            return Some(audio);
        }

        self.chunk_buffer.extend_from_slice(chunk);
        self.feed_count += 1;

        // Feed audio in exact window_size chunks — matching official C API pattern:
        // AcceptWaveform(window_size samples) → check !Empty() → Front() → Pop()
        while self.chunk_buffer.len() >= self.window_size {
            let window: Vec<f32> = self.chunk_buffer.drain(..self.window_size).collect();
            self.engine.accept_waveform(&window);
            self.window_count += 1;

            // Drain completed segments using !is_empty() (NOT is_speech_detected).
            // is_speech_detected() checks real-time voice status;
            // is_empty() checks if completed segments are in the queue.
            while !self.engine.is_empty() {
                self.detect_count += 1;
                if let Some(segment) = self.engine.front() {
                    if !segment.samples.is_empty() {
                        self.pending_segments.push(segment.samples);
                    }
                }
                self.engine.pop();
            }
        }

        // Update real-time speaking status from Detected()
        self.is_speaking = self.engine.is_speech_detected();

        // Print diagnostics every 3 seconds
        if self.last_diag.elapsed().as_secs() >= 3 {
            let rms = if !chunk.is_empty() {
                (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt()
            } else {
                0.0
            };
            tracing::debug!(
                "[Silero] feeds={} windows={} detects={} speaking={} pending={} chunk_buf={} rms={:.4}",
                self.feed_count, self.window_count, self.detect_count,
                self.is_speaking, self.pending_segments.len(),
                self.chunk_buffer.len(), rms,
            );
            self.last_diag = Instant::now();
        }

        self.next_pending(sample_rate)
    }

    fn flush(&mut self) -> Option<Vec<f32>> {
        // Feed any remaining partial window (pad with silence)
        if !self.chunk_buffer.is_empty() {
            while self.chunk_buffer.len() < self.window_size {
                self.chunk_buffer.push(0.0);
            }
            let window: Vec<f32> = self.chunk_buffer.drain(..self.window_size).collect();
            self.engine.accept_waveform(&window);
        }

        // Flush to force processing of remaining buffered audio
        self.engine.flush();

        // Drain final segments
        while !self.engine.is_empty() {
            self.detect_count += 1;
            if let Some(segment) = self.engine.front() {
                if !segment.samples.is_empty() {
                    self.pending_segments.push(segment.samples);
                }
            }
            self.engine.pop();
        }

        // Return first pending, rest are lost (flush is for exit)
        self.pending_segments.drain(..).next()
    }

    fn is_speaking(&self) -> bool {
        self.is_speaking
    }

    fn name(&self) -> &str {
        "Silero VAD"
    }
}

impl SileroVoiceDetector {
    /// Return the next pending segment that meets minimum length.
    fn next_pending(&mut self, sample_rate: u32) -> Option<Vec<f32>> {
        let min_samples = (sample_rate as f64 * 0.3) as usize;
        while !self.pending_segments.is_empty() {
            let segment = self.pending_segments.remove(0);
            if segment.len() >= min_samples {
                return Some(segment);
            }
            // Skip too-short segments
        }
        None
    }
}

// =============================================================================
// Factory
// =============================================================================

/// Create the best available voice detector.
/// Tries Silero VAD first, falls back to RMS energy.
pub fn create_detector(cfg: &crate::config::AppConfig) -> Box<dyn VoiceDetector> {
    let silero_result = (|| -> Result<SileroVoiceDetector> {
        let vad_path = crate::model::ensure_vad_model(cfg)?;
        SileroVoiceDetector::new(&SileroVadParams {
            model_path: vad_path,
            threshold: cfg.vad.threshold,
            min_silence_duration: cfg.vad.min_silence_duration,
            min_speech_duration: cfg.vad.min_speech_duration,
            max_speech_duration: cfg.vad.max_speech_duration,
            window_size: cfg.vad.window_size,
            sample_rate: cfg.audio.target_sample_rate,
        })
    })();

    match silero_result {
        Ok(detector) => {
            tracing::info!("[Voice] Using Silero VAD");
            Box::new(detector)
        }
        Err(e) => {
            tracing::warn!("[Voice] Silero unavailable ({}) — using RMS energy", e);
            Box::new(RmsVoiceDetector::new(cfg.audio.energy_threshold, 800, 15_000))
        }
    }
}
