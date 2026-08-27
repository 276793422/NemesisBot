//! Tests for `punct` (Phase 3 覆盖率，2026-08-25)。
//!
//! 引擎本体的 add_punctuation 走 sherpa-onnx FFI，成功语义需要真模型文件——
//! 进豁免证据表；这里钉可测分支：缺模型文件时的显式 bail（不触发任何 FFI
//! 调用）。
//!
//! R6（2026-08-27）修订：add_punctuation 的空串早退是纯 Rust 分支可直测；
//! 非空路径的封送代码用白盒 null 引擎测到 FFI 符号查找处 panic；Drop 同理。
//! 剩余不可达 = FFI 之后的成功返回值语义（真模型 + 真 DLL）。

use super::*;
use std::path::Path;

#[test]
fn punct_engine_new_missing_model_bails_without_ffi() {
    let err = match PunctEngine::new(Path::new(r"Z:\definitely\missing\punct-model.onnx"), 1) {
        Ok(_) => panic!("missing model must not succeed"),
        Err(e) => e,
    };
    let msg = format!("{err:#}");
    assert!(msg.contains("Punctuation model not found"), "{msg}");
    // 错误信息必须点名路径，帮部署排障。
    assert!(msg.contains("punct-model.onnx"), "{msg}");
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：模型在场时的结构构造臂
// 模型文件存在 → #[repr(C)] 配置构造 → FFI 符号查找处 panic（无 DLL 红线内）。
// ===========================================================================

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn punct_engine_new_with_model_builds_config_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("model.onnx");
    std::fs::write(&model, b"fixture").unwrap();
    let _ = PunctEngine::new(&model, 2);
}

// ===========================================================================
// R6（2026-08-27）：add_punctuation 分支 + 白盒 null 引擎封送路径。
// Drop 也 panic → 持 null 引擎的测试一律 mem::forget；方法 panic 用
// catch_panic_msg 截住展开（should_panic 会双 panic fail-fast，见 test_util.rs）。
// ===========================================================================

#[test]
fn punct_add_punctuation_empty_text_returns_empty_without_ffi() {
    let engine = PunctEngine {
        punct: std::ptr::null(),
    };
    let out = engine.add_punctuation("").unwrap();
    assert_eq!(out, "");
    std::mem::forget(engine);
}

#[test]
fn punct_add_punctuation_nonempty_panics_at_symbol_lookup() {
    let engine = PunctEngine {
        punct: std::ptr::null(),
    };
    let msg = crate::test_util::catch_panic_msg(|| engine.add_punctuation("你好世界"));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn punct_drop_null_engine_panics_at_destroy() {
    let _ = PunctEngine {
        punct: std::ptr::null(),
    };
}
