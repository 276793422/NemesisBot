use super::*;
use std::time::Duration;

#[test]
fn rms_new_initializes_not_speaking() {
    let d = RmsVoiceDetector::new(0.01, 300, 15_000);
    assert!(!d.is_speaking());
    assert_eq!(d.name(), "RMS");
}

#[test]
fn rms_process_empty_chunk_returns_none() {
    let mut d = RmsVoiceDetector::new(0.01, 300, 15_000);
    let out = d.process(&[], 16000);
    assert!(out.is_none());
    assert!(!d.is_speaking());
}

#[test]
fn rms_process_silent_chunk_does_not_start_speaking() {
    let mut d = RmsVoiceDetector::new(0.5, 300, 15_000);
    // Pure silence — RMS = 0, well below threshold 0.5
    let chunk = vec![0.0_f32; 512];
    let out = d.process(&chunk, 16000);
    assert!(out.is_none());
    assert!(!d.is_speaking());
}

#[test]
fn rms_process_loud_chunk_starts_speaking() {
    let mut d = RmsVoiceDetector::new(0.01, 300_000, 15_000_000);
    // High-amplitude signal — RMS >> 0.01 threshold
    let chunk: Vec<f32> = (0..512).map(|_| 1.0).collect();
    let _ = d.process(&chunk, 16000);
    assert!(d.is_speaking(), "after loud chunk, should be speaking");
}

#[test]
fn rms_process_returns_audio_when_silence_elapsed_exceeds_threshold() {
    // Use threshold = 0.5 so 1.0 amplitude is "speaking"
    // silence_ms very low (1ms) so silence trips after a real sleep
    let mut d = RmsVoiceDetector::new(0.5, 1, 60_000);

    // Feed loud audio — starts speaking, builds buffer ≥ min_samples
    let loud: Vec<f32> = (0..16000).map(|_| 1.0).collect(); // 1 second @ 16kHz
    let _ = d.process(&loud, 16000);
    assert!(d.is_speaking());

    // First silent chunk: triggers silence_start, no emit yet (elapsed=0)
    let silent: Vec<f32> = vec![0.0; 512];
    let _ = d.process(&silent, 16000);

    // Wait long enough that silence_start.elapsed() > silence_ms (1ms)
    std::thread::sleep(Duration::from_millis(5));

    // Second silent chunk: silence_elapsed > 1ms now, buffer ≥ 300ms → emit
    let out = d.process(&silent, 16000);
    assert!(
        out.is_some(),
        "should return completed utterance after silence elapsed"
    );
    let audio = out.unwrap();
    assert!(
        audio.len() >= (16000.0 * 0.3) as usize,
        "returned audio should meet min_samples"
    );
    assert!(!d.is_speaking(), "after emission, is_speaking should reset");
}

#[test]
fn rms_flush_returns_buffered_audio_when_nonempty() {
    let mut d = RmsVoiceDetector::new(0.01, 1_000_000, 60_000_000);
    let chunk: Vec<f32> = (0..1024).map(|_| 0.5).collect();
    let _ = d.process(&chunk, 16000);
    let out = d.flush();
    assert!(out.is_some());
    assert_eq!(out.unwrap().len(), 1024);
}

#[test]
fn rms_flush_returns_none_when_buffer_empty() {
    let mut d = RmsVoiceDetector::new(0.01, 300, 15_000);
    let out = d.flush();
    assert!(out.is_none());
}

#[test]
fn rms_flush_after_emit_returns_none() {
    let mut d = RmsVoiceDetector::new(0.5, 1, 60_000);
    let loud: Vec<f32> = (0..16000).map(|_| 1.0).collect();
    let _ = d.process(&loud, 16000);
    let silent: Vec<f32> = vec![0.0; 512];
    let _ = d.process(&silent, 16000);
    std::thread::sleep(Duration::from_millis(5));
    let _ = d.process(&silent, 16000);
    // Buffer should now be drained by the emit
    let out = d.flush();
    assert!(out.is_none(), "flush after emit should return None");
}

#[test]
fn rms_name_returns_constant() {
    let d = RmsVoiceDetector::new(0.5, 300, 15_000);
    assert_eq!(d.name(), "RMS");
}

#[test]
fn rms_silence_chunk_in_speaking_state_extends_buffer() {
    let mut d = RmsVoiceDetector::new(0.5, 1_000_000, 60_000_000);
    // Start speaking
    let loud: Vec<f32> = (0..1024).map(|_| 1.0).collect();
    let _ = d.process(&loud, 16000);
    assert!(d.is_speaking());

    // Feed silence — buffer should still grow (extends by chunk length)
    let silent: Vec<f32> = vec![0.0; 256];
    let _ = d.process(&silent, 16000);

    // Flush to verify buffer length
    let audio = d.flush().unwrap();
    assert_eq!(audio.len(), 1024 + 256);
}

#[test]
fn rms_repeated_loud_chunks_grow_buffer_until_emit() {
    let mut d = RmsVoiceDetector::new(0.5, 1_000_000, 60_000_000);
    for _ in 0..5 {
        let loud: Vec<f32> = vec![1.0; 1024];
        let out = d.process(&loud, 16000);
        assert!(out.is_none(), "buffer still below min_samples; no emit");
    }
    // After 5 chunks of 1024 = 5120 samples (< 4800 min at 16kHz), still no emit
    // Flush should give us all 5120
    let audio = d.flush().unwrap();
    assert_eq!(audio.len(), 5120);
}

#[test]
fn rms_threshold_zero_marks_speaking_on_empty_chunk_due_to_geq() {
    // Quirk: when threshold=0 and chunk is empty, rms=0.0, and `0.0 >= 0.0` is true,
    // so is_speaking flips true even though no audio was added to the buffer.
    // Pinning this behavior to prevent silent regression if the comparison changes.
    let mut d = RmsVoiceDetector::new(0.0, 1_000_000, 60_000_000);
    let _ = d.process(&[], 16000);
    assert!(
        d.is_speaking(),
        "threshold=0 + empty chunk flips is_speaking (quirk)"
    );
    // Buffer is still empty though
    assert!(d.flush().is_none(), "buffer should still be empty");
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：Silero 构造 fail-fast + 工厂回退
// 原注：Silero 的 process/flush 需要真 VAD 引擎——结构性豁免。
//
// R6（2026-08-27）修订：白盒构造（engine 指针为 null）后，next_pending /
// is_speaking / name 是纯 Rust 可直测；process/flush 的封送路径可测到 FFI
// 符号查找处 panic。剩余不可达 = 真引擎成功语义 + 3 秒诊断块（214 行
// is_speech_detected 先 panic，diag 块在无 DLL 时永远走不到）。
//
// 注意：null 引擎的 Drop 也 panic——持有 null_silero 的测试一律
// `mem::forget` 收尾；方法 panic 的测试用 `catch_panic_msg` 截住展开
//（should_panic 会因析构再 panic 触发双 panic fail-fast，见 test_util.rs）。
// ===========================================================================

use super::{SileroVadParams, SileroVoiceDetector, create_detector};

/// 白盒构造 Silero 检测器：VadEngine 指针为 null，不触发任何 FFI。
/// （VadEngine 的 null 构造器在 vad::tests 里——字段私有，只有 vad 子树能建。）
fn null_silero(pending: Vec<Vec<f32>>) -> SileroVoiceDetector {
    SileroVoiceDetector {
        engine: crate::vad::tests::null_engine(),
        window_size: 512,
        chunk_buffer: Vec::new(),
        pending_segments: pending,
        is_speaking: false,
        feed_count: 0,
        window_count: 0,
        detect_count: 0,
        last_diag: std::time::Instant::now(),
    }
}

#[test]
fn silero_name_returns_constant_and_is_speaking_default_false() {
    let d = null_silero(vec![]);
    assert_eq!(d.name(), "Silero VAD");
    assert!(!d.is_speaking());
    let mut d = d;
    d.is_speaking = true;
    assert!(d.is_speaking());
    std::mem::forget(d);
}

#[test]
fn silero_next_pending_skips_short_and_returns_long() {
    // 16kHz 下 min_samples = 4800：短段被跳过，长段返回
    let short = vec![0.1_f32; 100];
    let long = vec![0.2_f32; 4800];
    let mut d = null_silero(vec![short, long.clone()]);
    let out = d.next_pending(16000);
    assert_eq!(out.as_deref(), Some(long.as_slice()));
    // 队列已空 → None
    assert!(d.next_pending(16000).is_none());
    std::mem::forget(d);
}

#[test]
fn silero_next_pending_all_short_returns_none() {
    let mut d = null_silero(vec![vec![0.1_f32; 10], vec![0.2_f32; 20]]);
    assert!(d.next_pending(16000).is_none());
    std::mem::forget(d);
}

#[test]
fn silero_process_returns_pending_segment_before_touching_engine() {
    // process 开头先回吐 pending 段（185-187）——不碰引擎即可返回
    let long = vec![0.5_f32; 4800];
    let mut d = null_silero(vec![long.clone()]);
    let out = d.process(&[0.0_f32; 8], 16000);
    assert_eq!(out.as_deref(), Some(long.as_slice()));
    std::mem::forget(d);
}

#[test]
fn silero_process_small_chunk_panics_at_status_lookup() {
    // 小块不足一个窗口 → while 跳过 → 214 行 is_speech_detected FFI panic
    let mut d = null_silero(vec![]);
    let msg = crate::test_util::catch_panic_msg(|| d.process(&[0.0_f32; 8], 16000));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(d);
}

#[test]
fn silero_process_full_window_panics_at_accept_waveform() {
    // 整窗 → drain → accept_waveform FFI panic
    let mut d = null_silero(vec![]);
    let msg = crate::test_util::catch_panic_msg(|| d.process(&[0.0_f32; 512], 16000));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(d);
}

#[test]
fn silero_flush_empty_buffer_panics_at_engine_flush() {
    // chunk_buffer 空 → 跳过补窗 → engine.flush() FFI panic
    let mut d = null_silero(vec![]);
    let msg = crate::test_util::catch_panic_msg(|| d.flush());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(d);
}

#[test]
fn silero_flush_half_window_pads_then_panics_at_accept_waveform() {
    // 半窗 → 静音补齐到 window_size → drain → accept_waveform FFI panic
    let mut d = null_silero(vec![]);
    d.chunk_buffer = vec![0.1_f32; 100];
    let msg = crate::test_util::catch_panic_msg(|| d.flush());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(d);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn create_detector_with_local_vad_model_builds_silero_until_ffi() {
    // 本地 vad 模型在场 → ensure_vad_model Ok → VadEngine::new 结构构造 →
    // FFI 符号查找 panic（无 DLL 红线内）
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::AppConfig::default();
    cfg.base_dir = tmp.path().to_path_buf();
    cfg.models.dir = "./data".into();
    cfg.models.sources = vec![crate::config::ModelSource {
        name: "silero_vad".into(),
        category: "vad".into(),
        repo: "some/repo".into(),
        files: vec![crate::config::ModelFile {
            local: "silero_vad.onnx".into(),
            remote: "silero_vad.onnx".into(),
            url: String::new(),
        }],
    }];
    let dir = tmp.path().join("data").join("vad").join("silero_vad");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("silero_vad.onnx"), b"fixture").unwrap();
    let _ = create_detector(&cfg);
}

#[test]
fn silero_new_missing_model_propagates_vad_bail() {
    let tmp = tempfile::tempdir().unwrap();
    let params = SileroVadParams {
        model_path: tmp.path().join("silero_vad.onnx"),
        threshold: 0.5,
        min_silence_duration: 0.5,
        min_speech_duration: 0.25,
        max_speech_duration: 5.0,
        window_size: 512,
        sample_rate: 16000,
    };
    let err = format!(
        "{:#}",
        SileroVoiceDetector::new(&params).err().expect("must fail")
    );
    assert!(err.contains("VAD model not found"), "{err}");
}

#[test]
fn create_detector_falls_back_to_rms_when_vad_model_unavailable() {
    // base_dir 指向空 temp 目录且 sources 为空 → ensure_vad_model 失败 →
    // 工厂回退 RMS 能量检测器（311-323 的 Err 臂）
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = crate::config::AppConfig::default();
    cfg.base_dir = tmp.path().to_path_buf();
    cfg.models.dir = "./data".to_string();
    cfg.models.sources = vec![];

    let detector = create_detector(&cfg);
    assert_eq!(detector.name(), "RMS");
}
