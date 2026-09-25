//! Wave6B 覆盖率补充批次（loop_tools.rs 剩余可测行）：
//! - `checks_commands` node/go/python/maven 未覆盖格 + filter trim/空白
//! - 三个 `Default` 实现（McpDiscover/CliReference/HistorySearch）
//! - trait getter（is_read_only / is_parallel_safe / limit_categories）
//! - MessageTool set_context + 空 RequestContext 回退臂
//! - apply_edit_to_content 级联 Ambiguous / span_note 两臂
//! - Windows 文件锁错误注入（delete_file / create_dir / delete_dir）+ exec cwd
//! - RunChecks 非法 JSON / filter+timeout 提取 / 无生态早退
//! - CronTool create 空 ctx 回退臂 / TodoWrite 三错误臂 / peer_chat_callback
//! - ClusterRpcTool 空 ctx 回退 / SpawnTool 无槽早退 / memory 四工具非法 JSON
//! - SkillManage 审批 Err / JoinError / patch 安全拦截 / write_file 父目录是文件
//! - GitTool commit spawn 失败 / mcp_discover args 数组混合类型
//! - register_shared_tools：spawn_slot 共享槽 / claude+codex 探针 / lsp 双
//!   manager 形态 / workflow 三件套 / 收尾 info!（capture_logs 罩住参数行）
//! - WorkflowCreate 决策表 + WorkflowCapabilitiesTool

use super::*;
use crate::context::RequestContext as _Ctx;
use crate::test_support::capture_logs;

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat-covw6", "user-covw6", "agent:covw6/session")
}

/// 空 channel/chat_id 的 ctx——触发 stored 回退臂。
fn empty_ctx() -> RequestContext {
    _Ctx::new("", "", "u", "s")
}

// ---------------------------------------------------------------------------
// checks_commands 全生态矩阵
// ---------------------------------------------------------------------------

#[test]
fn checks_commands_node_matrix() {
    let b = checks_commands("node", "build", None).unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].1, "npm run build");
    let t = checks_commands("node", "test", None).unwrap();
    assert_eq!(t[0].1, "npm test");
    let l = checks_commands("node", "lint", None).unwrap();
    assert_eq!(l[0].1, "npm run lint");
    let all = checks_commands("node", "all", None).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].0, "build");
    assert_eq!(all[2].0, "lint");
}

#[test]
fn checks_commands_go_matrix_with_filter() {
    assert_eq!(
        checks_commands("go", "build", None).unwrap()[0].1,
        "go build ./..."
    );
    assert_eq!(
        checks_commands("go", "test", Some("cov_go_case")).unwrap()[0].1,
        "go test -run cov_go_case ./..."
    );
    assert_eq!(
        checks_commands("go", "test", None).unwrap()[0].1,
        "go test ./..."
    );
    assert_eq!(
        checks_commands("go", "lint", None).unwrap()[0].1,
        "go vet ./..."
    );
    let all = checks_commands("go", "all", None).unwrap();
    assert_eq!(all.len(), 3);
    assert_eq!(all[1].1, "go test ./...");
}

#[test]
fn checks_commands_python_and_maven() {
    assert_eq!(
        checks_commands("python", "test", Some("cov_py")).unwrap()[0].1,
        "pytest -q -k cov_py"
    );
    assert_eq!(
        checks_commands("python", "test", None).unwrap()[0].1,
        "pytest -q"
    );
    assert_eq!(
        checks_commands("python", "lint", None).unwrap()[0].1,
        "ruff check ."
    );
    // python 无 build、maven 无 lint——诚实不猜。
    assert!(checks_commands("python", "build", None).is_none());
    assert!(checks_commands("maven", "lint", None).is_none());
    assert_eq!(
        checks_commands("maven", "build", None).unwrap()[0].1,
        "mvn -q compile"
    );
    assert_eq!(
        checks_commands("maven", "test", Some("cov_mv")).unwrap()[0].1,
        "mvn -q test -Dtest=cov_mv"
    );
}

#[test]
fn checks_commands_filter_trim_and_whitespace() {
    // 前后空白被 trim。
    assert_eq!(
        checks_commands("rust", "test", Some("  my_cov_case  ")).unwrap()[0].1,
        "cargo test my_cov_case"
    );
    // 纯空白 filter 等价 None。
    assert_eq!(
        checks_commands("rust", "test", Some("   ")).unwrap()[0].1,
        "cargo test"
    );
    // 未知生态 → None。
    assert!(checks_commands("ruby", "build", None).is_none());
}

// ---------------------------------------------------------------------------
// Default 实现 + trait getter
// ---------------------------------------------------------------------------

#[test]
// 本测试的存在意义就是执行三个 unit struct 的 Default impl 体（5888-5890 /
// 5987-5989 / 6037-6039 是产品覆盖行）；clippy 建议的字面量写法会绕过
// default() 让这些行失覆盖——按目的豁免。
#[allow(clippy::default_constructed_unit_structs)]
fn default_impls_for_stateless_tools() {
    let _ = McpDiscoverTool::default();
    let _ = CliReferenceTool::default();
    let _ = HistorySearchTool::default();
}

#[test]
fn trait_getter_arms() {
    use super::WebSearchConfig;
    let ws = WebSearchTool::new(WebSearchConfig::default());
    assert!(Tool::is_read_only(&ws));

    let wf = WebFetchTool::new(50000);
    assert_eq!(wf.limit_categories(), &["web_fetch"]);
    assert!(Tool::is_read_only(&wf));

    let spawn = SpawnTool::new(SpawnConfig {
        default_model: "stub".into(),
        max_concurrent: 2,
        max_depth: 1,
    });
    assert!(Tool::is_parallel_safe(&spawn));
}

// ---------------------------------------------------------------------------
// MessageTool：set_context + 空 ctx 回退
// ---------------------------------------------------------------------------

#[tokio::test]
async fn message_tool_set_context_and_stored_fallback() {
    let tool = MessageTool::new();
    tool.set_context("covw6_ch", "covw6_chat");
    // 空 ctx channel/chat_id → 走 stored 回退（152/160 臂）；无 callback →
    // passthrough 返回 content。
    let out = tool
        .execute(r#"{"content":"hello covw6"}"#, &empty_ctx())
        .await
        .unwrap();
    assert_eq!(out, "hello covw6");
}

// ---------------------------------------------------------------------------
// apply_edit_to_content：级联 Ambiguous / span_note
// ---------------------------------------------------------------------------

#[test]
fn apply_edit_cascade_ambiguous_arm() {
    // exact 不命中（行前有两空格），line-trimmed 级两处命中 → Ambiguous。
    let res = apply_edit_to_content("  x\n  y\n  x\n  y\n", "x\ny", "z", false, "cov.txt");
    let err = res.err().unwrap();
    assert!(err.contains("unambiguous"), "got: {err}");
}

#[test]
fn apply_edit_cascade_span_note_arm() {
    // 全部级未命中，但 whitespace-normalized 有一个超跨度候选 →
    // NoMatch{span_note: Some} → 错误文案带 disproportionate 注记。
    let content = format!("hello{}world\n", " ".repeat(3000));
    let res = apply_edit_to_content(&content, "hello world", "X", false, "cov2.txt");
    let err = res.err().unwrap();
    assert!(err.contains("disproportionate"), "got: {err}");
    assert!(err.contains("old_text not found in cov2.txt"), "got: {err}");
}

#[test]
fn apply_edit_cascade_no_match_without_note() {
    let res = apply_edit_to_content("alpha\nbeta\n", "zzz", "X", false, "cov3.txt");
    let err = res.err().unwrap();
    assert!(err.contains("old_text not found in cov3.txt"));
    assert!(!err.contains("disproportionate"));
}

// ---------------------------------------------------------------------------
// Windows 文件锁错误注入 + exec cwd
// ---------------------------------------------------------------------------

/// 以 FILE_SHARE_READ-only 打开句柄：阻塞 rename/remove/write/delete。
#[cfg(windows)]
fn lock_shared_read_only(path: &Path) -> std::fs::File {
    use std::os::windows::fs::OpenOptionsExt;
    std::fs::File::open(path).unwrap();
    std::fs::OpenOptions::new()
        .read(true)
        .share_mode(1) // FILE_SHARE_READ only
        .open(path)
        .unwrap()
}

#[cfg(windows)]
#[tokio::test]
async fn delete_file_reports_remove_failure_when_handle_open() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("locked.txt");
    std::fs::write(&f, "x").unwrap();
    // FILE_SHARE_READ-only 句柄（无 FILE_SHARE_DELETE）→ remove_file 拒绝。
    let _handle = lock_shared_read_only(&f);
    let tool = DeleteFileTool::default();
    let err = tool
        .execute(&format!(r#"{{"path":{:?}}}"#, f), &ctx())
        .await
        .err()
        .unwrap();
    assert!(err.contains("Failed to delete file"), "got: {err}");
}

#[tokio::test]
async fn create_dir_reports_failure_when_parent_is_file() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, "f").unwrap();
    let tool = CreateDirTool::default();
    let err = tool
        .execute(
            &format!(r#"{{"path":{:?}}}"#, blocker.join("child")),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("Failed to create directory"), "got: {err}");
}

#[cfg(windows)]
#[tokio::test]
async fn delete_dir_reports_failure_when_file_inside_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("doomed");
    std::fs::create_dir_all(&dir).unwrap();
    let inner = dir.join("pin.txt");
    std::fs::write(&inner, "x").unwrap();
    let _handle = lock_shared_read_only(&inner);
    let tool = DeleteDirTool::default();
    let err = tool
        .execute(&format!(r#"{{"path":{:?}}}"#, dir), &ctx())
        .await
        .err()
        .unwrap();
    assert!(err.contains("Failed to remove directory"), "got: {err}");
}

#[tokio::test]
async fn exec_reports_spawn_failure_for_nonexistent_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no_such_dir_covw6");
    let tool = ExecTool::new(tmp.path().to_string_lossy().as_ref(), false);
    let err = tool
        .execute(
            &serde_json::json!({"command": "echo hi", "cwd": missing}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("Failed to execute command"), "got: {err}");
}

// ---------------------------------------------------------------------------
// RunChecksTool：非法 JSON / filter+timeout / 无生态早退
// ---------------------------------------------------------------------------

#[tokio::test]
async fn run_checks_invalid_json_and_no_eco_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = RunChecksTool::new(tmp.path().to_string_lossy().as_ref());

    let err = tool.execute("not-json", &ctx()).await.err().unwrap();
    assert!(err.contains("Invalid JSON arguments"), "got: {err}");

    // 合法 JSON + filter/timeout 提取后，空目录无生态标记 → 诚实早退。
    let err = tool
        .execute(
            &serde_json::json!({"scope":"build","filter":"  cov_f  ","timeout":5000}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("No recognizable project"), "got: {err}");
}

// ---------------------------------------------------------------------------
// CronTool：create 空 ctx 回退臂
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cron_create_uses_stored_context_fallback() {
    let tmp = tempfile::tempdir().unwrap();
    let store = tmp.path().join("cron.json");
    let svc = Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(store.to_string_lossy().as_ref()),
    ));
    let tool = CronTool::new(svc);
    let out = tool
        .execute(
            &serde_json::json!({
                "action": "create",
                "name": "covw6-job",
                "schedule": "every:3600s",
                "content": "covw6 hello"
            })
            .to_string(),
            &empty_ctx(),
        )
        .await
        .unwrap();
    assert!(out.contains("covw6-job"), "got: {out}");
}

// ---------------------------------------------------------------------------
// TodoWriteTool：三个落盘错误臂
// ---------------------------------------------------------------------------

fn todo_tool(ws: &Path) -> TodoWriteTool {
    TodoWriteTool::new(ws.to_path_buf(), None)
}

fn todo_args() -> String {
    serde_json::json!({"todos":[{"content":"a","status":"pending"}]}).to_string()
}

#[tokio::test]
async fn todowrite_reports_sessions_dir_creation_failure() {
    let tmp = tempfile::tempdir().unwrap();
    // <ws>/sessions 已是文件 → create_dir_all 失败。
    std::fs::write(tmp.path().join("sessions"), "f").unwrap();
    let tool = todo_tool(tmp.path());
    let err = tool
        .execute(
            &todo_args(),
            &RequestContext::new("web", "c", "u", "covw6a"),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("create sessions dir failed"), "got: {err}");
}

#[tokio::test]
async fn todowrite_reports_tmp_write_failure_when_tmp_is_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    std::fs::create_dir_all(&sessions).unwrap();
    // tmp 文件路径被目录占用 → write 失败。
    std::fs::create_dir_all(sessions.join("todo_covw6b.json.tmp")).unwrap();
    let tool = todo_tool(tmp.path());
    let err = tool
        .execute(
            &todo_args(),
            &RequestContext::new("web", "c", "u", "covw6b"),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("write todo tmp failed"), "got: {err}");
}

#[tokio::test]
async fn todowrite_reports_rename_failure_when_target_is_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let sessions = tmp.path().join("sessions");
    std::fs::create_dir_all(sessions.join("todo_covw6c.json")).unwrap();
    let tool = todo_tool(tmp.path());
    let err = tool
        .execute(
            &todo_args(),
            &RequestContext::new("web", "c", "u", "covw6c"),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("rename todo file failed"), "got: {err}");
}

// ---------------------------------------------------------------------------
// peer_chat_callback handler
// ---------------------------------------------------------------------------

#[test]
fn peer_chat_callback_handler_replies_received() {
    let _logs = capture_logs();
    let mut handlers: std::collections::HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
    > = std::collections::HashMap::new();
    register_peer_chat_handler(&mut handlers, |_payload| {
        Ok(serde_json::json!({"content": "llm"}))
    });
    let cb = handlers.get("peer_chat_callback").unwrap();
    let out = cb(serde_json::json!({"task_id": "covw6-t1", "content": "hi"})).unwrap();
    assert_eq!(out["status"], "received");
    assert_eq!(out["task_id"], "covw6-t1");
}

// ---------------------------------------------------------------------------
// ClusterRpcTool：空 ctx 回退 + 同步响应
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cluster_rpc_empty_context_falls_back_to_stored() {
    let mut tool = ClusterRpcTool::new(ClusterRpcConfig::default());
    tool.set_rpc_call_fn(Arc::new(|_t: &str, _a: &str, _p: serde_json::Value| {
        Box::pin(async { Ok(serde_json::json!({"content": "cov-rpc-ok"})) })
    }));
    let out = tool
        .execute(
            &serde_json::json!({"target_node": "peer-a", "message": "hi"}).to_string(),
            &empty_ctx(),
        )
        .await
        .unwrap();
    assert_eq!(out, "cov-rpc-ok");
}

// ---------------------------------------------------------------------------
// SpawnTool：无槽早退 + getter
// ---------------------------------------------------------------------------

#[tokio::test]
async fn spawn_without_slot_reports_not_available_via_fallback_ctx() {
    let tool = SpawnTool::new(SpawnConfig {
        default_model: "stub".into(),
        max_concurrent: 2,
        max_depth: 1,
    });
    let err = tool
        .execute(
            &serde_json::json!({"task": "covw6 task"}).to_string(),
            &empty_ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("not available"), "got: {err}");
}

// ---------------------------------------------------------------------------
// memory 四工具：非法 JSON 臂（有 executor 时才可达）
// ---------------------------------------------------------------------------

fn covw6_memory_executor() -> Arc<nemesis_memory::memory_tools::MemoryToolExecutor> {
    let tmp = tempfile::tempdir().unwrap();
    // 泄漏 tempdir（进程退出即回收）：executor 生命周期跨测试体。
    let path = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    let cfg = nemesis_memory::manager::Config::new(&path);
    let mgr = Arc::new(nemesis_memory::manager::MemoryManager::new(&cfg));
    Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr))
}

#[tokio::test]
async fn memory_search_store_forget_invalid_json() {
    let exec = covw6_memory_executor();
    let search = MemorySearchTool::new(Some(exec.clone()));
    let err = search.execute("nope", &ctx()).await.err().unwrap();
    assert!(err.contains("Invalid JSON arguments"), "got: {err}");

    let store = MemoryStoreTool::new(Some(exec.clone()));
    let err = store.execute("nope", &ctx()).await.err().unwrap();
    assert!(err.contains("Invalid JSON arguments"), "got: {err}");

    let forget = MemoryForgetTool::new(Some(exec.clone()));
    let err = forget.execute("nope", &ctx()).await.err().unwrap();
    assert!(err.contains("Invalid JSON arguments"), "got: {err}");
}

#[tokio::test]
async fn memory_list_invalid_json_falls_back_to_empty_object() {
    let exec = covw6_memory_executor();
    let list = MemoryListTool::new(Some(exec));
    // 非法 JSON → unwrap_or_else(json!({})) → 空库列举成功。
    let out = list.execute("nope", &ctx()).await.unwrap();
    assert!(!out.is_empty());
}

// ---------------------------------------------------------------------------
// SkillManage：审批 Err / JoinError / patch 安全拦截 / write_file 父目录是文件
// ---------------------------------------------------------------------------

/// request_approval_sync 直接返回 Err 的假审批管理器。
struct ErrApproval;
impl nemesis_security::auditor::ApprovalManager for ErrApproval {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        Err("covw6 approval backend down".into())
    }
}

/// request_approval_sync panic 的假审批管理器 → spawn_blocking JoinError。
struct PanickingApproval;
impl nemesis_security::auditor::ApprovalManager for PanickingApproval {
    fn is_running(&self) -> bool {
        true
    }
    fn request_approval_sync(
        &self,
        _request_id: &str,
        _operation: &str,
        _target: &str,
        _risk_level: &str,
        _reason: &str,
        _timeout_secs: u64,
    ) -> Result<nemesis_security::auditor::ApprovalVerdict, String> {
        panic!("covw6 deliberate panic in approval backend");
    }
}

fn approval_tool(
    manager: Arc<dyn nemesis_security::auditor::ApprovalManager>,
    ws: &Path,
) -> SkillManageTool {
    let slot: ApprovalManagerSlot = Arc::new(parking_lot::RwLock::new(Some(manager)));
    SkillManageTool::new(ws.to_string_lossy().to_string(), Some(slot), true)
}

#[cfg(feature = "security")]
#[tokio::test]
async fn skill_manage_approval_backend_error_is_honest_err() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = approval_tool(Arc::new(ErrApproval), tmp.path());
    let err = tool
        .execute(
            &serde_json::json!({"action":"create","name":"covw6-err","content":"x"}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("approval request failed"), "got: {err}");
}

#[cfg(feature = "security")]
#[tokio::test]
async fn skill_manage_approval_join_error_is_honest_err() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = approval_tool(Arc::new(PanickingApproval), tmp.path());
    let err = tool
        .execute(
            &serde_json::json!({"action":"create","name":"covw6-panic","content":"x"}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("approval task failed"), "got: {err}");
}

#[tokio::test]
async fn skill_manage_patch_rejects_destructive_content() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = SkillManageTool::new(tmp.path().to_string_lossy().to_string(), None, false);
    let created = tool
        .execute(
            &serde_json::json!({
                "action":"create",
                "name":"covw6-patch",
                "content":"---\nname: covw6-patch\ndescription: covw6 test skill. Use when covw6 asks.\n---\n# Cov\nsay hi\n"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    assert!(created.contains("created"), "got: {created}");

    // patch 后内容含破坏性命令 → 安全检查拦截，写盘不发生。
    let err = tool
        .execute(
            &serde_json::json!({
                "action":"patch",
                "name":"covw6-patch",
                "old":"say hi",
                "new":"run shutdown -h now"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("blocked by security check"), "got: {err}");
}

#[tokio::test]
async fn skill_manage_write_file_fails_when_parent_is_file() {
    let tmp = tempfile::tempdir().unwrap();
    let tool = SkillManageTool::new(tmp.path().to_string_lossy().to_string(), None, false);
    tool.execute(
        &serde_json::json!({
            "action":"create",
            "name":"covw6-wf",
            "content":"---\nname: covw6-wf\ndescription: covw6 test skill. Use when covw6 asks.\n---\n# Cov\nbody\n"
        })
        .to_string(),
        &ctx(),
    )
    .await
    .unwrap();
    // `sub` 是文件 → 为 "sub/x.txt" 建父目录失败。
    std::fs::write(tmp.path().join("skills").join("covw6-wf").join("sub"), "f").unwrap();
    let err = tool
        .execute(
            &serde_json::json!({
                "action":"write_file",
                "name":"covw6-wf",
                "path":"sub/x.txt",
                "content":"data"
            })
            .to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("failed to create dir"), "got: {err}");
}

// ---------------------------------------------------------------------------
// GitTool：commit spawn 失败（workspace 不存在）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn git_commit_spawn_failure_is_honest_err() {
    let tmp = tempfile::tempdir().unwrap();
    let missing = tmp.path().join("no_repo_covw6");
    let tool = GitTool::new(missing.to_string_lossy().to_string());
    let err = tool
        .execute(
            &serde_json::json!({"action":"commit","message":"m"}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("failed to run git"), "got: {err}");
}

// ---------------------------------------------------------------------------
// mcp_discover：args 数组混合类型过滤
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_discover_filters_non_string_args_and_reports_spawn_error() {
    let tool = McpDiscoverTool::new();
    let err = tool
        .execute(
            &serde_json::json!({
                "command": "no_such_covw6_binary_xyz",
                "args": ["a", 123, "b"]
            })
            .to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(!err.is_empty());
}

// ---------------------------------------------------------------------------
// register_shared_tools：spawn_slot / claude / codex / lsp / workflow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn register_shared_tools_spawn_slot_arm() {
    let _logs = capture_logs();
    let cfg = SharedToolConfig {
        spawn: Some(SpawnConfig {
            default_model: "stub".into(),
            max_concurrent: 2,
            max_depth: 1,
        }),
        spawn_slot: Some(Arc::new(std::sync::OnceLock::new())),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    let spawn = tools.get("spawn").expect("spawn tool registered");

    // 空 ctx → stored 回退臂；槽内无闭包 → 诚实 not available。
    let err = spawn
        .execute(
            &serde_json::json!({"task": "covw6"}).to_string(),
            &empty_ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("not available"), "got: {err}");
}

#[tokio::test]
async fn register_shared_tools_claude_and_codex_probes() {
    let _logs = capture_logs();
    let cfg = SharedToolConfig {
        claude_code_tool_enabled: true,
        codex_tool_enabled: true,
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    // 两条探针 info! 行都被执行（注册与否取决于本机 PATH，不硬断言）。
    let _ = tools.contains_key("claude_code");
    let _ = tools.contains_key("codex_delegate");
}

#[tokio::test]
async fn register_shared_tools_lsp_both_manager_arms() {
    let _logs = capture_logs();
    // 臂 1：外部单例 manager（with_manager）。
    let cfg_ext = SharedToolConfig {
        lsp_tool_enabled: true,
        lsp_manager: Some(Arc::new(nemesis_lsp::LspManager::new(
            None::<Duration>,
            None::<Duration>,
        ))),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg_ext);
    assert!(tools.contains_key("lsp"), "rust-analyzer expected on PATH");

    // 臂 2：自建 manager（LspTool::new 兜底）。
    let cfg_self = SharedToolConfig {
        lsp_tool_enabled: true,
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg_self);
    assert!(tools.contains_key("lsp"));
}

#[cfg(feature = "workflow")]
#[test]
fn register_shared_tools_workflow_trio_and_final_info() {
    let _logs = capture_logs();
    let engine = Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    let cfg = SharedToolConfig {
        workflow_engine: Some(engine),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(tools.contains_key("workflow_run"));
    assert!(tools.contains_key("workflow_create"));
    assert!(tools.contains_key("workflow_capabilities"));
}

// ---------------------------------------------------------------------------
// WorkflowCreateTool 决策表 + WorkflowCapabilitiesTool
// ---------------------------------------------------------------------------

#[cfg(feature = "workflow")]
fn covw6_create(engine: Arc<nemesis_workflow::engine::WorkflowEngine>) -> WorkflowCreateTool {
    WorkflowCreateTool::new(engine)
}

#[cfg(feature = "workflow")]
#[tokio::test]
async fn workflow_create_missing_definition_and_schema_mismatch() {
    let engine = Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    let tool = covw6_create(engine);

    let err = tool
        .execute(&serde_json::json!({}).to_string(), &ctx())
        .await
        .err()
        .unwrap();
    assert!(err.contains("'definition'"), "got: {err}");

    // nodes 不是数组 → schema mismatch 回灌。
    let err = tool
        .execute(
            &serde_json::json!({"definition": {"name":"x","nodes":"nope"}}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(
        err.contains("does not match the workflow schema"),
        "got: {err}"
    );
}

#[cfg(feature = "workflow")]
#[tokio::test]
async fn workflow_create_rejects_blank_name_and_missing_defs_dir() {
    let engine = Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    let tool = covw6_create(engine.clone());

    let err = tool
        .execute(
            &serde_json::json!({"definition": {"name":"   ","nodes":[]}}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(err.contains("non-empty string"), "got: {err}");

    // defs dir 未配置 → 草稿无处落，诚实报错。
    let err = tool
        .execute(
            &serde_json::json!({"definition": {"name":"covwf","nodes":[]}}).to_string(),
            &ctx(),
        )
        .await
        .err()
        .unwrap();
    assert!(
        err.contains("definitions directory is not configured"),
        "got: {err}"
    );
}

#[cfg(feature = "workflow")]
#[tokio::test]
async fn workflow_create_saves_draft_and_reports_payload() {
    let tmp = tempfile::tempdir().unwrap();
    let engine = Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    engine.set_workflow_defs_dir(tmp.path().join("defs"));
    let tool = covw6_create(engine);

    let out = tool
        .execute(
            &serde_json::json!({
                "definition": {
                    "name": "covw6_wf",
                    "description": "covw6 draft",
                    "triggers": [],
                    "nodes": []
                }
            })
            .to_string(),
            &ctx(),
        )
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["status"], "draft_saved");
    assert_eq!(v["name"], "covw6_wf");
    assert_eq!(v["node_count"], 0);
    assert!(!v["hints"].as_array().unwrap().is_empty());
}

#[cfg(feature = "workflow")]
#[tokio::test]
async fn workflow_capabilities_renders_capability_table() {
    let tool = WorkflowCapabilitiesTool;
    assert!(!tool.description().is_empty());
    assert!(tool.parameters().is_object());

    let out = tool.execute("{}", &ctx()).await.unwrap();
    assert!(out.contains("node"), "got: {out}");
}
