//! Tests for `channel_bridge`（S12 覆盖率冲刺，2026-08-26）。
//!
//! 可达面：`LocalVoiceTranscriber::new` 的 STT 模型缺失 fail-fast（空
//! voice_dir → load_or_default 拿默认配置 → ensure_stt_model 在空目录
//! sources 下 Err → 24-28 覆盖）。
//!
//! 结构性豁免：new 的其余部分（SttEngine::new 需要 FFI 识别器）和
//! `transcribe` 全链（SherpaOnnxReadWave / recognize / add_punctuation
//! 均为真 DLL FFI）——无缝可注。

use super::*;

#[test]
fn local_transcriber_new_without_models_fails_fast() {
    let tmp = tempfile::tempdir().unwrap();
    // 不放 config.toml（load_or_default 走默认配置）、不放任何模型
    let err = format!("{:#}", LocalVoiceTranscriber::new(tmp.path()).err().expect("must fail"));
    assert!(
        err.contains("not found in config [models.sources]") || err.contains("not found"),
        "{err}"
    );
}
