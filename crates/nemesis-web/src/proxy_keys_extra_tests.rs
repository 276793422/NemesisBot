//! `proxy_keys` 单测（docs/PLAN/2026-11-03_LLM代理调用方key管理.md §4 P2）：
//! 空 token 不参与匹配 / 白名单前置内存闸 / 命中注入 caller / 禁用 key 拒绝
//! / 缺文件回落 / 手填空 key 不开旁路 / 热生效（mtime 重载）/ workspace 缺席。
//! AppState 构造依赖 workflow 的 ChatSecretStore，feature 闸同
//! `proxy_usage_extra_tests`。

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::proxy_keys::match_proxy_caller;
use crate::session::SessionManager;
use nemesis_config::proxy_keys_file::{self, ProxyKeyEntry};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// 形态对齐 proxy_usage_extra_tests::make_state；workspace 可注入指向
// 临时目录（auth 面从 state.workspace 推导 config 路径）。
fn make_state(workspace: Option<String>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace,
        home: None,
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    })
}

fn entry(name: &str, key: &str) -> ProxyKeyEntry {
    ProxyKeyEntry {
        name: name.to_string(),
        key: key.to_string(),
        enabled: true,
        created_at: 0,
        note: String::new(),
    }
}

// TempDir 析构会删目录——需要跨 add/match 多步操作时持住句柄。
fn held_workspace(tag: &str) -> (tempfile::TempDir, String) {
    let dir = tempfile::Builder::new()
        .prefix(&format!("nb_proxy_keys_{tag}_"))
        .tempdir()
        .unwrap();
    let path = dir.path().to_string_lossy().to_string();
    (dir, path)
}

#[test]
fn empty_token_never_matches() {
    let (_dir, ws) = held_workspace("empty_tok");
    let config = std::path::Path::new(&ws).join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    assert!(match_proxy_caller(&state, "", "/v1/chat/completions").is_none());
}

#[test]
fn whitelist_outside_path_never_matches() {
    let (_dir, ws) = held_workspace("path_gate");
    let config = std::path::Path::new(&ws).join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    // 正确 key + Dashboard API 路径 = 不匹配（防提权；连盘都不 stat）。
    assert!(match_proxy_caller(&state, "nbk_aaa", "/api/usage/summary").is_none());
    assert!(match_proxy_caller(&state, "nbk_aaa", "/api/chat/history").is_none());
    // 白名单是 exact match：前缀延展路径不沾光。
    assert!(
        match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions/extra").is_none(),
        "exact match：子路径不算白名单内"
    );
}

#[test]
fn enabled_key_hit_returns_caller() {
    let (_dir, ws) = held_workspace("hit");
    let config = std::path::Path::new(&ws).join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    // 两条白名单 lane 都放行，caller 名称注入身份。
    let c1 = match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").unwrap();
    assert_eq!(c1.0.as_str(), "bot-a");
    let c2 = match_proxy_caller(&state, "nbk_aaa", "/api/chat/stream").unwrap();
    assert_eq!(c2.0.as_str(), "bot-a");
    // 无关 token 不命中（不存在 allow-all）。
    assert!(match_proxy_caller(&state, "nbk_other", "/v1/chat/completions").is_none());
}

#[test]
fn disabled_key_rejected_hot() {
    let (_dir, ws) = held_workspace("disabled");
    let config = std::path::Path::new(&ws).join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_some());
    // 禁用即失效（热生效：不重启不换 state）。
    proxy_keys_file::set_enabled(&config, "bot-a", false).unwrap();
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_none());
    // 重新启用恢复。
    proxy_keys_file::set_enabled(&config, "bot-a", true).unwrap();
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_some());
}

#[test]
fn no_keys_file_falls_back() {
    let (_dir, ws) = held_workspace("no_file");
    let state = make_state(Some(ws));
    // 零配置 = 功能关：匹配恒 None，主 token 校验不受影响（middleware 侧）。
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_none());
    assert!(
        !proxy_keys_file::proxy_keys_config_path(
            &std::path::Path::new(&state.workspace.as_ref().unwrap()).join("config")
        )
        .exists()
    );
}

#[test]
fn hand_written_empty_key_no_bypass() {
    let (_dir, ws) = held_workspace("empty_key");
    let path = proxy_keys_file::proxy_keys_config_path(&std::path::Path::new(&ws).join("config"));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"keys":[{"name":"evil","key":"","enabled":true}]}"#,
    )
    .unwrap();
    let state = make_state(Some(ws));
    // 空 key 条目不得造成 allow-all 旁路（verify_token 空串约定只作用于
    // 主 token 面，key 面请求 token 非空闸 + 长度比对双兜底）。
    assert!(match_proxy_caller(&state, "", "/v1/chat/completions").is_none());
    assert!(match_proxy_caller(&state, "anything", "/v1/chat/completions").is_none());
}

#[test]
fn hot_reload_picks_up_new_key() {
    let (_dir, ws) = held_workspace("hot");
    let config = std::path::Path::new(&ws).join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    assert!(match_proxy_caller(&state, "nbk_bbb", "/v1/chat/completions").is_none());
    // WSAPI add 落盘 → 下一请求看到新 key（写代号失配重读——不再赌 mtime
    // 粒度，BUG-20261103-350 后无 sleep）。
    proxy_keys_file::add_key(&config, entry("bot-b", "nbk_bbb")).unwrap();
    let caller = match_proxy_caller(&state, "nbk_bbb", "/v1/chat/completions").unwrap();
    assert_eq!(caller.0.as_str(), "bot-b");
    // 老 key 仍命中（缓存重载是整表替换不丢条目）。
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_some());
}

#[test]
fn workspace_absent_returns_none() {
    let state = make_state(None);
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_none());
}

// BUG-20261103-350 回归锁：同 tick 双写的缓存陈旧。NTFS LastWriteTime 有
// 毫秒级量化（本机实测零间隔 atomic 写 3/11 次撞同值；write_file_atomic
// 的临时名注释、HotReloader::force_reload 的 2026-10-22 实录、auth_tests
// 的 20ms sleep「不赌 mtime 粒度」三处在案）——第二次写与第一次写落在同
// 一 tick 时 mtime 相同，mtime 单信号缓存把第二次写当「未变」：禁用/删除
// 的 key 无限期继续放行（直到下次写盘或重启），「写盘即热生效」契约被破。
// 毒化缓存 = 确定性重建该碰撞态（mtime 与写代号都取当前真实值——即只剩
// 「表」是第二次写前的陈旧值，等价于旧 mtime 单信号下的自然碰撞态）：任何
// 单一信号下的新鲜判定都拦不住它，只有「表源自上次读，写代号已前进」的
// 代号失配才能重读。
#[test]
fn same_tick_double_write_disable_still_takes_effect() {
    let (dir, ws) = held_workspace("same_tick");
    let config = dir.path().join("config");
    proxy_keys_file::add_key(&config, entry("bot-a", "nbk_aaa")).unwrap();
    let state = make_state(Some(ws));
    assert!(match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_some());
    // 碰撞态锚点 = set_enabled 之前的代号（缓存里的表正是这个代号读的）。
    let stale_gen = proxy_keys_file::write_generation();
    proxy_keys_file::set_enabled(&config, "bot-a", false).unwrap();
    // 毒化：mtime 伪装成「最新」（取当前真实值——同 tick 双写下与缓存里
    // 的旧 mtime 数值相同），代号与表是 set_enabled 前的（自然碰撞态：
    // 缓存最后一次刷新停在第一次写，第二次写只动了 mtime 且没动成）。
    let path = proxy_keys_file::proxy_keys_config_path(&config);
    let mtime_now = std::fs::metadata(&path).unwrap().modified().unwrap();
    *crate::proxy_keys::CACHE.lock() = Some((
        path,
        mtime_now,
        stale_gen,
        Arc::new(vec![entry("bot-a", "nbk_aaa")]),
    ));
    let cur_gen = proxy_keys_file::write_generation();
    assert!(
        stale_gen != cur_gen,
        "set_enabled 必须递增写代号（本测试的失配判据）"
    );
    assert!(
        match_proxy_caller(&state, "nbk_aaa", "/v1/chat/completions").is_none(),
        "同 tick 双写（mtime 相同）后禁用必须仍然生效"
    );
}
