//! Acoustic Echo Cancellation (AEC) — SpeexDSP backend, runtime-loaded `aec.dll`.
//!
//! ## 为什么需要 AEC
//! STT（cpal 采集）和 TTS（cpal 播放）在同一进程、同一台机器上运行。
//! 不做回声消除时，TTS 播放的声音会被麦克风重新采到并转写，形成反馈环
//! （AI 的话被当成用户输入再次发给 LLM）。AEC 用已知的 TTS 播放信号
//! （far-end 参考）从麦克风信号（near-end）里减掉，使 STT/VAD 只听到
//! 用户——这是全双工对话 + barge-in（用户可随时打断 AI）的前提。
//!
//! ## 选型：SpeexDSP（`thewh1teagle/aec`，MIT）
//! - 预编译共享库，实际分发仅 ~182 KB/平台，**按需下载、运行时 dlopen**，
//!   不编译进二进制（与 sherpa-onnx-c-api.dll 同一套加载模式）。
//! - SpeexDSP 的自适应 MDF 滤波器在 `filter_length` 窗口内自动学习
//!   扬声器→麦克风的回声路径，**无需手动测延迟/校准时序**。
//!
//! ## 备选方案（仅留档，未实现 —— 未来在同 trait 下替换）
//! - **WebRTC AEC3**：质量 SOTA，但需 clang/cmake 编译或 MSYS2 预编译
//!   （与 MSVC 链接有 CRT 坑）。SpeexDSP 在廉价喇叭/强混响场景残留过多时升级用。
//! - **神经 AEC（DTLN）on ONNX**：可复用现有 ONNX Runtime，但模型质量参差、需调参。
//! 两者都实现同一个 [`EchoCanceller`] trait，换后端只动本文件。
//!
//! ## C API（libaec.h，cbindgen 生成）
//! ```text
//! Aec* AecNew(usize frame_size, i32 filter_length, u32 sample_rate, bool preprocess);
//! void AecCancelEcho(Aec*, const i16* rec, const i16* echo, i16* out, usize len);
//! void AecDestroy(Aec*);
//! ```
//! `rec`=近端(麦克风)、`echo`=远端参考(扬声器在放的)、`out`=干净输出；
//! 三段长度都 = `len` = `frame_size`，格式 int16。

use anyhow::Result;
use libloading::Library;
use std::path::Path;
use std::sync::OnceLock;

#[cfg(target_os = "windows")]
use libloading::os::windows as win_lib;

/// AEC 工作采样率（与 STT/VAD 目标率一致）。
pub const AEC_SAMPLE_RATE: u32 = 16000;
/// 默认帧大小：16kHz 下 10ms。必须与传给 `AecNew` 的一致。
pub const DEFAULT_FRAME_SIZE: usize = 160;
/// 默认 filter_length：16kHz 下 ~512ms 回声尾（8192 采样）。覆盖典型房间
/// 混响（RT60 ~300-500ms）+ 设备/采集缓冲延迟。可通过配置调大/调小。
pub const DEFAULT_FILTER_LENGTH: i32 = 8192;

// =============================================================================
// Opaque handle + C 函数签名
// =============================================================================

/// libaec 的不透明 AEC 实例句柄。
#[repr(C)]
pub struct Aec {
    _private: [u8; 0],
}

type AecNewFn = unsafe extern "C" fn(
    frame_size: usize,
    filter_length: i32,
    sample_rate: u32,
    enable_preprocess: bool,
) -> *mut Aec;
type AecCancelEchoFn = unsafe extern "C" fn(
    aec: *mut Aec,
    rec: *const i16,
    echo: *const i16,
    out: *mut i16,
    buffer_length: usize,
);
type AecDestroyFn = unsafe extern "C" fn(aec: *mut Aec);

// =============================================================================
// 运行时动态加载（照搬 sherpa::init 模式）
// =============================================================================

static AEC_LIB: OnceLock<Library> = OnceLock::new();

/// 加载 AEC 共享库（aec.dll / libaec.so）。
///
/// Windows 下用 `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR`，让传递依赖从库所在目录解析。
/// 必须在创建任何 AEC 实例前调用一次。已加载则直接返回 Ok。
pub fn init(dll_path: &Path) -> Result<()> {
    if AEC_LIB.get().is_some() {
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        // 0x100 = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
        // 0x1000 = LOAD_LIBRARY_SEARCH_DEFAULT_DIRS
        const FLAGS: u32 = 0x100 | 0x1000;
        let lib = unsafe { win_lib::Library::load_with_flags(dll_path, FLAGS) }
            .map_err(|e| anyhow::anyhow!("Failed to load AEC lib {}: {}", dll_path.display(), e))?;
        let _ = AEC_LIB.set(lib.into());
    }
    #[cfg(not(target_os = "windows"))]
    {
        let lib = unsafe { Library::new(dll_path) }
            .map_err(|e| anyhow::anyhow!("Failed to load AEC lib {}: {}", dll_path.display(), e))?;
        let _ = AEC_LIB.set(lib);
    }
    Ok(())
}

/// AEC 库是否已加载。
pub fn is_initialized() -> bool {
    AEC_LIB.get().is_some()
}

/// 从已加载的库中取符号。库未初始化或符号缺失时 panic（与 sherpa 一致）。
unsafe fn symbol<F>(name: &[u8]) -> libloading::Symbol<'static, F> {
    let lib = AEC_LIB.get().unwrap_or_else(|| {
        panic!(
            "AEC lib not initialized (looking for {})",
            String::from_utf8_lossy(name)
        )
    });
    // Library 装在 OnceLock 里、永不释放，故把 Symbol 生命周期 transmute 到 'static 是安全的。
    let sym: libloading::Symbol<'_, F> = unsafe {
        lib.get(name).unwrap_or_else(|e| {
            panic!(
                "AEC symbol {} not found: {}",
                String::from_utf8_lossy(name),
                e
            )
        })
    };
    unsafe { std::mem::transmute(sym) }
}

// =============================================================================
// 可插拔后端 trait
// =============================================================================

/// AEC 后端接口。`near`/`far`/返回值均为 16kHz 单声道 f32。
///
/// 实现需处理变长输入块 → 固定 frame_size 帧的攒帧、以及 near/far 长度不一致。
pub trait EchoCanceller: Send {
    /// 处理一段近端(麦克风)样本。`far` 是同一时间窗的远端(播放)参考，
    /// 没在播放时可为空或较短（实现按静音补齐）。返回去回声后的样本，
    /// 输出样本数总体跟随近端输入。
    fn process(&mut self, near: &[f32], far: &[f32]) -> Vec<f32>;
}

// =============================================================================
// SpeexDSP 实现
// =============================================================================

/// SpeexDSP AEC 实例。调用前必须先 [`init`]。
pub struct SpeexAec {
    handle: *mut Aec,
    frame_size: usize,
    // 变长输入块攒成固定 frame_size 帧
    near_buf: Vec<f32>,
    far_buf: Vec<f32>,
    // 已处理、待返回的输出
    out_buf: Vec<f32>,
    // 复用的 s16 scratch，避免每帧分配
    rec_s16: Vec<i16>,
    echo_s16: Vec<i16>,
    out_s16: Vec<i16>,
}

// 句柄由单一 STT pipeline 线程独占使用；跨线程不共享。
unsafe impl Send for SpeexAec {}

impl SpeexAec {
    /// 创建实例。`frame_size` 必须与 `filter_length` 匹配（filter_length 一般是
    /// frame_size 的整数倍，且覆盖目标回声尾）。
    pub fn new(
        frame_size: usize,
        filter_length: i32,
        sample_rate: u32,
        enable_preprocess: bool,
    ) -> Result<Self> {
        let new_fn: AecNewFn = unsafe { *symbol(b"AecNew\0") };
        let handle = unsafe { new_fn(frame_size, filter_length, sample_rate, enable_preprocess) };
        if handle.is_null() {
            anyhow::bail!(
                "AecNew returned null (frame_size={frame_size}, filter_length={filter_length}, sample_rate={sample_rate})"
            );
        }
        Ok(Self {
            handle,
            frame_size,
            near_buf: Vec::with_capacity(frame_size * 2),
            far_buf: Vec::with_capacity(frame_size * 2),
            out_buf: Vec::new(),
            rec_s16: Vec::with_capacity(frame_size),
            echo_s16: Vec::with_capacity(frame_size),
            out_s16: vec![0i16; frame_size],
        })
    }

    /// 用项目默认参数创建（16kHz / 10ms 帧 / 512ms 尾 / 开预处理降噪）。
    pub fn with_defaults() -> Result<Self> {
        Self::new(
            DEFAULT_FRAME_SIZE,
            DEFAULT_FILTER_LENGTH,
            AEC_SAMPLE_RATE,
            true,
        )
    }

    /// 攒够一帧 near 时调一次 C 的 `AecCancelEcho`。
    fn process_one_frame(&mut self) {
        debug_assert!(self.near_buf.len() >= self.frame_size);

        // near → s16
        self.rec_s16.clear();
        self.rec_s16.extend(
            self.near_buf[..self.frame_size]
                .iter()
                .map(|&s| f32_to_s16(s)),
        );

        // far → s16；不够一帧则取已有部分 + 静音补齐（表示当前没在播放，无回声可消）
        self.echo_s16.clear();
        if self.far_buf.len() >= self.frame_size {
            self.echo_s16.extend(
                self.far_buf[..self.frame_size]
                    .iter()
                    .map(|&s| f32_to_s16(s)),
            );
            self.far_buf.drain(..self.frame_size);
        } else {
            self.echo_s16
                .extend(self.far_buf.drain(..).map(|s| f32_to_s16(s)));
            self.echo_s16.resize(self.frame_size, 0);
        }
        self.near_buf.drain(..self.frame_size);

        // 调用 C
        let cancel_fn: AecCancelEchoFn = unsafe { *symbol(b"AecCancelEcho\0") };
        unsafe {
            cancel_fn(
                self.handle,
                self.rec_s16.as_ptr(),
                self.echo_s16.as_ptr(),
                self.out_s16.as_mut_ptr(),
                self.frame_size,
            );
        }

        // out → f32
        self.out_buf
            .extend(self.out_s16.iter().map(|&s| s16_to_f32(s)));
    }
}

impl EchoCanceller for SpeexAec {
    fn process(&mut self, near: &[f32], far: &[f32]) -> Vec<f32> {
        self.near_buf.extend_from_slice(near);
        self.far_buf.extend_from_slice(far);

        while self.near_buf.len() >= self.frame_size {
            self.process_one_frame();
        }

        // far 长期多于 near（播放缓冲堆积）会堆积延迟，裁掉超额部分。
        // 10 帧 ≈ 100ms 的容忍窗，超出说明 near/far 流速不匹配，丢弃旧的。
        let far_cap = self.frame_size * 10;
        if self.far_buf.len() > far_cap {
            let excess = self.far_buf.len() - far_cap;
            self.far_buf.drain(..excess);
        }

        std::mem::take(&mut self.out_buf)
    }
}

impl Drop for SpeexAec {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            let destroy_fn: AecDestroyFn = unsafe { *symbol(b"AecDestroy\0") };
            unsafe { destroy_fn(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

// =============================================================================
// 采样格式转换
// =============================================================================

#[inline]
fn f32_to_s16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

#[inline]
fn s16_to_f32(s: i16) -> f32 {
    s as f32 / 32768.0
}

#[cfg(test)]
mod tests;
