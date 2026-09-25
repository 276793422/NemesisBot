//! voice.rs 非豁免行覆盖第四子模块（Wave4 覆盖率批次，2026-09-25）。
//!
//! 聚焦前三批未触达的确定性命令臂（全部零硬件/零网络依赖）：
//! - `config_get` / `config_set`（缺失/存在/写读回环）
//! - `voice_config_get` / `voice_config_set`（全字段合并落盘回读）
//! - `chat_config_get` / `chat_config_set`（默认态 / 任意对象透传落盘）
//! - `speakers`（KOKORO 内置声表投影）
//! - `engine_stop` 三模型 + unknown（未加载形态 was_loaded:false）
//! - `pipeline_stop` / `stt_to_input_stop` / `stt_dialogue_stop` /
//!   `stt_dialogue_reset` / `speaker_test_stop` / `speaker_register_stop`
//!   的空状态诚实报错臂
//! - `tts_playback_stop` 未运行形态（was_running:false）
//! - `speaker_register_cancel` 空状态幂等成功
//! - `engine_status` 空态投影
//!
//! 竞态纪律（env-test-race-lock-pattern）：持 s10_tests::voice_state_lock
//! 同一把 crate 级锁串行化。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn w4_lock() -> std::sync::MutexGuard<'static, ()> {
    super::s10_tests::voice_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn w4_make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "w4".to_string(),
        chat_id: "w4".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn ok_val(v: Option<serde_json::Value>) -> serde_json::Value {
    v.expect("Ok arm must carry a payload")
}

// -----------------------------------------------------------------------
// config_get / config_set
// -----------------------------------------------------------------------

#[tokio::test]
async fn w4_config_get_reports_missing_then_present() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    let missing = ok_val(h.handle_cmd("config_get", None, &ctx).await.unwrap());
    assert_eq!(missing["exists"], false, "{missing}");
    assert_eq!(missing["content"], "", "{missing}");

    let body = serde_json::json!({ "content": "[models]\n" });
    let set = ok_val(h.handle_cmd("config_set", Some(body), &ctx).await.unwrap());
    assert_eq!(set["success"], true, "{set}");

    let present = ok_val(h.handle_cmd("config_get", None, &ctx).await.unwrap());
    assert_eq!(present["exists"], true, "{present}");
    assert_eq!(present["content"], "[models]\n", "{present}");
}

// -----------------------------------------------------------------------
// voice_config_get / voice_config_set（全字段合并）
// -----------------------------------------------------------------------

#[tokio::test]
async fn w4_voice_config_set_merges_every_documented_field() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    // 默认态：读出对象（read_voice_config 缺省回退已在 wweb2 覆盖，这里
    // 断言 dispatch 形态）。
    let before = ok_val(h.handle_cmd("voice_config_get", None, &ctx).await.unwrap());
    assert!(before.is_object(), "{before}");

    let body = serde_json::json!({
        "speaker_id": 7,
        "volume": 80,
        "speed": 1.25,
        "capture_device": "mic-0",
        "playback_device": "spk-1",
        "stt_enabled": true,
        "tts_enabled": false,
        "punct_enabled": true,
        "speaker_enabled": false,
        "silence_timeout": 4,
        "aec_enabled": true,
    });
    let set = ok_val(
        h.handle_cmd("voice_config_set", Some(body), &ctx)
            .await
            .unwrap(),
    );
    assert_eq!(set["success"], true, "{set}");

    let after = ok_val(h.handle_cmd("voice_config_get", None, &ctx).await.unwrap());
    assert_eq!(after["speaker_id"], 7, "{after}");
    assert_eq!(after["volume"], 80, "{after}");
    assert_eq!(after["speed"], 1.25, "{after}");
    assert_eq!(after["capture_device"], "mic-0", "{after}");
    assert_eq!(after["playback_device"], "spk-1", "{after}");
    assert_eq!(after["stt_enabled"], true, "{after}");
    assert_eq!(after["tts_enabled"], false, "{after}");
    assert_eq!(after["punct_enabled"], true, "{after}");
    assert_eq!(after["speaker_enabled"], false, "{after}");
    assert_eq!(after["silence_timeout"], 4, "{after}");
    assert_eq!(after["aec_enabled"], true, "{after}");
}

// -----------------------------------------------------------------------
// chat_config_get / chat_config_set
// -----------------------------------------------------------------------

#[tokio::test]
async fn w4_chat_config_get_set_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    let before = ok_val(h.handle_cmd("chat_config_get", None, &ctx).await.unwrap());
    assert!(before.is_object(), "{before}");

    let body = serde_json::json!({ "auto_send": true, "silence_ms": 1500 });
    let set = ok_val(
        h.handle_cmd("chat_config_set", Some(body), &ctx)
            .await
            .unwrap(),
    );
    assert_eq!(set["success"], true, "{set}");

    let after = ok_val(h.handle_cmd("chat_config_get", None, &ctx).await.unwrap());
    assert_eq!(after["auto_send"], true, "{after}");
    assert_eq!(after["silence_ms"], 1500, "{after}");
}

// -----------------------------------------------------------------------
// speakers / engine_status / engine_stop
// -----------------------------------------------------------------------

#[tokio::test]
async fn w4_speakers_lists_builtin_voice_table() {
    let h = VoiceHandler::new();
    let v = ok_val(
        h.handle_cmd(
            "speakers",
            None,
            &w4_make_ctx(&tempfile::tempdir().unwrap()),
        )
        .await
        .unwrap(),
    );
    let list = v["speakers"].as_array().expect("speakers array");
    assert!(!list.is_empty(), "{v}");
    for s in list {
        assert!(s["id"].is_string() && s["gender"].is_string(), "{s}");
    }
}

#[tokio::test]
async fn w4_engine_status_empty_state_reports_not_ready() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let h = VoiceHandler::new();
    let v = ok_val(
        h.handle_cmd("engine_status", None, &w4_make_ctx(&dir))
            .await
            .unwrap(),
    );
    assert_eq!(v["stt_ready"], false, "{v}");
    assert_eq!(v["tts_ready"], false, "{v}");
    assert_eq!(v["speaker_ready"], false, "{v}");
}

#[tokio::test]
async fn w4_engine_stop_not_loaded_reports_was_loaded_false() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    for model in ["stt", "tts"] {
        let v = ok_val(
            h.handle_cmd(
                "engine_stop",
                Some(serde_json::json!({ "model": model })),
                &ctx,
            )
            .await
            .unwrap(),
        );
        assert_eq!(v["stopped"], true, "{v}");
        assert_eq!(v["was_loaded"], false, "{v}");
        assert_eq!(v["model"], model, "{v}");
    }

    let v = ok_val(
        h.handle_cmd(
            "engine_stop",
            Some(serde_json::json!({ "model": "speaker" })),
            &ctx,
        )
        .await
        .unwrap(),
    );
    assert_eq!(v["stopped"], true, "{v}");

    let err = h
        .handle_cmd(
            "engine_stop",
            Some(serde_json::json!({ "model": "weird" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown model"), "{err}");
}

// -----------------------------------------------------------------------
// stop/reset 空状态诚实报错臂
// -----------------------------------------------------------------------

#[tokio::test]
async fn w4_stop_commands_report_honest_errors_when_idle() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    let err = h
        .handle_cmd(
            "pipeline_stop",
            Some(serde_json::json!({ "model": "stt" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("STT pipeline not running"), "{err}");

    let err = h
        .handle_cmd(
            "pipeline_stop",
            Some(serde_json::json!({ "model": "tts" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("pipeline not supported"), "{err}");

    let err = h
        .handle_cmd("stt_to_input_stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("STT dictation not running"), "{err}");

    let err = h
        .handle_cmd("stt_dialogue_stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("STT dialogue not running"), "{err}");

    let err = h
        .handle_cmd("stt_dialogue_reset", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("No dialogue session active"), "{err}");

    let err = h
        .handle_cmd("speaker_test_stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("No speaker test running"), "{err}");

    let err = h
        .handle_cmd("speaker_register_stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("No registration in progress"), "{err}");
}

#[tokio::test]
async fn w4_idle_safe_stops_succeed_quietly() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    // tts_playback_stop 空态 = Ok(was_running:false)（幂等语义）
    let v = ok_val(h.handle_cmd("tts_playback_stop", None, &ctx).await.unwrap());
    assert_eq!(v["stopped"], true, "{v}");
    assert_eq!(v["was_running"], false, "{v}");

    // speaker_register_cancel 空态 = Ok(cancelled:true)（幂等语义）
    let v = ok_val(
        h.handle_cmd("speaker_register_cancel", None, &ctx)
            .await
            .unwrap(),
    );
    assert_eq!(v["cancelled"], true, "{v}");
}

#[tokio::test]
async fn w4_unknown_command_lists_command_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .handle_cmd("definitely_not_a_command", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command: voice."), "{err}");
}

// ===========================================================================
// Wave5 覆盖批次：确定性命令臂——目录被文件占位的 create_dir_all 失败、
// config.toml 缺失门、install_model 锁/未知模型、tts 文本校验、stt_start
// 已运行/引擎自举失败、engine_start 未知模型、speaker 注册互斥。
// 全部零网络/零硬件；同样持 voice_state_lock 串行化全局状态。
// ===========================================================================

/// 把 `<ws>/tools/voice` 变成普通文件 → handler 内 voice_dir 上的
/// create_dir_all 必炸（setup/install_runtime/install_aec 三条线的失败门）。
fn w5_block_voice_dir(dir: &std::path::Path) {
    std::fs::create_dir_all(dir.join("tools")).unwrap();
    std::fs::write(dir.join("tools").join("voice"), "not a dir").unwrap();
}

/// 写一份最小 config.toml（load_or_default 容忍任意内容）。
fn w5_write_voice_config(dir: &std::path::Path) {
    let voice_dir = dir.join("tools").join("voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    std::fs::write(voice_dir.join("config.toml"), "[tts]\nmodel_name = \"x\"\n").unwrap();
}

#[tokio::test]
async fn w5_setup_fails_when_voice_dir_blocked() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    w5_block_voice_dir(dir.path());
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h.handle_cmd("setup", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to create voice dir"), "{err}");
}

#[tokio::test]
async fn w5_install_runtime_fails_when_voice_dir_blocked() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    w5_block_voice_dir(dir.path());
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .handle_cmd("install_runtime", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("failed to create voice dir"), "{err}");
}

#[tokio::test]
async fn w5_install_aec_fails_when_aec_dir_blocked() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    w5_block_voice_dir(dir.path());
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h.handle_cmd("install_aec", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to create aec dir"), "{err}");
}

#[tokio::test]
async fn w5_install_model_requires_config_toml() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({ "model": "stt" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("config.toml not found"), "{err}");
}

#[tokio::test]
async fn w5_install_model_unknown_type_fails_without_network() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    w5_write_voice_config(dir.path());
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({ "model": "bogus_model" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown model type: bogus_model"), "{err}");
    // 锁已随流程释放：再次调用同样报未知模型（不会卡「正在安装中」）。
    let err2 = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({ "model": "bogus_model" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err2.contains("unknown model type"), "{err2}");
}

#[tokio::test]
async fn w5_install_model_rejects_while_in_flight() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    w5_write_voice_config(dir.path());
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    // 直接占住 per-model 安装锁 → 命令必须立刻拒绝（1110-1112）。
    {
        let mut locks = install_locks().lock().unwrap();
        locks.insert("stt".to_string());
    }
    let err = h
        .handle_cmd(
            "install_model",
            Some(serde_json::json!({ "model": "stt" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("正在安装中"), "{err}");
    // 清理：提前退出不会走到释放段，手动摘除。
    install_locks().lock().unwrap().remove("stt");
}

#[tokio::test]
async fn w5_tts_validates_text_and_requires_setup() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    let err = h
        .handle_cmd("tts", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: text"), "{err}");

    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": "   " })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("text cannot be empty"), "{err}");

    let long_text = "a".repeat(1001);
    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": long_text })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("text too long"), "{err}");

    // 文本合法但没跑过 setup → 诚实报错（spawn_blocking 内 config 门）。
    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": "你好" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("Voice not set up. Run setup first."), "{err}");
}

#[tokio::test]
async fn w5_stt_start_rejects_when_already_running() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    {
        let mut state = stt_state().lock().await;
        *state = Some(SttSession {
            cancel: tokio_util::sync::CancellationToken::new(),
            dialogue_output: None,
        });
    }
    let err = h.handle_cmd("stt_start", None, &ctx).await.unwrap_err();
    assert!(err.contains("STT dictation already running"), "{err}");
    stt_state().lock().await.take();
}

#[tokio::test]
async fn w5_stt_start_fails_when_engine_autoload_fails() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    // 无 config.toml → 持久引擎自举失败 → stt_start 直接 Err，且不残留状态。
    let err = h.handle_cmd("stt_start", None, &ctx).await.unwrap_err();
    assert!(err.contains("config.toml not found"), "{err}");
    assert!(
        stt_state().lock().await.is_none(),
        "失败路径不得残留会话状态"
    );
}

#[tokio::test]
async fn w5_engine_start_rejects_unknown_model_and_requires_setup() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();

    let err = h
        .handle_cmd(
            "engine_start",
            Some(serde_json::json!({ "model": "bogus" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown model: bogus"), "{err}");

    for model in ["stt", "tts", "speaker"] {
        let err = h
            .handle_cmd(
                "engine_start",
                Some(serde_json::json!({ "model": model })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("config.toml not found"), "{model}: {err}");
    }
}

#[tokio::test]
async fn w5_speaker_test_start_surfaces_stt_autoload_failure() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = w4_make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .cmd_speaker_test_start(
            &dir.path().join("tools").join("voice"),
            &dir.path().join("config"),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("STT engine auto-load failed"), "{err}");
}

#[tokio::test]
async fn w5_speaker_register_start_rejects_while_in_progress() {
    let _guard = w4_lock();
    let dir = tempfile::tempdir().unwrap();
    let h = VoiceHandler::new();

    {
        let mut reg = speaker_register_state().lock().unwrap();
        *reg = Some(SpeakerRegistration {
            name: "owner".to_string(),
            samples: std::sync::Mutex::new(Vec::new()),
            sample_rate: 16000,
            start_time: std::time::Instant::now(),
            cancel: tokio_util::sync::CancellationToken::new(),
        });
    }
    let err = h
        .cmd_speaker_register_start(&dir.path().join("config"), "other")
        .unwrap_err();
    assert!(
        err.contains("Speaker registration already in progress"),
        "{err}"
    );
    speaker_register_state().lock().unwrap().take();
}
