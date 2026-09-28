//! chunk_source — realtime P1（W7 测试注入面）
//!
//! 把「音频从哪来」从 STT 管线里抽出来：生产 = 麦克风（[`MicSource`]），
//! 测试 = WAV 文件逐块喂入（[`WavSource`]）。管线只认 [`AudioChunkSource`]，
//! 输出统一为**目标采样率**的单声道 f32 块（重采样在 source 内部完成）。

use anyhow::Context;
use anyhow::Result;

/// 采集源抽象：非阻塞取一块**已重采样到目标采样率**的单声道样本。
/// `None` = 暂无数据（管线侧自行 sleep 重试，与既有 try_receive 轮询节奏一致）。
/// **无 Send 约束**：cpal Stream（AudioCapture 内部）是 !Send——源在管线线程内
/// 构造并使用，从不过线程边界（spawn_blocking 闭包只搬运设备名字符串）。
pub trait AudioChunkSource {
    fn try_chunk(&mut self) -> Option<Vec<f32>>;
}

/// 麦克风采集源：AudioCapture 原始块 → Resampler 到目标采样率。
/// 真实现随 `voice-capture` feature；无设备 IO 构建下 `new` 诚实报错
/// （与 audio_stub 同语义：消费者在装配点同步拿到 Err）。
pub struct MicSource {
    capture: crate::AudioCapture,
    resampler: crate::Resampler,
}

impl MicSource {
    pub fn new(device: &str, target_sr: u32) -> Result<Self> {
        let capture = crate::AudioCapture::new(device).context("Audio capture init failed")?;
        let resampler = crate::Resampler::new(capture.sample_rate, target_sr)?;
        Ok(Self { capture, resampler })
    }
}

impl AudioChunkSource for MicSource {
    fn try_chunk(&mut self) -> Option<Vec<f32>> {
        self.capture
            .try_receive()
            .map(|c| self.resampler.resample(&c))
    }
}

/// WAV 文件源（测试注入面）：整文件读入内存，按 `chunk_samples` 切块、
/// 重采样到目标采样率后逐块吐出。
pub struct WavSource {
    samples: Vec<f32>,
    pos: usize,
    chunk_samples: usize,
    resampler: crate::Resampler,
}

impl WavSource {
    /// Load a WAV file, targeting `target_sr` output, chunked at `chunk_samples`.
    pub fn from_file(path: &std::path::Path, target_sr: u32, chunk_samples: usize) -> Result<Self> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read WAV file: {}", path.display()))?;
        let (samples, file_sr) = crate::wav::parse_wav(&bytes)?;
        let resampler = crate::Resampler::new(file_sr, target_sr)?;
        Ok(Self {
            samples,
            pos: 0,
            chunk_samples: chunk_samples.max(1),
            resampler,
        })
    }

    /// Remaining raw (pre-resample) sample count — mostly for tests.
    pub fn remaining(&self) -> usize {
        self.samples.len() - self.pos
    }
}

impl AudioChunkSource for WavSource {
    fn try_chunk(&mut self) -> Option<Vec<f32>> {
        if self.pos >= self.samples.len() {
            return None;
        }
        let end = (self.pos + self.chunk_samples).min(self.samples.len());
        let raw = &self.samples[self.pos..end];
        self.pos = end;
        Some(self.resampler.resample(raw))
    }
}

// 重采样行为依赖真实现（stub 的 Resampler::new 诚实 bail），测试随 voice-capture。
#[cfg(all(test, feature = "voice-capture"))]
mod chunk_source_tests;
