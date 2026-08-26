//! Tests for `vad`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：`VadEngine::new` 缺模型 fail-fast；全套夹具下的
//! #[repr(C)] 结构构造（SileroVad / TenVad / VadModelConfig）→ 无 DLL 时在
//! FFI 符号查找处 panic（不加载任何真 DLL）。
//!
//! 结构性豁免：accept_waveform/is_speech_detected/is_empty/front/pop/flush/
//! reset/Drop —— 全部需要已构造的 VAD 实例（= 真 DLL + 真模型）。

use super::*;

#[test]
fn vad_new_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!(
        "{:#}",
        VadEngine::new(&tmp.path().join("silero_vad.onnx"), 0.5, 0.5, 0.25, 5.0, 512, 16000)
            .err()
            .expect("must fail")
    );
    assert!(err.contains("VAD model not found"), "{err}");
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn vad_new_with_model_builds_structs_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("silero_vad.onnx");
    std::fs::write(&model, b"fixture").unwrap();
    let _ = VadEngine::new(&model, 0.5, 0.5, 0.25, 5.0, 512, 16000);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn vad_new_nondefault_params_builds_structs_until_ffi() {
    // 非默认参数变体（threshold/durations/window 传不同值），确保结构构造各字段取值路径
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("silero_vad.onnx");
    std::fs::write(&model, b"fixture").unwrap();
    let _ = VadEngine::new(&model, 0.75, 1.0, 0.5, 10.0, 1024, 8000);
}
