    use super::*;
    use crate::ws_router::RequestContext;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
    use std::time::Instant;

    fn make_ctx(estop: Option<Arc<nemesis_agent::estop::EstopState>>) -> RequestContext {
        let state = Arc::new(crate::api_handlers::AppState {
            auth_token: String::new(),
            session_count: Arc::new(AtomicUsize::new(0)),
            workspace: None,
            home: None,
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
            model_base: Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: Arc::new(AtomicBool::new(false)),
            event_hub: Arc::new(crate::events::EventHub::new()),
            running: Arc::new(AtomicBool::new(true)),
            session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
            estop,
            cron: None,
        });
        RequestContext {
            session_id: "s".to_string(),
            chat_id: "c".to_string(),
            workspace: None,
            home: None,
            state,
            auth_method: crate::session::AuthMethod::default(),
        }
    }

    #[tokio::test]
    async fn trigger_release_status_roundtrip() {
        let estop = Arc::new(nemesis_agent::estop::EstopState::new());
        let ctx = make_ctx(Some(estop.clone()));
        let h = EstopHandler;

        // status → 未触发
        let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["engaged"], false);

        // trigger → 触发
        let r = h.handle_cmd("trigger", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["engaged"], true);
        assert!(estop.is_engaged());

        // status → 已触发
        let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["engaged"], true);

        // release → 释放
        let r = h.handle_cmd("release", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["engaged"], false);
        assert!(!estop.is_engaged());
    }

    #[tokio::test]
    async fn status_when_not_available_reports_false() {
        // estop 未接线（None）→ status 仍可查，返回 engaged:false（不报错）。
        let ctx = make_ctx(None);
        let h = EstopHandler;
        let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
        assert_eq!(r["engaged"], false);
    }

    #[tokio::test]
    async fn trigger_release_error_when_not_available() {
        let ctx = make_ctx(None);
        let h = EstopHandler;
        assert!(h.handle_cmd("trigger", None, &ctx).await.is_err());
        assert!(h.handle_cmd("release", None, &ctx).await.is_err());
    }

    #[tokio::test]
    async fn unknown_command_errors() {
        let estop = Arc::new(nemesis_agent::estop::EstopState::new());
        let ctx = make_ctx(Some(estop));
        let h = EstopHandler;
        let err = h.handle_cmd("frobnicate", None, &ctx).await.unwrap_err();
        assert!(err.contains("unknown command"), "实际: {}", err);
    }
