//! P21（2026-09-25 能力扩展 WS1）：`sandbox.denials.list` 查询面测试。
//!
//! 覆盖：空账（无文件）→ 空列表不炸；种子账 → 记录回传（六字段齐全）；
//! limit 缺省/截断/上限；最新在前排序；ledger 路径回显。台账写入侧语义由
//! nemesis-sandbox 的 denial_tests 钉死，这里只测 WSAPI 只读面。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 与 tests.rs / agt_tests.rs 同款 AppState 脚手架（本文件自包含——兄弟测试
/// 模块的 fn 不互见，照抄最小化模板）。
fn denials_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "denials".to_string(),
        chat_id: "denials".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 种台账文件（绕过写入侧，直接落 JSONL——查询面不关心写入 API）。
fn seed_ledger(home: &std::path::Path, records: &[serde_json::Value]) {
    let ws = home.join("workspace");
    std::fs::create_dir_all(ws.join("logs")).unwrap();
    let body: String = records
        .iter()
        .map(|r| r.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(ws.join("logs").join("sandbox_denials.jsonl"), body + "\n").unwrap();
}

fn seed_record(
    ts: &str,
    backend: &str,
    op: &str,
    target: &str,
    reason: &str,
    visible: bool,
) -> serde_json::Value {
    serde_json::json!({
        "ts": ts,
        "backend": backend,
        "op": op,
        "target": target,
        "reason": reason,
        "model_visible": visible,
    })
}

/// 空账（workspace/logs 都不存在）→ 空 denials + count 0 + limit/ledger 回显。
#[tokio::test]
async fn denials_list_empty_ledger_is_empty_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = denials_ctx(&dir);
    let v = SandboxHandler::new()
        .handle_cmd("denials.list", None, &ctx)
        .await
        .expect("empty ledger must not error")
        .expect("payload");
    assert_eq!(v["denials"], serde_json::json!([]));
    assert_eq!(v["count"], 0);
    assert_eq!(v["limit"], 50, "default limit is 50");
    assert!(
        v["ledger"]
            .as_str()
            .unwrap_or("")
            .ends_with("sandbox_denials.jsonl"),
        "ledger path echoed: {v}"
    );
}

/// 注入三条假拒绝 → 查询全量回传（六字段 + 最新在前）。
#[tokio::test]
async fn denials_list_returns_seeded_records_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    seed_ledger(
        dir.path(),
        &[
            seed_record(
                "2026-09-25T08:00:00.000Z",
                "landlock",
                "write_file",
                "/etc/a",
                "os error 13",
                true,
            ),
            seed_record(
                "2026-09-25T08:01:00.000Z",
                "bwrap",
                "exec",
                "curl example.com",
                "exit code 1",
                true,
            ),
            seed_record(
                "2026-09-25T08:02:00.000Z",
                "sandboxie",
                "exec",
                "cmd /c x",
                "Access is denied",
                false,
            ),
        ],
    );
    let ctx = denials_ctx(&dir);
    let v = SandboxHandler::new()
        .handle_cmd("denials.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["count"], 3);
    let arr = v["denials"].as_array().unwrap();
    // 最新在前（文件尾反转到头部）
    assert_eq!(arr[0]["backend"], "sandboxie");
    assert_eq!(arr[0]["model_visible"], false);
    assert_eq!(arr[1]["backend"], "bwrap");
    assert_eq!(arr[2]["backend"], "landlock");
    // 字段形状（前端契约）
    for r in arr {
        assert!(r["ts"].is_string());
        assert!(r["op"].is_string());
        assert!(r["target"].is_string());
        assert!(r["reason"].is_string());
        assert!(r["model_visible"].is_boolean());
    }
}

/// limit 截断取最近 N 条；limit=0/负值钳到 1；超上限钳到 500。
#[tokio::test]
async fn denials_list_limit_truncates_and_clamps() {
    let dir = tempfile::tempdir().unwrap();
    let records: Vec<_> = (0..6)
        .map(|i| {
            seed_record(
                &format!("2026-09-25T08:0{i}:00.000Z"),
                "landlock",
                &format!("op{i}"),
                "t",
                "os error 13",
                true,
            )
        })
        .collect();
    seed_ledger(dir.path(), &records);
    let ctx = denials_ctx(&dir);
    let h = SandboxHandler::new();

    // limit=2 → 最近两条（op5/op4）
    let v = h
        .handle_cmd(
            "denials.list",
            Some(serde_json::json!({ "limit": 2 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["count"], 2);
    let ops: Vec<_> = v["denials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["op"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(ops, vec!["op5", "op4"], "newest first: {ops:?}");

    // limit=0 → 钳到 1（不返回空；上限语义诚实）
    let v = h
        .handle_cmd(
            "denials.list",
            Some(serde_json::json!({ "limit": 0 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["limit"], 1);
    assert_eq!(v["count"], 1);

    // limit=99999 → 钳到上限 500
    let v = h
        .handle_cmd(
            "denials.list",
            Some(serde_json::json!({ "limit": 99999 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["limit"], 500);

    // limit 非 number → 缺省 50（不报错，只读查询面宽容入参）
    let v = h
        .handle_cmd(
            "denials.list",
            Some(serde_json::json!({ "limit": "many" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["limit"], 50);
}

/// 台账里有损坏行（半截写入）→ 查询诚实跳过，好行照常返回（不炸）。
#[tokio::test]
async fn denials_list_skips_corrupt_lines() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(ws.join("logs")).unwrap();
    let good = seed_record(
        "2026-09-25T09:00:00.000Z",
        "landlock",
        "edit_file",
        "/t",
        "e",
        true,
    );
    std::fs::write(
        ws.join("logs").join("sandbox_denials.jsonl"),
        format!("{{torn line\n{}\n", good),
    )
    .unwrap();
    let ctx = denials_ctx(&dir);
    let v = SandboxHandler::new()
        .handle_cmd("denials.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["count"], 1, "only the well-formed record survives: {v}");
    assert_eq!(v["denials"][0]["op"], "edit_file");
}
