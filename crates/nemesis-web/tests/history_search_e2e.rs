//! logs.history_search e2e — ISOLATED test binary（对齐 nemesis-agent 侧
//! `tests/history_search_fts.rs` 的隔离模式）。
//!
//! WHY not in-lib (`handlers/logs/history_tests.rs`)：`default_path_manager()`
//! 是进程级 OnceLock 单例，home 由进程内首解析烤定——本 crate lib 测试二进制
//! 三千余个兄弟测试必然先烤掉 home（真实全局 `~/.nemesisbot`），库内任何
//! 重定向都来不及。且 nextest 每测试独立进程，进程内 IDX_LOCK /
//! HOME_RACE_LOCK 全部失效，e2e 与所有 chat_log 追加测试的 append-hook
//! 并发争抢真实全局 `history_index.db`（BUSY 让 reindex/插入静默失败），
//! 3×200ms 重试窗口在满载机器上必漏（实录 2026-10-10：本地全量 nextest
//! `test_history_search_finds_appended_message_e2e` 假红，单测必绿；
//! cargo test 二进制串行掩盖至今）。
//!
//! 本二进制只含本家族：进程首解析前把 `NEMESISBOT_HOME` 钉进一次性
//! tempdir —— 私有 session_logs + 私有 history_index.db，无跨进程争抢、
//! 无跨 run 幽灵行，重试循环随之退役（私有 home 里没有第二个写者）。
//! 不要移回 lib tests——兄弟测试先烤单例 = 隔离即刻丢失（同款告诫见
//! nemesis-agent/tests/history_search_fts.rs 头注）。输入校验 / reindex
//! 计数这类不依赖索引内容的测试仍留在 lib（对竞争不可失败）。

use nemesis_web::api_handlers::AppState;
use nemesis_web::events::EventHub;
use nemesis_web::handlers::logs::LogsHandler;
use nemesis_web::session::AuthMethod;
use nemesis_web::session::SessionManager;
use nemesis_web::ws_router::{ModuleHandler, RequestContext};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// Per-process isolated home（created+set exactly once，before this process
/// resolves any path）。`resolve_home_dir()` joins `.nemesisbot` onto the env
/// value, so the actual home is `<tempdir>/.nemesisbot`.
fn isolated_home() -> &'static std::path::Path {
    static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();
    HOME.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!(
            "nb_web_fts_home_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: 测试进程此刻单线程（无其他线程在解析路径），env 写入先于
        // 单例首解析；随后立即烤单例，把本进程全部 chat_log / history_search
        // 落盘钉进 tempdir。
        unsafe { std::env::set_var("NEMESISBOT_HOME", &dir) };
        let _ = nemesis_path::default_path_manager();
        dir
    })
    .as_path()
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = std::sync::Arc::new(AppState {
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: std::sync::Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
        event_hub: std::sync::Arc::new(EventHub::new()),
        running: std::sync::Arc::new(AtomicBool::new(true)),
        session_manager: std::sync::Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        // AppState dual-declares these two fields (workflow-gated real /
        // `Arc<()>` stub — api_handlers.rs)；同 lib 侧 make_ctx 的双臂处理。
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: std::sync::Arc::new(
            nemesis_web::handlers::workflow::WebhookRateLimiter::new(),
        ),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: AuthMethod::default(),
    }
}

fn unique_marker(prefix: &str) -> String {
    format!(
        "zzq{}{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

/// End-to-end through the WSAPI handler: append via chat_log, search through
/// `LogsHandler::handle_cmd`（reindex-before-search 装配的受测行为），assert
/// the hit carries the file-stem session_key that `logs.session_detail`
/// expects。私有 home 下没有第二个写者——首查即命中，无需重试。
#[tokio::test]
async fn test_history_search_finds_appended_message_e2e() {
    let _home = isolated_home();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = LogsHandler;

    let marker = unique_marker("marker");
    let key = format!("test:hsearch:{}", marker);
    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::append_chat_log(&key, "user", &format!("please locate {marker} now"));

    // The handler reindexes (mtime-incremental) before searching, so the
    // freshly appended line is findable on the first call — deterministically
    // here: the private home has no concurrent writer, so the reindex cannot
    // hit SQLITE_BUSY（旧 lib 版本的 3×200ms 重试循环只服务那个竞争面）。
    let stem = key.replace(':', "_");
    let out = h
        .handle_cmd(
            "history_search",
            Some(serde_json::json!({"query": marker, "limit": 20})),
            &ctx,
        )
        .await
        .unwrap()
        .expect("search returns a payload");
    let hits = out
        .get("hits")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    assert!(
        !hits.is_empty(),
        "marker {marker} must be findable via logs.history_search: {out}"
    );

    // Hit shape: stem session_key (== what session_detail expects), role,
    // snippet around the match, plus envelope fields.
    let hit = hits
        .iter()
        .find(|hit| hit.get("session_key").and_then(|s| s.as_str()) == Some(stem.as_str()))
        .unwrap_or_else(|| panic!("our stem {stem} missing from hits: {hits:?}"));
    assert_eq!(
        hit.get("role").and_then(|r| r.as_str()),
        Some("user"),
        "hit role: {hit}"
    );
    let snippet = hit.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
    assert!(snippet.contains(&marker), "snippet: {snippet}");
    // No cross-session leak: every hit's session_key must be our stem or at
    // least contain the unique marker (unique per run, so only ours).
    for other in &hits {
        let sk = other
            .get("session_key")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        assert!(
            sk.contains(&marker),
            "foreign session leaked in: {sk} (marker {marker})"
        );
    }

    // limit=1 caps the result count.
    let out = h
        .handle_cmd(
            "history_search",
            Some(serde_json::json!({"query": marker, "limit": 1})),
            &ctx,
        )
        .await
        .unwrap()
        .expect("search returns a payload");
    let capped = out.get("hits").and_then(|v| v.as_array()).map(|a| a.len());
    assert_eq!(capped, Some(1), "limit=1 must cap hits: {out}");
    assert_eq!(
        out.get("query").and_then(|q| q.as_str()),
        Some(marker.as_str())
    );

    // Unknown subcommand still errors.
    let err = h.handle_cmd("history_nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command"), "got: {err}");

    nemesis_agent::chat_log::delete_chat_log(&key);
}
