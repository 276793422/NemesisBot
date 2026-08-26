//! Tests for `punct` (Phase 3 覆盖率，2026-08-25)。
//!
//! 引擎本体的 add_punctuation 走 sherpa-onnx FFI，需要真模型文件——
//! 结构性不测（进豁免证据表）；这里钉可测分支：缺模型文件时的显式
//! bail（不触发任何 FFI 调用）。

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
// add_punctuation / Drop 需要真引擎实例——结构性豁免。
// ===========================================================================

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn punct_engine_new_with_model_builds_config_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    let model = tmp.path().join("model.onnx");
    std::fs::write(&model, b"fixture").unwrap();
    let _ = PunctEngine::new(&model, 2);
}
