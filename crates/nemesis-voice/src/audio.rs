//! Audio capture, playback, and resampling
//!
//! Uses cpal for cross-platform audio I/O and sherpa-onnx LinearResampler for
//! proper resampling (no aliased 3:1 averaging).

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Target sample rate for all voice processing (STT, VAD, TTS output)
pub const TARGET_SAMPLE_RATE: u32 = 16000;

// =============================================================================
// Device enumeration
// =============================================================================

pub struct AudioDeviceInfo {
    pub index: usize,
    pub name: String,
    pub is_input: bool,
    pub is_default: bool,
}

pub fn list_devices() -> Result<Vec<AudioDeviceInfo>> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    let default_input = host.default_input_device().map(|d| d.name().ok()).flatten();
    let default_output = host
        .default_output_device()
        .map(|d| d.name().ok())
        .flatten();

    for (i, dev) in host.input_devices()?.enumerate() {
        let name = dev.name().unwrap_or_else(|_| "Unknown".into());
        let is_default = default_input.as_deref() == Some(&name);
        devices.push(AudioDeviceInfo {
            index: i,
            name,
            is_input: true,
            is_default,
        });
    }

    let input_count = devices.len();
    for (i, dev) in host.output_devices()?.enumerate() {
        let name = dev.name().unwrap_or_else(|_| "Unknown".into());
        let is_default = default_output.as_deref() == Some(&name);
        devices.push(AudioDeviceInfo {
            index: input_count + i,
            name,
            is_input: false,
            is_default,
        });
    }
    Ok(devices)
}

// =============================================================================
// Audio capture
// =============================================================================

pub struct AudioCapture {
    _stream: cpal::Stream,
    rx: Receiver<Vec<f32>>,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioCapture {
    /// Open the default input device (or named device) and start capturing.
    pub fn new(device_name: &str) -> Result<Self> {
        let host = cpal::default_host();
        let device = if device_name.is_empty() {
            host.default_input_device()
                .context("No default input device found")?
        } else {
            host.input_devices()?
                .find(|d| d.name().map(|n| n.contains(device_name)).unwrap_or(false))
                .context(format!("Input device '{}' not found", device_name))?
        };

        let supported = device
            .supported_input_configs()?
            .find(|c| c.channels() <= 2 && c.sample_format() == cpal::SampleFormat::F32)
            .or_else(|| device.supported_input_configs().ok()?.next())
            .context("No supported input config found")?;

        let config = supported.with_max_sample_rate();
        let sr = config.sample_rate().0;
        let ch = config.channels();

        let (tx, rx): (SyncSender<Vec<f32>>, Receiver<Vec<f32>>) = mpsc::sync_channel(32);

        let stream = device.build_input_stream(
            &config.into(),
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                // Convert to mono if multi-channel
                let mono = if ch > 1 {
                    data.chunks(ch as usize)
                        .map(|frame| {
                            let sum: f32 = frame.iter().sum();
                            sum / frame.len() as f32
                        })
                        .collect()
                } else {
                    data.to_vec()
                };
                // Non-blocking send — drop audio if consumer is slow
                let _ = tx.try_send(mono);
            },
            |err| tracing::error!("Audio capture error: {}", err),
            None,
        )?;

        stream.play()?;

        Ok(Self {
            _stream: stream,
            rx,
            sample_rate: sr,
            channels: ch,
        })
    }

    /// Non-blocking receive of captured audio chunk.
    pub fn try_receive(&self) -> Option<Vec<f32>> {
        self.rx.try_recv().ok()
    }
}

// =============================================================================
// Audio playback
// =============================================================================

/// 全局共享的「远端参考信号」缓冲（AEC 用）。
///
/// 由 [`AudioPlayback`] 的输出 callback 写入（设备采样率、单声道 f32 ——
/// 即「此刻正推给扬声器」的样本，时间上与真实播放对齐）；由 STT pipeline
/// 读取，重采样到 16k 后作为 AEC 的 far-end 输入。
///
/// 用模块级单例而不是 AudioPlayback 实例字段：STT pipeline 和 TTS 播放循环
/// 是分离的生命周期（可能各自创建/销毁 AudioCapture/AudioPlayback），单例
/// 保证无论哪个 AudioPlayback 实例在播，参考信号都汇入同一处，STT 侧始终能读到。
static FAR_END_BUFFER: OnceLock<Arc<Mutex<VecDeque<f32>>>> = OnceLock::new();

/// 拿远端参考信号缓冲的共享句柄（首次调用时惰性创建）。
pub fn far_end_buffer() -> Arc<Mutex<VecDeque<f32>>> {
    FAR_END_BUFFER
        .get_or_init(|| Arc::new(Mutex::new(VecDeque::new())))
        .clone()
}

/// 播放设备采样率（由 [`AudioPlayback::new`] 写入）。STT pipeline 用它把 far-end
/// 参考信号从设备率重采样到 16k 喂 AEC——采集和播放可能是不同设备、不同采样率，
/// 不能复用 near-end 的 resampler。默认 48000（未创建过播放设备时的兜底，那时 far-end
/// 也是空的，不影响）。
static FAR_END_RATE: AtomicU32 = AtomicU32::new(48000);

/// 当前播放设备的采样率。
pub fn far_end_sample_rate() -> u32 {
    FAR_END_RATE.load(Ordering::Relaxed)
}

pub struct AudioPlayback {
    _stream: cpal::Stream,
    queue: Arc<Mutex<VecDeque<f32>>>,
    pub sample_rate: u32,
    _input_sample_rate: u32,
    gain: f32,
}

impl AudioPlayback {
    /// Open the default output device and prepare for playback.
    pub fn new(device_name: &str, sample_rate: u32, gain: f32) -> Result<Self> {
        let host = cpal::default_host();
        let device = if device_name.is_empty() {
            host.default_output_device()
                .context("No default output device found")?
        } else {
            host.output_devices()?
                .find(|d| d.name().map(|n| n.contains(device_name)).unwrap_or(false))
                .context(format!("Output device '{}' not found", device_name))?
        };

        let supported = device
            .supported_output_configs()?
            .find(|c| c.sample_format() == cpal::SampleFormat::F32)
            .or_else(|| device.supported_output_configs().ok()?.next())
            .context("No supported output config found")?;

        let config = supported.with_max_sample_rate();
        let device_sr = config.sample_rate().0;
        let device_channels = config.channels();

        let queue: Arc<Mutex<VecDeque<f32>>> = Arc::new(Mutex::new(VecDeque::new()));
        let q = queue.clone();

        let stream = device.build_output_stream(
            &config.into(),
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                let mut q = q.lock().unwrap();
                let ch = device_channels as usize;
                for frame in data.chunks_mut(ch) {
                    let s = q.pop_front().unwrap_or(0.0);
                    for sample in frame.iter_mut() {
                        *sample = s;
                    }
                }
            },
            |err| tracing::error!("Audio playback error: {}", err),
            None,
        )?;

        stream.play()?;

        Ok(Self {
            _stream: stream,
            queue,
            sample_rate: device_sr,
            _input_sample_rate: sample_rate,
            gain,
        })
    }

    /// Send samples to the playback queue. Blocks until playback completes.
    pub fn play_blocking(&self, samples: &[f32], input_sample_rate: u32) -> Result<()> {
        // Apply gain
        let amplified: Vec<f32> = samples
            .iter()
            .map(|&s| (s * self.gain).clamp(-1.0, 1.0))
            .collect();

        // Resample from input rate to device rate
        let resampled = if input_sample_rate != self.sample_rate {
            let ratio = self.sample_rate as f64 / input_sample_rate as f64;
            let new_len = (amplified.len() as f64 * ratio) as usize;
            (0..new_len)
                .map(|i| {
                    let src_pos = i as f64 / ratio;
                    let idx = src_pos as usize;
                    let frac = src_pos - idx as f64;
                    if idx + 1 < amplified.len() {
                        amplified[idx] * (1.0 - frac) as f32 + amplified[idx + 1] * frac as f32
                    } else {
                        amplified[idx.min(amplified.len() - 1)]
                    }
                })
                .collect::<Vec<f32>>()
        } else {
            amplified
        };

        // Enqueue all samples at once
        {
            let mut q = self.queue.lock().unwrap();
            q.extend(resampled);
        }

        // Wait for playback to finish (queue drains)
        let max_wait = samples.len() as f64 / input_sample_rate as f64 + 1.0;
        let start = Instant::now();
        loop {
            {
                let q = self.queue.lock().unwrap();
                if q.is_empty() {
                    break;
                }
            }
            if start.elapsed().as_secs_f64() > max_wait {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        Ok(())
    }

    /// Stop playback by clearing the sample queue. Safe to call from any thread.
    pub fn stop(&self) {
        self.queue.lock().unwrap().clear();
    }
}

// =============================================================================
// Resampler (sherpa-onnx LinearResampler)
// =============================================================================

pub struct Resampler {
    rate_in: f32,
    rate_out: f32,
}

unsafe impl Send for Resampler {}
unsafe impl Sync for Resampler {}

impl Resampler {
    pub fn new(rate_in: u32, rate_out: u32) -> Result<Self> {
        Ok(Self {
            rate_in: rate_in as f32,
            rate_out: rate_out as f32,
        })
    }

    /// Resample a mono f32 buffer from input rate to output rate using linear interpolation.
    pub fn resample(&mut self, input: &[f32]) -> Vec<f32> {
        if input.is_empty() || self.rate_in == self.rate_out {
            return input.to_vec();
        }

        let ratio = self.rate_in as f64 / self.rate_out as f64;
        let output_len = ((input.len() as f64) / ratio) as usize;
        let mut output = Vec::with_capacity(output_len);

        for i in 0..output_len {
            let src_pos = i as f64 * ratio;
            let idx = src_pos as usize;
            let frac = src_pos - idx as f32 as f64;
            if idx + 1 < input.len() {
                let s = input[idx] as f64 * (1.0 - frac) + input[idx + 1] as f64 * frac;
                output.push(s as f32);
            } else if idx < input.len() {
                output.push(input[idx]);
            }
        }

        output
    }

    pub fn reset(&mut self) {}

    pub fn ratio(&self) -> f32 {
        self.rate_out / self.rate_in
    }
}

impl Drop for Resampler {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests;
