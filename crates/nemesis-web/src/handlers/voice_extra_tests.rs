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
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
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
        let r = h.handle_cmd("config_get", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn config_set_then_get() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        h.handle_cmd("config_set", Some(serde_json::json!({"content": "# test config"})), &ctx).await.unwrap();
        let r = h.handle_cmd("config_get", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn voice_config_get_default() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("voice_config_get", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn chat_config_get_default() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("chat_config_get", None, &ctx).await.unwrap().unwrap();
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

    #[tokio::test]
    async fn devices_returns_object() {
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
        let r = h.handle_cmd("speaker_list", None, &ctx).await.unwrap().unwrap();
        assert!(r.is_object());
    }

    #[tokio::test]
    async fn engine_status_idle() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("engine_status", None, &ctx).await.unwrap().unwrap();
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
        let err = h.handle_cmd("engine_start", Some(serde_json::json!({})), &ctx).await.unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn install_model_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("install_model", Some(serde_json::json!({})), &ctx).await.unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn pipeline_start_missing_model_field() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("pipeline_start", Some(serde_json::json!({})), &ctx).await.unwrap_err();
        assert!(err.contains("missing field: model"));
    }

    #[tokio::test]
    async fn speaker_remove_missing_name() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("speaker_remove", Some(serde_json::json!({})), &ctx).await.unwrap_err();
        assert!(err.contains("missing field: name"));
    }

    #[tokio::test]
    async fn speaker_set_threshold_missing_threshold() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("speaker_set_threshold", Some(serde_json::json!({})), &ctx).await.unwrap_err();
        assert!(err.contains("missing field: threshold"));
    }

    // ---- config-set I/O commands (no audio) ----

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
            })),
            &ctx,
        )
        .await
        .unwrap();
        let r = h.handle_cmd("voice_config_get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["speaker_id"], 99);
        assert_eq!(r["volume"], 80);
        assert_eq!(r["speed"], 1.5);
        assert_eq!(r["stt_enabled"], true);
        assert_eq!(r["tts_enabled"], false);
        assert_eq!(r["capture_device"], "Mic0");
    }

    #[tokio::test]
    async fn voice_config_set_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("voice_config_set", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn chat_config_set_writes_and_success() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h
            .handle_cmd("chat_config_set", Some(serde_json::json!({"stt": true})), &ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(r["success"], true);
        // Round-trip via chat_config_get to prove it persisted.
        let g = h.handle_cmd("chat_config_get", None, &ctx).await.unwrap().unwrap();
        assert_eq!(g["stt"], true);
    }

    #[tokio::test]
    async fn chat_config_set_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("chat_config_set", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    #[tokio::test]
    async fn speaker_status_reports_threshold_and_enabled() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("speaker_status", None, &ctx).await.unwrap().unwrap();
        // Enabled defaults to false in tests (no engine loaded); threshold
        // falls back to the default when no voiceprint config exists.
        assert!(r["enabled"].is_boolean());
        assert!(r["threshold"].is_number());
    }

    // ---- stop / idle commands (no audio device required) ----

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
            .handle_cmd("engine_stop", Some(serde_json::json!({"model": "stt"})), &ctx)
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
            .handle_cmd("engine_stop", Some(serde_json::json!({"model": "tts"})), &ctx)
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
            .handle_cmd("engine_stop", Some(serde_json::json!({"model": "speaker"})), &ctx)
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
            .handle_cmd("engine_stop", Some(serde_json::json!({"model": "bogus"})), &ctx)
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
        let err = h.handle_cmd("stt_dialogue_reset", None, &ctx).await.unwrap_err();
        assert!(err.contains("No dialogue session active"));
    }

    #[tokio::test]
    async fn speaker_register_cancel_idle_ok() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let r = h.handle_cmd("speaker_register_cancel", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["cancelled"], true);
    }

    #[tokio::test]
    async fn speaker_register_start_missing_data() {
        let h = VoiceHandler::new();
        let dir = tempfile::tempdir().unwrap();
        let ctx = make_ctx(&dir);
        let err = h.handle_cmd("speaker_register_start", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"));
    }

    // ---- tts / tts_playback argument validation (no audio reached) ----

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
        let err = h.handle_cmd("tts", Some(serde_json::json!({})), &ctx).await.unwrap_err();
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
}
