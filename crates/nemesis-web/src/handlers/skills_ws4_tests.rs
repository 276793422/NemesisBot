//! WS4 供应链工作流 WSAPI 面覆盖（2026-09-25）：`skills.install` 走
//! SkillInstaller 漏斗（lockfile 记账 / gate 拒绝）、`skills.uninstall`
//! 联动记账移除、`skills.verify` 漂移检测命令。
//!
//! GitHub 直装臂（wiremock 假 GitHub）在 `nemesis-skills` 的
//! `installer::ws4_tests` 覆盖（installer 可注入 api/raw base URL；
//! WSAPI 侧无注入 seam，诚实豁免真外网请求）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// -----------------------------------------------------------------------
// helpers
// -----------------------------------------------------------------------

fn make_ctx_with_gate(
    dir: &tempfile::TempDir,
    gate: Option<nemesis_skills::install_gate::SharedInstallGate>,
) -> RequestContext {
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
        signature_verify: None,
        skills_install_gate: gate,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "ws4-session".to_string(),
        chat_id: "ws4-chat".to_string(),
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    make_ctx_with_gate(dir, None)
}

fn write_clawhub_config(ws: &Path, server_uri: &str) {
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let cfg = serde_json::json!({
        "clawhub": {
            "enabled": true,
            "base_url": server_uri,
            "convex_url": server_uri,
            "convex_site_url": server_uri,
        }
    });
    std::fs::write(
        ws.join("config/config.skills.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn zip_with(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content.as_bytes()).unwrap();
        }
        writer.finish().unwrap();
    }
    buf
}

fn convex_ok(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "status": "success", "value": value })
}

// -----------------------------------------------------------------------
// verify 命令
// -----------------------------------------------------------------------

#[tokio::test]
async fn verify_unrecorded_slug_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();

    let err = handler
        .handle_cmd("verify", Some(serde_json::json!({ "slug": "ghost" })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("not recorded"), "err: {err}");
    assert!(err.contains("ghost"), "err: {err}");
}

#[tokio::test]
async fn verify_all_without_lockfile_reports_zero_totals() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();

    let r = handler
        .handle_cmd("verify", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["total"], 0);
    assert_eq!(r["clean"], 0);
    assert!(r["skills"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn verify_reports_clean_after_wsapi_install() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(convex_ok(serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "ws4-skill", "displayName": "Ws4",
                          "summary": "clean", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "1.0.0"},
                "resolvedSlug": ""
            }))),
        )
        .mount(&server)
        .await;
    let zip = zip_with(&[("ws4-skill/SKILL.md", "# Ws4\nbody")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "ws4-skill"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip),
        )
        .mount(&server)
        .await;

    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();

    let r = handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "ws4-skill" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["installed"], true);
    assert_eq!(r["is_malware_blocked"], false);

    // P14：lockfile 落盘且带逐文件记账。
    let lock_path = ws.join("skills.lock.json");
    assert!(lock_path.exists(), "skills.lock.json must exist");
    let lock: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&lock_path).unwrap()).unwrap();
    let entry = &lock["skills"]["ws4-skill"];
    assert_eq!(entry["source"], "registry:clawhub/ws4-skill");
    assert!(
        entry["files"]["SKILL.md"].is_string(),
        "SKILL.md must be hashed: {entry}"
    );

    // 漂移检测：刚装的应 clean。
    let r = handler
        .handle_cmd(
            "verify",
            Some(serde_json::json!({ "slug": "ws4-skill" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["clean"], true, "report: {r}");
}

#[tokio::test]
async fn verify_detects_drift_after_tamper() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(convex_ok(serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "tampered", "displayName": "T",
                          "summary": "t", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "1.0.0"},
                "resolvedSlug": ""
            }))),
        )
        .mount(&server)
        .await;
    let zip = zip_with(&[("tampered/SKILL.md", "# T\noriginal")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "tampered"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip),
        )
        .mount(&server)
        .await;

    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();
    handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "tampered" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

    // 篡改已装技能内容。
    std::fs::write(ws.join("skills/tampered/SKILL.md"), "# T\nEVIL EDIT").unwrap();

    let r = handler
        .handle_cmd(
            "verify",
            Some(serde_json::json!({ "slug": "tampered" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["clean"], false);
    let modified = r["modified_files"].as_array().unwrap();
    assert_eq!(modified.len(), 1, "report: {r}");
    assert_eq!(modified[0], serde_json::json!("SKILL.md"));
}

#[tokio::test]
async fn uninstall_via_wsapi_removes_lockfile_entry() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(convex_ok(serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "shortlived", "displayName": "S",
                          "summary": "s", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "1.0.0"},
                "resolvedSlug": ""
            }))),
        )
        .mount(&server)
        .await;
    let zip = zip_with(&[("shortlived/SKILL.md", "# S\nbye")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "shortlived"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip),
        )
        .mount(&server)
        .await;

    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();
    handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "shortlived" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

    let r = handler
        .handle_cmd(
            "uninstall",
            Some(serde_json::json!({ "name": "shortlived" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["uninstalled"], true);
    assert!(!ws.join("skills/shortlived").exists());

    // P14：记账同步移除——verify 回到未记账态。
    let err = handler
        .handle_cmd(
            "verify",
            Some(serde_json::json!({ "slug": "shortlived" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not recorded"), "err: {err}");
}

#[tokio::test]
async fn deny_gate_blocks_wsapi_install_without_writing() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(convex_ok(serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "gated", "displayName": "G",
                          "summary": "g", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "1.0.0"},
                "resolvedSlug": ""
            }))),
        )
        .mount(&server)
        .await;
    let zip = zip_with(&[("gated/SKILL.md", "# G\nnope")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "gated"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip),
        )
        .mount(&server)
        .await;

    let ctx = make_ctx_with_gate(
        &dir,
        Some(Arc::new(nemesis_skills::install_gate::AlwaysDenyGate)),
    );
    let handler = skills::SkillsHandler::new();
    let err = handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "gated" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("install denied by approval gate"),
        "err: {err}"
    );

    // P13：deny 全程不落盘（无技能目录、无 lockfile、staging 无技能残留；
    // `.skill-staging` 空父目录允许留存）。
    assert!(!ws.join("skills/gated").exists());
    assert!(!ws.join("skills.lock.json").exists());
    assert!(
        !ws.join(".skill-staging/gated").exists(),
        "staged skill must be cleaned on deny"
    );
}

#[tokio::test]
async fn force_reinstall_replaces_locked_skill() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(convex_ok(serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "refill", "displayName": "R",
                          "summary": "r", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "1.0.0"},
                "resolvedSlug": ""
            }))),
        )
        .mount(&server)
        .await;
    let zip_v1 = zip_with(&[("refill/SKILL.md", "# R\nv1")]);
    let zip_v2 = zip_with(&[("refill/SKILL.md", "# R\nv2")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "refill"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip_v2),
        )
        .mount(&server)
        .await;
    let _ = zip_v1;

    let ctx = make_ctx(&dir);
    let handler = skills::SkillsHandler::new();
    handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "refill" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

    // 不带 force：already_installed。
    let r = handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({ "registry": "clawhub", "slug": "refill" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["already_installed"], true);

    // 带 force：卸旧（含记账）再装新，内容替换。
    let r = handler
        .handle_cmd(
            "install",
            Some(serde_json::json!({
                "registry": "clawhub", "slug": "refill", "force": true
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["installed"], true);
    let content = std::fs::read_to_string(ws.join("skills/refill/SKILL.md")).unwrap();
    assert_eq!(content, "# R\nv2");

    // 记账哈希对应新内容——verify 仍 clean。
    let r = handler
        .handle_cmd(
            "verify",
            Some(serde_json::json!({ "slug": "refill" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["clean"], true, "report: {r}");
}
