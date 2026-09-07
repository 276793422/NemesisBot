use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::{AuthMethod, SessionManager};
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tempfile::TempDir;

/// 搭一个最小补全测试工作区：顶层文件 + 子目录 + 被忽略目录 + 含空白名。
fn setup_workspace() -> TempDir {
    let tmp = TempDir::new().unwrap();
    let ws = tmp.path();
    std::fs::write(ws.join("alpha.txt"), "a").unwrap();
    std::fs::write(ws.join("beta.rs"), "b").unwrap();
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src").join("main.rs"), "m").unwrap();
    std::fs::write(ws.join("src").join("util.rs"), "u").unwrap();
    // 共享忽略表里的运行时目录（补全必须不可见）。
    std::fs::create_dir_all(ws.join("node_modules").join("dep")).unwrap();
    std::fs::write(ws.join("node_modules").join("dep").join("index.js"), "x").unwrap();
    std::fs::create_dir_all(ws.join("logs")).unwrap();
    std::fs::write(ws.join("logs").join("app.log"), "l").unwrap();
    // 含空白的文件名（@token 语义敲不出来）。
    std::fs::write(ws.join("my file.txt"), "w").unwrap();
    tmp
}

fn complete(ws: &TempDir, prefix: &str) -> (Vec<String>, bool) {
    let h = FsHandler::new();
    let v = h
        .complete_path_in(ws.path().to_str().unwrap(), prefix)
        .unwrap();
    let paths = v["paths"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_str().unwrap().to_string())
        .collect();
    (paths, v["truncated"].as_bool().unwrap())
}

#[test]
fn test_empty_prefix_lists_top_level() {
    let ws = setup_workspace();
    let (paths, truncated) = complete(&ws, "");
    assert!(!truncated);
    assert!(paths.contains(&"alpha.txt".to_string()), "{:?}", paths);
    assert!(paths.contains(&"beta.rs".to_string()), "{:?}", paths);
    assert!(paths.contains(&"src/".to_string()), "{:?}", paths);
}

#[test]
fn test_prefix_matches_dir_and_children() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "src");
    assert!(paths.contains(&"src/".to_string()), "{:?}", paths);
    assert!(paths.contains(&"src/main.rs".to_string()), "{:?}", paths);
    assert!(paths.contains(&"src/util.rs".to_string()), "{:?}", paths);
    assert!(!paths.iter().any(|p| p.starts_with("alpha")), "{:?}", paths);
}

#[test]
fn test_basename_match_without_dir() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "main.rs");
    assert_eq!(paths, vec!["src/main.rs".to_string()]);
}

#[test]
fn test_case_insensitive() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "MAIN");
    assert!(paths.contains(&"src/main.rs".to_string()), "{:?}", paths);
}

#[test]
fn test_subdir_partial_prefix() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "src/ma");
    assert_eq!(paths, vec!["src/main.rs".to_string()]);
}

#[test]
fn test_ignored_dirs_and_files_never_listed() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "");
    assert!(
        !paths.iter().any(|p| p.contains("node_modules")),
        "{:?}",
        paths
    );
    assert!(!paths.iter().any(|p| p.contains("logs/")), "{:?}", paths);
    assert!(!paths.iter().any(|p| p.contains("app.log")), "{:?}", paths);
}

#[test]
fn test_whitespace_names_skipped() {
    let ws = setup_workspace();
    let (paths, _) = complete(&ws, "my");
    assert!(paths.is_empty(), "{:?}", paths);
}

#[test]
fn test_cap_twenty_and_truncated_flag() {
    let ws = TempDir::new().unwrap();
    let many = ws.path().join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..25 {
        std::fs::write(many.join(format!("f{:02}.txt", i)), "x").unwrap();
    }
    let h = FsHandler::new();
    let v = h
        .complete_path_in(ws.path().to_str().unwrap(), "many/")
        .unwrap();
    assert_eq!(v["paths"].as_array().unwrap().len(), MAX_COMPLETIONS);
    assert!(v["truncated"].as_bool().unwrap());
}

#[test]
fn test_nonexistent_workspace_errors() {
    let h = FsHandler::new();
    let err = h
        .complete_path_in("Z:/definitely/not/here", "x")
        .unwrap_err();
    assert!(err.contains("workspace not found"), "{}", err);
}

// ---------------------------------------------------------------------------
// handle_cmd 层（RequestContext 直构；build_state 形态同 memory/s10b_tests）
// ---------------------------------------------------------------------------

fn build_state(ws: &str) -> AppState {
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
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
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
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    }
}

fn ctx_with_workspace(ws: &TempDir) -> RequestContext {
    let ws_str = ws.path().to_string_lossy().into_owned();
    let state = Arc::new(build_state(&ws_str));
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "web:test".to_string(),
        workspace: Some(ws_str.clone()),
        home: Some(ws_str),
        state,
        auth_method: AuthMethod::default(),
    }
}

#[tokio::test]
async fn test_handle_cmd_complete_path() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let out = h
        .handle_cmd(
            "complete_path",
            Some(serde_json::json!({ "prefix": "beta" })),
            &ctx_with_workspace(&ws),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["paths"][0], "beta.rs");
}

/// data 缺失 / prefix 非字符串 → 按空前缀兜底（顶层枚举），不报错。
#[tokio::test]
async fn test_handle_cmd_missing_data_defaults_empty_prefix() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let out = h
        .handle_cmd("complete_path", None, &ctx_with_workspace(&ws))
        .await
        .unwrap()
        .unwrap();
    assert!(!out["paths"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_handle_cmd_unknown_cmd_rejected() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let err = h
        .handle_cmd("nope", None, &ctx_with_workspace(&ws))
        .await
        .unwrap_err();
    assert!(err.contains("unknown command: fs.nope"), "{}", err);
}

#[tokio::test]
async fn test_handle_cmd_requires_workspace() {
    let h = FsHandler::new();
    let state = Arc::new(build_state("/unused"));
    let ctx = RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: AuthMethod::default(),
    };
    let err = h.handle_cmd("complete_path", None, &ctx).await.unwrap_err();
    assert!(err.contains("workspace not configured"), "{}", err);
}

#[test]
fn test_module_name() {
    assert_eq!(FsHandler.module_name(), "fs");
}

// ---------------------------------------------------------------------------
// M4：fs.tree 目录树（懒展开 / 忽略表 / 500 上限 / 越权拒绝）
// ---------------------------------------------------------------------------

fn tree(ws: &TempDir, path: &str, depth: u64) -> serde_json::Value {
    let h = FsHandler::new();
    h.tree_in(ws.path().to_str().unwrap(), path, depth as usize)
        .unwrap()
}

/// 根层列出：忽略表同补全（node_modules/logs 不可见）、目录在前文件在后、
/// depth 边界子目录 children=null（懒展开点）、展开的空目录 children=[]。
#[test]
fn tree_root_lists_depth_and_ignore() {
    let ws = setup_workspace();
    // 空目录（展开后应呈 children=[]，非 null）。
    std::fs::create_dir_all(ws.path().join("empty_dir")).unwrap();

    let v = tree(&ws, "", 2);
    assert_eq!(v["path"], "");
    assert_eq!(v["truncated"], false);
    let entries = v["entries"].as_array().unwrap();

    // 目录在前（empty_dir, src），文件在后（alpha.txt, beta.rs）。
    let names: Vec<&str> = entries
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        vec!["empty_dir", "src", "alpha.txt", "beta.rs"],
        "{v}"
    );

    let src = entries.iter().find(|e| e["name"] == "src").unwrap();
    assert_eq!(src["type"], "dir");
    // depth=2：src 的子层已展开（非 null）。
    let children = src["children"].as_array().unwrap();
    assert_eq!(children.len(), 2, "{v}");
    assert_eq!(children[0]["path"], "src/main.rs");
    assert_eq!(children[0]["type"], "file");

    // 忽略表同补全：node_modules/logs 永不出现。
    assert!(
        !names
            .iter()
            .any(|n| *n == "node_modules" || *n == "logs" || *n == "my file.txt"),
        "{v}"
    );
}

/// depth=1：所有子目录 children=null（边界未加载）——懒展开请求形态。
#[test]
fn tree_depth_one_marks_children_null() {
    let ws = setup_workspace();
    let v = tree(&ws, "", 1);
    let src = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["name"] == "src")
        .unwrap();
    assert!(src["children"].is_null(), "{v}");
}

/// 懒展开语义：对子目录再查 {path, depth:1} 得到该层文件。
#[test]
fn tree_lazy_expand_subdir() {
    let ws = setup_workspace();
    let v = tree(&ws, "src", 1);
    assert_eq!(v["path"], "src");
    let names: Vec<&str> = v["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["main.rs", "util.rs"], "{v}");
}

/// 越权/不存在诚实拒绝：`..` 逃逸、绝对路径（含盘符形态）、缺失目录。
#[test]
fn tree_rejects_traversal_and_missing() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ws_str = ws.path().to_str().unwrap();

    for bad in ["../outside", "src/../../..", ".."] {
        let err = h.tree_in(ws_str, bad, 1).unwrap_err();
        assert!(err.contains("越出 workspace"), "bad={bad} err={err}");
    }
    let err = h.tree_in(ws_str, "src/../..//etc", 1).unwrap_err();
    assert!(err.contains("越出 workspace"), "{err}");

    let err = h.tree_in(ws_str, "no_such_dir", 1).unwrap_err();
    assert!(err.contains("目录不存在"), "{err}");

    let err = h.tree_in("Z:/definitely/not/here", "", 1).unwrap_err();
    assert!(err.contains("workspace not found"), "{err}");
}

/// 条目上限 500（所有层级合计）+ truncated 诚实标记。
#[test]
fn tree_entries_cap_and_truncated() {
    let ws = TempDir::new().unwrap();
    let many = ws.path().join("many");
    std::fs::create_dir_all(&many).unwrap();
    for i in 0..MAX_TREE_ENTRIES + 10 {
        std::fs::write(many.join(format!("f{:04}.txt", i)), "x").unwrap();
    }
    let v = tree(&ws, "many", 1);
    assert_eq!(v["truncated"], true, "{v}");
    let entries = v["entries"].as_array().unwrap();
    assert!(entries.len() <= MAX_TREE_ENTRIES, "{}", entries.len());
}

/// handle_cmd 层：默认 depth=3；缺 data 同根查询。
#[tokio::test]
async fn tree_handle_cmd_defaults() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ctx = ctx_with_workspace(&ws);

    let out = h.handle_cmd("tree", None, &ctx).await.unwrap().unwrap();
    assert_eq!(out["path"], "");

    let out = h
        .handle_cmd(
            "tree",
            Some(serde_json::json!({ "path": "src", "depth": 1 })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["path"], "src");
    assert_eq!(out["entries"].as_array().unwrap().len(), 2);
}
