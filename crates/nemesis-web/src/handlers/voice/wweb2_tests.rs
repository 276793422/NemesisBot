//! voice.rs 纯逻辑覆盖（Phase 3 批次 18，2026-08-25）。
//!
//! 子模块（非 sibling 文件）以获得私有项访问。只测不依赖音频设备 / DLL 的
//! 纯逻辑：DialogueSttOutput 缓冲语义、check_model_subdir_any 文件探测、
//! voice 配置读回退、cmd_status 文件探测、speaker 持久化与阈值校验、
//! tts 长度上限、空状态 shutdown / 全关初始化 no-op。
//!
//! 结构性豁免（见台账 §9.4）：WASAPI 采集、SpeexDSP AEC、kokoro TTS、
//! espeak、SenseVoice STT 的真 DLL 加载与推理臂；setup/install 真下载臂。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 收集 push 回调参数的简易 sink。
fn collector() -> (Arc<std::sync::Mutex<Vec<String>>>, Arc<dyn Fn(&str) + Send + Sync>) {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let f = move |s: &str| seen2.lock().unwrap().push(s.to_string());
    (seen, Arc::new(f))
}

// -----------------------------------------------------------------------
// 纯映射 / 探测 helper
// -----------------------------------------------------------------------

#[test]
fn model_label_maps_known_and_passthrough() {
    assert_eq!(model_label("stt"), "STT");
    assert_eq!(model_label("vad"), "VAD");
    assert_eq!(model_label("tts"), "TTS");
    assert_eq!(model_label("punct"), "标点");
    assert_eq!(model_label("speaker"), "声纹");
    assert_eq!(model_label("custom-thing"), "custom-thing");
}

#[test]
fn check_model_subdir_any_variants() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    // 1) 不存在的目录 → false
    assert!(!check_model_subdir_any(&base.join("missing")));
    // 2) 空目录 → false
    std::fs::create_dir_all(base.join("empty")).unwrap();
    assert!(!check_model_subdir_any(&base.join("empty")));
    // 3) 子目录存在但没有 .onnx → false
    let no_onnx = base.join("m1").join("sub");
    std::fs::create_dir_all(&no_onnx).unwrap();
    std::fs::write(no_onnx.join("weights.bin"), b"x").unwrap();
    assert!(!check_model_subdir_any(&base.join("m1")));
    // 4) 子目录里有 .onnx → true
    let with_onnx = base.join("m2").join("sub");
    std::fs::create_dir_all(&with_onnx).unwrap();
    std::fs::write(with_onnx.join("model.onnx"), b"x").unwrap();
    assert!(check_model_subdir_any(&base.join("m2")));
    // 5) .onnx 直接放顶层（不在子目录里）→ false（模型必须按子目录组织）
    let top = base.join("m3");
    std::fs::create_dir_all(&top).unwrap();
    std::fs::write(top.join("model.onnx"), b"x").unwrap();
    assert!(!check_model_subdir_any(&top));
}

// -----------------------------------------------------------------------
// DialogueSttOutput / InputBoxSttOutput 缓冲语义
// -----------------------------------------------------------------------

fn make_dialogue(
    push: Arc<dyn Fn(&str) + Send + Sync>,
) -> DialogueSttOutput {
    DialogueSttOutput {
        push_fn: Box::new(move |s: &str| push(s)),
        state: Arc::new(std::sync::Mutex::new(DialogueState {
            buffer: String::new(),
            silence_timeout_secs: 3.0,
            reset_flag: false,
        })),
    }
}

#[test]
fn dialogue_output_accumulates_with_space_and_prefix() {
    let (seen, push) = collector();
    let out = make_dialogue(push);
    out.send_text("你好");
    out.send_text("世界");
    let got = seen.lock().unwrap().clone();
    assert_eq!(got, vec!["accumulate:你好", "accumulate:你好 世界"]);
    // flush 取回完整累计并清空
    assert_eq!(out.flush().as_deref(), Some("你好 世界"));
    assert_eq!(out.flush(), None);
}

#[test]
fn dialogue_output_skips_empty_and_status_messages() {
    let (seen, push) = collector();
    let out = make_dialogue(push);
    out.send_text("");
    out.send_text("   ");
    out.send_text("[听写已开始]");
    assert!(seen.lock().unwrap().is_empty(), "no push expected");
    assert_eq!(out.flush(), None, "buffer must stay empty");
}

#[test]
fn dialogue_output_reset_starts_fresh_buffer() {
    let (seen, push) = collector();
    let out = make_dialogue(push);
    out.send_text("first");
    out.reset();
    out.send_text("second");
    let got = seen.lock().unwrap().clone();
    // reset 置 reset_flag：下一条 send 先清缓冲再累计（旧内容不泄漏）
    assert_eq!(got, vec!["accumulate:first", "accumulate:second"]);
    assert_eq!(out.flush().as_deref(), Some("second"));
}

#[test]
fn dialogue_wrapper_delegates_to_inner() {
    let (seen, push) = collector();
    let inner = Arc::new(make_dialogue(push));
    let wrapper = DialogueSttOutputWrapper { inner: inner.clone() };
    wrapper.send_text("via-wrapper");
    assert_eq!(seen.lock().unwrap().last().map(String::as_str), Some("accumulate:via-wrapper"));
    assert_eq!(inner.flush().as_deref(), Some("via-wrapper"));
}

#[test]
fn input_box_output_invokes_push_fn_verbatim() {
    let (seen, push) = collector();
    let out = InputBoxSttOutput { push_fn: Box::new(move |s: &str| push(s)) };
    out.send_text("verbatim text");
    let got = seen.lock().unwrap().clone();
    // 输入框模式不做 accumulate: 前缀、不做缓冲
    assert_eq!(got, vec!["verbatim text"]);
}

// -----------------------------------------------------------------------
// voice / voiceprint 配置读写
// -----------------------------------------------------------------------

#[test]
fn ensure_voice_config_writes_default_once() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    let path = ensure_voice_config(&cfg_dir);
    assert!(path.exists(), "default config must be created");
    let created = std::fs::read_to_string(&path).unwrap();
    assert_eq!(created, DEFAULT_VOICE_CONFIG);
    // 已存在时不覆盖
    std::fs::write(&path, "{\"stt_enabled\": true}").unwrap();
    ensure_voice_config(&cfg_dir);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "{\"stt_enabled\": true}",
        "existing config must not be clobbered"
    );
}

#[test]
fn read_voice_config_corrupt_json_falls_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join(VOICE_CONFIG_FILENAME), "{not json").unwrap();
    let got = read_voice_config(&cfg_dir);
    let default: serde_json::Value =
        serde_json::from_str(DEFAULT_VOICE_CONFIG).unwrap();
    assert_eq!(got, default, "corrupt JSON must fall back to embedded default");
}

#[test]
fn read_voiceprint_config_defaults_when_missing_or_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    // 文件缺失 → 默认 {threshold: 0.65, speakers: {}}
    let got = read_voiceprint_config(&cfg_dir);
    assert_eq!(got["threshold"], DEFAULT_SPEAKER_THRESHOLD);
    assert_eq!(got["speakers"], serde_json::json!({}));
    // 文件损坏 → 同样回退默认
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join(VOICEPRINT_CONFIG_FILENAME), "<<<").unwrap();
    let got = read_voiceprint_config(&cfg_dir);
    assert_eq!(got["threshold"], DEFAULT_SPEAKER_THRESHOLD);
    assert_eq!(got["speakers"], serde_json::json!({}));
}

// -----------------------------------------------------------------------
// cmd_status 文件探测
// -----------------------------------------------------------------------

#[test]
fn cmd_status_full_layout_reports_ready() {
    let dir = tempfile::tempdir().unwrap();
    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    // 三个必需 DLL + config.toml（model_dir 重定向到 mymodels）+ stt 模型 + aec
    for dll in nemesis_voice::bootstrap::required_lib_names() {
        std::fs::write(voice_dir.join(dll), b"fake-dll-bytes").unwrap();
    }
    let toml = nemesis_voice::bootstrap::default_config_toml()
        .replace("dir = \"./data\"", "dir = \"mymodels\"");
    assert!(toml.contains("mymodels"), "template must contain the models dir line to replace");
    std::fs::write(voice_dir.join("config.toml"), toml).unwrap();
    let stt_sub = voice_dir.join("mymodels").join("stt").join("sense");
    std::fs::create_dir_all(&stt_sub).unwrap();
    std::fs::write(stt_sub.join("model.onnx"), b"x").unwrap();
    std::fs::create_dir_all(voice_dir.join("aec")).unwrap();
    std::fs::write(voice_dir.join("aec").join("aec.dll"), b"x").unwrap();

    let r = VoiceHandler::new()
        .cmd_status(&voice_dir)
        .unwrap()
        .unwrap();
    assert_eq!(r["ready"], true, "all dlls + config.toml → ready");
    assert_eq!(r["all_dlls_present"], true);
    assert_eq!(r["config_exists"], true);
    // 每个dll条目带 exists/size_bytes
    let dlls = r["dlls"].as_array().unwrap();
    assert_eq!(dlls.len(), nemesis_voice::bootstrap::required_lib_names().len());
    for d in dlls {
        assert_eq!(d["exists"], true);
        assert!(d["size_bytes"].as_u64().unwrap() > 0);
    }
    // model_dir 跟着 config.toml 重定向
    assert!(
        r["model_dir"].as_str().unwrap().contains("mymodels"),
        "model_dir must follow config.toml [models] dir, got {}",
        r["model_dir"]
    );
    assert_eq!(r["models"]["stt"]["ready"], true);
    assert_eq!(r["models"]["vad"]["ready"], false);
    assert_eq!(r["models"]["tts"]["ready"], false);
    assert_eq!(r["models"]["punct"]["ready"], false);
    assert_eq!(r["models"]["speaker"]["ready"], false);
    assert_eq!(r["aec"]["ready"], true);
}

#[test]
fn cmd_status_without_config_uses_data_dir_and_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    // 只有 DLL、没有 config.toml → model_dir 回退 <voice_dir>/data，ready false
    for dll in nemesis_voice::bootstrap::required_lib_names() {
        std::fs::write(voice_dir.join(dll), b"x").unwrap();
    }
    let r = VoiceHandler::new()
        .cmd_status(&voice_dir)
        .unwrap()
        .unwrap();
    assert_eq!(r["ready"], false);
    assert_eq!(r["all_dlls_present"], true);
    assert_eq!(r["config_exists"], false);
    assert!(r["model_dir"].as_str().unwrap().contains("data"));
}

// -----------------------------------------------------------------------
// speaker 持久化 / 阈值校验
// -----------------------------------------------------------------------

fn seed_voiceprint(cfg_dir: &std::path::Path, names: &[&str]) {
    std::fs::create_dir_all(cfg_dir).unwrap();
    let mut speakers = serde_json::Map::new();
    for n in names {
        speakers.insert(
            n.to_string(),
            serde_json::json!({ "embedding": [0.1, 0.2], "registered_at": "2026-08-25" }),
        );
    }
    std::fs::write(
        cfg_dir.join(VOICEPRINT_CONFIG_FILENAME),
        serde_json::to_string_pretty(&serde_json::json!({
            "threshold": 0.65,
            "speakers": speakers,
        }))
        .unwrap(),
    )
    .unwrap();
}

#[test]
fn cmd_speaker_remove_deletes_entry_and_persists() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    seed_voiceprint(&cfg_dir, &["alice", "bob"]);
    let r = VoiceHandler::new()
        .cmd_speaker_remove(&cfg_dir, "alice")
        .unwrap()
        .unwrap();
    assert_eq!(r["removed"], true);
    assert_eq!(r["name"], "alice");
    let on_disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(cfg_dir.join(VOICEPRINT_CONFIG_FILENAME)).unwrap(),
    )
    .unwrap();
    assert!(on_disk["speakers"]["alice"].is_null(), "alice must be gone");
    assert!(!on_disk["speakers"]["bob"].is_null(), "bob must survive");
}

#[test]
fn cmd_speaker_remove_on_missing_config_still_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    // 没有声纹配置 → 读默认 {speakers:{}}，remove 是 no-op，仍写回成功
    let r = VoiceHandler::new()
        .cmd_speaker_remove(&cfg_dir, "ghost")
        .unwrap()
        .unwrap();
    assert_eq!(r["removed"], true);
    assert!(cfg_dir.join(VOICEPRINT_CONFIG_FILENAME).exists());
}

#[test]
fn cmd_speaker_set_threshold_valid_and_persisted() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    let r = VoiceHandler::new()
        .cmd_speaker_set_threshold(&cfg_dir, 0.8)
        .unwrap()
        .unwrap();
    // f32→JSON 序列化带精度尾巴（0.8f32 → 0.800000011920929），用容差比较
    assert!((r["threshold"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    let on_disk: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(cfg_dir.join(VOICEPRINT_CONFIG_FILENAME)).unwrap(),
    )
    .unwrap();
    assert!((on_disk["threshold"].as_f64().unwrap() - 0.8).abs() < 1e-6);
    // 边界值合法
    assert!(VoiceHandler::new().cmd_speaker_set_threshold(&cfg_dir, 0.0).is_ok());
    assert!(VoiceHandler::new().cmd_speaker_set_threshold(&cfg_dir, 1.0).is_ok());
}

#[test]
fn cmd_speaker_set_threshold_out_of_range_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    for bad in [-0.1f32, 1.5f32] {
        let err = VoiceHandler::new()
            .cmd_speaker_set_threshold(&cfg_dir, bad)
            .unwrap_err();
        assert!(err.contains("between 0.0 and 1.0"), "err: {err}");
    }
    // 校验先于写盘：非法值不得产生配置文件
    assert!(!cfg_dir.join(VOICEPRINT_CONFIG_FILENAME).exists());
}

#[test]
fn cmd_speaker_list_reads_seeded_voiceprint() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    seed_voiceprint(&cfg_dir, &["alice", "bob"]);
    let r = VoiceHandler::new()
        .cmd_speaker_list(&cfg_dir)
        .unwrap()
        .unwrap();
    let mut names: Vec<String> = r["speakers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["alice", "bob"]);
}

// -----------------------------------------------------------------------
// shutdown / init / tts 上限
// -----------------------------------------------------------------------

#[tokio::test]
async fn voice_shutdown_idle_is_safe() {
    // 拿 S10a 引入的 crate 级测试锁：voice_shutdown 会 take() 全部全局会话状态，
    // 与 s10_tests 的注入类测试并发会互相偷状态（env-test-race-lock-pattern）。
    let _guard = super::s10_tests::voice_state_lock().lock().unwrap();
    // 无任何活跃会话时 shutdown 不 panic、状态保持空。
    voice_shutdown().await;
    assert!(stt_state().lock().await.is_none());
}

#[tokio::test]
async fn init_engines_from_config_all_disabled_noop() {
    let dir = tempfile::tempdir().unwrap();
    // 默认模板所有引擎开关都是 false → auto-init 不装载任何引擎
    init_engines_from_config(dir.path()).await;
    assert!(stt_engine_state().lock().unwrap().is_none());
    assert!(tts_engine_state().lock().unwrap().is_none());
    assert!(speaker_engine_state().lock().unwrap().is_none());
    assert!(punct_engine_state().lock().unwrap().is_none());
    assert!(aec_state().lock().unwrap().is_none());
}

#[tokio::test]
async fn tts_rejects_text_over_1000_chars() {
    let h = VoiceHandler::new();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let long = "x".repeat(1001);
    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": long })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("too long"), "err: {err}");
    // 恰好 1000 字节不触发长度拒绝（会走到引擎缺载错误，但不是长度错）
    let exact = "y".repeat(1000);
    let r = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": exact })), &ctx)
        .await;
    match r {
        Err(e) => assert!(!e.contains("too long"), "1000 chars must not hit the length cap: {e}"),
        Ok(_) => {}
    }
}
