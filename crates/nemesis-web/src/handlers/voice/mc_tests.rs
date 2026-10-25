//! MC 覆盖率批次（2026-10-25）：模型设置弹窗三命令的纯逻辑测试——
//! cmd_model_catalog（分组/installed/is_current/缺 config 回退内嵌模板）、
//! cmd_set_model（就地改写保注释/未安装拒绝/未知变体/类别错误/缺 config）、
//! cmd_status 扩展（current/installed[]/stt_stream 条目）。
//! 三命令均为纯 FS 读写，不触 setup_cancel/install_locks 全局单例，
//! 无需 voice_state_lock 串行化。

use super::*;

// 最小合法 config.toml：注释 + 行尾注释 + 2 个 TTS 变体 + 1 个 STT 变体。
// auto_download=false：误触发下载路径会当场 bail 而不是碰网。
fn minimal_config_toml() -> &'static str {
    r#"# user comment keep me
[stt]
model_name = "stt-a"
language = "auto"
use_itn = true
num_threads = 1

[vad]
model_name = "vad-a"
threshold = 0.5
min_silence_duration = 0.3
min_speech_duration = 0.25
max_speech_duration = 30.0
window_size = 512

[tts]
model_name = "tts-a"  # trailing comment keep
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
base = "https://example.com"

[[models.sources]]
name = "tts-a"
category = "tts"
repo = "u/a"
files = [{ local = "model.onnx", remote = "m.onnx" }]

[[models.sources]]
name = "tts-b"
category = "tts"
repo = "u/b"
description = "variant b desc"
files = [{ local = "model.onnx", remote = "m.onnx" }, { local = "tokens.txt", remote = "t.txt" }]

[[models.sources]]
name = "stt-a"
category = "stt"
repo = "u/s"
files = [{ local = "model.onnx", remote = "m.onnx" }]
"#
}

/// 装 tts-a 全部声明文件（model.onnx 一个）
fn install_tts_a(voice_dir: &std::path::Path) {
    let dir = voice_dir.join("data").join("tts").join("tts-a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"m").unwrap();
}

/// 装 tts-b 全部声明文件（model.onnx + tokens.txt）
fn install_tts_b(voice_dir: &std::path::Path, complete: bool) {
    let dir = voice_dir.join("data").join("tts").join("tts-b");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), b"m").unwrap();
    if complete {
        std::fs::write(dir.join("tokens.txt"), b"t").unwrap();
    }
}

fn seeded_voice_dir() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let voice_dir = tmp.path().join("voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    std::fs::write(voice_dir.join("config.toml"), minimal_config_toml()).unwrap();
    (tmp, voice_dir)
}

#[test]
fn catalog_groups_by_category_with_installed_and_current_flags() {
    let (_tmp, voice_dir) = seeded_voice_dir();
    install_tts_a(&voice_dir);
    // tts-b 半截下载（缺 tokens.txt）→ catalog installed 必须 false（强判）
    install_tts_b(&voice_dir, false);

    let out = VoiceHandler::new()
        .cmd_model_catalog(&voice_dir)
        .unwrap()
        .unwrap();
    let cats = out["categories"].as_object().unwrap();

    // BTreeMap 分组：恰好 stt + tts 两组（键序无关，按存在性断言）
    assert_eq!(cats.len(), 2, "categories: {cats:?}");
    let tts = cats["tts"].as_array().unwrap();
    let stt = cats["stt"].as_array().unwrap();
    assert_eq!(tts.len(), 2);
    assert_eq!(stt.len(), 1);

    // tts-a：installed=true + is_current=true（配置默认）+ 空描述
    let a = tts.iter().find(|v| v["name"] == "tts-a").unwrap();
    assert_eq!(a["installed"], true);
    assert_eq!(a["is_current"], true);
    assert_eq!(a["description"], "");
    assert_eq!(a["file_count"], 1);

    // tts-b：半截下载 installed=false；非当前；description 透传
    let b = tts.iter().find(|v| v["name"] == "tts-b").unwrap();
    assert_eq!(b["installed"], false);
    assert_eq!(b["is_current"], false);
    assert_eq!(b["description"], "variant b desc");
    assert_eq!(b["file_count"], 2);
    assert_eq!(b["repo"], "u/b");
}

#[test]
fn catalog_completes_installed_when_all_files_present() {
    let (_tmp, voice_dir) = seeded_voice_dir();
    install_tts_b(&voice_dir, true);
    let out = VoiceHandler::new()
        .cmd_model_catalog(&voice_dir)
        .unwrap()
        .unwrap();
    let b = out["categories"]["tts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "tts-b")
        .unwrap();
    assert_eq!(b["installed"], true);
    assert_eq!(b["is_current"], false);
}

#[test]
fn catalog_falls_back_to_embedded_template_when_config_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let voice_dir = tmp.path().join("voice");
    std::fs::create_dir_all(&voice_dir).unwrap();

    let out = VoiceHandler::new()
        .cmd_model_catalog(&voice_dir)
        .unwrap()
        .unwrap();
    let cats = out["categories"].as_object().unwrap();
    // 内嵌模板自带完整目录（含 5 个 TTS 变体），模型目录空 → 全部未安装
    assert!(cats.contains_key("tts"), "categories: {cats:?}");
    assert!(cats["tts"].as_array().unwrap().len() >= 5);
    assert!(
        cats["tts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|v| v["installed"] == false)
    );
}

#[test]
fn set_model_rewrites_config_in_place_preserving_comments() {
    let (_tmp, voice_dir) = seeded_voice_dir();
    install_tts_b(&voice_dir, true);

    let out = VoiceHandler::new()
        .cmd_set_model(&voice_dir, "tts", "tts-b")
        .unwrap()
        .unwrap();
    assert_eq!(out["success"], true);
    assert_eq!(out["category"], "tts");
    assert_eq!(out["name"], "tts-b");

    // 就地改写：注释与行尾注释保真；其他节不受影响
    let content = std::fs::read_to_string(voice_dir.join("config.toml")).unwrap();
    assert!(content.contains("# user comment keep me"), "{content}");
    assert!(
        content.contains("model_name = \"tts-b\"  # trailing comment keep"),
        "{content}"
    );
    assert!(content.contains("model_name = \"stt-a\""), "{content}");

    // 改写结果可解析且值正确（stt 默认不动）
    let cfg = nemesis_voice::AppConfig::load(&voice_dir.join("config.toml")).unwrap();
    assert_eq!(cfg.tts.model_name, "tts-b");
    assert_eq!(cfg.stt.model_name, "stt-a");
}

#[test]
fn set_model_rejects_uninstalled_variant_and_leaves_file_untouched() {
    let (_tmp, voice_dir) = seeded_voice_dir();
    let before = std::fs::read_to_string(voice_dir.join("config.toml")).unwrap();

    let err = VoiceHandler::new()
        .cmd_set_model(&voice_dir, "tts", "tts-b")
        .unwrap_err();
    assert!(err.contains("尚未安装"), "{err}");
    assert!(err.contains("tts-b"), "{err}");

    let after = std::fs::read_to_string(voice_dir.join("config.toml")).unwrap();
    assert_eq!(before, after, "rejected set_model must not touch the file");
}

#[test]
fn set_model_error_surface_unknown_variant_category_mismatch_and_missing_config() {
    let (_tmp, voice_dir) = seeded_voice_dir();

    // 未知变体
    let err = VoiceHandler::new()
        .cmd_set_model(&voice_dir, "tts", "ghost")
        .unwrap_err();
    assert!(err.contains("不在 config [models.sources] 目录中"), "{err}");

    // 类别与变体不符（stt-a 是 stt，请求成 tts）
    let err = VoiceHandler::new()
        .cmd_set_model(&voice_dir, "tts", "stt-a")
        .unwrap_err();
    assert!(err.contains("属于类别 'stt'"), "{err}");
    assert!(err.contains("与请求的类别 'tts' 不符"), "{err}");

    // 非法类别（不在 SETTABLE 白名单）
    let err = VoiceHandler::new()
        .cmd_set_model(&voice_dir, "boom", "tts-a")
        .unwrap_err();
    assert!(err.contains("unknown model category"), "{err}");

    // config.toml 缺席
    let tmp = tempfile::tempdir().unwrap();
    let empty = tmp.path().join("v");
    std::fs::create_dir_all(&empty).unwrap();
    let err = VoiceHandler::new()
        .cmd_set_model(&empty, "tts", "tts-a")
        .unwrap_err();
    assert!(err.contains("Run setup first"), "{err}");
}

#[test]
fn status_reports_current_installed_and_stt_stream_entry() {
    let (_tmp, voice_dir) = seeded_voice_dir();
    install_tts_a(&voice_dir);
    install_tts_b(&voice_dir, true);

    let out = VoiceHandler::new().cmd_status(&voice_dir).unwrap().unwrap();
    let models = out["models"].as_object().unwrap();

    // stt_stream 条目补齐（此前缺席，VoiceTab 流式行读不到状态）
    assert!(models.contains_key("stt_stream"), "models: {models:?}");
    let ss = &models["stt_stream"];
    assert_eq!(ss["current"], "streaming-zipformer-bilingual-zh-en");

    // tts：current = 配置默认 tts-a；installed 目录扫含两个变体
    let tts = &models["tts"];
    assert_eq!(tts["current"], "tts-a");
    let installed: Vec<&str> = tts["installed"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(installed.contains(&"tts-a"), "installed: {installed:?}");
    assert!(installed.contains(&"tts-b"), "installed: {installed:?}");
    assert_eq!(tts["ready"], true, "tts-a has model.onnx in subdir");

    // set_model 后 status 的 current 跟随配置文件（不读缓存）
    install_tts_b(&voice_dir, true);
    VoiceHandler::new()
        .cmd_set_model(&voice_dir, "tts", "tts-b")
        .unwrap();
    let out = VoiceHandler::new().cmd_status(&voice_dir).unwrap().unwrap();
    assert_eq!(out["models"]["tts"]["current"], "tts-b");
}

#[test]
fn status_survives_missing_config_with_default_current_names() {
    let tmp = tempfile::tempdir().unwrap();
    let voice_dir = tmp.path().join("voice");
    std::fs::create_dir_all(&voice_dir).unwrap();

    let out = VoiceHandler::new().cmd_status(&voice_dir).unwrap().unwrap();
    // config_exists=false 依旧；current 有默认名可报；installed 空数组不缺字段
    assert_eq!(out["config_exists"], false);
    let models = out["models"].as_object().unwrap();
    for key in ["stt", "stt_stream", "vad", "tts", "punct", "speaker"] {
        let m = &models[key];
        assert!(
            m["current"].is_string(),
            "{key}.current must be a string: {m:?}"
        );
        assert!(m["installed"].is_array(), "{key}.installed must be array");
        assert_eq!(m["ready"], false);
    }
}
