//! Tests for `sherpa` 纯 helper（Phase 3 覆盖率，2026-08-25）。
//!
//! FFI 函数表本体需要真 sherpa-onnx DLL（结构性不测，进豁免证据表）；
//! 这里钉：CString 包装语义（内嵌 NUL 的输入降级为空串而非 panic——
//! 错误的模型路径/文本不能让引擎崩）+ 未初始化状态判定。

use super::*;

#[test]
fn to_cstr_plain_ascii_round_trips() {
    let c = to_cstr("cpu");
    assert_eq!(c.to_bytes(), b"cpu");
}

#[test]
fn to_cstr_utf8_chinese_round_trips() {
    let c = to_cstr("模型路径/中文");
    assert_eq!(c.to_bytes(), "模型路径/中文".as_bytes());
}

#[test]
fn to_cstr_embedded_nul_degrades_to_empty_not_panic() {
    // CString::new 对内嵌 \0 返回 Err——helper 必须降级为空串。
    // FFI 侧拿到空串会走各自的失败路径，而不是让 gateway panic。
    let c = to_cstr("bad\0path");
    assert_eq!(c.to_bytes(), b"");
}

#[test]
fn null_cstr_points_at_single_nul() {
    let p = null_cstr();
    // SAFETY: null_cstr 的契约就是指向单个 NUL 字节。
    let s = unsafe { std::ffi::CStr::from_ptr(p) };
    assert!(s.to_bytes().is_empty());
}

#[test]
fn is_initialized_false_when_dll_never_loaded() {
    // 测试进程不加载 sherpa-onnx DLL → 全局句柄表保持空。
    // （若未来某测试真的 init 了 DLL，这个断言会提醒拆分隔离——
    // 目前 voice 测试套件没有任何 init 调用。）
    assert!(!is_initialized());
}

// ===========================================================================
// S12 覆盖率冲刺（2026-08-26）：init 失败臂 + 符号查找 panic 臂 + TTS safe 包装
// 全部走「DLL 永不加载」前提：init 只喂坏路径/哑文件（绝不成功），其余
// should_panic 测试正好覆盖 get_fn/get_raw_fn/macro 包装体的未初始化分支。
// ===========================================================================

#[test]
fn init_nonexistent_path_fails_without_setting_lib() {
    let err = format!("{:#}", init(std::path::Path::new("Z:/no/such/sherpa.dll")).unwrap_err());
    assert!(err.contains("Failed to load"), "{err}");
    assert!(err.contains("Z:/no/such/sherpa.dll"), "{err}");
    // 失败不得污染全局状态
    assert!(!is_initialized());
}

#[test]
fn init_dummy_file_is_not_a_valid_dll() {
    let tmp = tempfile::tempdir().unwrap();
    let fake = tmp.path().join("sherpa-onnx-c-api.dll");
    std::fs::write(&fake, b"not-a-pe-file-at-all").unwrap();
    let res = init(&fake);
    assert!(res.is_err(), "dummy file must never load as a DLL");
    assert!(!is_initialized());
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn fn_wrapper_get_fn_panics_when_lib_missing() {
    // sherpa_fn! 宏体（有返回值形态）：符号查找在调用前 panic
    unsafe {
        SherpaOnnxDestroyOfflineRecognizer(std::ptr::null());
    }
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn fn_wrapper_void_return_panics_when_lib_missing() {
    // sherpa_fn! 宏体（void 返回形态）
    unsafe {
        SherpaOnnxVoiceActivityDetectorPop(std::ptr::null());
    }
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn safe_create_offline_tts_panics_when_lib_missing() {
    // C++ shim 包装（651-664）：先 get_raw_fn 查符号 → panic
    let _ = safe_create_offline_tts(std::ptr::null());
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn safe_tts_generate_audio_panics_when_lib_missing() {
    let _ = safe_tts_generate_audio(std::ptr::null(), std::ptr::null(), 0, 1.0);
}
