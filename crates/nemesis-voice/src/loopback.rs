//! WASAPI loopback 采集 —— 抓"系统播放混音"作为 AEC 的 far-end 参考。
//!
//! 在默认输出设备（render endpoint）上以 `Direction::Capture` + shared mode 初始化
//! AudioClient，wasapi crate 会自动设 `AUDCLNT_STREAMFLAGS_LOOPBACK`（见其 api.rs
//! `initialize_client`：device=Render + 方向=Capture + Shared ⇒ LOOPBACK）。
//!
//! 采到的是"正在播放的全部声音"（bot TTS + RustDesk + 音乐 + 视频），降混成 mono
//! 灌进全局 [`crate::far_end_buffer`]。pipeline 里 AEC 用它抵消麦克里混入的播放声
//! ——不管播放声是声学漏进麦、还是驱动层 bleed 进麦，只要是输出的延迟副本就能减掉。
//!
//! 这是 Windows-only，且独立于 cpal 的麦克风采集（第二条采集流）。

use crate::far_end_buffer;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// loopback 线程的停止标志；None = 未启动。
fn loopback_slot() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static SLOT: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 启动 loopback 采集线程（幂等：已在跑则直接返回）。
/// 失败只记日志、不抛——此时 far-end 为空，AEC 等效直通（不抵消），但不影响 STT。
pub fn start_loopback() {
    {
        let slot = loopback_slot().lock().unwrap();
        if let Some(ref flag) = *slot {
            if !flag.load(Ordering::SeqCst) {
                tracing::debug!("[AEC Loopback] already running");
                return;
            }
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    {
        let mut slot = loopback_slot().lock().unwrap();
        *slot = Some(stop.clone());
    }
    if std::thread::Builder::new()
        .name("voice-loopback".into())
        .spawn(move || run_loopback(stop))
        .is_err()
    {
        tracing::warn!("[AEC Loopback] failed to spawn capture thread");
    }
}

/// 停止 loopback 采集线程（标志置位，线程下一轮退出）。
pub fn stop_loopback() {
    if let Some(ref flag) = *loopback_slot().lock().unwrap() {
        flag.store(true, Ordering::SeqCst);
    }
}

fn run_loopback(stop: Arc<AtomicBool>) {
    match run_loopback_inner(&stop) {
        Ok(()) => {}
        Err(e) => tracing::warn!("[AEC Loopback] capture ended with error: {}", e),
    }
    // 线程退出后清掉自己，方便 start_loopback 重新拉起
    let mut slot = loopback_slot().lock().unwrap();
    *slot = None;
}

fn run_loopback_inner(stop: &Arc<AtomicBool>) -> Result<(), String> {
    use wasapi::*;

    initialize_mta()
        .ok()
        .map_err(|e| format!("initialize_mta: {}", e))?;

    let device = get_default_device(&Direction::Render)
        .map_err(|e| format!("get_default_device(Render): {}", e))?;
    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| format!("get_iaudioclient: {}", e))?;

    // 请求 f32 / 48k / 立体声（autoconvert 让 WASAPI 转成这个格式给我）
    let format = WaveFormat::new(32, 32, &SampleType::Float, 48000, 2, None);
    let blockalign = format.get_blockalign() as usize; // bytes/frame，f32 立体声 = 8
    let channels = blockalign / 4; // f32 = 4 bytes/sample

    let (_def_time, min_time) = audio_client
        .get_device_period()
        .map_err(|e| format!("get_device_period: {}", e))?;
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: min_time,
    };
    // 关键：device 是 Render，这里传 Capture ⇒ wasapi 自动设 LOOPBACK flag
    audio_client
        .initialize_client(&format, &Direction::Capture, &mode)
        .map_err(|e| format!("initialize_client(loopback): {}", e))?;
    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| format!("set_get_eventhandle: {}", e))?;
    let capture_client = audio_client
        .get_audiocaptureclient()
        .map_err(|e| format!("get_audiocaptureclient: {}", e))?;
    audio_client
        .start_stream()
        .map_err(|e| format!("start_stream: {}", e))?;

    // loopback 跑在 48k（我们请求的），FAR_END_RATE 默认也是 48000，
    // far_resampler 用 far_end_sample_rate()=48000 → 16k，一致。

    let far_end = far_end_buffer();
    tracing::info!(
        "[AEC Loopback] capturing system mix (f32/48k/{}ch → mono) → far_end_buffer",
        channels
    );

    let mut iter: u64 = 0;
    while !stop.load(Ordering::SeqCst) {
        // 等数据就绪（100ms 超时，便于周期性查 stop）
        if h_event.wait_for_event(100).is_err() {
            tracing::warn!("[AEC Loopback] event wait timeout/error, stopping");
            break;
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }

        let mut byte_queue: VecDeque<u8> = VecDeque::new();
        capture_client
            .read_from_device_to_deque(&mut byte_queue)
            .map_err(|e| format!("read_from_device_to_deque: {}", e))?;

        // bytes → f32 帧 → mono
        let bytes: Vec<u8> = byte_queue.drain(..).collect();
        let n_frames = bytes.len() / blockalign;
        if n_frames == 0 {
            continue;
        }
        let mut mono: Vec<f32> = Vec::with_capacity(n_frames);
        for f in 0..n_frames {
            let base = f * blockalign;
            let mut sum = 0.0f32;
            for c in 0..channels {
                let off = base + c * 4;
                if off + 4 <= bytes.len() {
                    let s = f32::from_le_bytes([
                        bytes[off],
                        bytes[off + 1],
                        bytes[off + 2],
                        bytes[off + 3],
                    ]);
                    sum += s;
                }
            }
            mono.push(sum / channels as f32);
        }

        // 灌进共享 far-end 缓冲（try_lock，不阻塞；消费者持锁就丢这块）
        let pushed = if let Ok(mut fe) = far_end.try_lock() {
            const FAR_CAP: usize = 96_000; // ~2s @48k
            let extra = (fe.len() + mono.len()).saturating_sub(FAR_CAP);
            if extra > 0 {
                fe.drain(..extra);
            }
            fe.extend(mono);
            true
        } else {
            false
        };

        // 周期日志（每 ~1s，按 100ms 周期≈10 次）确认在抓
        iter += 1;
        if iter % 10 == 0 {
            let len = far_end.lock().map(|fe| fe.len()).unwrap_or(0);
            tracing::info!(
                "[AEC Loopback] iter={} frames_captured_now pushed={} far_buf_len={}",
                iter,
                pushed,
                len
            );
        }
    }

    let _ = audio_client.stop_stream();
    tracing::info!("[AEC Loopback] stopped (iters={})", iter);
    Ok(())
}
