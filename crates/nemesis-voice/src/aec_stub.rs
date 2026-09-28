//! AEC（回声消除）的诚实降级 stub（`voice-capture` feature 未启用时编译）。
//!
//! 真实现在同目录 `aec.rs`（运行时 dlopen aec.dll 的 SpeexDSP 后端）。
//! 本 stub 保持**签名兼容**：`aec::init` / `SpeexAec::new` / `with_defaults`
//! 在运行期诚实报「voice-capture 未编译」，`EchoCanceller` trait 原样导出
//! （nemesis-web voice.rs 顶部 `use nemesis_voice::EchoCanceller` 零改动编译）。

use anyhow::{bail, Result};
use std::path::Path;

/// AEC 工作采样率（与 STT/VAD 目标率一致）。
pub const AEC_SAMPLE_RATE: u32 = 16000;
/// 默认帧大小：16kHz 下 10ms。
pub const DEFAULT_FRAME_SIZE: usize = 160;
/// 默认 filter_length：16kHz 下 ~512ms 回声尾。
pub const DEFAULT_FILTER_LENGTH: i32 = 8192;

pub fn init(_dll_path: &Path) -> Result<()> {
    bail!("voice-capture feature 未编译：AEC 不可用（当前构建不含 cpal/wasapi）")
}

pub fn is_initialized() -> bool {
    false
}

pub trait EchoCanceller: Send {
    /// 处理一段近端(麦克风)样本。`far` 是同一时间窗的远端(播放)参考。
    fn process(&mut self, near: &[f32], far: &[f32]) -> Vec<f32>;
}

/// SpeexDSP AEC 实例（stub 态无实例可构造）。
pub struct SpeexAec {
    _private: (),
}

impl SpeexAec {
    pub fn new(
        _frame_size: usize,
        _filter_length: i32,
        _sample_rate: u32,
        _enable_preprocess: bool,
    ) -> Result<Self> {
        bail!("voice-capture feature 未编译：AEC 不可用（当前构建不含 cpal/wasapi）")
    }

    pub fn with_defaults() -> Result<Self> {
        bail!("voice-capture feature 未编译：AEC 不可用（当前构建不含 cpal/wasapi）")
    }
}

impl EchoCanceller for SpeexAec {
    /// 无实例可构造（`new` 恒 Err），本体不可达；仅为签名兼容保留。
    fn process(&mut self, _near: &[f32], _far: &[f32]) -> Vec<f32> {
        Vec::new()
    }
}
