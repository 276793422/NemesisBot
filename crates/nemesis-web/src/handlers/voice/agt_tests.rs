//! voice.rs 非豁免行覆盖第三子模块（AGT 覆盖率批次，2026-09-24）。
//!
//! 与 wweb2_tests / s10_tests 同款子模块声明以获得私有项访问。聚焦 s10
//! 未覆盖的确定性可达臂：
//! - `VoiceHandler::default` / `module_name` / `commands`（模块接口面）
//! - `cmd_setup` / `cmd_install_runtime` / `cmd_install_aec` 的 create_dir_all
//!   失败臂（voice_dir 被预置为普通文件 → spawn_blocking 内快速失败，不碰网络）
//! - `cmd_install_model` 三段：per-model 锁冲突臂 / config.toml 缺失臂 /
//!   unknown model 全错误路径（进度回调装配 + SSE publish + 锁释放）
//! - `cmd_tts` 纯校验段：空文本 / 超 1000 字符 / config.toml 缺失门
//! - `cmd_voice_config_set` 现值非对象（config.voice.json 写成 `[]`）
//! - `init_engines_from_config` 三引擎 enabled=true 的 fail-fast warn 臂
//! - `cmd_tts_playback`：空文本 / channel-closed / manager 创建 + 后台循环
//!   在 config.toml 缺失时即刻退出（无网络）
//! - `cmd_speaker_remove` 注入 SpeakerManager 后的 manager.remove 臂 +
//!   `cmd_speaker_status` 状态投影
//! - `voice_shutdown` 注入 SpeakerManager 后的 manager 释放臂
//!
//! 不触碰（结构性豁免，见报告 VE-voice-2）：一切需要真 ONNX 引擎对象 /
//! sherpa DLL / 模型下载（init_speaker 之后的路径全是网络依赖）/ 真麦克风的臂。
//!
//! 竞态纪律（env-test-race-lock-pattern）：所有会注入/清空 voice.rs 全局状态
//! 的测试持 s10_tests::voice_state_lock 同一把 crate 级锁。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_voice_state_lock() -> &'static std::sync::Mutex<()> {
    super::s10_tests::voice_state_lock()
}

/// 持锁（毒化自愈：一个测试 panic 后其余测试照常串行，不再级联 PoisonError）。
fn agt_lock() -> std::sync::MutexGuard<'static, ()> {
    agt_voice_state_lock()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

fn agt_make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 清空本批次会触碰的全部全局状态（engine 对象无法构造，本来就保持 None）。
async fn agt_clear_all() {
    *speaker_register_state().lock().unwrap() = None;
    *speaker_manager_state().lock().unwrap() = None;
    *speaker_test_state().lock().await = None;
    *tts_playback_state().lock().await = None;
    *stt_state().lock().await = None;
    *dialogue_state().lock().await = None;
    *setup_cancel().lock().unwrap() = None;
    *speaker_enabled_state().lock().unwrap() = false;
    install_locks().lock().unwrap().clear();
}

/// 把 workspace/tools/voice 预置为普通文件 → 一切 create_dir_all(voice_dir)
/// 必然失败（Windows：路径组件是文件）。
fn make_voice_dir_a_file(dir: &tempfile::TempDir) {
    let tools = dir.path().join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(tools.join("voice"), b"not a dir").unwrap();
}

// -----------------------------------------------------------------------
// 模块接口面
// -----------------------------------------------------------------------

#[test]
fn agt_handler_default_module_name_and_command_table() {
    let h = VoiceHandler::default();
    assert_eq!(h.module_name(), "voice");
    let cmds = h.commands();
    assert!(cmds.contains(&"status"));
    assert!(cmds.contains(&"tts_playback"));
    assert!(cmds.contains(&"speaker_register_start"));
    assert!(cmds.len() >= 38, "command table size: {}", cmds.len());
    // 声明表不得有重复项（前端命令面板按表渲染）
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "duplicate command in table");
}

// -----------------------------------------------------------------------
// setup / install_runtime / install_aec：voice_dir 为文件 → create_dir 失败
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_setup_and_install_runtime_fail_when_voice_dir_is_file() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    make_voice_dir_a_file(&dir);
    let ctx = agt_make_ctx(&dir);
    let h = VoiceHandler::new();

    for cmd in ["setup", "install_runtime"] {
        let err = h
            .handle_cmd(cmd, None, &ctx)
            .await
            .unwrap_err_or(format!("{cmd} must fail when voice dir is a file"));
        assert!(
            err.contains("failed to create voice dir"),
            "cmd={cmd} err={err}"
        );
    }
    // 两个命令退出前都清理 setup 令牌
    assert!(setup_cancel().lock().unwrap().is_none());
    agt_clear_all().await;
}

#[tokio::test]
async fn agt_install_aec_fails_when_voice_dir_is_file() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    make_voice_dir_a_file(&dir);
    let ctx = agt_make_ctx(&dir);
    let err = VoiceHandler::new()
        .handle_cmd("install_aec", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("failed to create aec dir"), "err: {err}");
    agt_clear_all().await;
}

// -----------------------------------------------------------------------
// install_model：锁冲突 / config 缺失 / unknown model 全错误路径
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_install_model_lock_config_gate_and_unknown_model_path() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir);
    let h = VoiceHandler::new();
    let data = |m: &str| Some(serde_json::json!({ "model": m }));

    // ① per-model 锁已占用 → 中文提示（不进 spawn_blocking）
    install_locks().lock().unwrap().insert("stt".to_string());
    let err = h
        .handle_cmd("install_model", data("stt"), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("STT") && err.contains("正在安装中"),
        "err: {err}"
    );
    install_locks().lock().unwrap().clear();

    // ② config.toml 缺失 → 快速失败；锁在退出路径自动释放
    let err = h
        .handle_cmd("install_model", data("stt"), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("config.toml not found"), "err: {err}");
    assert!(
        !install_locks().lock().unwrap().contains("stt"),
        "lock must be released on early return"
    );

    // ③ unknown model：config.toml 存在 → 进度回调装配 + SSE publish +
    //    cancel 未取消 + match 落 `_` 臂 + 锁释放 + 令牌清理
    let voice_dir = dir.path().join("tools").join("voice");
    std::fs::create_dir_all(&voice_dir).unwrap();
    std::fs::write(voice_dir.join("config.toml"), "[models]\n").unwrap();
    let err = h
        .handle_cmd("install_model", data("weird"), &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("model install failed") && err.contains("unknown model type: weird"),
        "err: {err}"
    );
    assert!(
        !install_locks().lock().unwrap().contains("weird"),
        "lock must be released after failure"
    );
    assert!(setup_cancel().lock().unwrap().is_none());
    agt_clear_all().await;
}

// -----------------------------------------------------------------------
// tts 纯校验段
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_tts_validation_and_setup_gate() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir);
    let h = VoiceHandler::new();

    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": "   " })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "text cannot be empty");

    let long = "a".repeat(1001);
    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": long })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("text too long"), "err: {err}");

    // 合法文本但没跑过 setup → config.toml 门（spawn_blocking 内快速失败）
    let err = h
        .handle_cmd("tts", Some(serde_json::json!({ "text": "你好" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "Voice not set up. Run setup first.");
}

// -----------------------------------------------------------------------
// voice_config_set：现值非对象（数组）→ 跳过合并但仍写回
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_voice_config_set_tolerates_non_object_current() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // 合法 JSON 但不是对象 → as_object_mut None → 整段合并跳过
    std::fs::write(cfg_dir.join(VOICE_CONFIG_FILENAME), "[]").unwrap();
    let ctx = agt_make_ctx(&dir);
    let r = VoiceHandler::new()
        .handle_cmd(
            "voice_config_set",
            Some(serde_json::json!({ "volume": 7 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["success"], true);
    let on_disk = std::fs::read_to_string(cfg_dir.join(VOICE_CONFIG_FILENAME)).unwrap();
    assert_eq!(
        on_disk.trim(),
        "[]",
        "non-object current must be kept as-is"
    );
}

// -----------------------------------------------------------------------
// init_engines_from_config：三开关全开 → 全部 fail-fast（无 config.toml）
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_init_engines_from_config_fail_fast_all_three() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join(VOICE_CONFIG_FILENAME),
        serde_json::json!({
            "stt_enabled": true,
            "tts_enabled": true,
            "speaker_enabled": true
        })
        .to_string(),
    )
    .unwrap();

    // 三个引擎启动都在 config.toml 检查处失败 → 只 warn 不 panic、不阻断
    init_engines_from_config(dir.path()).await;

    assert!(stt_engine_state().lock().unwrap().is_none());
    assert!(tts_engine_state().lock().unwrap().is_none());
    assert!(speaker_engine_state().lock().unwrap().is_none());
    agt_clear_all().await;
}

// -----------------------------------------------------------------------
// tts_playback：校验 / channel-closed / 队列生命周期
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_tts_playback_validation_channel_closed_and_queue() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_make_ctx(&dir);
    let h = VoiceHandler::new();

    // ① 空文本
    let err = h
        .handle_cmd(
            "tts_playback",
            Some(serde_json::json!({ "text": " " })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "text cannot be empty");

    // ② 全新 manager：config.toml 缺失 → spawn 的后台循环看到缺 config 即刻
    //    return（rx 随之 drop，不碰网络/DLL）；首次 send 在循环尚未启动时入队成功
    let r = h
        .handle_cmd(
            "tts_playback",
            Some(serde_json::json!({ "text": "你好", "speaker": 43, "speed": 1.2, "volume": 60 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["queued"], true);
    assert!(tts_playback_state().lock().await.is_some());

    // ③ 循环死后 rx 已 drop → 复用臂（mgr 已存在）send 失败 → channel closed。
    //    轮询等待 spawn_blocking 真正跑完（config 缺失 → 立即 return），上限 2s。
    let mut closed = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        match h
            .handle_cmd(
                "tts_playback",
                Some(serde_json::json!({ "text": "第二句" })),
                &ctx,
            )
            .await
        {
            Err(e) if e == "TTS playback channel closed" => {
                closed = true;
                break;
            }
            Ok(Some(_)) => continue, // 循环还没起来（入队成功），继续等
            Ok(None) => continue,
            Err(e) => panic!("unexpected err: {e}"),
        }
    }
    assert!(closed, "playback loop must exit on missing config.toml");

    // ④ 停止 → 消费 manager
    let r = h
        .handle_cmd("tts_playback_stop", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["stopped"], true);
    assert!(tts_playback_state().lock().await.is_none());
    agt_clear_all().await;
}

// -----------------------------------------------------------------------
// speaker_status：状态投影（engine/manager 不可注入——SpeakerManager::new
// 需要 sherpa-onnx FFI，未初始化直接 panic——故 ready 恒 false、manager 臂豁免）
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_speaker_status_projects_state_and_voiceprint_file() {
    let _guard = agt_lock();
    agt_clear_all().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(
        cfg_dir.join(VOICEPRINT_CONFIG_FILENAME),
        serde_json::to_string(&serde_json::json!({
            "threshold": 0.5,
            "speakers": {
                "alice": { "embedding": [0.1, 0.2], "created_at": "t" },
                "bob": { "embedding": [0.3], "created_at": "t" }
            }
        }))
        .unwrap(),
    )
    .unwrap();
    let ctx = agt_make_ctx(&dir);
    let r = VoiceHandler::new()
        .handle_cmd("speaker_status", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["enabled"], false);
    assert_eq!(r["ready"], false);
    assert!((r["threshold"].as_f64().unwrap() - 0.5).abs() < 1e-6);
    let mut names = r["speakers"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(names, vec!["alice".to_string(), "bob".to_string()]);
    assert_eq!(r["stt_dialogue_active"], false);
    agt_clear_all().await;
}

// -----------------------------------------------------------------------
// 辅助 trait（与 s10_tests 同款，仅测试用）
// -----------------------------------------------------------------------

trait AgtUnwrapErrOr<T> {
    fn unwrap_err_or(self, msg: impl std::fmt::Display) -> T;
}

impl<T> AgtUnwrapErrOr<String> for Result<T, String> {
    fn unwrap_err_or(self, msg: impl std::fmt::Display) -> String {
        match self {
            Err(e) => e,
            Ok(_) => panic!("{msg}"),
        }
    }
}
