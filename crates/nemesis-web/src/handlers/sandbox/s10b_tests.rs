//! (quality-hardening goal 冲刺, web): sandbox handler coverage
//! for the box-FS selection paths and remaining field arms — all offline,
//! against a FAKE box tree (no Sandboxie, no UAC, no downloads):
//!
//! - `set_executor_config` (start/stop config flip helper — direct call; the
//!   `start`/`stop` commands that reach it are -exempt real-engine paths),
//! - `parse_selection` defaults + non-string filtering,
//! - `select_box_files` case-insensitive needle filter (187-196),
//! - `pending` / `commit` / `delete` against a fake box tree under
//!   `<home>/workspace/tools/sandboxie/box/NemesisBox`,
//! - `commit` happy + error arm (real target is a directory),
//! - `delete` removes only the in-box file (real path untouched),
//! - `set_config` allow_network field arm (513-515).

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
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
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
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

/// Build a fake in-box tree: `drive/C/<rel>` maps to `C:\<rel>` in the real
/// FS, so a commit of `drive/C/<tempdir-relative>` lands inside `dir`.
/// Returns (box_root, c_rel_prefix) where c_rel_prefix is `dir` relative to
/// the drive root (e.g. `Users/Zoo/AppData/Local/Temp/.tmpXXXX`) — or None on
/// non-C: temp dirs (commit-mapping test is then skipped, everything else
/// still runs).
fn fake_box(dir: &tempfile::TempDir) -> (PathBuf, Option<String>) {
    let home = dir.path();
    let paths = nemesis_sandbox::SandboxPaths::new(home);
    let box_root = paths.box_root.clone();
    let c_rel = home
        .to_string_lossy()
        .strip_prefix("C:\\")
        .map(|s| s.replace('\\', "/"));
    (box_root, c_rel)
}

#[tokio::test]
async fn set_executor_config_flips_both_switches_and_preserves_siblings() {
    if nemesis_config::global().is_some() {
        eprintln!("skip: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{ "executor": { "enabled": false, "sandbox": false, "allow_network": true } }"#,
    )
    .unwrap();
    set_executor_config(dir.path(), true, true).unwrap();
    let cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(cfg["executor"]["enabled"], true);
    assert_eq!(cfg["executor"]["sandbox"], true);
    assert_eq!(cfg["executor"]["allow_network"], true, "sibling preserved");
}

#[test]
fn parse_selection_defaults_and_filters_non_strings() {
    let (all, files) = parse_selection(&serde_json::json!({}));
    assert!(!all && files.is_empty());

    let (all, files) = parse_selection(&serde_json::json!({ "all": true }));
    assert!(all && files.is_empty());

    let (all, files) = parse_selection(&serde_json::json!({ "files": ["a.txt", 42, "b.txt"] }));
    assert!(!all);
    assert_eq!(files, vec!["a.txt".to_string(), "b.txt".to_string()]);
}

#[tokio::test]
async fn select_box_files_needle_filter_is_case_insensitive_substring() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    // BUG-20261123-039 后 select_box_files 走 pending_workspace（工作区收容），
    // 选中集 = 工作区子树镜像；工作区外条目（下方 evil）必须被排除。
    let a = box_root
        .join("drive/C")
        .join(format!("{c_rel}/workspace/safe/alpha/REPORT.txt"));
    let b = box_root
        .join("drive/C")
        .join(format!("{c_rel}/workspace/safe/beta/other.log"));
    let evil = box_root
        .join("drive/C")
        .join(format!("{c_rel}/AppData/evil.txt"));
    for p in [
        a.parent().unwrap(),
        b.parent().unwrap(),
        evil.parent().unwrap(),
    ] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(&a, b"a").unwrap();
    std::fs::write(&b, b"b").unwrap();
    std::fs::write(&evil, b"e").unwrap();

    let paths = nemesis_sandbox::SandboxPaths::new(dir.path());
    // No match.
    let sel = select_box_files(&paths, false, &["zzz-nope".to_string()]).unwrap();
    assert!(sel.is_empty());
    // Case-insensitive substring match.
    let sel = select_box_files(&paths, false, &["report.TXT".to_string()]).unwrap();
    assert_eq!(sel.len(), 1);
    assert!(sel[0].real_path.ends_with("REPORT.txt"));
    // all=true = 工作区子树全部（盒外 evil 不入集——收容 backstop）。
    let sel = select_box_files(&paths, true, &[]).unwrap();
    assert_eq!(sel.len(), 2, "workspace-subtree entries only");
    assert!(
        sel.iter()
            .all(|p| !p.real_path.to_string_lossy().contains("evil")),
        "out-of-workspace box write must never be selectable"
    );
}

#[tokio::test]
async fn pending_command_lists_box_files() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    // 工作区子树镜像（BUG-20261123-039 后清单只含工作区子树）。
    let f = box_root
        .join("drive/C")
        .join(format!("{c_rel}/workspace/one.txt"));
    std::fs::create_dir_all(f.parent().unwrap()).unwrap();
    std::fs::write(&f, b"hello").unwrap();
    // 盒外镜像（工作区外真实路径）不得入清单。
    let evil = box_root
        .join("drive/C")
        .join(format!("{c_rel}/outside/x.txt"));
    std::fs::create_dir_all(evil.parent().unwrap()).unwrap();
    std::fs::write(&evil, b"x").unwrap();
    // Unmapped box file (not user/ or drive/) must be skipped.
    let meta = box_root.join("RegHive");
    std::fs::write(&meta, b"m").unwrap();

    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd("pending", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let files = out["files"].as_array().unwrap();
    assert_eq!(
        files.len(),
        1,
        "unmapped RegHive and out-of-workspace mirror must not be listed: {files:?}"
    );
    assert_eq!(files[0]["size"], serde_json::json!(5));
    assert!(files[0]["real_path"].as_str().unwrap().ends_with("one.txt"));
}

#[tokio::test]
async fn commit_copies_selected_files_into_real_paths() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    // Box file mapping to a fresh path inside this temp workspace
    // （BUG-20261123-039 后提交作用域 = 工作区子树）。
    let target_rel = format!("{c_rel}/workspace/committed/hello.txt");
    let box_file = box_root.join("drive/C").join(&target_rel);
    std::fs::create_dir_all(box_file.parent().unwrap()).unwrap();
    std::fs::write(&box_file, b"payload").unwrap();

    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd(
            "commit",
            // Real paths use OS separators, so match on the file name only.
            Some(serde_json::json!({ "files": ["hello.txt"] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["committed"], serde_json::json!(1));
    assert_eq!(out["total"], serde_json::json!(1));
    let real = PathBuf::from("C:\\").join(target_rel.replace('/', "\\"));
    assert_eq!(
        std::fs::read(&real).unwrap(),
        b"payload",
        "real file must receive the box copy"
    );
}

#[tokio::test]
async fn commit_reports_error_when_real_target_is_a_directory() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    // Real target already exists as a DIRECTORY → fs::copy fails.
    let target_rel = format!("{c_rel}/workspace/target-dir");
    std::fs::create_dir_all(PathBuf::from("C:\\").join(target_rel.replace('/', "\\"))).unwrap();
    let box_file = box_root.join("drive/C").join(&target_rel);
    std::fs::create_dir_all(box_file.parent().unwrap()).unwrap();
    std::fs::write(&box_file, b"x").unwrap();

    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd("commit", Some(serde_json::json!({ "all": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["committed"], serde_json::json!(0));
    assert_eq!(out["total"], serde_json::json!(1));
    assert!(!out["errors"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn delete_removes_in_box_file_only() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    // Seed both sides: the box file AND the already-committed real file
    // （BUG-20261123-039 后删除作用域 = 工作区子树）。
    let target_rel = format!("{c_rel}/workspace/keepme.txt");
    let real = PathBuf::from("C:\\").join(target_rel.replace('/', "\\"));
    std::fs::create_dir_all(real.parent().unwrap()).unwrap();
    std::fs::write(&real, b"real-content").unwrap();
    let box_file = box_root.join("drive/C").join(&target_rel);
    std::fs::create_dir_all(box_file.parent().unwrap()).unwrap();
    std::fs::write(&box_file, b"box-content").unwrap();

    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd("delete", Some(serde_json::json!({ "all": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["deleted"], serde_json::json!(1));
    assert!(!box_file.exists(), "box file removed");
    assert!(real.exists(), "real file untouched");
    assert_eq!(std::fs::read(&real).unwrap(), b"real-content");
}

/// BUG-20261123-039 回归锁：WSAPI `commit {all:true}` 的提交作用域必须
/// 收容在工作区子树——修复前 select_box_files 走无过滤 enumerate_box，
/// 盒内对工作区外的写入（exec 损害镜像）会被提交回真盘物化（CLI 侧
/// pending_workspace 收容 backstop 在 WSAPI 面分叉失效）。修复后：盒外
/// 条目不进选择集，真实路径永不被创建。
#[tokio::test]
async fn commit_all_never_materializes_out_of_workspace_box_writes() {
    let dir = tempfile::tempdir().unwrap();
    let (box_root, Some(c_rel)) = fake_box(&dir) else {
        eprintln!("skip: tempdir not on C:, drive/C mapping unavailable");
        return;
    };
    let ws_file = box_root
        .join("drive/C")
        .join(format!("{c_rel}/workspace/out.txt"));
    let evil_box = box_root
        .join("drive/C")
        .join(format!("{c_rel}/AppData/Roaming/evil.dll"));
    for p in [ws_file.parent().unwrap(), evil_box.parent().unwrap()] {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(&ws_file, b"w").unwrap();
    std::fs::write(&evil_box, b"payload").unwrap();
    let evil_real = PathBuf::from("C:\\").join(format!("{c_rel}\\AppData\\Roaming\\evil.dll"));

    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd("commit", Some(serde_json::json!({ "all": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        out["committed"],
        serde_json::json!(1),
        "only the workspace write commits"
    );
    assert_eq!(out["total"], serde_json::json!(1));
    assert!(
        !evil_real.exists(),
        "盒外损害镜像绝不能被提交物化到真盘: {}",
        evil_real.display()
    );
    let ws_real = dir.path().join("workspace").join("out.txt");
    assert_eq!(std::fs::read(&ws_real).unwrap(), b"w");
    // 收尾：清掉本测试在真实盘 %TEMP% 下的物化物（工作区内文件在 tempdir，
    // 随 tempdir 回收；evil 真实路径修复后不产生，防御性清理不删父目录）。
    let _ = std::fs::remove_file(&evil_real);
}

#[tokio::test]
async fn set_config_allow_network_field_round_trips() {
    if nemesis_config::global().is_some() {
        eprintln!("skip: process-global ConfigStore installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        r#"{ "executor": { "enabled": true, "sandbox": false, "strict": true } }"#,
    )
    .unwrap();
    let ctx = make_ctx(&dir);
    let out = SandboxHandler::new()
        .handle_cmd(
            "set_config",
            Some(serde_json::json!({ "allow_network": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["executor"]["allow_network"], serde_json::json!(true));
    assert_eq!(
        out["executor"]["enabled"],
        serde_json::json!(true),
        "sibling kept"
    );
    assert_eq!(
        out["executor"]["sandbox"],
        serde_json::json!(false),
        "sibling kept"
    );
    assert_eq!(
        out["executor"]["strict"],
        serde_json::json!(true),
        "sibling kept"
    );
}
