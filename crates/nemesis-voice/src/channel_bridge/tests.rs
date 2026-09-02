//! Tests for `channel_bridge`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：`LocalVoiceTranscriber::new` 的 STT 模型缺失 fail-fast（空
//! voice_dir → load_or_default 拿默认配置 → ensure_stt_model 在空目录
//! sources 下 Err）。
//!
//! R6（2026-08-27）修订：new 的全套本地夹具可走到 SttEngine 的 FFI 符号
//! 查找处 panic；is_available / transcribe 前半（ReadWave 封送）用白盒
//! null SttEngine 可测。剩余不可达 = FFI 成功语义（真模型 + 真 DLL）。

use super::*;

#[test]
fn local_transcriber_new_without_models_fails_fast() {
    let tmp = tempfile::tempdir().unwrap();
    // 不放 config.toml（load_or_default 走默认配置）、不放任何模型
    let err = format!(
        "{:#}",
        LocalVoiceTranscriber::new(tmp.path())
            .err()
            .expect("must fail")
    );
    assert!(
        err.contains("not found in config [models.sources]") || err.contains("not found"),
        "{err}"
    );
}

// ===========================================================================
// R6（2026-08-27）：全套本地夹具 + 白盒 null 引擎
// ===========================================================================

/// 最小完整 config.toml（AppConfig 非 serde(default) 字段全给）+
/// 本地 STT 模型夹具。返回 voice_dir 路径。
fn voice_dir_with_local_stt(tmp: &std::path::Path) -> std::path::PathBuf {
    let dir = tmp.to_path_buf();
    std::fs::write(
        dir.join("config.toml"),
        r#"
[stt]
model_name = "sensevoice-small"
language = "zh"
use_itn = false
num_threads = 1

[vad]
model_name = "silero_vad"
threshold = 0.5
min_silence_duration = 0.3
min_speech_duration = 0.25
max_speech_duration = 30.0
window_size = 512

[tts]
model_name = "kokoro-multi-lang-v1_1"
speaker_id = 45
speed = 1.0
num_threads = 4

[audio]
capture_device = ""
playback_device = ""
target_sample_rate = 16000

[models]
dir = "./data"
auto_download = false

[models.mirror]
base = "http://127.0.0.1:1"

[[models.sources]]
name = "sensevoice-small"
category = "stt"
repo = "some/repo"

[[models.sources.files]]
local = "model_sherpa.onnx"
remote = "model.int8.onnx"

[[models.sources.files]]
local = "tokens.txt"
remote = "tokens.txt"
"#,
    )
    .unwrap();
    let model_dir = dir.join("data").join("stt").join("sensevoice-small");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model_sherpa.onnx"), b"fixture").unwrap();
    std::fs::write(model_dir.join("tokens.txt"), b"fixture").unwrap();
    dir
}

#[test]
#[should_panic(expected = "sherpa-onnx not initialized")]
fn local_transcriber_new_with_local_models_builds_stt_until_ffi() {
    let tmp = tempfile::tempdir().unwrap();
    let voice_dir = voice_dir_with_local_stt(tmp.path());
    let _ = LocalVoiceTranscriber::new(&voice_dir);
}

/// 白盒 transcriber：null STT 引擎（不触发任何 FFI 构造）。
fn whitebox_transcriber() -> LocalVoiceTranscriber {
    LocalVoiceTranscriber {
        stt_engine: std::sync::Arc::new(crate::stt::tests::null_stt_engine()),
        punct_engine: None,
        sample_rate: 16000,
    }
}

#[test]
fn local_transcriber_is_available_always_true() {
    let t = whitebox_transcriber();
    use nemesis_channels::base::VoiceTranscriber as _;
    assert!(t.is_available());
    // SttEngine 的 Drop 也 panic（null 指针）——泄漏 Arc 跳过析构
    std::mem::forget(t);
}

#[test]
fn local_transcriber_transcribe_panics_at_read_wave_symbol_lookup() {
    // async 块里第一个 FFI 调用 ReadWave → 无 DLL 在符号查找处 panic。
    // body 在首次 poll 才执行，必须真正 block_on。Drop 也 panic →
    // catch_panic_msg 截住展开 + forget（见 test_util.rs）。
    use nemesis_channels::base::VoiceTranscriber as _;
    let t = whitebox_transcriber();
    let msg = crate::test_util::catch_panic_msg(|| {
        let fut = t.transcribe("whatever.wav");
        let _ = futures::executor::block_on(fut);
    });
    assert!(msg.contains("sherpa-onnx not initialized"), "{msg}");
    std::mem::forget(t);
}
