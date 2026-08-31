//! voice.rs 非豁免行覆盖（quality-hardening goal 冲刺 S10a，2026-08-25）。
//!
//! 与 wweb2_tests 同款子模块声明以获得私有项访问。聚焦 VE1 豁免范围之外的：
//! WSAPI dispatch 提取臂、start/stop 状态机（通过注入会话/令牌到全局状态）、
//! 配置读写（config.toml / config.voice.json / config.chat.json）、
//! 纯逻辑 helper（handle_tts_failure / punctuate_if_loaded）、push 广播 spawn 路径。
//!
//! 不触碰：真音频 DLL、模型下载、真麦克风（VE1 范围见台账 §9.4）。
//!
//! 竞态纪律（env-test-race-lock-pattern）：voice.rs 的全局 OnceLock 状态是
//! 进程级共享的，所有会注入/清空全局状态的本模块测试 + wweb2_tests 的
//! voice_shutdown_idle_is_safe 都必须先拿同一把 crate 级测试锁。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// crate 级共享测试锁：串行化所有会写 voice.rs 全局状态的测试。
/// `pub(super)` 让兄弟模块 wweb2_tests 也能拿到（voice_shutdown_idle_is_safe）。
pub(super) fn voice_state_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

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
        board: None,
    });
    RequestContext {
        session_id: "s10".to_string(),
        chat_id: "c10".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn noop_push_fn() -> Box<dyn Fn(&str) + Send + Sync> {
    Box::new(|_: &str| {})
}

fn make_dialogue_output() -> Arc<DialogueSttOutput> {
    Arc::new(DialogueSttOutput {
        push_fn: noop_push_fn(),
        state: Arc::new(std::sync::Mutex::new(DialogueState {
            buffer: String::new(),
            silence_timeout_secs: 3.0,
            reset_flag: false,
        })),
    })
}

/// 清空所有会注入的全局会话状态（engine 状态持真引擎对象，测试无法构造，跳过）。
async fn clear_session_states() {
    *stt_state().lock().await = None;
    *dialogue_state().lock().await = None;
    *tts_playback_state().lock().await = None;
    *speaker_register_state().lock().unwrap() = None;
    *speaker_test_state().lock().await = None;
    *setup_cancel().lock().unwrap() = None;
}

// -----------------------------------------------------------------------
// WSAPI dispatch：提取错误 / 未知命令 / workspace 缺失
// -----------------------------------------------------------------------

#[tokio::test]
async fn dispatch_missing_data_and_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // None data → "missing data"（所有需要 data 的命令）
    for cmd in [
        "install_model",
        "config_set",
        "voice_config_set",
        "tts",
        "engine_start",
        "engine_stop",
        "pipeline_start",
        "pipeline_stop",
        "speaker_register_start",
        "speaker_remove",
        "speaker_set_threshold",
    ] {
        let err = h
            .handle_cmd(cmd, None, &ctx)
            .await
            .unwrap_err_or(format!("cmd {cmd} must reject None data"));
        assert_eq!(err, "missing data", "cmd={cmd}");
    }

    // 空 JSON → 各自的 missing field
    let empty = serde_json::json!({});
    for cmd in [
        "install_model",
        "engine_start",
        "engine_stop",
        "pipeline_start",
        "pipeline_stop",
    ] {
        let err = h
            .handle_cmd(cmd, Some(empty.clone()), &ctx)
            .await
            .unwrap_err_or(format!("cmd {cmd} must reject empty data"));
        assert_eq!(err, "missing field: model", "cmd={cmd}");
    }
    assert_eq!(
        h.handle_cmd("speaker_remove", Some(empty.clone()), &ctx)
            .await
            .unwrap_err_or("speaker_remove must reject empty"),
        "missing field: name"
    );
    assert_eq!(
        h.handle_cmd("speaker_set_threshold", Some(empty.clone()), &ctx)
            .await
            .unwrap_err_or("speaker_set_threshold must reject empty"),
        "missing field: threshold"
    );
    assert_eq!(
        h.handle_cmd("tts", Some(empty), &ctx)
            .await
            .unwrap_err_or("tts must reject empty"),
        "missing field: text"
    );
}

#[tokio::test]
async fn dispatch_unknown_command_and_missing_workspace() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();
    assert_eq!(
        h.handle_cmd("bogus", None, &ctx).await.unwrap_err(),
        "unknown command: voice.bogus"
    );

    // workspace 未配置 → require_workspace 拦截
    let mut no_ws_ctx = make_ctx(&dir);
    no_ws_ctx.workspace = None;
    assert_eq!(
        h.handle_cmd("status", None, &no_ws_ctx).await.unwrap_err(),
        "workspace not configured"
    );
}

// -----------------------------------------------------------------------
// stop_setup：有 / 无进行中的 setup 令牌
// -----------------------------------------------------------------------

#[tokio::test]
async fn stop_setup_with_and_without_token() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 无令牌 → Err
    assert_eq!(
        h.handle_cmd("stop_setup", None, &ctx).await.unwrap_err(),
        "no setup in progress"
    );

    // 注入令牌 → stopped:true，令牌被消费
    *setup_cancel().lock().unwrap() = Some(CancellationToken::new());
    let r = h.handle_cmd("stop_setup", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["stopped"], true);
    assert!(setup_cancel().lock().unwrap().is_none(), "token consumed");
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// config.toml / config.voice.json / config.chat.json 读写
// -----------------------------------------------------------------------

#[tokio::test]
async fn config_get_missing_then_present() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    let r = h.handle_cmd("config_get", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["exists"], false);
    assert_eq!(r["content"], "");

    let voice_dir = dir.path().join("tools/voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    std::fs::write(voice_dir.join("config.toml"), "key = 1").unwrap();
    let r = h.handle_cmd("config_get", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["exists"], true);
    assert_eq!(r["content"], "key = 1");
}

#[tokio::test]
async fn config_set_writes_file_and_creates_dir() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    let r = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({ "content": "[models]\ndir = \"data\"" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["success"], true);
    let on_disk =
        std::fs::read_to_string(dir.path().join("tools/voice/config.toml")).unwrap();
    assert_eq!(on_disk, "[models]\ndir = \"data\"");
}

#[tokio::test]
async fn voice_config_get_returns_default_and_set_updates_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 无文件 → 嵌入默认模板（stt_enabled 等布尔字段存在）
    let r = h
        .handle_cmd("voice_config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r.get("stt_enabled").is_some(), "default config keys expected");

    // 全字段更新
    let r = h
        .handle_cmd(
            "voice_config_set",
            Some(serde_json::json!({
                "speaker_id": 47,
                "volume": 80,
                "speed": 1.5,
                "capture_device": "Mic 1",
                "playback_device": "Speakers",
                "stt_enabled": true,
                "tts_enabled": true,
                "punct_enabled": true,
                "speaker_enabled": true,
                "silence_timeout": 4.5,
                "aec_enabled": true,
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["success"], true);

    let got = h
        .handle_cmd("voice_config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["speaker_id"], 47);
    assert_eq!(got["volume"], 80);
    assert_eq!(got["speed"], 1.5);
    assert_eq!(got["capture_device"], "Mic 1");
    assert_eq!(got["playback_device"], "Speakers");
    assert_eq!(got["stt_enabled"], true);
    assert_eq!(got["tts_enabled"], true);
    assert_eq!(got["punct_enabled"], true);
    assert_eq!(got["speaker_enabled"], true);
    assert_eq!(got["silence_timeout"], 4.5);
    assert_eq!(got["aec_enabled"], true);
    // 持久化到磁盘（不是只在返回值里）
    let path = dir.path().join("config").join(VOICE_CONFIG_FILENAME);
    assert!(path.exists(), "voice config must persist to disk");
}

#[tokio::test]
async fn chat_config_get_creates_default_and_set_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 首次 get 创建默认文件并返回其内容（JSON 对象）
    let r = h
        .handle_cmd("chat_config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert!(r.is_object(), "default chat config is a JSON object");
    assert!(dir.path().join("config").join(CHAT_CONFIG_FILENAME).exists());

    // set 整体替换
    let r = h
        .handle_cmd(
            "chat_config_set",
            Some(serde_json::json!({ "auto_send": true, "max_len": 200 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["success"], true);
    let got = h
        .handle_cmd("chat_config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got["auto_send"], true);
    assert_eq!(got["max_len"], 200);
}

// -----------------------------------------------------------------------
// 状态查询类命令
// -----------------------------------------------------------------------

#[tokio::test]
async fn engine_status_all_idle_and_dialogue_flag_false() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let r = VoiceHandler::new()
        .handle_cmd("engine_status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stt_ready"], false);
    assert_eq!(r["tts_ready"], false);
    assert_eq!(r["speaker_ready"], false);
    assert_eq!(r["stt_dialogue_active"], false);
}

#[tokio::test]
async fn speakers_lists_kokoro_table() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let r = VoiceHandler::new()
        .handle_cmd("speakers", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let arr = r["speakers"].as_array().unwrap();
    assert_eq!(arr.len(), KOKORO_SPEAKERS.len());
    for (i, s) in arr.iter().enumerate() {
        assert_eq!(s["id"], KOKORO_SPEAKERS[i].0);
        assert_eq!(s["speaker_id"], KOKORO_SPEAKERS[i].2);
        assert!(s["gender"].is_string());
    }
}

#[tokio::test]
async fn devices_tolerant_of_host_audio_stack() {
    // list_devices 走宿主音频栈枚举：无断言具体硬件，只断言两种合法出口之一。
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let r = VoiceHandler::new().handle_cmd("devices", None, &ctx).await;
    match r {
        Ok(Some(v)) => {
            assert!(v["input"].is_array());
            assert!(v["output"].is_array());
            assert!(v["total"].is_u64());
        }
        Ok(None) => panic!("devices must return a payload"),
        Err(e) => assert!(e.contains("failed to list audio devices"), "err: {e}"),
    }
}

// -----------------------------------------------------------------------
// STT start/stop 状态机（注入会话）
// -----------------------------------------------------------------------

#[tokio::test]
async fn stt_start_already_running_then_engine_load_fails_fast() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 注入 dictation 会话 → already running
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    assert_eq!(
        h.handle_cmd("stt_start", None, &ctx).await.unwrap_err(),
        "STT dictation already running"
    );
    *stt_state().lock().await = None;

    // 空 voice 目录 → 引擎加载在 config.toml 检查处快速失败（不碰 DLL 下载）
    let err = h.handle_cmd("stt_start", None, &ctx).await.unwrap_err();
    assert!(err.contains("config.toml not found"), "err: {err}");
    clear_session_states().await;
}

#[tokio::test]
async fn stt_stop_both_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    let r = h.handle_cmd("stt_stop", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["stopped"], true);

    assert_eq!(
        h.handle_cmd("stt_stop", None, &ctx).await.unwrap_err(),
        "STT dictation not running"
    );
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// engine_start / engine_stop 分派
// -----------------------------------------------------------------------

#[tokio::test]
async fn engine_start_dispatch_all_four_arms() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // stt/tts/speaker 都在 spawn_blocking 里先查 config.toml → 空目录快速失败
    for model in ["stt", "tts", "speaker"] {
        let err = h
            .handle_cmd("engine_start", Some(serde_json::json!({ "model": model })), &ctx)
            .await
            .unwrap_err_or(format!("engine_start {model} must fail on empty dir"));
        assert!(err.contains("config.toml not found"), "model={model} err={err}");
    }
    // 未知 model
    assert_eq!(
        h.handle_cmd("engine_start", Some(serde_json::json!({ "model": "weird" })), &ctx)
            .await
            .unwrap_err(),
        "unknown model: weird"
    );
}

#[tokio::test]
async fn engine_stop_all_four_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 未加载的引擎 → stopped + was_loaded:false
    let r = h
        .handle_cmd("engine_stop", Some(serde_json::json!({ "model": "stt" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert_eq!(r["was_loaded"], false);

    let r = h
        .handle_cmd("engine_stop", Some(serde_json::json!({ "model": "tts" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert_eq!(r["was_loaded"], false);

    // speaker 臂无条件清 engine/manager 并把 enabled 置 false
    *speaker_enabled_state().lock().unwrap() = true;
    let r = h
        .handle_cmd("engine_stop", Some(serde_json::json!({ "model": "speaker" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert_eq!(r["model"], "speaker");
    assert!(!*speaker_enabled_state().lock().unwrap(), "enabled must be reset");

    assert_eq!(
        h.handle_cmd("engine_stop", Some(serde_json::json!({ "model": "weird" })), &ctx)
            .await
            .unwrap_err(),
        "unknown model: weird"
    );
}

// -----------------------------------------------------------------------
// pipeline start/stop
// -----------------------------------------------------------------------

#[tokio::test]
async fn pipeline_start_unsupported_model_and_engine_not_loaded() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    assert_eq!(
        h.handle_cmd("pipeline_start", Some(serde_json::json!({ "model": "tts" })), &ctx)
            .await
            .unwrap_err(),
        "pipeline not supported for model: tts"
    );
    // stt → 引擎未加载先于会话检查
    assert_eq!(
        h.handle_cmd("pipeline_start", Some(serde_json::json!({ "model": "stt" })), &ctx)
            .await
            .unwrap_err(),
        "STT engine not loaded. Enable STT toggle first."
    );
    clear_session_states().await;
}

#[tokio::test]
async fn pipeline_stop_all_three_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    let r = h
        .handle_cmd("pipeline_stop", Some(serde_json::json!({ "model": "stt" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);

    assert_eq!(
        h.handle_cmd("pipeline_stop", Some(serde_json::json!({ "model": "stt" })), &ctx)
            .await
            .unwrap_err(),
        "STT pipeline not running"
    );

    assert_eq!(
        h.handle_cmd("pipeline_stop", Some(serde_json::json!({ "model": "vad" })), &ctx)
            .await
            .unwrap_err(),
        "pipeline not supported for model: vad"
    );
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// dictation（stt_to_input）
// -----------------------------------------------------------------------

#[tokio::test]
async fn stt_to_input_start_prechecks_and_stop_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 已有会话 → already running（先于引擎检查）
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    assert_eq!(
        h.handle_cmd("stt_to_input_start", None, &ctx).await.unwrap_err(),
        "STT already running. Stop current session first."
    );
    *stt_state().lock().await = None;

    // 引擎未加载
    assert_eq!(
        h.handle_cmd("stt_to_input_start", None, &ctx).await.unwrap_err(),
        "STT engine not loaded. Enable STT in voice settings first."
    );

    // stop：Some → stopped；None → not running
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    let r = h
        .handle_cmd("stt_to_input_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert_eq!(
        h.handle_cmd("stt_to_input_stop", None, &ctx).await.unwrap_err(),
        "STT dictation not running"
    );
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// dialogue 模式
// -----------------------------------------------------------------------

#[tokio::test]
async fn stt_dialogue_start_prechecks_and_timeout_extraction() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 已有会话 → already running
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: None,
    });
    assert_eq!(
        h.handle_cmd("stt_dialogue_start", None, &ctx).await.unwrap_err(),
        "STT already running. Stop current session first."
    );
    *stt_state().lock().await = None;

    // 引擎未加载（None data → 默认 3.0 超时提取臂；带 silence_timeout → 显式提取臂）
    assert_eq!(
        h.handle_cmd("stt_dialogue_start", None, &ctx).await.unwrap_err(),
        "STT engine not loaded. Enable STT in voice settings first."
    );
    assert_eq!(
        h.handle_cmd(
            "stt_dialogue_start",
            Some(serde_json::json!({ "silence_timeout": 7.5 })),
            &ctx
        )
        .await
        .unwrap_err(),
        "STT engine not loaded. Enable STT in voice settings first."
    );
    clear_session_states().await;
}

#[tokio::test]
async fn stt_dialogue_stop_flushes_and_clears_state() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 注入带 dialogue_output 的会话 + dialogue 全局态 → stop 走 flush 分支
    let out = make_dialogue_output();
    out.send_text("残留文本");
    *dialogue_state().lock().await = Some(out.clone());
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: Some(out),
    });
    let r = h
        .handle_cmd("stt_dialogue_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert!(stt_state().lock().await.is_none());
    assert!(dialogue_state().lock().await.is_none(), "dialogue state cleared");
    // flush 已把缓冲取走
    let cleared = dialogue_state().lock().await.clone();
    assert!(cleared.is_none() || cleared.unwrap().flush().is_none());

    assert_eq!(
        h.handle_cmd("stt_dialogue_stop", None, &ctx).await.unwrap_err(),
        "STT dialogue not running"
    );
    clear_session_states().await;
}

#[tokio::test]
async fn stt_dialogue_reset_both_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    assert_eq!(
        h.handle_cmd("stt_dialogue_reset", None, &ctx).await.unwrap_err(),
        "No dialogue session active"
    );

    *dialogue_state().lock().await = Some(make_dialogue_output());
    let r = h
        .handle_cmd("stt_dialogue_reset", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["reset"], true);
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// TTS playback stop
// -----------------------------------------------------------------------

#[tokio::test]
async fn tts_playback_stop_both_arms() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 注入 manager（tx 用普通 mpsc channel 的 sender，rx 直接丢弃）
    let (tx, _rx) = std::sync::mpsc::channel::<TtsPlaybackItem>();
    *tts_playback_state().lock().await = Some(TtsPlaybackManager {
        tx,
        cancel: CancellationToken::new(),
    });
    let r = h
        .handle_cmd("tts_playback_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert!(r.get("was_running").is_none(), "was_running only on idle arm");
    assert!(tts_playback_state().lock().await.is_none());

    // 空闲臂
    let r = h
        .handle_cmd("tts_playback_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert_eq!(r["was_running"], false);
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// speaker register / test
// -----------------------------------------------------------------------

fn make_registration(
    start_time: std::time::Instant,
    samples: Vec<f32>,
) -> SpeakerRegistration {
    SpeakerRegistration {
        name: "alice".to_string(),
        samples: std::sync::Mutex::new(samples),
        sample_rate: 16000,
        start_time,
        cancel: CancellationToken::new(),
    }
}

#[tokio::test]
async fn speaker_register_start_requires_engine_and_default_name() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // ensure_speaker_engine 在 AudioCapture 之前先查 config.toml → 快速失败，
    // 绝不会碰麦克风。data 无 name → 默认 "owner" 提取臂也被走到。
    let err = h
        .handle_cmd("speaker_register_start", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("config.toml not found"), "err: {err}");
}

#[tokio::test]
async fn speaker_register_stop_error_ladder() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 1) 无进行中的注册
    assert_eq!(
        h.handle_cmd("speaker_register_stop", None, &ctx)
            .await
            .unwrap_err(),
        "No registration in progress"
    );

    // 2) 录音时间不足 5s（有样本也拦）
    *speaker_register_state().lock().unwrap() =
        Some(make_registration(std::time::Instant::now(), vec![0.1, 0.2]));
    let err = h
        .handle_cmd("speaker_register_stop", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("录音时间过短"), "err: {err}");
    assert!(err.contains("至少需要5秒"), "err: {err}");

    // 3) 时间够但没录到样本
    let past = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(6))
        .expect("QPC clock supports going 6s back");
    *speaker_register_state().lock().unwrap() = Some(make_registration(past, vec![]));
    assert_eq!(
        h.handle_cmd("speaker_register_stop", None, &ctx)
            .await
            .unwrap_err(),
        "未录到音频数据"
    );

    // 4) 时间够、有样本、但引擎没加载
    *speaker_register_state().lock().unwrap() =
        Some(make_registration(past, vec![0.5; 160]));
    assert_eq!(
        h.handle_cmd("speaker_register_stop", None, &ctx)
            .await
            .unwrap_err(),
        "Speaker engine not loaded"
    );
    clear_session_states().await;
}

#[tokio::test]
async fn speaker_register_cancel_with_and_without_session() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    *speaker_register_state().lock().unwrap() =
        Some(make_registration(std::time::Instant::now(), vec![]));
    let r = h
        .handle_cmd("speaker_register_cancel", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cancelled"], true);
    assert!(speaker_register_state().lock().unwrap().is_none());

    // 无会话也是 Ok（幂等取消）
    let r = h
        .handle_cmd("speaker_register_cancel", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cancelled"], true);
    clear_session_states().await;
}

#[tokio::test]
async fn speaker_test_stop_three_paths() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    assert_eq!(
        h.handle_cmd("speaker_test_stop", None, &ctx).await.unwrap_err(),
        "No speaker test running"
    );

    *speaker_test_state().lock().await = Some(SpeakerTestSession {
        cancel: CancellationToken::new(),
        auto_loaded_stt: true,
    });
    let r = h
        .handle_cmd("speaker_test_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);

    *speaker_test_state().lock().await = Some(SpeakerTestSession {
        cancel: CancellationToken::new(),
        auto_loaded_stt: false,
    });
    let r = h
        .handle_cmd("speaker_test_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    clear_session_states().await;
}

#[tokio::test]
async fn speaker_remove_and_threshold_via_dispatch() {
    let _guard = voice_state_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();

    // 声纹文件里预置 alice → speaker_remove 走 dispatch 臂删除并落盘
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join(VOICEPRINT_CONFIG_FILENAME),
        serde_json::to_string(&serde_json::json!({
            "threshold": 0.65,
            "speakers": { "alice": { "embedding": [0.1] } }
        }))
        .unwrap(),
    )
    .unwrap();
    let r = h
        .handle_cmd(
            "speaker_remove",
            Some(serde_json::json!({ "name": "alice" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["removed"], true);
    assert_eq!(r["name"], "alice");

    // 阈值：合法落盘 / 越界拒绝
    let r = h
        .handle_cmd(
            "speaker_set_threshold",
            Some(serde_json::json!({ "threshold": 0.9 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!((r["threshold"].as_f64().unwrap() - 0.9).abs() < 1e-6);
    let err = h
        .handle_cmd(
            "speaker_set_threshold",
            Some(serde_json::json!({ "threshold": 2.0 })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("between 0.0 and 1.0"), "err: {err}");
}

// -----------------------------------------------------------------------
// voice_shutdown：注入会话后的取消臂
// -----------------------------------------------------------------------

#[tokio::test]
async fn voice_shutdown_cancels_injected_sessions_and_clears_state() {
    let _guard = voice_state_lock().lock().unwrap();
    clear_session_states().await;

    let out = make_dialogue_output();
    *stt_state().lock().await = Some(SttSession {
        cancel: CancellationToken::new(),
        dialogue_output: Some(out),
    });
    *dialogue_state().lock().await = Some(make_dialogue_output());
    *speaker_register_state().lock().unwrap() =
        Some(make_registration(std::time::Instant::now(), vec![]));
    *speaker_test_state().lock().await = Some(SpeakerTestSession {
        cancel: CancellationToken::new(),
        auto_loaded_stt: false,
    });
    let (tx, _rx) = std::sync::mpsc::channel::<TtsPlaybackItem>();
    *tts_playback_state().lock().await = Some(TtsPlaybackManager {
        tx,
        cancel: CancellationToken::new(),
    });

    voice_shutdown().await;

    assert!(stt_state().lock().await.is_none());
    assert!(speaker_register_state().lock().unwrap().is_none());
    assert!(speaker_test_state().lock().await.is_none());
    assert!(tts_playback_state().lock().await.is_none());
    clear_session_states().await;
}

// -----------------------------------------------------------------------
// 纯逻辑 helper
// -----------------------------------------------------------------------

#[test]
fn handle_tts_failure_three_branches() {
    // 1) 未达阈值 → 不动引擎也不计重启
    let mut attempts = 0u32;
    handle_tts_failure(0, 3, &mut attempts, 2);
    assert_eq!(attempts, 0);
    handle_tts_failure(2, 3, &mut attempts, 2);
    assert_eq!(attempts, 0);

    // 2) 达阈值 + 重启预算未耗尽 → 重启计数 +1
    handle_tts_failure(3, 3, &mut attempts, 2);
    assert_eq!(attempts, 1);

    // 3) 达阈值 + 预算耗尽 → 引擎判死，计数不再增长
    handle_tts_failure(3, 3, &mut attempts, 2);
    assert_eq!(attempts, 2, "max-restarts reached, counter must stop");
}

#[test]
fn punctuate_without_engine_returns_text_verbatim() {
    assert_eq!(punctuate_if_loaded("你好世界"), "你好世界");
    assert_eq!(punctuate_if_loaded(""), "");
}

// -----------------------------------------------------------------------
// push 广播 spawn 路径（session 不存在 → warn 臂）
// -----------------------------------------------------------------------

#[tokio::test]
async fn push_helpers_broadcast_to_missing_session_take_warn_arm() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    // session "ghost" 未注册 → broadcast Err → 闭包内 warn 分支被执行
    push_speaker_rejected("ghost", mgr.clone());
    push_stt_result("ghost", mgr.clone(), "text");
    push_stt_to_input("ghost", mgr.clone(), "text");
    push_stt_dialogue("ghost", mgr.clone(), "stt_dialogue_text", "text");
    push_stt_dialogue("ghost", mgr.clone(), "stt_accumulate", "acc");
    // 给 spawn 的任务留运行窗口（Handle::current() spawn 到本测试 runtime）
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

#[tokio::test]
async fn ws_stt_output_delegates_to_push() {
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let out = WsSttOutput {
        session_id: "ghost".to_string(),
        session_mgr: mgr,
    };
    out.send_text("hello");
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

#[test]
fn noop_tts_input_listen_is_a_noop() {
    let input = NoopTtsInput;
    input.listen(Box::new(|_: &str| panic!("NoopTtsInput must never fire")));
}

// -----------------------------------------------------------------------
// 辅助 trait：让 Option 处理更可读（仅测试用）
// -----------------------------------------------------------------------

trait UnwrapErrOr<T> {
    fn unwrap_err_or(self, msg: impl std::fmt::Display) -> T;
}

impl<T> UnwrapErrOr<String> for Result<T, String> {
    fn unwrap_err_or(self, msg: impl std::fmt::Display) -> String {
        match self {
            Err(e) => e,
            Ok(_) => panic!("{msg}"),
        }
    }
}

// -----------------------------------------------------------------------
// S10a 补充批次 2：llvm-cov 复测后仍缺的臂
// -----------------------------------------------------------------------

#[tokio::test]
async fn status_probes_model_subdirs_with_and_without_onnx() {
    // check_model_subdir_any / has_onnx_file 的 true 与 false 路径：
    // stt 子目录带 .onnx → ready=true；tts 子目录只有 txt → ready=false；
    // 其余目录缺席 → exists-false 臂（已覆盖，此处保持断言完整）。
    let _guard = voice_state_lock().lock().unwrap_or_else(|e| e.into_inner());
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let voice = dir.path().join("tools").join("voice");
    let stt_sub = voice.join("data").join("stt").join("m1");
    std::fs::create_dir_all(&stt_sub).unwrap();
    std::fs::write(stt_sub.join("model.onnx"), b"fake").unwrap();
    let tts_sub = voice.join("data").join("tts").join("m1");
    std::fs::create_dir_all(&tts_sub).unwrap();
    std::fs::write(tts_sub.join("readme.txt"), b"not a model").unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();
    let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["models"]["stt"]["ready"], true);
    assert_eq!(r["models"]["tts"]["ready"], false);
    assert_eq!(r["models"]["vad"]["ready"], false);
    assert_eq!(r["config_exists"], false);
}

#[tokio::test]
async fn stt_start_fails_fast_when_engine_unloaded_and_config_missing() {
    // cmd_stt_start 的 needs_load 臂：引擎未加载 → 先起引擎 →
    // voice_dir 无 config.toml → spawn_blocking 内 fail-fast，
    // 全程不碰 DLL / 模型文件 / 麦克风。
    let _guard = voice_state_lock().lock().unwrap_or_else(|e| e.into_inner());
    clear_session_states().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = VoiceHandler::new();
    let err = h
        .handle_cmd("stt_start", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("config.toml"), "unexpected err: {err}");
}
