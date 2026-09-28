//! stt_stream.rs FFI 测试（Windows + sherpa DLL 环境纪律对齐 stt/tests.rs）。
//!
//! 无 DLL 时 sherpa_fn 包装器在符号查找处 panic "sherpa-onnx not
//! initialized"——夹具测试正好钉住文件校验与 config 组装路径。
//! 真模型往返测试用 `NEMESIS_VOICE_STREAM_MODEL_DIR` 环境变量显式开启
//! （M4 真机验证钩子；CI 无该变量时 eprintln 跳过）。

use super::StreamSttEngine;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// 文件校验 fail-fast（不碰 FFI，确定性）
// ---------------------------------------------------------------------------

#[test]
fn stream_new_empty_dir_bails_model_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let err = format!(
        "{:#}",
        StreamSttEngine::new(
            tmp.path(),
            "encoder.onnx",
            "decoder.onnx",
            "joiner.onnx",
            "tokens.txt",
            1,
            2.4,
            1.2
        )
        .err()
        .expect("must fail")
    );
    assert!(err.contains("Streaming STT model file not found"), "{err}");
}

#[test]
fn stream_new_missing_joiner_bails() {
    let tmp = tempfile::tempdir().unwrap();
    for f in ["encoder.onnx", "decoder.onnx", "tokens.txt"] {
        std::fs::write(tmp.path().join(f), b"m").unwrap();
    }
    let err = format!(
        "{:#}",
        StreamSttEngine::new(
            tmp.path(),
            "encoder.onnx",
            "decoder.onnx",
            "joiner.onnx",
            "tokens.txt",
            1,
            2.4,
            1.2
        )
        .err()
        .expect("must fail")
    );
    assert!(err.contains("joiner.onnx"), "{err}");
}

// ---------------------------------------------------------------------------
// 夹具齐全 → config 组装 → 未初始化 panic（FFI 符号查找边界）
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    let tmp = tempfile::tempdir().unwrap();
    for f in ["encoder.onnx", "decoder.onnx", "joiner.onnx", "tokens.txt"] {
        std::fs::write(tmp.path().join(f), b"m").unwrap();
    }
    // tempdir 在返回时会被清理——泄漏它换取路径长期有效（测试进程生命周期）
    let path = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    path
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stream_new_full_fixture_builds_structs_until_ffi() {
    let dir = fixture_dir();
    let _ = StreamSttEngine::new(
        &dir,
        "encoder.onnx",
        "decoder.onnx",
        "joiner.onnx",
        "tokens.txt",
        2,
        2.4,
        1.2,
    );
}

// ---------------------------------------------------------------------------
// 白盒 null 引擎 → 方法 FFI panic 路径（catch_panic_msg + forget 防 Drop 双 panic）
// ---------------------------------------------------------------------------

fn null_engine() -> StreamSttEngine {
    StreamSttEngine {
        recognizer: std::ptr::null(),
        stream: std::ptr::null(),
        _keep: Vec::new(),
    }
}

#[test]
fn stream_partial_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.partial());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
fn stream_decode_null_engine_panics_at_symbol_lookup() {
    let engine = null_engine();
    let msg = crate::test_util::catch_panic_msg(|| engine.decode());
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(engine);
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn stream_drop_null_engine_panics_at_destroy() {
    let _ = null_engine();
}

// ---------------------------------------------------------------------------
// 真模型往返（M4 真机钩子；默认跳过）
// ---------------------------------------------------------------------------

#[test]
fn stream_real_model_roundtrip_if_env_set() {
    let Some(dir) = std::env::var_os("NEMESIS_VOICE_STREAM_MODEL_DIR").map(PathBuf::from) else {
        eprintln!("skip: NEMESIS_VOICE_STREAM_MODEL_DIR not set");
        return;
    };
    if !crate::sherpa::is_initialized() {
        eprintln!("skip: sherpa DLL not initialized in test process");
        return;
    }
    let Ok(engine) = StreamSttEngine::new(
        &dir,
        "encoder-epoch-99-avg-1.onnx",
        "decoder-epoch-99-avg-1.onnx",
        "joiner-epoch-99-avg-1.onnx",
        "tokens.txt",
        2,
        2.4,
        1.2,
    ) else {
        panic!(
            "real model dir set but engine creation failed: {}",
            dir.display()
        );
    };
    // 静音喂入：partial 为空串或任意文本都不该 panic；端点布尔可读；reset 可用
    let silence = vec![0.0f32; 16000];
    engine.accept_waveform(&silence, 16000);
    engine.decode();
    let _ = engine.partial();
    let _ = engine.is_endpoint();
    engine.reset();
}
