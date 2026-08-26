//! skills.rs 私有面覆盖（quality-hardening goal 冲刺 S10a，web 批次 1，
//! 2026-08-26）。
//!
//! `skills_more_tests.rs` / `skills_extra_tests.rs`（handlers 级公共 API）
//! 已覆盖 installed/detail/open_dir/uninstall/config.*/source.*/dispatch
//! 校验臂、install already+unknown-registry、shop_detail/shop_code 的
//! unknown-registry 臂、browse 的 sort/limit 参数臂。本模块补它们够不到的：
//!
//! - `learn` 三态（bridge 缺失 / 队列 send 失败 / 成功投递 prompt）
//! - `parse_github_url` 全解析臂（https/http/.git 剥离/git@/shorthand/错误）
//! - `load_registry_config` 三臂（文件缺失/损坏/合法）
//! - `source_add` 重名拒绝臂（在 spawn_blocking 探测**之前**，无网络）
//! - ClawHub 网络臂（wiremock 假 ClawHub）：search 成功+installed 标记 /
//!   search 失败映射 / browse 成功+next_cursor / shop_detail 成功+convex
//!   error / shop_code file API / install ZIP 成功 + convex error
//!
//! 结构性豁免候选：`detect_skill_structure` 及 `source_add` 的探测成功/
//! 失败臂——硬编码 `https://api.github.com` / `raw.githubusercontent.com`
//! 无注入 seam，测试触碰即真外网请求（见最终报告第 4 节）。

use super::*;
use std::io::Write;
use std::sync::Arc;

// ---------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------

/// 把 clawhub 三 URL 指向 wiremock 服务器的 config.skills.json 写进工作区。
fn write_clawhub_config(ws: &std::path::Path, server_uri: &str, enabled: bool) {
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let cfg = serde_json::json!({
        "clawhub": {
            "enabled": enabled,
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

/// convex getBySlug 成功响应体。
fn convex_ok(value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "status": "success", "value": value })
}

/// learn 测试用的最小 RequestContext（可注入 inbound_tx）。
fn learn_ctx(
    ws: &std::path::Path,
    inbound_tx: Option<tokio::sync::mpsc::UnboundedSender<
        crate::websocket_handler::IncomingMessage,
    >>,
) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let ws_str = ws.to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws_str.clone()),
        home: Some(ws_str.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx,
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
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "s10-session".to_string(),
        chat_id: "s10-chat".to_string(),
        workspace: Some(ws_str),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// ---------------------------------------------------------------------
// parse_github_url 全臂
// ---------------------------------------------------------------------

#[test]
fn parse_github_url_https_http_git_suffix_and_shorthand() {
    let (o, r) = parse_github_url("https://github.com/user/repo").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo"));

    // http scheme
    let (o, r) = parse_github_url("http://github.com/user/repo2").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo2"));

    // .git 后缀剥离 + 尾部斜杠
    let (o, r) = parse_github_url("https://github.com/user/repo3.git/").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo3"));

    // git@ SSH 形式（含 .git）
    let (o, r) = parse_github_url("git@github.com:user/repo4.git").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo4"));

    // owner/repo shorthand
    let (o, r) = parse_github_url("user/repo5").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo5"));

    // 前后空白被 trim
    let (o, r) = parse_github_url("  user/repo6  ").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("user", "repo6"));
}

#[test]
fn parse_github_url_rejects_owner_only_and_spacey() {
    // 只有 owner 没有两段 → Err。(BUG #25) 原实现前缀命中后掉进
    // shorthand 分支产出 ("https:", "/github.com/onlyowner") 垃圾解析，
    // 现在直接报 URL 解析错误。
    let err = parse_github_url("https://github.com/onlyowner").unwrap_err();
    assert!(err.contains("无法解析 URL"), "err: {err}");
    let err = parse_github_url("http://github.com/onlyowner").unwrap_err();
    assert!(err.contains("无法解析 URL"), "err: {err}");

    // shorthand 只有 owner
    assert!(parse_github_url("onlyowner").is_err());
    // 带空格 → 不走 shorthand
    assert!(parse_github_url("has space/x").is_err());
    // 空字符串
    assert!(parse_github_url("").is_err());
    // 非 github host 的完整 URL 走 shorthand：两段非空 → Ok（str 拆分，
    // 既有宽松设计，测试钉住）
    let (o, r) = parse_github_url("https://gitlab.com/a/b").unwrap();
    assert_eq!((o.as_str(), r.as_str()), ("https:", "/gitlab.com/a/b"));
}

// ---------------------------------------------------------------------
// load_registry_config 三臂
// ---------------------------------------------------------------------

#[test]
fn load_registry_config_missing_corrupt_and_valid_arms() {
    let dir = tempfile::tempdir().unwrap();

    // 1) 文件缺失 → default（clawhub 未启用）
    let cfg = load_registry_config(&dir.path().join("nope/config.skills.json"));
    assert!(!cfg.clawhub.enabled);
    assert!(cfg.github_sources.is_empty());

    // 2) 文件损坏 → default
    let p = dir.path().join("config.skills.json");
    std::fs::write(&p, "{not json").unwrap();
    let cfg = load_registry_config(&p);
    assert!(!cfg.clawhub.enabled);

    // 3) 合法文件 → 值透传
    std::fs::write(
        &p,
        r#"{"clawhub":{"enabled":true,"base_url":"http://x","convex_url":"http://y"}}"#,
    )
    .unwrap();
    let cfg = load_registry_config(&p);
    assert!(cfg.clawhub.enabled);
    assert_eq!(cfg.clawhub.base_url, "http://x");
    assert_eq!(cfg.clawhub.convex_url, "http://y");
}

// ---------------------------------------------------------------------
// learn 三态
// ---------------------------------------------------------------------

#[tokio::test]
async fn learn_without_bridge_and_with_dead_channel_error() {
    let dir = tempfile::tempdir().unwrap();

    // 1) inbound_tx 缺失
    let ctx = learn_ctx(dir.path(), None);
    let err = SkillsHandler::new()
        .handle_cmd(
            "learn",
            Some(serde_json::json!({ "source": "some doc text" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "agent chat bridge is not available");

    // 2) 通道已关闭（rx 被 drop）→ send 失败
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    drop(rx);
    let ctx = learn_ctx(dir.path(), Some(tx));
    let err = SkillsHandler::new()
        .handle_cmd(
            "learn",
            Some(serde_json::json!({ "source": "some doc text" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "failed to enqueue learn request");

    // 3) 缺 source 字段
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = learn_ctx(dir.path(), Some(tx));
    let err = SkillsHandler::new()
        .handle_cmd("learn", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("source"), "err: {err}");
}

#[tokio::test]
async fn learn_success_enqueues_prompt_with_source_and_name_hint() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let ctx = learn_ctx(dir.path(), Some(tx));

    let payload = SkillsHandler::new()
        .handle_cmd(
            "learn",
            Some(serde_json::json!({
                "source": "THE-SOURCE-DOC for distillation",
                "name": "my-skill"
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(payload["status"], "started");
    assert_eq!(payload["chat_id"], "s10-chat");

    let incoming = rx.try_recv().unwrap();
    assert_eq!(incoming.session_id, "s10-session");
    assert_eq!(incoming.sender_id, "s10-session"); // sender = session
    assert_eq!(incoming.chat_id, "s10-chat");
    assert!(incoming.content.contains("THE-SOURCE-DOC"));
    assert!(incoming.content.contains("'my-skill'"), "name hint embedded");
    assert!(incoming.content.contains("learn"));

    // 无 name → prompt 不含 hint 片段
    let (tx2, mut rx2) = tokio::sync::mpsc::unbounded_channel();
    let ctx2 = learn_ctx(dir.path(), Some(tx2));
    SkillsHandler::new()
        .handle_cmd(
            "learn",
            Some(serde_json::json!({ "source": "plain source" })),
            &ctx2,
        )
        .await
        .unwrap();
    let incoming2 = rx2.try_recv().unwrap();
    assert!(incoming2.content.contains("plain source"));
    assert!(!incoming2.content.contains("as the skill name"));
}

// ---------------------------------------------------------------------
// source_add：重名拒绝臂（无网络——dup 检查先于 spawn_blocking 探测）
// ---------------------------------------------------------------------

#[tokio::test]
async fn source_add_duplicate_name_rejected_before_detection() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let cfg_dir = ws.join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // 预置一个名为 duprepo 的 github 源
    std::fs::write(
        ws.join("config/config.skills.json"),
        r#"{"github_sources":[{"name":"duprepo","repo":"someone/duprepo","enabled":true}]}"#,
    )
    .unwrap();

    let err = SkillsHandler::new()
        .source_add(
            &ws.to_string_lossy(),
            &serde_json::json!({ "url": "https://github.com/user/duprepo" }),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "源 'duprepo' 已存在");
}

// ---------------------------------------------------------------------
// ClawHub 网络臂（wiremock 假 ClawHub）
// ---------------------------------------------------------------------

#[tokio::test]
async fn search_ok_via_wiremock_marks_installed() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri(), true);
    // 本地装了一个同名 skill → installed 标记为 true
    std::fs::create_dir_all(ws.join("skills/s10-skill")).unwrap();

    Mock::given(method("GET"))
        .and(path("/api/search"))
        .and(query_param("q", "anything"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "results": [
                {"score": 4.0, "slug": "s10-skill", "displayName": "S10 Skill", "summary": "Test summary"},
                {"score": 0.5, "slug": "other-skill", "displayName": "Other", "summary": "Not installed"}
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let r = SkillsHandler::new()
        .search("anything", &ws.to_string_lossy())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["query"], "anything");
    let results = r["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    // score>1 归一化 4.0/5.0 = 0.8
    assert_eq!(results[0]["slug"], "s10-skill");
    assert_eq!(results[0]["name"], "S10 Skill");
    assert_eq!(results[0]["source"], "clawhub");
    assert_eq!(results[0]["installed"], true);
    assert_eq!(results[1]["slug"], "other-skill");
    assert_eq!(results[1]["installed"], false);
}

#[tokio::test]
async fn search_registry_failure_maps_chinese_prefix() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_clawhub_config(dir.path(), &server.uri(), true);

    // 唯一源返回 500 → search_all 全失败 → “搜索失败: ...”
    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let err = SkillsHandler::new()
        .search("q", &dir.path().to_string_lossy())
        .await
        .unwrap_err();
    assert!(err.starts_with("搜索失败"), "err: {err}");
    assert!(err.contains("500"), "err: {err}");
}

#[tokio::test]
async fn browse_ok_returns_items_next_cursor_and_installed() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri(), true);
    std::fs::create_dir_all(ws.join("skills/known-skill")).unwrap();

    // 默认参数：registry=clawhub sort=trending limit=20；cursor 透传
    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .and(query_param("sort", "trending"))
        .and(query_param("limit", "20"))
        .and(query_param("cursor", "page2"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [
                {"slug": "known-skill", "displayName": "Known", "summary": "s1", "stats": {"downloads": 7.0}},
                {"slug": "fresh-skill", "displayName": "Fresh", "summary": "s2", "stats": {"downloads": 0.0}}
            ],
            "nextCursor": "page3"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let r = SkillsHandler::new()
        .browse(
            // 不传 registry/sort/limit → 默认；cursor 显式给
            &serde_json::json!({ "cursor": "page2" }),
            &ws.to_string_lossy(),
        )
        .await
        .unwrap()
        .unwrap();
    let items = r["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["slug"], "known-skill");
    assert_eq!(items[0]["downloads"], 7);
    assert_eq!(items[0]["installed"], true);
    assert_eq!(items[1]["installed"], false);
    assert_eq!(r["next_cursor"], "page3");
}

#[tokio::test]
async fn shop_detail_ok_and_convex_error_arms() {
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // 1) Ok 臂：convex 返回完整 detail
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri(), true);
    std::fs::create_dir_all(ws.join("skills/detail-skill")).unwrap();

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .and(body_partial_json(serde_json::json!({
            "path": "skills:getBySlug",
            "args": {"slug": "detail-skill"}
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_ok(
            serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "detail-skill", "displayName": "Detail",
                          "summary": "Sum", "stats": {"downloads": 100.0}},
                "latestVersion": {"version": "1.2.0"},
                "resolvedSlug": ""
            })
        )))
        .expect(1)
        .mount(&server)
        .await;

    let r = SkillsHandler::new()
        .shop_detail("clawhub", "detail-skill", &ws.to_string_lossy())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["slug"], "detail-skill");
    assert_eq!(r["name"], "Detail");
    assert_eq!(r["version"], "1.2.0");
    assert_eq!(r["registry"], "clawhub");
    assert_eq!(r["author"], "alice");
    assert_eq!(r["downloads"], 100);
    assert_eq!(r["installed"], true);

    // 2) convex error → “获取详情失败: convex error: ...”（不走 GitHub 回退）
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_clawhub_config(dir.path(), &server.uri(), true);
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "error", "value": null, "errorMessage": "db down"
        })))
        .mount(&server)
        .await;

    let err = SkillsHandler::new()
        .shop_detail("clawhub", "whatever", &dir.path().to_string_lossy())
        .await
        .unwrap_err();
    assert!(err.starts_with("获取详情失败"), "err: {err}");
    assert!(err.contains("db down"), "err: {err}");
}

#[tokio::test]
async fn shop_code_ok_via_file_api() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_clawhub_config(dir.path(), &server.uri(), true);

    Mock::given(method("GET"))
        .and(path("/api/v1/skills/code-skill/file"))
        .and(query_param("path", "SKILL.md"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("# Code Skill\n\nbody"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = SkillsHandler::new()
        .shop_code("clawhub", "code-skill", &dir.path().to_string_lossy())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["slug"], "code-skill");
    assert_eq!(r["filename"], "SKILL.md");
    assert!(r["code"].as_str().unwrap().contains("# Code Skill"));
}

#[tokio::test]
async fn install_ok_downloads_zip_and_extracts() {
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    write_clawhub_config(ws, &server.uri(), true);

    // 1) convex getBySlug → owner + version
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_ok(
            serde_json::json!({
                "owner": {"handle": "alice"},
                "skill": {"slug": "zip-skill", "displayName": "Zip",
                          "summary": "zipped", "stats": {"downloads": 0.0}},
                "latestVersion": {"version": "2.0.0"},
                "resolvedSlug": ""
            })
        )))
        .expect(1)
        .mount(&server)
        .await;

    // 2) ZIP 下载（真实 ZIP 字节，顶层目录会被拍平）
    let zip = zip_with(&[("zip-skill/SKILL.md", "# Zip Skill\nhello")]);
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "zip-skill"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip),
        )
        .expect(1)
        .mount(&server)
        .await;

    let r = SkillsHandler::new()
        .install(
            &serde_json::json!({ "registry": "clawhub", "slug": "zip-skill" }),
            &ws.to_string_lossy(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["installed"], true);
    assert_eq!(r["slug"], "zip-skill");
    assert_eq!(r["version"], "2.0.0");
    assert_eq!(r["is_malware_blocked"], false);
    assert_eq!(r["summary"], "zipped");
    // 解压落地（顶层目录拍平）
    let skill_md = ws.join("skills/zip-skill/SKILL.md");
    assert!(skill_md.exists(), "SKILL.md must be extracted");
    assert!(std::fs::read_to_string(skill_md).unwrap().contains("Zip Skill"));
}

#[tokio::test]
async fn install_convex_error_maps_chinese_prefix() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // convex error → download_and_install 在 ZIP/GitHub 之前就 Err，
    // 不会触发 GitHub 回退（无真外网请求）。
    let server = MockServer::start().await;
    let dir = tempfile::tempdir().unwrap();
    write_clawhub_config(dir.path(), &server.uri(), true);

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "error", "value": null, "errorMessage": "no such skill"
        })))
        .mount(&server)
        .await;

    let err = SkillsHandler::new()
        .install(
            &serde_json::json!({ "registry": "clawhub", "slug": "ghost-skill" }),
            &dir.path().to_string_lossy(),
        )
        .await
        .unwrap_err();
    assert!(err.starts_with("安装失败"), "err: {err}");
    assert!(err.contains("no such skill"), "err: {err}");
}
