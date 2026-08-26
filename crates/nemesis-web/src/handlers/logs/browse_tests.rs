//! Logs handler 的「浏览面」覆盖（Phase 3 覆盖率，2026-08-25）。
//!
//! `requests` / `request_detail` / `cluster_task_list` / `cluster_task_detail`
//! / `session_list`(BM25 过滤) / `session_detail` 都是「扫磁盘目录 → 组装
//! JSON 条目」的纯文件逻辑——这里用临时 workspace 造 request_logs /
//! cluster_logs / session_logs 目录树，直接调 impl 内方法钉行为：
//! 分页、device 过滤、self/peer perspective 选择、cron 标记透传。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn handler() -> LogsHandler {
    LogsHandler
}

// -----------------------------------------------------------------------
// requests / request_detail
// -----------------------------------------------------------------------

/// 造一个 request_logs/{ts}_{rand}/ 目录，含 envelope + markdown 文件。
fn make_request_dir(ws: &std::path::Path, name: &str, model: &str, round: u64) {
    let dir = ws.join("logs/request_logs").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("00.request.md"),
        "# Request\n\n**Model**: whatever\n\nuser asks about rust testing\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("01.AI.Request.raw.json"),
        serde_json::json!({
            "round": round,
            "body": { "model": model }
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("02.AI.Response.raw.json"),
        serde_json::json!({
            "duration_ms": 120,
            "body": { "choices": [ { "message": { "tool_calls": [
                { "id": "t1" }, { "id": "t2" }
            ] } } ] }
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn requests_empty_when_dir_missing() {
    let dir = tempfile::tempdir().unwrap();
    let out = handler()
        .requests(dir.path().to_string_lossy().as_ref(), 50, 0)
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 0);
    assert_eq!(out["entries"].as_array().unwrap().len(), 0);
    assert_eq!(out["limit"], 50);
}

#[test]
fn requests_aggregates_and_paginates() {
    let dir = tempfile::tempdir().unwrap();
    make_request_dir(dir.path(), "2026-08-25_07-00-00_aaaa", "model-a", 1);
    make_request_dir(dir.path(), "2026-08-25_06-00-00_bbbb", "model-b", 3);
    make_request_dir(dir.path(), "2026-08-25_05-00-00_cccc", "model-c", 1);
    // 无效名目录（无合法时间戳前缀）必须被跳过，不进列表。
    make_request_dir(dir.path(), "not-a-timestamp", "model-x", 9);

    let out = handler()
        .requests(dir.path().to_string_lossy().as_ref(), 50, 0)
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 3, "invalid dir name must be skipped");
    let entries = out["entries"].as_array().unwrap();
    // 按时间戳降序。
    assert_eq!(entries[0]["model"], "model-a");
    assert_eq!(entries[1]["model"], "model-b");
    // 聚合（真实字段名来自 build_request_entry）：messageCount 取 round
    // 最大值（无 response.md 的 LLM Rounds 头时）、toolCallCount 计数、
    // duration_ms 求和。
    assert_eq!(entries[1]["messageCount"], 3);
    assert_eq!(entries[0]["toolCallCount"], 2);
    assert_eq!(entries[0]["duration_ms"], 120);

    // 分页：offset=1 limit=1 → 只剩第二条。
    let page = handler()
        .requests(dir.path().to_string_lossy().as_ref(), 1, 1)
        .unwrap()
        .unwrap();
    assert_eq!(page["total"], 3);
    let p = page["entries"].as_array().unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(p[0]["model"], "model-b");
}

#[test]
fn request_detail_not_found_errors() {
    let dir = tempfile::tempdir().unwrap();
    let err = handler()
        .request_detail(dir.path().to_string_lossy().as_ref(), "missing-id")
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[test]
fn request_detail_builds_entry_with_iterations() {
    let dir = tempfile::tempdir().unwrap();
    make_request_dir(dir.path(), "2026-08-25_07-00-00_zzzz", "model-z", 2);
    // 多塞一轮 envelope，凑成 iterations 列表。
    let d = dir.path().join("logs/request_logs/2026-08-25_07-00-00_zzzz");
    std::fs::write(
        d.join("03.AI.Request.raw.json"),
        serde_json::json!({ "round": 2, "body": { "model": "model-z" } }).to_string(),
    )
    .unwrap();

    let out = handler()
        .request_detail(
            dir.path().to_string_lossy().as_ref(),
            "2026-08-25_07-00-00_zzzz",
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["model"], "model-z");
    assert!(out.get("iterations").is_some(), "detail 必须带 iterations 字段");
}

// -----------------------------------------------------------------------
// cluster_task_list / cluster_task_detail
// -----------------------------------------------------------------------

fn make_cluster_task_dir(ws: &std::path::Path, dev: &str, name: &str) {
    let dir = ws.join("logs/cluster_logs").join(dev).join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("00.request.md"), "# Peer Chat\n\nhello from peer\n").unwrap();
}

#[tokio::test]
async fn cluster_task_list_empty_and_device_filtered() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    // 空目录 → 空列表（root 不存在的早退臂）。
    let out = handler()
        .cluster_task_list(&ctx, &ws, 50, 0, None)
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 0);

    make_cluster_task_dir(dir.path(), "devA", "2026-08-25_07-00-00_task1");
    make_cluster_task_dir(dir.path(), "devB", "2026-08-25_08-00-00_task2");

    let out = handler()
        .cluster_task_list(&ctx, &ws, 50, 0, None)
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 2);

    // device_id 过滤。条目不带 device 字段（build_cluster_task_entry 只有
    // direction/peerNode）；local=None 时 direction="unknown"、peerNode=""。
    let out = handler()
        .cluster_task_list(&ctx, &ws, 50, 0, Some("devA".to_string()))
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1);
    let e = &out["entries"][0];
    assert_eq!(e["id"], "task1");
    assert_eq!(e["direction"], "unknown");
    assert_eq!(e["peerNode"], "");
}

#[tokio::test]
async fn cluster_task_detail_selects_perspective() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();
    // local=None：self/peer 两条 find 都 miss → 各自 fallback first，
    // 只断言 order 无关字段（id + iterations 存在）。
    make_cluster_task_dir(dir.path(), "devA", "2026-08-25_07-00-00_task9");
    make_cluster_task_dir(dir.path(), "devB", "2026-08-25_09-00-00_task9");

    let self_view = handler()
        .cluster_task_detail(&ctx, &ws, "task9", "self")
        .unwrap()
        .unwrap();
    assert_eq!(self_view["id"], "task9");
    assert!(self_view.get("iterations").is_some());

    // not found。
    let err = handler()
        .cluster_task_detail(&ctx, &ws, "no-such-task", "self")
        .unwrap_err();
    assert!(err.contains("not found"), "{err}");
}

#[cfg(feature = "cluster")]
#[tokio::test]
async fn cluster_task_detail_perspective_with_real_local_node() {
    use nemesis_cluster::cluster::Cluster;
    use nemesis_cluster::types::ClusterConfig;

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    // 真实 Cluster 提供 local node_id；设备目录名跟着 node_id 走 →
    // 两视角的选择完全确定性（不依赖 read_dir 顺序）。
    let mut ctx = make_ctx(&dir);
    let cluster = Arc::new(Cluster::with_workspace(
        ClusterConfig::default(),
        dir.path().to_path_buf(),
    ));
    ctx.state = Arc::new(crate::api_handlers::AppState {
        cluster: Some(cluster.clone()),
        ..(*ctx.state).clone()
    });
    let local_id = cluster.node_id().to_string();
    make_cluster_task_dir(dir.path(), &local_id, "2026-08-25_07-00-00_taskK");
    make_cluster_task_dir(dir.path(), "remote-peer", "2026-08-25_09-00-00_taskK");

    // self 视角：挑 local 设备的目录 → direction=outbound、peerNode 空。
    let self_view = handler()
        .cluster_task_detail(&ctx, &ws, "taskK", "self")
        .unwrap()
        .unwrap();
    assert_eq!(self_view["id"], "taskK");
    assert_eq!(self_view["direction"], "outbound");
    assert_eq!(self_view["peerNode"], "");

    // peer 视角：挑第一个非 local 设备 → direction=inbound、peerNode=对端。
    let peer_view = handler()
        .cluster_task_detail(&ctx, &ws, "taskK", "peer")
        .unwrap()
        .unwrap();
    assert_eq!(peer_view["id"], "taskK");
    assert_eq!(peer_view["direction"], "inbound");
    assert_eq!(peer_view["peerNode"], "remote-peer");
}

// -----------------------------------------------------------------------
// session_detail / session_list(BM25)
// -----------------------------------------------------------------------

fn make_session_log(ws: &std::path::Path, id: &str, lines: &[(&str, &str)]) {
    let dir = ws.join("logs/session_logs");
    std::fs::create_dir_all(&dir).unwrap();
    let body: String = lines
        .iter()
        .map(|(role, content)| {
            serde_json::json!({
                "role": role,
                "content": content,
                "timestamp": "2026-08-25T07:00:00",
            })
            .to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(dir.join(format!("{id}.jsonl")), body + "\n").unwrap();
}

#[tokio::test]
async fn session_detail_reads_jsonl_and_passes_cron_markers() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    let d = dir.path().join("logs/session_logs");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("sched_1.jsonl"),
        concat!(
            r#"{"role":"user","content":"hi","timestamp":"2026-08-25T07:00:00"}"#, "\n",
            r#"{"role":"assistant","content":"done","timestamp":"2026-08-25T07:00:05","cron_job_id":"job-7","cron_job_name":"nightly"}"#, "\n",
        ),
    )
    .unwrap();

    let out = handler()
        .session_detail(&ctx, &ws, "sched_1")
        .await
        .unwrap()
        .unwrap();
    let msgs = out["messages"].as_array().unwrap();
    assert_eq!(msgs.len(), 2);
    assert_eq!(msgs[0]["role"], "user");
    // cron 标记必须透传到前端可见字段。
    assert_eq!(msgs[1]["cron_job_id"], "job-7");
    assert_eq!(msgs[1]["cron_job_name"], "nightly");
    // 无标记消息不得带 cron 字段。
    assert!(msgs[0].get("cron_job_id").is_none());
}

#[tokio::test]
async fn session_list_bm25_query_filters_and_ranks() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();

    make_session_log(
        dir.path(),
        "web_alpha",
        &[("user", "tell me about rust async runtime"), ("assistant", "tokio")],
    );
    make_session_log(
        dir.path(),
        "web_beta",
        &[("user", "what is the weather today"), ("assistant", "sunny")],
    );

    // 无 query：两个会话都在。
    let all = handler()
        .session_list(&ctx, &ws, None, 50, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(all["total"], 2);

    // query 命中 rust 会话：只剩 alpha。
    let hit = handler()
        .session_list(&ctx, &ws, Some("rust async".to_string()), 50, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hit["total"], 1, "BM25 过滤后只留命中会话");
    // 响应键名是 sessions（见 session_list 的 json!），条目内 id=文件名 stem。
    assert_eq!(hit["sessions"][0]["id"], "web_alpha");

    // query 命不中任何会话：空。
    let miss = handler()
        .session_list(&ctx, &ws, Some("zzzqqqxxx".to_string()), 50, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(miss["total"], 0);
}

#[tokio::test]
async fn security_list_empty_when_dir_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();
    let out = handler().security(&ws, 50, 0, None).unwrap().unwrap();
    assert_eq!(out["total"], 0);
    assert_eq!(out["entries"].as_array().unwrap().len(), 0);
    drop(ctx);
}
