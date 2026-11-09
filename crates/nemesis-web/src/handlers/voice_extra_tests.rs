//! Voice handler coverage — simple commands (config read/write, status) that
//! don't need an actual audio device or running engine. All voice commands are
//! gated on `target_os = "windows"`, so the whole module is too.

#[cfg(all(test, target_os = "windows"))]
mod voice_extra_tests {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::handlers::voice::VoiceHandler;
    use crate::session::SessionManager;
    use crate::ws_router::{ModuleHandler, RequestContext};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
        let ws = dir.path().to_string_lossy().to_string();
        let state = Arc::new(AppState {
            cors_manager: None,
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
            quic_relay: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            #[cfg(feature = "workflow")]
            chat_secret_store: Arc::new(
                nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
            ),
            #[cfg(not(feature = "workflow"))]
            chat_secret_store: Arc::new(()),
            #[cfg(feature = "workflow")]
            webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
            #[cfg(not(feature = "workflow"))]
            webhook_rate_limiter: Arc::new(()),
            internal_cmd_tx: None,
            estop: None,
            background_registry: None,
            signature_verify: None,
            skills_install_gate: None,
            session_stats: None,
            cron: None,
            board: None,
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

    #[tokio::test]
    async fn status_no_install() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["ready"], false);
        assert!(r["dlls"].is_array());
        assert_eq!(r["config_exists"], false);
    }

    #[tokio::test]
    async fn check_alias_status() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("check", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["ready"], false);
    }

    #[tokio::test]
    async fn config_get_creates_default() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn config_set_then_get() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        h.handle_cmd(
            "config_set",
            Some(serde_json::json!({"content": "# test config"})),
            &ctx,
        )
        .await
        .unwrap();
        let r = h
            .handle_cmd("config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn voice_config_get_default() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("voice_config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn chat_config_get_default() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("chat_config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn speakers_returns_object() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("speakers", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object() || r.is_array());
    }

    // 本测试经 cmd_devices 穿透真实音频设备枚举，需要 voice-capture feature
    // （voice 不隐含它——IoT 可只留转写不带设备 IO）。未编译时 devices 命令
    // 诚实报错，测试前提不成立 → 编译期移除（与 scanner 测试挂 scanner
    // feature 同款门控纪律）；CI 虚拟音频栈防护闸保留在函数体内。
    #[cfg(feature = "voice-capture")]
    #[tokio::test]
    async fn devices_returns_object() {
        // 【 CI nemesis-web 0xc0000005】本测试经 cmd_devices() 穿透
        // 调用 nemesis_voice::audio:list_devices() → cpal host 设备枚举。
        // cpal 0.15 WASAPI 在 GitHub runner 虚拟音频栈上**枚举/查询同样挂死 +
        // native ACCESS_VIOLATION**（voice crate /09-02 二连崩同族雷；
        // nemesis-web 测试此前从未在 CI 跑到——voice 被排除后首跑即暴露）。
        // CI=true 且未 opt-in 时 SKIP（与 nemesis-voice audio::tests 的
        // ci_audio_unsafe 同语义；ci.yml 已 --exclude nemesis-voice，本闸是
        // 穿透面的第二道防线）。
        if !std::env::var("NEMESISBOT_VOICE_HW_TESTS").is_ok_and(|v| v == "1")
            && std::env::var("CI").is_ok_and(|v| v == "true")
        {
            eprintln!(
                "SKIP: 经 cmd_devices 穿透 cpal 设备枚举（GitHub runner 虚拟音频栈 native AV，2026-09-02）— 补齐命令：设 NEMESISBOT_VOICE_HW_TESTS=1"
            );
            return;
        }
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("devices", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object() || r.is_array());
    }

    #[tokio::test]
    async fn speaker_list_returns_object() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("speaker_list", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    // BUG-20261106-020 回归锁：NUL 名在 WSAPI 边界诚实拒绝（crate 层
    // SpeakerManager 也会拒，但边界拒绝给调用方明确错误，且 register_start
    // 先于 ensure_speaker_engine 重装配——机器上无语音模型也确定性命中）。
    #[tokio::test]
    async fn speaker_remove_rejects_nul_name() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd(
                "speaker_remove",
                Some(serde_json::json!({ "name": "a\u{0000}b" })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("NUL"), "{err}");
    }

    #[tokio::test]
    async fn speaker_register_start_rejects_nul_name() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd(
                "speaker_register_start",
                Some(serde_json::json!({ "name": "a\u{0000}b" })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("NUL"), "{err}");
    }

    #[tokio::test]
    async fn engine_status_idle() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("engine_status", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn unknown_command_rejected() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("bogus", None, &ctx).await.unwrap_err();
        assert!(err.contains("unknown command"));
    }

    #[tokio::test]
    async fn engine_start_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("engine_start", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn install_model_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("install_model", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn pipeline_start_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("pipeline_start", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn speaker_remove_missing_name() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("speaker_remove", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: name"));
    }

    #[tokio::test]
    async fn speaker_set_threshold_missing_threshold() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("speaker_set_threshold", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: threshold"));
    }

    // config-set I/O commands (no audio) ----

    #[tokio::test]
    async fn voice_config_set_persists_fields() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        h.handle_cmd(
            "voice_config_set",
            Some(serde_json::json!({
                "speaker_id": 99,
                "volume": 80,
                "speed": 1.5,
                "stt_enabled": true,
                "tts_enabled": false,
                "capture_device": "Mic0",
                "aec_filter_length": 2048,
                "aec_preprocess": false,
            })),
            &ctx,
        )
        .await
        .unwrap();
        let r = h
            .handle_cmd("voice_config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["speaker_id"], 99);
        assert_eq!(r["volume"], 80);
        assert_eq!(r["speed"], 1.5);
        assert_eq!(r["stt_enabled"], true);
        assert_eq!(r["tts_enabled"], false);
        assert_eq!(r["capture_device"], "Mic0");
        // AEC 进阶参数持久化（此前漏接——FE 发送、
        // STT 启动读取、保存不落盘 = 改了存不上）。
        assert_eq!(r["aec_filter_length"], 2048);
        assert_eq!(r["aec_preprocess"], false);
    }

    #[tokio::test]
    async fn voice_config_set_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("voice_config_set", None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn chat_config_set_writes_and_success() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd(
                "chat_config_set",
                Some(serde_json::json!({"stt": true})),
                &ctx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["success"], true);
        // Round-trip via chat_config_get to prove it persisted.
        let g = h
            .handle_cmd("chat_config_get", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(g["stt"], true);
    }

    #[tokio::test]
    async fn chat_config_set_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("chat_config_set", None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn speaker_status_reports_threshold_and_enabled() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("speaker_status", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        // Enabled defaults to false in tests (no engine loaded); threshold
        // falls back to the default when no voiceprint config exists.
        assert!(r["enabled"].is_boolean());
        assert!(r["threshold"].is_number());
    }

    // stop / idle commands (no audio device required) ----

    #[tokio::test]
    async fn stop_setup_when_idle_errors() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("stop_setup", None, &ctx).await.unwrap_err();
        assert!(err.contains("no setup in progress"));
    }

    #[tokio::test]
    async fn engine_stop_stt_reports_not_loaded() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd(
                "engine_stop",
                Some(serde_json::json!({"model": "stt"})),
                &ctx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["model"], "stt");
        assert_eq!(r["was_loaded"], false);
    }

    #[tokio::test]
    async fn engine_stop_tts_reports_not_loaded() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd(
                "engine_stop",
                Some(serde_json::json!({"model": "tts"})),
                &ctx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["model"], "tts");
        assert_eq!(r["was_loaded"], false);
    }

    #[tokio::test]
    async fn engine_stop_speaker_releases() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd(
                "engine_stop",
                Some(serde_json::json!({"model": "speaker"})),
                &ctx,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["model"], "speaker");
        assert_eq!(r["stopped"], true);
    }

    #[tokio::test]
    async fn engine_stop_unknown_model_rejected() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd(
                "engine_stop",
                Some(serde_json::json!({"model": "bogus"})),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("unknown model"));
    }

    #[tokio::test]
    async fn engine_stop_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("engine_stop", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn engine_stop_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("engine_stop", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn pipeline_stop_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("pipeline_stop", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn pipeline_stop_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("pipeline_stop", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn stt_stop_when_idle_errors() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("stt_stop", None, &ctx).await.unwrap_err();
        assert!(err.contains("not running"));
    }

    #[tokio::test]
    async fn stt_dialogue_reset_when_idle_errors() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("stt_dialogue_reset", None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("No dialogue session active"));
    }

    #[tokio::test]
    async fn speaker_register_cancel_idle_ok() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("speaker_register_cancel", None, &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["cancelled"], true);
    }

    #[tokio::test]
    async fn speaker_register_start_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("speaker_register_start", None, &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing data"));
    }

    // tts / tts_playback argument validation (no audio reached) ----

    #[tokio::test]
    async fn tts_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("tts", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn tts_missing_text_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("tts", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: text"));
    }

    #[tokio::test]
    async fn tts_empty_text_rejected() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("tts", Some(serde_json::json!({"text": "   "})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("cannot be empty"));
    }

    #[tokio::test]
    async fn tts_playback_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("tts_playback", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn tts_playback_missing_text_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("tts_playback", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: text"));
    }

    #[tokio::test]
    async fn tts_playback_empty_text_rejected() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("tts_playback", Some(serde_json::json!({"text": ""})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("cannot be empty"));
    }

    #[tokio::test]
    async fn config_set_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("config_set", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn config_set_missing_content_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h
            .handle_cmd("config_set", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("missing field: content"));
    }

    // -------------------------------------------------------------------
    // BUG-20261107-018 回归锁：引擎加载串行闸 + 会话槽原子占位。
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn try_insert_is_atomic_under_concurrency() {
        use crate::handlers::voice::try_insert;
        // 8 个并发 claim 恰好一个成功；胜者值在槽内；后续 claim 全部失败。
        //（注意：断言之间不得持有槽锁——tokio Mutex 非重入，本测试首个
        // 版本曾在持锁状态下调 try_insert 把自己永久阻塞。）
        let slot = Arc::new(tokio::sync::Mutex::new(None::<u32>));
        let handles: Vec<_> = (0..8u32)
            .map(|i| {
                let slot = slot.clone();
                tokio::spawn(async move { try_insert(&slot, i).await })
            })
            .collect();
        let mut wins = 0usize;
        let mut winner: Option<u32> = None;
        for (i, h) in handles.into_iter().enumerate() {
            if h.await.expect("join") {
                wins += 1;
                winner = Some(i as u32);
            }
        }
        assert_eq!(wins, 1, "并发 claim 必须恰好一个成功");
        assert_eq!(
            slot.lock().await.as_ref(),
            winner.as_ref(),
            "胜者值必须在槽内"
        );
        // 已占位：后续 claim 失败且不覆盖（旧无条件覆盖写形态在这里翻车）。
        assert!(!try_insert(&slot, 99u32).await, "占位后 claim 必须失败");
        assert_eq!(
            slot.lock().await.as_ref(),
            winner.as_ref(),
            "失败 claim 不得覆盖现有值"
        );
    }

    #[tokio::test]
    async fn engine_start_blocked_while_gate_held() {
        // 闸持有期间 engine_start 必须阻塞（无闸的旧代码：检查锁一放就
        // 直通 spawn_blocking，双 start 各自构造完整引擎）。无 config.toml
        // 的临时 workspace → 闸放行后诚实报 config not found，全程不触碰
        // 真实模型。走 handle_cmd 真实派发路径（engine_start → stt 臂）。
        use crate::handlers::voice::stt_engine_gate;
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        // spawn 要求 'static：手工构造 owned 副本（RequestContext 全 owned 字段）。
        let task_ctx = RequestContext {
            session_id: ctx.session_id.clone(),
            chat_id: ctx.chat_id.clone(),
            workspace: ctx.workspace.clone(),
            home: ctx.home.clone(),
            state: ctx.state.clone(),
            auth_method: crate::session::AuthMethod::default(),
        };

        let _hold = stt_engine_gate().lock().await;
        let h = tokio::spawn(async move {
            let handler = VoiceHandler::new();
            handler
                .handle_cmd(
                    "engine_start",
                    Some(serde_json::json!({ "model": "stt" })),
                    &task_ctx,
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(
            !h.is_finished(),
            "闸持有期间 engine_start 必须阻塞（若已结束=闸缺失，并发加载回归）"
        );
        drop(_hold);
        let res = h
            .await
            .expect("join")
            .expect_err("无 config.toml 应诚实失败");
        assert!(
            res.contains("config.toml not found"),
            "闸放行后应继续原路径报错: {res}"
        );
    }
}
