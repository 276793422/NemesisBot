//! Tests for `vad`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：`VadEngine::new` 缺模型 fail-fast；全套夹具下的
//! #[repr(C)] 结构构造（SileroVad / TenVad / VadModelConfig）→ 无 DLL 时在
//! FFI 符号查找处 panic（不加载任何真 DLL）。
//!
//! R6（2026-08-27）修订：S12 原把 accept_waveform / is_speech_detected /
//! is_empty / front / pop / flush / reset / Drop 归为「结构性豁免」——过保守。
//! 同 crate 测试可以**白盒构造** `VadEngine { vad: null }`（字段私有但子模块
//! 可见），每个包装方法都会执行到 FFI 符号查找处 panic——参数封送代码真实
//! 走过。真正剩下的不可达只有：`new` 的 `is_null → bail` 臂（创建调用在
//! panic 前不会返回 null）和各方法的**成功返回值语义**（需要真 DLL + 真模型）。

use super::*;

/// 白盒构造一个 vad 指针为 null 的引擎（不触发任何 FFI 构造）。
/// pub(crate)：供跨模块测试（voice_detect 的 Silero 白盒）复用。
pub(crate) fn null_engine() -> VadEngine {
    VadEngine {
        vad: std::ptr::null(),
    }
}

#[test]
fn vad_new_missing_model_bails() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!(
        "{:#}",
        VadEngine::new(
            &tmp.path().join("silero_vad.onnx"),
            0.5,
            0.5,
            0.25,
            5.0,
            512,
            16000
        )
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

// ---------------------------------------------------------------------------
// R6（2026-08-27）：白盒 null 引擎 → 各包装方法的封送代码走到 FFI 符号查找。
// 注意：这些测试**不能**用 `#[should_panic]`——VadEngine 的 Drop 同样在 null
// 上 panic，展开时析构再 panic = 双 panic → fail-fast 击穿进程。统一
// `catch_panic_msg`（闭包边界截住展开）+ `mem::forget`（跳过析构），见
// test_util.rs 头注。
// ---------------------------------------------------------------------------

#[test]
fn vad_accept_waveform_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.accept_waveform(&[0.0_f32; 512]));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_is_speech_detected_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.is_speech_detected());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_is_empty_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.is_empty());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_front_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.front());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_pop_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.pop());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_flush_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.flush());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn vad_reset_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.reset());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn vad_drop_null_engine_panics_at_destroy() {
    let _ = null_engine();
}
