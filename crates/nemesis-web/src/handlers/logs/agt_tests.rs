//! logs.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与既有 9 个测试子模块互补，聚焦仍缺的确定性臂：
//! - dispatch 层参数提取臂：request_detail 的 `session` 兜底键、
//!   cluster_task_list 的 `device_id`、cluster_task_detail 的 `perspective`、
//!   session_list 的 `query`、replay_verify 缺 `round` 报错 + `request_id`/
//!   `trace_id` 载体
//! - `parse_cluster_dir_name` None 臂、`sorted_files` 目录缺失臂
//! - `scan_session_logs`：空 jsonl 跳过、sidecar `undelivered`/`title` 回填
//! - 审计链：`collect_audit_segments` 根路径 parent=None、`read_all_audit_events`
//!   段目录不可读 / 空行、`chain_list` 的 prev_hash mismatch breakReason
//! - `build_request_entry` / `build_cluster_task_entry` 的坏 JSON 与无头
//!   response.md 落穿、Local.md `### Error` → status failed
//! - `parse_request_iterations`：响应先于请求的 stub 迭代、Local.md 挂接、
//!   坏 JSON 请求的 round 兜底
//! - `read_round_envelope` 坏文件/轮次不匹配/终 None、`envelope_matches_round`
//!   三拒臂
//! - replay：显式 request_id 目录缺该轮 envelope → no_recording；
//!   台账记录 session_key 为空 → store_key 回退 → DegradedSubsequence
//! - `parse_local_tool_results`：空名 flush、代码栅栏、坏 Duration、无时长
//! - `build_iteration_json` 的 Local.md 不可读臂
//!
//! 结构性豁免（见报告）：非 UTF-8 文件名防御臂（787/870/1819/1940/2046——
//! Windows 下需 unsafe OsStr 构造）、设备目录 read_dir 失败臂（920——需
//! ACL 操作）、episodic 富集臂（1197-1207——需实装 MemoryManager）、
//! agent_loop 存活槽臂（1544/1636-1638——需实装 AgentLoop）、spill 保留期
//! 0 臂（1670——依赖进程级全局 ConfigStore 置 0）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use nemesis_agent::r#loop::LlmMessage;
use nemesis_agent::replay::{InjectionRecord, RequestProjectionRecord};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

const RAW_KEY: &str = "agent:main:session:agtl";
const STEM: &str = "agent_main_session_agtl";

fn agt_ctx(dir: &tempfile::TempDir) -> RequestContext {
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

fn agt_msgs() -> Vec<LlmMessage> {
    vec![
        LlmMessage {
            role: "user".into(),
            content: "hi".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "assistant".into(),
            content: "hello".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
        LlmMessage {
            role: "user".into(),
            content: "how are you".into(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        },
    ]
}

/// g2_tests 同款请求目录夹具（id 需满足 `{ts}_{rand}` 形态）。
fn agt_request_dir(ws: &Path, id: &str, round: usize, msgs: &[LlmMessage]) -> PathBuf {
    let dir = request_log_dir(ws.to_str().unwrap()).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let envelope = serde_json::json!({
        "timestamp": "2026-08-28T07:00:00+08:00",
        "round": round,
        "body": {
            "model": "test-model",
            "messages": msgs
                .iter()
                .map(|m| serde_json::to_value(m).unwrap())
                .collect::<Vec<_>>(),
        },
    });
    std::fs::write(dir.join("00.request.md"), "first user line").unwrap();
    std::fs::write(dir.join("01.AI.Request.raw.json"), envelope.to_string()).unwrap();
    dir
}

fn agt_projection_record(round: usize, session_key: &str) -> RequestProjectionRecord {
    let roles: Vec<String> = agt_msgs().iter().map(|m| m.role.clone()).collect();
    let count = roles.len();
    RequestProjectionRecord {
        trace_id: format!("trace-{}", round),
        session_key: session_key.to_string(),
        round,
        ts: "2026-08-28T07:00:00+08:00".to_string(),
        messages_count: count,
        roles,
        history_len_at_build: count,
        injections: Vec::<InjectionRecord>::new(),
        voice_append: None,
        summary_as_of: None,
        vision_projected: false,
    }
}

fn agt_write_ledger(ws: &Path, recs: &[RequestProjectionRecord]) {
    let dir = boundary_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for r in recs {
        let mut v = serde_json::to_value(r).unwrap();
        v["kind"] = serde_json::json!("request_projection");
        body.push_str(&serde_json::to_string(&v).unwrap());
        body.push('\n');
    }
    std::fs::write(dir.join(format!("{}.replay.jsonl", STEM)), body).unwrap();
}

fn agt_seed_store(ws: &Path) {
    let store =
        nemesis_agent::session::SessionStore::new_with_storage(ws.join("workspace/sessions"));
    let _ = store.get_or_create(RAW_KEY);
    for m in agt_msgs() {
        store.add_message(RAW_KEY, &m.role, &m.content);
    }
    store.save(RAW_KEY).unwrap();
}

/// 集群任务目录夹具。
fn agt_cluster_task(ws: &Path, device: &str, dir_name: &str, files: &[(&str, Vec<u8>)]) {
    let dir = cluster_log_dir(ws.to_str().unwrap())
        .join(device)
        .join(dir_name);
    std::fs::create_dir_all(&dir).unwrap();
    for (name, content) in files {
        std::fs::write(dir.join(name), content).unwrap();
    }
}

// ============================================================
// dispatch 层参数提取臂
// ============================================================

#[tokio::test]
async fn agt_dispatch_extraction_fallback_ladder() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();
    let h = LogsHandler;

    // request_detail：无 `id` → `session` 兜底键（dispatch 66 臂）
    let id = "2026-08-28_07-00-00_s1";
    agt_request_dir(ws, id, 1, &agt_msgs());
    let out = h
        .handle_cmd(
            "request_detail",
            Some(serde_json::json!({ "session": id })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], id);

    // cluster_task_list：device_id 过滤（dispatch 74-76 臂）
    agt_cluster_task(
        ws,
        "devA",
        "2026-08-28_07-00-00_t1",
        &[(
            "00.request.md",
            "## Message\nhello dev\n".as_bytes().to_vec(),
        )],
    );
    agt_cluster_task(
        ws,
        "devB",
        "2026-08-28_07-00-01_t2",
        &[("00.request.md", b"other dev".to_vec())],
    );
    let out = h
        .handle_cmd(
            "cluster_task_list",
            Some(serde_json::json!({ "device_id": "devA" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1, "device filter must hide devB: {out}");
    assert_eq!(out["entries"][0]["id"], "t1");
    assert_eq!(out["entries"][0]["firstMessage"], "hello dev");

    // cluster_task_detail：显式 perspective（dispatch 84 臂）
    let out = h
        .handle_cmd(
            "cluster_task_detail",
            Some(serde_json::json!({ "task_id": "t1", "perspective": "peer" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], "t1");
    assert_eq!(out["direction"], "unknown", "no local node in bare ctx");

    // cluster_task_detail：cluster_logs 目录整体缺失 → 候选为空 Err（936 臂）
    let dir2 = tempfile::tempdir().unwrap();
    let ctx2 = agt_ctx(&dir2);
    let err = h
        .handle_cmd(
            "cluster_task_detail",
            Some(serde_json::json!({ "task_id": "ghost" })),
            &ctx2,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "err: {err}");

    // session_list：query 参数（dispatch 111-113 臂）
    let sdir = session_log_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&sdir).unwrap();
    std::fs::write(
        sdir.join("web_a.jsonl"),
        "{\"role\":\"user\",\"content\":\"hello world\",\"timestamp\":\"2026-08-28T07:00:00Z\"}\n",
    )
    .unwrap();
    let out = h
        .handle_cmd(
            "session_list",
            Some(serde_json::json!({ "query": "hello" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(out["total"].as_u64().unwrap() >= 1, "{out}");

    // replay_verify：缺 round → Err（dispatch 152 臂）
    let err = h
        .handle_cmd(
            "replay_verify",
            Some(serde_json::json!({ "session": STEM })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("round is required"), "err: {err}");
}

#[tokio::test]
async fn agt_replay_explicit_request_id_without_round_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();
    let h = LogsHandler;

    agt_seed_store(ws);
    agt_write_ledger(ws, &[agt_projection_record(1, RAW_KEY)]);

    // 台账记录 round 1，但显式指向的目录里只有 round 2 的 envelope
    // → read_round_envelope(dir, 1) = None → no_recording（1524-1532 臂，
    //   同时覆盖 dispatch 层 request_id / trace_id 载体提取 156-157/161-162）
    let decoy = agt_request_dir(ws, "2026-08-28_07-00-02_d2", 2, &agt_msgs());
    let out = h
        .replay_verify(
            &ctx,
            ws.to_str().unwrap(),
            STEM,
            1,
            Some(decoy.file_name().unwrap().to_str().unwrap()),
            Some("trace-1"),
        )
        .unwrap()
        .unwrap();
    assert_eq!(out["verdict"], "no_recording", "{out}");
    assert_eq!(out["request_id"], "2026-08-28_07-00-02_d2");
    let note = out["note"].as_str().unwrap();
    assert!(note.contains("没有此轮次"), "note: {note}");
}

#[cfg(feature = "memory")]
#[tokio::test]
async fn agt_session_list_bm25_query_on_empty_workspace() {
    // query 无可匹配会话 → docs 空 → avg_len 0 守卫臂（1237-1238）+ BM25
    // 块完整走空（1260）
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let out = LogsHandler
        .handle_cmd(
            "session_list",
            Some(serde_json::json!({ "query": "hello" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 0, "{out}");
}

// ============================================================
// 文件名/目录助手
// ============================================================

#[test]
fn agt_parse_cluster_dir_name_none_and_sorted_files_missing_dir() {
    // 无时间戳前缀 → None（568/570 臂）
    assert_eq!(parse_cluster_dir_name("junk_task"), None);
    assert_eq!(parse_cluster_dir_name(""), None);
    // 毫秒形态与朴素形态都认
    let (ts, id) =
        parse_cluster_dir_name("2026-08-28_07-00-00-123_task9").expect("ms form must parse");
    assert_eq!(id, "task9");
    assert!(ts.starts_with("2026-08-28_07-00-00"));
    let (_, id) = parse_cluster_dir_name("2026-08-28_07-00-00_task9").expect("plain form");
    assert_eq!(id, "task9");

    // 目录缺失 → 空表（594 臂）
    let empty = sorted_files(Path::new("Z:/agt-no-such-dir-9x"));
    assert!(empty.is_empty());
}

// ============================================================
// scan_session_logs：空文件跳过 + undelivered/title 回填
// ============================================================

#[test]
fn agt_scan_session_logs_empty_file_skipped_and_meta_backfilled() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let sdir = session_log_dir(ws.to_str().unwrap());
    std::fs::create_dir_all(&sdir).unwrap();

    // 空 jsonl → lines 空 → continue（286 臂）；非 jsonl 忽略
    std::fs::write(sdir.join("web_empty.jsonl"), "").unwrap();
    std::fs::write(sdir.join("notes.txt"), "not a session").unwrap();
    // 正常会话 + sidecar meta（title + undelivered>0 → 352 臂）
    std::fs::write(
        sdir.join("web_a.jsonl"),
        "{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":\"2026-08-28T07:00:00Z\"}\n",
    )
    .unwrap();
    std::fs::write(
        sdir.join("web_a.meta.json"),
        "{\"title\":\"My Title\",\"undelivered\":2}",
    )
    .unwrap();

    let sessions = scan_session_logs(ws.to_str().unwrap());
    assert_eq!(sessions.len(), 1, "empty jsonl and txt must be skipped");
    let s = &sessions[0];
    assert_eq!(s["id"], "web_a");
    assert_eq!(s["title"], "My Title");
    assert_eq!(s["undelivered"], 2);
    assert_eq!(s["messageCount"], 1);
    assert_eq!(s["channel"], "web");
}

// ============================================================
// 审计链段收集 + prev_hash mismatch（security 门控）
// ============================================================

#[cfg(feature = "security")]
fn agt_audit_event(prev_hash: &str, operation: &str) -> serde_json::Value {
    serde_json::json!({
        "id": format!("evt-{operation}"),
        "timestamp": "2026-08-28T07:00:00Z",
        "operation": operation,
        "tool_name": "exec",
        "user": "agt",
        "source": "web",
        "target": "/tmp/x",
        "decision": "allow",
        "reason": "rule",
        "hash": "",
        "prev_hash": prev_hash,
    })
}

#[cfg(feature = "security")]
#[test]
fn agt_audit_chain_segment_collection_arms() {
    // 根路径无 parent → 段扫描提前返回（629 臂）。
    // 该路径本身作为盘符目录存在，因此最多只包含它自己。
    let root = Path::new("C:\\");
    let root_only = collect_audit_segments(root);
    assert!(
        root_only.iter().all(|p| p == root),
        "parentless path must not gain segment entries: {root_only:?}"
    );
    assert!(root_only.len() <= 1);

    // 主链文件 + 空行（660）+ 段目录不可读（656）+ 段文件
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("audit_chain.jsonl");
    let e1 = agt_audit_event(&"0".repeat(64), "op1");
    let e2 = agt_audit_event(&"0".repeat(64), "op2");
    std::fs::write(
        &main,
        format!(
            "{}\n\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e2).unwrap()
        ),
    )
    .unwrap();
    // 段命名匹配 `audit_chain*_seg` → 是目录 → read_to_string 失败 → 跳过
    std::fs::create_dir_all(dir.path().join("audit_chain_seg1.jsonl")).unwrap();
    std::fs::write(
        dir.path().join("audit_chain_seg2.jsonl"),
        serde_json::to_string(&agt_audit_event(&"0".repeat(64), "op3")).unwrap(),
    )
    .unwrap();

    let events = read_all_audit_events(&main);
    assert_eq!(events.len(), 3, "blank line and unreadable seg dir skipped");
    assert_eq!(events[0].operation, "op1");
    assert_eq!(events[2].operation, "op3");
}

#[cfg(feature = "security")]
#[tokio::test]
async fn agt_chain_list_prev_hash_mismatch_break_reason() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let main = audit_chain_path(dir.path().to_str().unwrap());

    // ev1 合法（prev 全零）；ev2 prev_hash 非零且 ≠ ev1.hash、自身 hash 正确
    // → hash_match 但 prev_ok=false → breakReason "prev_hash mismatch"（1071 臂）
    let mut e1 = agt_audit_event(&"0".repeat(64), "op1");
    let ev1: nemesis_security::integrity::AuditEvent = serde_json::from_value(e1.clone()).unwrap();
    e1["hash"] = serde_json::json!(compute_audit_hash(&ev1));
    let mut e2 = agt_audit_event(&"f".repeat(64), "op2");
    let ev2: nemesis_security::integrity::AuditEvent = serde_json::from_value(e2.clone()).unwrap();
    e2["hash"] = serde_json::json!(compute_audit_hash(&ev2));
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    std::fs::write(
        &main,
        format!(
            "{}\n{}\n",
            serde_json::to_string(&e1).unwrap(),
            serde_json::to_string(&e2).unwrap()
        ),
    )
    .unwrap();

    let out = LogsHandler
        .handle_cmd("chain_list", Some(serde_json::json!({ "limit": 10 })), &ctx)
        .await
        .unwrap()
        .unwrap();
    let segs = out["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 2);
    assert_eq!(segs[0]["valid"], true);
    assert_eq!(segs[0]["breakReason"], serde_json::Value::Null);
    assert_eq!(segs[1]["breakReason"], "prev_hash mismatch", "{out}");
    assert_eq!(segs[1]["valid"], false);
}

// ============================================================
// build_request_entry / build_cluster_task_entry 落穿臂
// ============================================================

#[tokio::test]
async fn agt_request_entry_survives_invalid_json_and_headerless_md() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();

    let d = request_log_dir(ws.to_str().unwrap()).join("2026-08-28_07-00-00_f1");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("01.AI.Request.raw.json"), b"{not json").unwrap(); // 1739 落穿
    std::fs::write(d.join("02.AI.Response.raw.json"), b"not json at all").unwrap(); // 1757 落穿
    std::fs::write(d.join("03.response.md"), "# no headers here\n").unwrap(); // 1771 落穿
    let good_req = serde_json::json!({
        "round": 1,
        "body": { "model": "m1", "messages": [{"role":"user","content":"q"}] },
    });
    std::fs::write(d.join("04.AI.Request.raw.json"), good_req.to_string()).unwrap();
    let good_resp = serde_json::json!({
        "round": 1,
        "duration_ms": 120,
        "body": { "choices": [ { "message": { "role": "assistant", "content": "a",
            "tool_calls": [{"id":"1"},{"id":"2"}] } } ] },
    });
    std::fs::write(d.join("05.AI.Response.raw.json"), good_resp.to_string()).unwrap();

    let out = LogsHandler
        .handle_cmd("requests", Some(serde_json::json!({ "limit": 10 })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1, "{out}");
    let entry = &out["entries"][0];
    assert_eq!(entry["id"], "2026-08-28_07-00-00_f1");
    assert_eq!(entry["model"], "m1");
    assert_eq!(entry["duration_ms"], 120, "sum of valid response raw");
    assert_eq!(entry["toolCallCount"], 2);
}

#[tokio::test]
async fn agt_cluster_task_entry_error_local_md_marks_failed() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();

    // 坏 JSON 响应（1844 落穿）+ 带 Error 的 Local.md（1846-1851）→ failed（1874）
    agt_cluster_task(
        ws,
        "devE",
        "2026-08-28_07-00-00_tE",
        &[
            ("01.AI.Response.raw.json", b"{bad".to_vec()),
            ("02.Local.md", b"### Error\nboom\n".to_vec()),
            ("03.Local.md", b"all good here\n".to_vec()),
        ],
    );
    let out = LogsHandler
        .handle_cmd(
            "cluster_task_list",
            Some(serde_json::json!({ "device_id": "devE" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let entry = &out["entries"][0];
    assert_eq!(entry["status"], "failed", "{out}");
}

// ============================================================
// parse_request_iterations：stub 迭代 + Local.md 挂接 + round 兜底
// ============================================================

#[test]
fn agt_parse_request_iterations_stub_and_locals_attachment() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("iter");
    std::fs::create_dir_all(&d).unwrap();

    // 响应先于任何请求出现（iterations 为空）→ stub（1977-1984 臂）
    std::fs::write(d.join("00.AI.Response.raw.json"), b"{\"round\": 5}").unwrap();
    let r1 = serde_json::json!({
        "round": 1,
        "body": { "model": "m", "messages": [{"role":"user","content":"q"}] },
    });
    std::fs::write(d.join("01.AI.Request.raw.json"), r1.to_string()).unwrap();
    std::fs::write(
        d.join("02.Local.md"),
        "## Operation 1: exec\n**Name**: exec\n**Status**: ok\n### Arguments\n{\"path\":\"a\"}\n### Result\nhello\n### Duration 0.05s\n",
    )
    .unwrap();
    let resp1 = serde_json::json!({
        "round": 1, "duration_ms": 10,
        "body": { "choices": [ { "message": { "role": "assistant", "content": "ok" } } ] },
    });
    std::fs::write(d.join("03.AI.Response.raw.json"), resp1.to_string()).unwrap();
    // 坏 JSON 请求 → round 兜底 = iterations.len()+1 = 3（1946 unwrap_or 臂）
    std::fs::write(d.join("04.AI.Request.raw.json"), b"{nope").unwrap();

    let iterations = parse_request_iterations(&d);
    let arr = iterations.as_array().unwrap();
    assert_eq!(arr.len(), 3, "{iterations}");
    assert_eq!(arr[0]["round"], 5, "orphan response opens a stub iteration");
    assert_eq!(
        arr[0]["request"]["model"], "",
        "stub has no request envelope"
    );
    assert_eq!(arr[0]["response"]["duration_ms"], 0);
    assert_eq!(arr[1]["round"], 1);
    // 02.Local.md 挂到迭代 1（1987-1988 臂）
    let tools = arr[1]["toolResults"].as_array().unwrap();
    assert_eq!(tools.len(), 1, "tools: {tools:?}");
    assert_eq!(tools[0]["name"], "exec");
    assert_eq!(tools[0]["result"]["output"], "hello\n");
    assert_eq!(tools[0]["duration_ms"], 50);
    assert_eq!(tools[0]["args"]["path"], "a");
    // 迭代 1 的响应已配对
    assert_eq!(arr[1]["response"]["content"], "ok");
    // 迭代 2：坏 JSON 请求 → round=3，envelope 不可读 → 默认空
    assert_eq!(arr[2]["round"], 3);
    assert_eq!(arr[2]["request"]["model"], "");
}

// ============================================================
// read_round_envelope / envelope_matches_round 拒绝臂
// ============================================================

#[test]
fn agt_read_round_envelope_skip_arms_and_matches_rejections() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join("env");
    std::fs::create_dir_all(&d).unwrap();
    // 不可读（非法 UTF-8 字节）→ 2052 臂
    std::fs::write(d.join("00.AI.Request.raw.json"), [0xFFu8, 0xFE, 0x00]).unwrap();
    // 坏 JSON → 2055 臂
    std::fs::write(d.join("01.AI.Request.raw.json"), b"{nope").unwrap();
    // round 1 / round 2 各一份合法
    let m1 = serde_json::json!({
        "round": 1,
        "body": { "messages": [
            {"role":"user","content":"a"},{"role":"assistant","content":"b"},{"role":"user","content":"c"}
        ]},
    });
    std::fs::write(d.join("02.AI.Request.raw.json"), m1.to_string()).unwrap();
    let m2 =
        serde_json::json!({ "round": 2, "body": { "messages": [{"role":"user","content":"x"}] } });
    std::fs::write(d.join("03.AI.Request.raw.json"), m2.to_string()).unwrap();

    let hit = read_round_envelope(&d, 1).expect("round 1 must resolve past bad files");
    assert_eq!(hit.len(), 3);
    // 全部不匹配 → 终 None（2058 continue + 2065 None 臂）
    assert_eq!(read_round_envelope(&d, 9), None);

    // round 缺失 → false（2027 臂）
    assert!(!envelope_matches_round(&d, 9, &["user".into()], 1));
    // 条数不符 → false（2030 臂）
    assert!(!envelope_matches_round(
        &d,
        1,
        &["user".into(), "assistant".into()],
        2
    ));
    // 全匹配 → true
    assert!(envelope_matches_round(
        &d,
        1,
        &["user".into(), "assistant".into(), "user".into()],
        3
    ));
}

// ============================================================
// replay：台账记录 session_key 为空 → store_key 回退 STEM → Unavailable
// ============================================================

#[tokio::test]
async fn agt_replay_empty_record_key_store_fallback_unavailable() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();

    agt_seed_store(ws);
    // 记录 session_key 为空 → store_key 回退为 STEM（1559 臂）→ 以 STEM 查
    // 会话库为空 → Unavailable（1602-1613 臂）
    agt_write_ledger(ws, &[agt_projection_record(1, "")]);
    agt_request_dir(ws, "2026-08-28_07-00-01_r1", 1, &agt_msgs());

    let out = LogsHandler
        .replay_verify(&ctx, ws.to_str().unwrap(), STEM, 1, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(out["verdict"], "unavailable", "{out}");
    assert_eq!(out["ok"], false);
    assert_eq!(out["needed"], 3);
    assert_eq!(out["available"], 0);
    assert_eq!(out["request_id"], "2026-08-28_07-00-01_r1");
}

// ============================================================
// parse_local_tool_results / build_iteration_json 边臂
// ============================================================

#[test]
fn agt_parse_local_tool_results_empty_name_and_duration_arms() {
    // 空 content：无 op → 无 flush → 空
    assert!(parse_local_tool_results("").is_empty());
    // op 头后无 **Name** 即遇分隔 → flush 空名直接 return（2245 臂）
    assert!(parse_local_tool_results("## Operation 1: T\n---\n").is_empty());

    let content = concat!(
        "## Operation 1: A\n",
        "**Name**: exec\n",
        "**Status**: ok\n",
        "### Arguments\n",
        "```json\n",
        "{\"x\":1}\n",
        "```\n",
        "### Result\n",
        "out1\n",
        "### Duration 0.02s\n",
        "## Operation 2: B\n",
        "**Name**: read\n",
        "**Status**: ok\n",
        "### Arguments\n",
        "not-json\n",
        "### Result\n",
        "out2\n",
        "## Operation 3: C\n",
        "**Name**: rm\n",
        "**Status**: err\n",
        "### Error\n",
        "denied\n",
        "### Duration: junk\n",
    );
    let out = parse_local_tool_results(content);
    assert_eq!(out.len(), 3, "{out:?}");
    // op1：代码栅栏跳过（2357 臂），args 解析为 JSON，duration 20ms
    assert_eq!(out[0]["name"], "exec");
    assert_eq!(out[0]["args"]["x"], 1);
    assert_eq!(out[0]["duration_ms"], 20);
    // op2：无 Duration 行 → 不插 duration 字段（2272 落穿臂）；args 非法 → 字符串兜底
    assert!(out[1].get("duration_ms").is_none(), "{:?}", out[1]);
    assert_eq!(out[1]["args"], "not-json");
    // op3：error 臂 + 坏 Duration 落穿（2335 臂）→ 仍无 duration 字段
    assert_eq!(out[2]["result"]["error"], "denied\n");
    assert!(out[2].get("duration_ms").is_none());
}

#[test]
fn agt_build_iteration_json_unreadable_local_yields_empty_tool_results() {
    let dir = tempfile::tempdir().unwrap();
    let local = dir.path().join("09.Local.md");
    std::fs::write(&local, [0xFFu8, 0xFE, 0x00]).unwrap(); // 非法 UTF-8 → 读取失败（2168-2170 臂）
    let group = IterationFiles {
        request: None,
        response: None,
        locals: vec![local],
        round: 3,
    };
    let json = build_iteration_json(&group, 0);
    assert_eq!(json["round"], 3);
    assert_eq!(json["toolResults"], serde_json::json!([]));
    assert_eq!(
        json["request"]["model"], "",
        "no request envelope → defaults"
    );
}

// -----------------------------------------------------------------------
// Wave5 批次：cluster 任务目录 walk 的 continue 臂 + replay_verify dispatch
// 层完整载体提取（round/request_id/trace_id 的 Some 臂——直调 bypass
// dispatch 测不到）。
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_w5_walk_continue_arms_and_replay_dispatch_carriers() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx(&dir);
    let ws = dir.path();
    let h = LogsHandler;

    // cluster_task_list：junk 任务目录名（parse_cluster_dir_name 不认）→
    // walk continue，不计入 total
    agt_cluster_task(
        ws,
        "devA",
        "2026-08-28_07-00-00_t1",
        &[("00.request.md", b"x".to_vec())],
    );
    std::fs::create_dir_all(
        cluster_log_dir(ws.to_str().unwrap())
            .join("devA")
            .join("junk_task"),
    )
    .unwrap();
    let out = h
        .handle_cmd("cluster_task_list", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1, "junk dir must be skipped: {out}");

    // cluster_task_detail：cluster_logs 下的非目录项 → walk continue（不炸）
    std::fs::write(
        cluster_log_dir(ws.to_str().unwrap()).join("stray.txt"),
        b"not a device",
    )
    .unwrap();
    let err = h
        .handle_cmd(
            "cluster_task_detail",
            Some(serde_json::json!({ "task_id": "ghost" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "err: {err}");

    // replay_verify dispatch 层：round + request_id + trace_id 齐备的 Some 臂
    // （台账记 round 1，显式 request_id 指向只有 round 2 的目录 → no_recording）
    agt_seed_store(ws);
    agt_write_ledger(ws, &[agt_projection_record(1, RAW_KEY)]);
    let decoy = agt_request_dir(ws, "2026-08-28_07-00-02_d2", 2, &agt_msgs());
    let out = h
        .handle_cmd(
            "replay_verify",
            Some(serde_json::json!({
                "session": STEM,
                "round": 1,
                "request_id": decoy.file_name().unwrap().to_str().unwrap(),
                "trace_id": "trace-1"
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["verdict"], "no_recording", "{out}");
}
