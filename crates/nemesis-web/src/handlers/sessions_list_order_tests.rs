//! BUG-20261118-168 回归：sessions.list 后端收尾排序（最近活跃优先）。
//!
//! scan_session_logs 返回序 = read_dir 枚举序（平台实现细节：NTFS 字典序
//! vs ext4 哈希序）。前端 SessionSidebar 对非 pinned 会话自排序，但
//! pinned 组语义是「互相对序=列表原序不变」——原序即本列表序，直传
//! 枚举序会让置顶区排列跨平台不一致。修 = list 收尾统一
//! session_recency_cmp（BUG-20261116-011 同源，logs.session_list 之后
//! 同链路第三处跟进）。
//!
//! 构造文件名字典序与活跃序相反：`sess_a_old` 先写（lastTime 早）、
//! `sess_z_new` 后写（lastTime 晚）——NTFS 字典序枚举 a 先；期望返回
//! z 在前（最近活跃优先）。修复前本测试在 Windows 上必败（枚举序直传
//! a 在前）；ext4 哈希序平台枚举两态皆可能，断言以解析序为准仍恒过。
//!
//! home 单例经 HomeGuard 重定向（sessions_l22_tests 同款）。

use super::sessions::SessionsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct HomeGuard {
    old: PathBuf,
    _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.old.clone());
    }
}

fn isolate(home: &Path) -> HomeGuard {
    let lock = crate::test_home::lock_home();
    let old = nemesis_path::default_path_manager().home_dir();
    nemesis_path::default_path_manager().set_home_dir(home.to_path_buf());
    HomeGuard { old, _lock: lock }
}

fn make_ctx(home: &Path) -> RequestContext {
    let ws = home.join("workspace").to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        mail_plugin: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "list-order".to_string(),
        chat_id: "list-order".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn list_orders_by_last_active_desc() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let _guard = isolate(&home);

    // 先写老会话、后写新会话；sleep 保证 lastTime 毫秒可分（同毫秒撞刻
    // 会落 id 破平序，区分不出排序效果）。
    nemesis_agent::chat_log::append_chat_log("agent:main:session:sess_a_old", "user", "old");
    std::thread::sleep(std::time::Duration::from_millis(30));
    nemesis_agent::chat_log::append_chat_log("agent:main:session:sess_z_new", "user", "new");

    let ctx = make_ctx(&home);
    let resp = SessionsHandler
        .handle_cmd("list", None, &ctx)
        .await
        .expect("sessions.list must succeed");
    let sessions = resp
        .as_ref()
        .and_then(|v| v.get("sessions"))
        .and_then(|s| s.as_array())
        .cloned()
        .expect("sessions array");
    let ids: Vec<String> = sessions
        .iter()
        .filter_map(|s| s["id"].as_str().map(String::from))
        .filter(|i| i.starts_with("sess_"))
        .collect();
    assert_eq!(
        ids,
        ["sess_z_new".to_string(), "sess_a_old".to_string()],
        "最近活跃优先：sess_z_new（lastTime 晚）必须排在 sess_a_old 前——\
         枚举序直传（NTFS 字典序 a 先）则本断言失败"
    );
}
