//! memory.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 tests / s10b_tests 互补，聚焦仍缺的确定性臂：
//! - `require_team_memory_store`：board 缺席的诚实报错臂
//! - team_memory 命令组（M4.5，master board.db）：team.list 的 scope/
//!   include_deprecated 提取、team.search、team.set_deprecated、team.remove
//! - `migrate_legacy_vector_store` 的旧树一次性拷贝成功臂
//! - entries.get / entries.delete 的 JSONL 兜底路径：store 缺失臂、
//!   空行跳过臂
//! - 语义路径带真实条目：entries.search 的逐条映射、entries.get /
//!   entries.delete 的 manager 臂（test-fixture 嵌入函数注入，ONNX
//!   插件在测试进程不可用）
//!
//! 结构性豁免（见报告）：env_setup/env_check 的插件就绪分支（需要
//! plugin_onnx.dll 与模型文件，真下载）、migrate 的 parent=None（目标
//! 路径恒有父目录）与拷贝失败臂（create_dir_all 成功后的竞态/ACL 才会
//! 触发）、`overall: "ready"`（依赖插件发现）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use nemesis_board::models::NewTeamMemory;
use nemesis_memory::manager::{Config as MemoryConfig, MemoryManager};
use nemesis_types::cluster::NodeRole;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_state(
    ws: &str,
    memory_manager: Option<Arc<MemoryManager>>,
    board: Option<nemesis_board::BoardService>,
) -> AppState {
    AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
        home: Some(ws.to_string()),
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
        memory_manager,
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
        board,
    }
}

fn agt_ctx_full(
    dir: &tempfile::TempDir,
    memory_manager: Option<Arc<MemoryManager>>,
    board: Option<nemesis_board::BoardService>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(agt_state(&ws, memory_manager, board));
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn agt_run(
    ctx: &RequestContext,
    cmd: &str,
    data: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    MemoryHandler.handle_cmd(cmd, Some(data), ctx).await
}

// ---------------------------------------------------------------------------
// team_memory：board 缺席报错 + 全命令组
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_team_memory_requires_board_service() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx_full(&dir, None, None);
    // 私有取用函数直接断言（BoardStore 无 Debug，用 .err() 取错误）
    let err = require_team_memory_store(&ctx).err().unwrap();
    assert!(err.contains("board service not available"), "err: {err}");
    // dispatch 层同样诚实报错（在参数提取之前）
    let err2 = agt_run(&ctx, "team.list", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err2.contains("board service not available"), "err: {err2}");
}

#[tokio::test]
async fn agt_team_memory_command_group_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store =
        nemesis_board::BoardStore::open(&dir.path().join("board.db"), "NB").expect("open store");
    // 两条经验：一条正常，一条软删
    let (id_active, _) = store
        .add_team_memory(NewTeamMemory {
            category: "lesson".into(),
            scope: "rust".into(),
            content: "cargo check before commit".into(),
            source: "agt".into(),
            author: "agt".into(),
        })
        .unwrap();
    let (id_soft, _) = store
        .add_team_memory(NewTeamMemory {
            category: "lesson".into(),
            scope: "other".into(),
            content: "deprecated note".into(),
            source: "agt".into(),
            author: "agt".into(),
        })
        .unwrap();
    store.set_team_memory_deprecated(id_soft, true).unwrap();

    let service = nemesis_board::BoardService::new(Arc::new(store), NodeRole::Coordinator);
    let ctx = agt_ctx_full(&dir, None, Some(service));

    // team.list：默认滤掉软删（include_deprecated=false 缺省臂）
    let out = agt_run(&ctx, "team.list", serde_json::json!({}))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1, "{out}");
    assert_eq!(out["entries"][0]["id"], serde_json::json!(id_active));

    // include_deprecated=true + scope 过滤（158/161-162 提取臂）
    let out = agt_run(
        &ctx,
        "team.list",
        serde_json::json!({ "scope": "other", "include_deprecated": true }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["total"], 1, "{out}");
    assert_eq!(out["entries"][0]["id"], serde_json::json!(id_soft));

    // team.search（LIKE 命中）
    let out = agt_run(&ctx, "team.search", serde_json::json!({ "query": "cargo" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 1, "{out}");
    assert!(
        out["entries"][0]["content"]
            .as_str()
            .unwrap()
            .contains("cargo")
    );

    // team.set_deprecated：恢复活跃（193-199 提取与缺省臂）
    let out = agt_run(
        &ctx,
        "team.set_deprecated",
        serde_json::json!({ "id": id_soft, "deprecated": false }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["updated"], true);
    assert_eq!(out["deprecated"], false);

    // team.remove（183 提取臂）
    let out = agt_run(&ctx, "team.remove", serde_json::json!({ "id": id_soft }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["removed"], true);
    assert_eq!(out["id"], serde_json::json!(id_soft));

    // 参数形态臂：id 缺失 / deprecated 缺省=true
    let err = agt_run(&ctx, "team.set_deprecated", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("missing field: id"), "err: {err}");
    let err = agt_run(&ctx, "team.remove", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("missing field: id"), "err: {err}");
}

// ---------------------------------------------------------------------------
// migrate_legacy_vector_store：旧树一次性拷贝
// ---------------------------------------------------------------------------

#[test]
fn agt_migrate_legacy_vector_store_copies_once() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let legacy = dir
        .path()
        .join("memory")
        .join("vector")
        .join("vector_store.jsonl");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::write(&legacy, "{\"id\":\"legacy-1\"}\n").unwrap();

    // 目标不存在 → 拷贝成功（265-266 臂）
    migrate_legacy_vector_store(&ws);
    let target = vector_store_jsonl_path(&ws);
    assert!(target.is_file());
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "{\"id\":\"legacy-1\"}\n"
    );

    // 目标已存在 → 不再覆盖（253 早退臂，保持幂等）
    std::fs::write(&legacy, "{\"id\":\"changed\"}\n").unwrap();
    migrate_legacy_vector_store(&ws);
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "{\"id\":\"legacy-1\"}\n"
    );
}

// ---------------------------------------------------------------------------
// entries.get / entries.delete 的 JSONL 兜底路径边臂
// ---------------------------------------------------------------------------

fn agt_write_vector_jsonl(ws: &std::path::Path, body: &str) {
    let path = vector_store_jsonl_path(&ws.to_string_lossy());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, body).unwrap();
}

#[tokio::test]
async fn agt_entries_get_delete_keyword_fallback_edges() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = agt_ctx_full(&dir, None, None);

    // store 缺失：get → entry null（1002 臂）；delete → deleted false（1045 臂）
    let out = agt_run(&ctx, "entries.get", serde_json::json!({ "id": "nope" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["entry"], serde_json::Value::Null);
    let out = agt_run(&ctx, "entries.delete", serde_json::json!({ "id": "nope" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["deleted"], false);

    // 空行 + 条目：get 命中（1009 空行跳过臂）；delete 重写跳过空行（1054 臂）
    agt_write_vector_jsonl(
        dir.path(),
        concat!(
            "\n",
            "{\"id\":\"agt-e1\",\"type\":\"long_term\",\"content\":\"keep me\",\"metadata\":{},\"tags\":[],\"score\":0.0,\"created_at\":\"2026-09-24T00:00:00+08:00\",\"updated_at\":\"2026-09-24T00:00:00+08:00\"}\n",
            "\n"
        ),
    );
    let out = agt_run(&ctx, "entries.get", serde_json::json!({ "id": "agt-e1" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["entry"]["content"], "keep me", "{out}");

    let out = agt_run(
        &ctx,
        "entries.delete",
        serde_json::json!({ "id": "agt-e1" }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["deleted"], true);
    let out = agt_run(&ctx, "entries.get", serde_json::json!({ "id": "agt-e1" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["entry"], serde_json::Value::Null);
}

// ---------------------------------------------------------------------------
// 语义路径带真实条目（test-fixture 嵌入注入）
// ---------------------------------------------------------------------------

/// 确定性 64 维字节袋嵌入——同串自相似 1.0，保证跨过任意阈值。
fn agt_embed() -> nemesis_memory::vector::EmbeddingFunc {
    Box::new(|text: &str| {
        let mut v = vec![0.0f32; 64];
        for b in text.to_lowercase().bytes() {
            v[(b as usize) % 64] += 1.0;
        }
        Ok(v)
    })
}

#[tokio::test]
async fn agt_semantic_search_get_delete_with_live_vector_store() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = dir.path().join("memory_vector");
    let mgr = Arc::new(MemoryManager::new(&MemoryConfig::new(&data_dir)));
    let vs_config = nemesis_memory::vector::StoreConfig {
        embedding_tier: "agt-test".into(),
        plugin_path: None,
        config_dir: None,
        max_results: 10,
        similarity_threshold: 0.0,
        storage_path: data_dir
            .join("vector")
            .join("vector_store.jsonl")
            .to_string_lossy()
            .to_string(),
    };
    mgr.init_vector_store_with_embed(agt_embed(), vs_config)
        .unwrap();
    mgr.set_vector_enabled(true);

    let ctx = agt_ctx_full(&dir, Some(mgr.clone()), None);

    // entries.store → manager 路径写入向量库
    let out = agt_run(
        &ctx,
        "entries.store",
        serde_json::json!({ "content": "agt semantic needle content" }),
    )
    .await
    .unwrap()
    .unwrap();
    let id = out["id"].as_str().unwrap().to_string();
    assert_eq!(out["stored"], true);

    // entries.search → 语义命中 + 逐条映射（845-855 臂）
    let out = agt_run(
        &ctx,
        "entries.search",
        serde_json::json!({ "query": "agt semantic needle content", "limit": 5 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["search_type"], "semantic", "{out}");
    assert_eq!(out["total"], 1, "{out}");
    assert_eq!(out["results"][0]["id"], serde_json::json!(id));
    assert_eq!(out["results"][0]["content"], "agt semantic needle content");
    assert!(out["results"][0]["score"].is_number());
    assert!(out["results"][0]["created_at"].is_string());

    // entries.get → manager 臂（988-996）
    let out = agt_run(&ctx, "entries.get", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["entry"]["content"], "agt semantic needle content");

    // entries.delete → manager 臂（1035-1039）
    let out = agt_run(&ctx, "entries.delete", serde_json::json!({ "id": id }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["deleted"], true);
    // 注（疑似 BUG，仅记录不固化）：manager delete/forget 只删 keyword
    // store，不 purge 向量库副本（delete_by_id 才会）——删除后 entries.get
    // 经向量库 fallback 仍能取回该条目。见交付报告疑似 BUG 清单。
}
