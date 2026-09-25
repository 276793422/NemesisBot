//! forge.rs AGT 覆盖率批次（2026-09-25）。与 s10b/extra 两套互补，聚焦仍缺
//! 的确定性臂：
//! - config.save 的运行时 start/stop 联动（真 Forge 实例 + is_running 翻转）
//! - learning.toggle 的运行时 set_learning_enabled 联动 + 非 object 的
//!   config.forge.json 跳过臂（as_object_mut None）
//! - compute_experience_stats 的空行 continue / 非法行跳过 / 全非法 →
//!   avg 0.0 兜底
//! - 深目录读取器：count_jsonl_in_subdirs 两层 if-let 体、find_latest_file
//!   循环体、read_learning_cycles 读完整体 + 读失败 continue、
//!   read_recent_experiences / read_registry_artifacts 的读失败兜底
//!   （路径存在但其实是目录 → read_to_string 必败，Windows/Unix 同构）
//!
//! 结构性豁免（见报告）：
//! - 16-18 外全部为 Default 转发（本批已补）；
//! - 104 / 112：load_live / save_live 全局 store 命中臂——装全局 ConfigStore
//!   会劫持并行测试的 home 隔离（同 models.rs 101 / coding.rs 232 裁决）。

use super::*;
use crate::api_handlers::AppState;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

fn agt_ctx(
    dir: &tempfile::TempDir,
    forge: Option<std::sync::Arc<nemesis_forge::forge::Forge>>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        state: std::sync::Arc::new(AppState {
            auth_token: String::new(),
            session_count: std::sync::Arc::new(AtomicUsize::new(0)),
            workspace: Some(ws.clone()),
            home: Some(ws.clone()),
            version: "test".to_string(),
            start_time: Instant::now(),
            model_name: std::sync::Arc::new(parking_lot::Mutex::new("m".to_string())),
            model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
            model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
            event_hub: std::sync::Arc::new(crate::events::EventHub::new()),
            running: std::sync::Arc::new(AtomicBool::new(true)),
            session_manager: std::sync::Arc::new(
                crate::session::SessionManager::with_default_timeout(),
            ),
            inbound_tx: None,
            streaming_provider: None,
            ws_router: None,
            agent_service: None,
            data_store: None,
            memory_manager: None,
            forge,
            agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
            cluster: None,
            cluster_service: None,
            cluster_log_dir: None,
            workflow_engine: None,
            #[cfg(feature = "workflow")]
            chat_secret_store: std::sync::Arc::new(
                nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
            ),
            #[cfg(not(feature = "workflow"))]
            chat_secret_store: std::sync::Arc::new(()),
            #[cfg(feature = "workflow")]
            webhook_rate_limiter: std::sync::Arc::new(
                crate::handlers::workflow::WebhookRateLimiter::new(),
            ),
            #[cfg(not(feature = "workflow"))]
            webhook_rate_limiter: std::sync::Arc::new(()),
            internal_cmd_tx: None,
            estop: None,
            signature_verify: None,
            cron: None,
            board: None,
        }),
        auth_method: crate::session::AuthMethod::default(),
    }
}

async fn run(
    ctx: &RequestContext,
    cmd: &str,
    data: Option<serde_json::Value>,
) -> Result<Option<serde_json::Value>, String> {
    ForgeHandler::new().handle_cmd(cmd, data, ctx).await
}

// ---------------------------------------------------------------------------
// Default 转发
// ---------------------------------------------------------------------------

#[test]
fn agt_default_impl_matches_new() {
    let _ = ForgeHandler::default();
    let _ = ForgeHandler::new();
}

// ---------------------------------------------------------------------------
// config.save：运行时 start / stop 联动
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_config_save_runtime_start_then_stop() {
    let dir = tempfile::tempdir().unwrap();
    // 主配置：初始 forge.enabled=false。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({ "forge": { "enabled": false } }).to_string(),
    )
    .unwrap();
    let forge = std::sync::Arc::new(nemesis_forge::forge::Forge::new(
        nemesis_forge::config::ForgeConfig::default(),
        dir.path().to_path_buf(),
    ));
    let ctx = agt_ctx(&dir, Some(forge.clone()));
    assert!(!forge.is_running(), "初始未运行");

    // 开启：运行时实例存在且未运行 → spawn start。
    let out = run(
        &ctx,
        "config.save",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(forge.is_running(), "后台 start 已生效");

    // 关闭：was_enabled=true 且运行中 → spawn stop。
    let out = run(
        &ctx,
        "config.save",
        Some(serde_json::json!({ "enabled": false })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], true);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!forge.is_running(), "后台 stop 已生效");
}

// ---------------------------------------------------------------------------
// learning.toggle：运行时联动 + 非 object 配置跳过臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_learning_toggle_runtime_flag_and_nonobject_config_skip() {
    let dir = tempfile::tempdir().unwrap();
    let forge = std::sync::Arc::new(nemesis_forge::forge::Forge::new(
        nemesis_forge::config::ForgeConfig::default(),
        dir.path().to_path_buf(),
    ));
    let ctx = agt_ctx(&dir, Some(forge));

    // 常规 toggle：config.forge.json 自动创建 → learning 键补写 → 运行时旗标。
    let out = run(
        &ctx,
        "learning.toggle",
        Some(serde_json::json!({ "enabled": true })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], true);
    assert_eq!(out["learning_enabled"], true);

    // config.forge.json 写成非 object（如 123）→ as_object_mut None →
    // 更新跳过但命令仍成功。
    let cfg_path = nemesis_path::resolve_forge_config_path_in_workspace(dir.path());
    std::fs::write(&cfg_path, "123").unwrap();
    let out = run(
        &ctx,
        "learning.toggle",
        Some(serde_json::json!({ "enabled": false })),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(out["saved"], true);
    assert_eq!(out["learning_enabled"], false);
}

// ---------------------------------------------------------------------------
// experiences.stats：空行 / 非法行 / 全非法
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_experiences_stats_line_variants_and_zero_avg() {
    // ① 合法 + 空行 + 非法行：只有合法行计入。
    let dir = tempfile::tempdir().unwrap();
    let exp_dir = dir.path().join("forge").join("experiences");
    std::fs::create_dir_all(&exp_dir).unwrap();
    let exp_file = exp_dir.join("experiences.jsonl");
    std::fs::write(
        &exp_file,
        format!(
            "{}\n\nnot json at all\n",
            serde_json::json!({
                "experience": {
                    "id": "e1",
                    "tool_name": "exec",
                    "success": true,
                    "duration_ms": 120,
                    "input_summary": "ls",
                    "output_summary": "ok",
                    "timestamp": "2026-09-25T00:00:00+08:00",
                    "session_key": "agt",
                }
            })
        ),
    )
    .unwrap();
    let ctx = agt_ctx(&dir, None);
    let out = run(&ctx, "experiences.stats", None).await.unwrap().unwrap();
    assert_eq!(out["total"], 1, "{out}");
    assert_eq!(out["success"], 1);
    assert_eq!(out["avg_duration_ms"], 120.0);
    assert_eq!(out["recent"].as_array().map(Vec::len), Some(1));

    // ② 只有非法行：total=0 → avg 兜底 0.0。
    let dir2 = tempfile::tempdir().unwrap();
    let exp_dir2 = dir2.path().join("forge").join("experiences");
    std::fs::create_dir_all(&exp_dir2).unwrap();
    std::fs::write(exp_dir2.join("experiences.jsonl"), "garbage {\n").unwrap();
    let ctx2 = agt_ctx(&dir2, None);
    let out = run(&ctx2, "experiences.stats", None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["total"], 0, "{out}");
    assert_eq!(out["avg_duration_ms"], 0.0);
}

// ---------------------------------------------------------------------------
// 深目录读取器：目录型文件兜底 + learning/reflections 深路径
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_forge_tree_readers_dir_fallbacks_and_deep_paths() {
    let dir = tempfile::tempdir().unwrap();
    let fd = dir.path().join("forge");

    // experiences.jsonl 与 registry.json 放成目录（存在但读必败 → 兜底空）。
    std::fs::create_dir_all(fd.join("experiences").join("experiences.jsonl")).unwrap();
    std::fs::create_dir_all(fd.join("registry.json")).unwrap();
    // learning 月份子目录：一个可读 jsonl（两行：合法 + 空行）+ 一个目录型 jsonl。
    let month = fd.join("learning").join("2026-09");
    std::fs::create_dir_all(&month).unwrap();
    std::fs::write(
        month.join("data.jsonl"),
        "{\"cycle\":1,\"pattern\":\"p\"}\n\n",
    )
    .unwrap();
    std::fs::create_dir_all(month.join("blocked.jsonl")).unwrap();
    // reflections 一份 md。
    std::fs::create_dir_all(fd.join("reflections")).unwrap();
    std::fs::write(fd.join("reflections").join("report.md"), "# R\n").unwrap();

    let ctx = agt_ctx(&dir, None);

    // experiences.stats：recent 读失败兜底空（文件其实是目录）。
    let out = run(&ctx, "experiences.stats", None).await.unwrap().unwrap();
    assert_eq!(out["recent"].as_array().map(Vec::len), Some(0), "{out}");

    // stats：汇总各读取器（含 learn 月份双层遍历 + 最新 md + registry 兜底）。
    let out = run(&ctx, "stats", None).await.unwrap().unwrap();
    assert_eq!(out["cycles"]["total"], 1, "{out}");
    assert_eq!(out["artifacts"]["total"], 0, "{out}");
    assert!(
        out["reflections"]["latest"]["name"]
            .as_str()
            .unwrap()
            .ends_with(".md"),
        "{out}"
    );

    // registry.list：registry.json 为目录 → artifacts 兜底空。
    let out = run(&ctx, "registry.list", None).await.unwrap().unwrap();
    assert_eq!(out["artifacts"].as_array().map(Vec::len), Some(0), "{out}");

    // cycles.list：可读月份 jsonl 进结果；目录型 jsonl 读失败跳过不炸。
    let out = run(&ctx, "cycles.list", None).await.unwrap().unwrap();
    assert_eq!(out["cycles"].as_array().map(Vec::len), Some(1), "{out}");
}
