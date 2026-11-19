use super::normalize_lang_token;

#[test]
fn normalize_strips_token_wrapper() {
    assert_eq!(normalize_lang_token("<|zh|>"), "zh");
    assert_eq!(normalize_lang_token("<|en|>"), "en");
    assert_eq!(normalize_lang_token("  <|ja|>  "), "ja");
    assert_eq!(normalize_lang_token("<|yue|>"), "yue");
    assert_eq!(normalize_lang_token("zh"), "zh"); // 裸值原样返回
    assert_eq!(normalize_lang_token(""), "");
}

// ===========================================================================
// 覆盖率冲刺：引擎构造分支 + FFI 结构甄别
// 无 DLL 时 sherpa_fn 包装器在符号查找处 panic "sherpa-onnx not
// initialized"——should_panic 测试正好覆盖全部 #[repr(C)] 结构构造。
// ===========================================================================

use super::SttEngine;

// ---------------------------------------------------------------------------
// build_recognizer 参数校验 fail-fast（不碰 FFI）
// ---------------------------------------------------------------------------

#[test]
fn stt_new_empty_dir_bails_model_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    // model_sherpa.onnx / model.onnx 都缺 → 报 model.onnx（回退名）
    let err = format!(
        "{:#}",
        SttEngine::new(tmp.path(), "m", "auto", true, true, 1)
            .err()
            .expect("must fail")
    );
    assert!(err.contains("STT model not found"), "{err}");
    assert!(err.contains("model.onnx"), "{err}");
}

#[test]
fn stt_new_model_onnx_without_tokens_bails_tokens() {
    // 只放 model.onnx（非 _sherpa 名）→ 命中 model.onnx 回退分支 + 缺 tokens
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model.onnx"), b"m").unwrap();
    let err = format!(
        "{:#}",
        SttEngine::new(tmp.path(), "m", "auto", true, true, 1)
            .err()
            .expect("must fail")
    );
    assert!(err.contains("STT tokens not found"), "{err}");
}

#[test]
fn stt_new_model_sherpa_without_tokens_bails_tokens() {
    // model_sherpa.onnx 优先命中
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model_sherpa.onnx"), b"m").unwrap();
    let err = format!(
        "{:#}",
        SttEngine::new(tmp.path(), "m", "auto", true, true, 1)
            .err()
            .expect("must fail")
    );
    assert!(err.contains("STT tokens not found"), "{err}");
}

/// BUG-20261115-014 回归锁：config 来源 language 含内嵌 NUL 必须走 Err
/// 错误臂而非 panic（旧行为 `CString::new(language).unwrap()` 在此 panic，
/// 破坏构造路径的 Result 设计）。哑 model.onnx + tokens.txt 越过存在性闸
/// 后，NUL 恰落在 lang_c 转换点——该点先于一切 FFI 调用，确定性、不依赖
/// sherpa 运行时。
#[test]
fn stt_new_language_with_embedded_nul_returns_err_not_panic() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model.onnx"), b"m").unwrap();
    std::fs::write(tmp.path().join("tokens.txt"), b"t").unwrap();
    let err = format!(
        "{:#}",
        SttEngine::new(tmp.path(), "m", "a\u{0}b", false, false, 1)
            .err()
            .expect("language 含内嵌 NUL 必须返回 Err")
    );
    assert!(err.contains("NUL"), "错误信息应点名 NUL: {err}");
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stt_new_use_itn_false_and_remedy_off_builds_structs_until_ffi() {
    // use_itn=false（125 行三元 false 臂）+ lang_remedy=false → 不走补救分支
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model_sherpa.onnx"), b"m").unwrap();
    std::fs::write(tmp.path().join("tokens.txt"), b"t").unwrap();
    let _ = SttEngine::new(tmp.path(), "m", "zh", false, false, 2);
}

// ---------------------------------------------------------------------------
// 全套夹具 → 结构构造 → 未初始化 panic（覆盖 110-250 的结构甄别）
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stt_new_full_fixture_model_sherpa_builds_structs_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model_sherpa.onnx"), b"m").unwrap();
    std::fs::write(tmp.path().join("tokens.txt"), b"t").unwrap();
    let _ = SttEngine::new(tmp.path(), "m", "auto", true, true, 2);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stt_new_full_fixture_model_onnx_builds_structs_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("model.onnx"), b"m").unwrap();
    std::fs::write(tmp.path().join("tokens.txt"), b"t").unwrap();
    let _ = SttEngine::new(tmp.path(), "m", "en", false, true, 1);
}

// ---------------------------------------------------------------------------
// decode_recognizer —— 空识别器 → FFI 符号查找 panic（292 行调用臂）
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn decode_recognizer_null_recognizer_panics_at_symbol_lookup() {
    let _ = super::decode_recognizer(std::ptr::null(), &[0.0f32; 4], 16000);
}

// ---------------------------------------------------------------------------
// 白盒 null 引擎 → recognize / recognize_detail / Drop 封送路径
// `SttEngine { recognizer: null, restriction: None }` 不触发任何 FFI 构造；
// recognize → recognize_detail → decode_recognizer 在 FFI 符号查找处 panic。
// 剩余不可达 = FFI 成功返回值语义（真模型 + 真 DLL）。
// 方法 panic 测试用 catch_panic_msg + mem::forget（Drop 也 panic，should_panic
// 会双 panic fail-fast，见 test_util.rs）。
// ---------------------------------------------------------------------------

/// 白盒 null 识别器引擎（不触发任何 FFI 构造）。
/// pub(crate)：供跨模块测试（channel_bridge 的白盒）复用。
pub(crate) fn null_stt_engine() -> SttEngine {
    SttEngine {
        lock: std::sync::Mutex::new(()),
        recognizer: std::ptr::null(),
        restriction: None,
    }
}

#[test]
fn stt_recognize_null_engine_panics_at_symbol_lookup() {
    let engine = null_stt_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.recognize(&[0.0f32; 8], 16000));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn stt_recognize_detail_null_engine_no_restriction_panics_at_symbol_lookup() {
    let engine = null_stt_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.recognize_detail(&[0.0f32; 8], 16000));
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stt_drop_null_engine_panics_at_destroy() {
    let _ = null_stt_engine();
}
