//! audio 设备 IO 的诚实降级 stub（`voice-capture` feature 未启用时编译）。
//!
//! 真实现在同目录 `audio.rs`（cpal 驱动，含设备枚举/采集/播放/重采样）。
//! 本 stub 保持**签名兼容**：nemesis-web voice handler 与 `nemesisbot voice`
//! 命令零改动编译；任何构造/枚举调用在运行期拿到明确的「voice-capture 未编译」
//! 错误——诚实失败，非静默空转。STT/TTS/VAD/标点/声纹（缓冲区与文件驱动，
//! 不经过本模块）不受影响。

use anyhow::{Result, bail};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Target sample rate for all voice processing (STT, VAD, TTS output)
pub const TARGET_SAMPLE_RATE: u32 = 16000;

pub struct AudioDeviceInfo {
    pub index: usize,
    pub name: String,
    pub is_input: bool,
    pub is_default: bool,
}

pub fn list_devices() -> Result<Vec<AudioDeviceInfo>> {
    bail!("voice-capture feature 未编译：音频设备枚举不可用（当前构建不含 cpal/wasapi）")
}

pub struct AudioCapture {
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioCapture {
    pub fn new(_device_name: &str) -> Result<Self> {
        bail!("voice-capture feature 未编译：麦克风采集不可用（当前构建不含 cpal/wasapi）")
    }

    /// 无实例可构造（`new` 恒 Err），本体不可达；仅为签名兼容保留。
    pub fn try_receive(&self) -> Option<Vec<f32>> {
        None
    }
}

/// 无播放设备写入（真实现由 [`AudioPlayback::new`] 更新），恒为空缓冲。
pub fn far_end_buffer() -> Arc<Mutex<VecDeque<f32>>> {
    Arc::new(Mutex::new(VecDeque::new()))
}

/// 真实现默认 48000（无播放设备时的兜底值），stub 保持同值。
pub fn far_end_sample_rate() -> u32 {
    48000
}

pub struct AudioPlayback {
    pub sample_rate: u32,
}

impl AudioPlayback {
    pub fn new(_device_name: &str, _sample_rate: u32, _gain: f32) -> Result<Self> {
        bail!("voice-capture feature 未编译：扬声器播放不可用（当前构建不含 cpal/wasapi）")
    }

    /// 无实例可构造（`new` 恒 Err），本体不可达；仅为签名兼容保留。
    pub fn play_blocking(&self, _samples: &[f32], _input_sample_rate: u32) -> Result<()> {
        bail!("voice-capture feature 未编译：扬声器播放不可用")
    }

    /// 无实例可构造（`new` 恒 Err），本体不可达；仅为签名兼容保留。
    pub fn stop(&self) {}
}

pub struct Resampler {
    _rate_in: u32,
    _rate_out: u32,
}

impl Resampler {
    pub fn new(_rate_in: u32, _rate_out: u32) -> Result<Self> {
        // 重采样本身是纯数学，但其全部消费方都在设备采集路径上
        //（先有 AudioCapture 才有数据可重采样），采集不可用则本类型无真实用途，
        // 诚实拒绝而非提供假重采样。
        bail!("voice-capture feature 未编译：音频重采样随设备采集路径不可用")
    }

    /// 无实例可构造（`new` 恒 Err），本体不可达；仅为签名兼容保留。
    pub fn resample(&mut self, input: &[f32]) -> Vec<f32> {
        input.to_vec()
    }

    pub fn reset(&mut self) {}

    pub fn ratio(&self) -> f32 {
        1.0
    }
}
