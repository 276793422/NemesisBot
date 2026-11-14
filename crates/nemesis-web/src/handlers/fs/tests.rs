use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::projects::{
    BRIDGE_TEST_LOCK, ProjectInfo, ProjectsBridge, set_projects_bridge_for_test,
};
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
    // 共享忽略表里的运行时目录（补全必须不可）。
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

// handle_cmd 层（RequestContext 直构；build_state 形态同 memory/s10b_tests）

fn build_state(ws: &str) -> AppState {
    AppState {
        cors_manager: None,
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
        quic_relay: None,
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
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
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

// fs.tree 目录树（懒展开 / 忽略表 / 500 上限 / 越权拒绝）

fn tree(ws: &TempDir, path: &str, depth: u64) -> serde_json::Value {
    let h = FsHandler::new();
    h.tree_in(ws.path().to_str().unwrap(), path, depth as usize)
        .unwrap()
}

/// 根层列出：忽略表同补全（node_modules/logs 不可）、目录在前文件在后、
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

/// 越权/不存在诚实拒绝：`..` 逃逸、绝对路径（含盘符形态——：join 遇
/// 绝对路径替换整个 root，显式拒绝）、缺失目录。
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

/// 回归锁：绝对路径（含 Windows 盘符形态）显式拒绝——
/// `PathBuf::join` 遇绝对路径替换整个 root，此前只拒 `..` 时
/// `fs.tree {path: "C:/Windows"}` 可列举任意目录。盘符形态是 Windows
/// 语义（Unix 上归一化剥前导 `/` 后无绝对形态存活，闸不可达）。
#[test]
fn tree_rejects_absolute_paths() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ws_str = ws.path().to_str().unwrap();

    if cfg!(windows) {
        for bad in ["C:/Windows", "C:\\Windows", "c:/windows/system32"] {
            let err = h.tree_in(ws_str, bad, 1).unwrap_err();
            assert!(err.contains("越出 workspace"), "bad={bad} err={err}");
        }
    } else {
        // Unix 同形态输入按相对路径处理（诚实报目录不存在，不越界）。
        let err = h.tree_in(ws_str, "C:/Windows", 1).unwrap_err();
        assert!(err.contains("目录不存在"), "{err}");
    }

    // 相对合法输入不受纵深闸影响（join 恒在 root 下）。
    let v = h.tree_in(ws_str, "src", 1).unwrap();
    assert_eq!(v["path"], "src");
    assert_eq!(v["entries"].as_array().unwrap().len(), 2);
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

// fs.open：聊天文件链接打开（决策纯函数面——spawn 臂不进单测）

#[test]
fn openable_extension_whitelist() {
    fn mk(name: &str) -> &Path {
        Path::new(name)
    }
    assert!(is_openable_extension(mk("a.md")));
    assert!(is_openable_extension(mk("REPORT.MD"))); // 大小写不敏感
    assert!(is_openable_extension(mk("x/y/data.json")));
    assert!(is_openable_extension(mk("shot.PNG")));
    for deny in ["evil.bat", "run.cmd", "x.exe", "s.ps1", "m.msi", "noext"] {
        assert!(!is_openable_extension(mk(deny)), "{deny}");
    }
}

/// 包含判定边界：`C:\foo2` 不得命中 `C:\foo`；分隔符两态等价。
#[test]
fn path_contains_boundary() {
    if cfg!(windows) {
        let foo = Path::new("C:/work/foo");
        assert!(path_contains(foo, Path::new("C:/work/foo")));
        assert!(path_contains(foo, Path::new("C:/work/foo/bar/a.md")));
        assert!(path_contains(
            Path::new("C:\\work\\foo"),
            Path::new("c:/WORK/foo/bar/a.md")
        ));
        assert!(!path_contains(foo, Path::new("C:/work/foo2/a.md")));
        assert!(!path_contains(foo, Path::new("C:/work/fo")));
    } else {
        let foo = Path::new("/work/foo");
        assert!(path_contains(foo, Path::new("/work/foo/bar/a.md")));
        assert!(!path_contains(foo, Path::new("/work/foo2/a.md")));
    }
}

/// 回归锁：target 含多字节字符且字节总长超过 root 时，
/// 旧实现 `t[..r.len()]` 字节切片会切在多字节字符中间 → str 切片 panic
/// （`fs.open` 带模型输出的 CJK 绝对路径即可触发，WS handler 任务崩）。
/// 修复 = `str::get(..r.len())` 边界安全切片，非边界诚实返回 false。
#[test]
fn path_contains_multibyte_target_no_panic() {
    if cfg!(windows) {
        // root 5 字节；target "C:/中" 6 字节（C/: 3 + 中 3）——t[..5]
        // 落在中文字符第二字节 → 修复前 panic，修复后 false。
        let root = Path::new("C:/ws");
        assert!(!path_contains(root, Path::new("C:/中")));
        // 边界对齐形态（root_len 恰是 target 字符起点）：前缀不等 → false，
        // 不 panic（修复前后都应 false，钉住判定语义本身）。
        assert!(!path_contains(root, Path::new("C:/ab中")));
        // 前缀相等且后续多字节：正常 containment 命中。
        assert!(path_contains(
            Path::new("C:/ws"),
            Path::new("C:/ws/中文/文档.md")
        ));
        // root 自身含多字节（用户名形态）+ target 前缀不等：同样不 panic。
        let root_cn = Path::new("C:/usrs/张三");
        assert!(!path_contains(root_cn, Path::new("C:/usrs/李四/文件.md")));
    } else {
        // Unix 臂 starts_with 天然边界安全：语义同验。
        let root = Path::new("/ws");
        assert!(!path_contains(root, Path::new("/ws2/中")));
        assert!(path_contains(root, Path::new("/ws/中文.md")));
    }
}

fn resolve(ws: &TempDir, raw: &str) -> Result<ResolvedTarget, String> {
    resolve_click_target(
        ws.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        raw,
    )
}

/// 相对路径（含裸文件名）：拼 workspace root → 存在判定 + 类型。
/// Windows 臂同时钉分隔符统一（解析产物不得保留 join 的 `/` 混合形态）。
#[test]
fn open_resolve_relative_hits_workspace_file_and_dir() {
    let ws = setup_workspace();
    let t = resolve(&ws, "alpha.txt").unwrap();
    assert!(!t.is_dir);
    assert!(t.exists);
    assert_eq!(t.path, ws.path().join("alpha.txt"));

    std::fs::create_dir_all(ws.path().join("reports")).unwrap();
    let t = resolve(&ws, "reports").unwrap();
    assert!(t.is_dir);
    assert!(t.exists);
    assert_eq!(t.path, ws.path().join("reports"));

    let t = resolve(&ws, "src/main.rs").unwrap();
    assert_eq!(t.path, ws.path().join("src").join("main.rs"));

    // 反斜杠形态等价。
    let t = resolve(&ws, "src\\main.rs").unwrap();
    assert_eq!(t.path, ws.path().join("src").join("main.rs"));
    // 分隔符统一（2026-11-14 根修）：混合分隔符让 explorer /select 不稳，
    // 解析产物在 Windows 上必须是纯 `\`。
    if cfg!(windows) {
        assert!(
            !t.path.to_string_lossy().contains('/'),
            "解析产物仍有 `/`: {}",
            t.path.display()
        );
    }
}

/// `..` 段 / 空路径 → 诚实报错；不存在的文件 → **祖先回退**（2026-11-14
/// 新语义：返回最近存在祖先目录 exists=false，前端 toast 注记「文件未
/// 找到」——不再 Err 死链接）。
#[test]
fn open_resolve_rejects_escape_and_missing() {
    let ws = setup_workspace();
    for bad in ["../outside", "src/../../x", ""] {
        let err = resolve(&ws, bad).unwrap_err();
        assert!(
            err.contains("越出 workspace") || err.contains("path 缺失"),
            "bad={bad:?} err={err}"
        );
    }
    let t = resolve(&ws, "no_such.md").unwrap();
    assert!(!t.exists);
    assert!(t.is_dir);
    assert_eq!(t.path, ws.path());
}

/// 祖先回退剥到最近**存在的**目录层（不是一律回根）：`src/no_such.md`
/// → src（存在）；`missing_dir/deeper/x.md` → ws 根（中间层全缺失）。
#[test]
fn open_resolve_ancestor_is_deepest_existing_dir() {
    let ws = setup_workspace();
    let t = resolve(&ws, "src/no_such.md").unwrap();
    assert!(!t.exists);
    assert!(t.is_dir);
    assert_eq!(t.path, ws.path().join("src"));

    let t = resolve(&ws, "missing_dir/deeper/x.md").unwrap();
    assert!(!t.exists);
    assert_eq!(t.path, ws.path());
}

/// 绝对路径：workspace 内放行；集合外（系统目录）拒绝；集合内缺失 →
/// 祖先回退（绝不越界）。
#[test]
fn open_resolve_absolute_containment() {
    let ws = setup_workspace();
    let inside = ws.path().join("alpha.txt");
    let t = resolve(&ws, &inside.to_string_lossy()).unwrap();
    assert!(!t.is_dir);
    assert!(t.exists);
    assert_eq!(t.path, inside);

    let sys_dir = if cfg!(windows) { "C:/Windows" } else { "/etc" };
    let err = resolve(&ws, sys_dir).unwrap_err();
    assert!(err.contains("越出 workspace"), "sys={sys_dir} err={err}");

    // 集合内绝对缺失 → 祖先回退到 ws 根。
    let missing_abs = ws.path().join("no_such.md");
    let t = resolve(&ws, &missing_abs.to_string_lossy()).unwrap();
    assert!(!t.exists);
    assert_eq!(t.path, ws.path());
}

/// main_workspace 与 session_root 分工（T4 形态，2026-11-14 用户实测
/// 失败场景回归锁）：session_root 指向项目目录时，相对路径先拼项目
/// 目录；**项目根未命中再试主 workspace**（锚链第二根）——项目会话里
/// 点主 workspace 文件此前必败，现在命中。
#[test]
fn open_resolve_session_root_vs_main_workspace() {
    let ws = setup_workspace();
    let proj = TempDir::new().unwrap();
    std::fs::write(proj.path().join("note.md"), "n").unwrap();

    // 相对路径拼项目目录（项目文件命中）。
    let t = resolve_click_target(
        proj.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        "note.md",
    )
    .unwrap();
    assert_eq!(t.path, proj.path().join("note.md"));
    assert!(t.exists);

    // 锚链跨根：项目根没有的文件回落主 workspace 命中（B1 根修回归锁）。
    let t = resolve_click_target(
        proj.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        "beta.rs",
    )
    .unwrap();
    assert!(t.exists);
    assert_eq!(t.path, ws.path().join("beta.rs"));

    // 锚链顺序：两根都有同名文件 → session root（项目目录）优先。
    std::fs::write(proj.path().join("alpha.txt"), "proj-copy").unwrap();
    let t = resolve_click_target(
        proj.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        "alpha.txt",
    )
    .unwrap();
    assert_eq!(t.path, proj.path().join("alpha.txt"));

    // 主 workspace 内的文件经绝对路径仍可开（containment 集合含主 ws）。
    let inside_main = ws.path().join("alpha.txt");
    let t = resolve_click_target(
        proj.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        &inside_main.to_string_lossy(),
    )
    .unwrap();
    assert_eq!(t.path, inside_main);
}

/// 锚链允许根的 bridge 集成（T3 形态，2026-11-14 用户实测失败场景回归
/// 锁）：主会话（session root == 主 workspace）点**已注册项目目录**内的
/// 相对路径文件——此前单根必败，现在锚链第二根命中。
#[test]
fn open_resolve_anchor_chain_reaches_registered_project() {
    let _serial = BRIDGE_TEST_LOCK.lock();
    let ws = setup_workspace();
    let proj = TempDir::new().unwrap();
    std::fs::write(proj.path().join("note.md"), "n").unwrap();

    struct ProbeBridge(PathBuf);
    impl ProjectsBridge for ProbeBridge {
        fn list(&self) -> Vec<ProjectInfo> {
            vec![ProjectInfo {
                id: "p1".to_string(),
                name: "proj".to_string(),
                path: self.0.to_string_lossy().to_string(),
                created_at: "t".to_string(),
                running: false,
                instructions_trusted: true,
            }]
        }
        fn create(&self, _n: &str, _p: &str) -> Result<ProjectInfo, String> {
            Err("noop".to_string())
        }
        fn remove(&self, _id: &str) -> Result<ProjectInfo, String> {
            Err("noop".to_string())
        }
        fn rename(&self, _id: &str, _n: &str) -> Result<ProjectInfo, String> {
            Err("noop".to_string())
        }
        fn set_trust(&self, _id: &str, _t: bool) -> Result<ProjectInfo, String> {
            Err("noop".to_string())
        }
        fn owner_of(&self, _s: &str) -> Option<String> {
            None
        }
        fn loop_for_session(&self, _id: &str) -> Option<Arc<nemesis_agent::r#loop::AgentLoop>> {
            None
        }
        fn project_path(&self, _id: &str) -> Option<PathBuf> {
            None
        }
        fn bind_session(&self, _s: &str, _id: &str) -> Result<(), String> {
            Err("noop".to_string())
        }
        fn forget_session(&self, _s: &str) {}
    }
    set_projects_bridge_for_test(Some(Arc::new(ProbeBridge(proj.path().to_path_buf()))));
    let result = resolve_click_target(
        ws.path().to_str().unwrap(),
        ws.path().to_str().unwrap(),
        "note.md",
    );
    // 断言前先摘 bridge（进程级共享槽——即使 assert 失败也不污染兄弟测试）。
    set_projects_bridge_for_test(None);
    let t = result.unwrap();
    assert!(t.exists);
    assert!(!t.is_dir);
    assert_eq!(t.path, proj.path().join("note.md"));
}

/// handle_cmd 层错误路径（不触发 spawn）：缺 path / 越界。
#[tokio::test]
async fn open_handle_cmd_error_paths() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ctx = ctx_with_workspace(&ws);

    let err = h
        .handle_cmd("open", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("path 缺失"), "{err}");

    let sys_dir = if cfg!(windows) { "C:/Windows" } else { "/etc" };
    let err = h
        .handle_cmd("open", Some(serde_json::json!({ "path": sys_dir })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("越出 workspace"), "{err}");
}

// fs.reveal：右键「打开所在目录」（错误面不 spawn；成功臂调系统文件
// 管理器，不进单测——open 同纪律。解析/containment/祖先回退语义与 open
// 全同源，已由 resolve_click_target 测试组覆盖，此处只钉 handle_cmd
// 错误路径 + 命令清单在册。**祖先回退后 reveal 对缺失文件不再报错**
// （开最近存在祖先目录 = spawn 臂，结构性豁免不进单测）。

#[tokio::test]
async fn reveal_handle_cmd_error_paths() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ctx = ctx_with_workspace(&ws);

    let err = h
        .handle_cmd("reveal", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("path 缺失"), "{err}");

    let sys_dir = if cfg!(windows) { "C:/Windows" } else { "/etc" };
    let err = h
        .handle_cmd("reveal", Some(serde_json::json!({ "path": sys_dir })), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("越出 workspace"), "{err}");
}

#[test]
fn module_commands_include_reveal_and_resolve() {
    let cmds = FsHandler::new().commands();
    assert!(cmds.contains(&"reveal"));
    assert!(cmds.contains(&"resolve"));
}

// fs.resolve：纯决策面契约（不 spawn——复制路径等只需要「绝对路径 +
// 存在性」；同一解析阶梯，open/reveal 全同源）。

#[tokio::test]
async fn resolve_handle_cmd_contract() {
    let ws = setup_workspace();
    let h = FsHandler::new();
    let ctx = ctx_with_workspace(&ws);

    // 存在文件：绝对路径 + 类型 + 存在性。
    let out = h
        .handle_cmd(
            "resolve",
            Some(serde_json::json!({ "path": "alpha.txt" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out["path"],
        ws.path().join("alpha.txt").to_string_lossy().to_string()
    );
    assert_eq!(out["kind"], "file");
    assert_eq!(out["exists"], true);

    // 缺失文件：祖先回退产物（dir + exists=false）——前端复制路径 toast
    // 注记「文件未找到」的数据面。
    let out = h
        .handle_cmd(
            "resolve",
            Some(serde_json::json!({ "path": "no_such.md" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["path"], ws.path().to_string_lossy().to_string());
    assert_eq!(out["kind"], "dir");
    assert_eq!(out["exists"], false);

    // 越界照常诚实报错（不回退不 spawn）。
    let sys_dir = if cfg!(windows) { "C:/Windows" } else { "/etc" };
    let err = h
        .handle_cmd(
            "resolve",
            Some(serde_json::json!({ "path": sys_dir })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("越出 workspace"), "{err}");
}

// 存在性回退（用户设计：CJK 引导语粘连剥前缀找真实文件）----

/// 相对路径头粘连：「见docs/总结.md」→ 剥引导语 → docs/总结.md 命中。
#[test]
fn open_resolve_fallback_strips_cjk_prefix_relative() {
    let ws = setup_workspace();
    std::fs::create_dir_all(ws.path().join("docs")).unwrap();
    std::fs::write(ws.path().join("docs/总结.md"), "# s").unwrap();

    let t = resolve(&ws, "见docs/总结.md").unwrap();
    assert!(!t.is_dir);
    assert!(t.exists);
    assert_eq!(t.path, ws.path().join("docs").join("总结.md"));
}

/// 绝对路径头粘连：「文件在<abs>」→ 剥引导语 → 绝对路径命中（containment 照过）。
#[test]
fn open_resolve_fallback_strips_cjk_prefix_absolute() {
    let ws = setup_workspace();
    let abs = ws.path().join("alpha.txt");
    let raw = format!("文件在{}", abs.to_string_lossy());

    let t = resolve(&ws, &raw).unwrap();
    assert!(!t.is_dir);
    assert!(t.exists);
    assert_eq!(t.path, abs);
}

/// 回退候选不放松任何闸：候选含 `..` 段被 normalize 闸拒绝（不进解析
/// 链），工作区外的真实文件即使存在也**绝不被打开**。祖先回退产物落在
/// inner 根（最近存在祖先，仍在工作区内）——安全命门回归锁。
#[test]
fn open_resolve_fallback_skips_dotdot_candidates() {
    let outer = TempDir::new().unwrap();
    let inner = outer.path().join("ws");
    std::fs::create_dir_all(&inner).unwrap();
    std::fs::write(outer.path().join("sibling.md"), "outside").unwrap();

    let t = resolve_click_target(
        inner.to_str().unwrap(),
        inner.to_str().unwrap(),
        "见../sibling.md",
    )
    .unwrap();
    // 候选「../sibling.md」被.. 闸跳过 → 无命中 → 祖先回退 = inner 根
    // （若候选闸失效，这里会打开 outer/sibling.md = 逃逸）。
    assert!(!t.exists);
    assert!(t.is_dir);
    assert_eq!(t.path, inner);
    assert_ne!(t.path, outer.path().join("sibling.md"));
}

/// 纯 ASCII 不存在路径无 CJK 候选：直接落祖先回退（ws 根，exists=false）。
#[test]
fn open_resolve_ascii_missing_falls_back_to_ancestor() {
    let ws = setup_workspace();
    let t = resolve(&ws, "no_such.md").unwrap();
    assert!(!t.exists);
    assert!(t.is_dir);
    assert_eq!(t.path, ws.path());
}

/// 候选生成纯函数：边界扫描口径（CJK→ASCII 词字符/分隔符才切；
/// 全角标点不切；同候选去重）。
#[test]
fn cjk_boundary_candidates_scan() {
    // 引导语粘连：单边界 → 单候选。
    assert_eq!(cjk_boundary_candidates("见docs/总结.md"), ["docs/总结.md"]);
    // 绝对路径盘符边界。
    assert_eq!(cjk_boundary_candidates("文件在C:/ws/a.md"), ["C:/ws/a.md"]);
    // 无边界 → 空。
    assert!(cjk_boundary_candidates("plain/path.md").is_empty());
    // 全角冒号后不切。
    assert!(cjk_boundary_candidates("路径：docs/a.md").is_empty());
    // CJK 文件名后跟扩展名点：切出的 ".md" 碎片被过滤，不产生垃圾候选。
    assert_eq!(cjk_boundary_candidates("见docs/总结.md"), ["docs/总结.md"]);
    assert!(cjk_boundary_candidates("报告总结.md").is_empty());
}
