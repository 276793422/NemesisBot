use super::*;
use nemesis_tools::registry::Tool as RegistryTool;

/// The adapter must delegate to the wrapped agent tool: a `write_file` call
/// through the nemesis-tools `Tool` trait should actually write the file
/// and return a success result. (This is the core of the fix — the workflow
/// tool node previously hit an empty registry and always failed.)
#[tokio::test]
async fn test_adapter_delegates_to_agent_tool() {
    // Use a temp path but drop the handle so write_file can create the
    // file fresh (avoids Windows handle locks).
    let tmp_path =
        std::env::temp_dir().join(format!("nemesis_adapter_test_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&tmp_path);
    let path_str = tmp_path.to_string_lossy().to_string();

    let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool::default());
    #[cfg(feature = "security")]
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner, None);
    #[cfg(not(feature = "security"))]
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner);

    let args = serde_json::json!({ "path": path_str, "content": "hello workflow" });
    let result = adapter.execute(&args).await;
    assert!(
        !result.is_error,
        "expected success, got: {}",
        result.for_llm
    );
    assert!(result.for_llm.contains("Successfully wrote"));

    let written = std::fs::read_to_string(&tmp_path).unwrap();
    assert_eq!(written, "hello workflow");
    let _ = std::fs::remove_file(&tmp_path);
}

/// Agent-tool errors must surface as `is_error` ToolResults (not panics),
/// so the workflow node marks itself Failed and the run can branch.
#[tokio::test]
async fn test_adapter_surfaces_agent_tool_error() {
    let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::ReadFileTool::default());
    #[cfg(feature = "security")]
    let adapter = AgentToolAdapter::new("read_file".to_string(), inner, None);
    #[cfg(not(feature = "security"))]
    let adapter = AgentToolAdapter::new("read_file".to_string(), inner);

    let args = serde_json::json!({ "path": "/no/such/path/nemesis_adapter_missing_file_xyz" });
    let result = adapter.execute(&args).await;
    assert!(result.is_error, "expected error for missing file");
}

/// `parameters()` must forward the agent tool's schema verbatim so the
/// workflow canvas can render a schema-driven form.
#[test]
fn test_adapter_forwards_schema_and_metadata() {
    let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool::default());
    #[cfg(feature = "security")]
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner, None);
    #[cfg(not(feature = "security"))]
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner);

    assert_eq!(adapter.name(), "write_file");
    assert!(!adapter.description().is_empty());
    let params = adapter.parameters();
    let required = params
        .get("required")
        .and_then(|v| v.as_array())
        .expect("schema has required array");
    let names: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"path"));
    assert!(names.contains(&"content"));
}

/// Security deny path: with a plugin whose default action is "deny" and no
/// rules, a known write tool is blocked at the ABAC layer BEFORE the inner
/// tool runs — the adapter must surface `⛔ SECURITY BLOCKED: …` (never
/// execute the wrapped tool).
#[cfg(feature = "security")]
#[tokio::test]
async fn test_adapter_security_deny_blocks_execution() {
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};

    // default config = enabled, default_action "deny", zero rules → every
    // classified operation denies at Layer 3 (ABAC default action).
    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig::default()));

    let tmp_path =
        std::env::temp_dir().join(format!("nemesis_adapter_deny_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&tmp_path);

    let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool::default());
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner, Some(plugin));

    let args = serde_json::json!({ "path": tmp_path.display().to_string(), "content": "must not be written" });
    let result = adapter.execute(&args).await;
    assert!(result.is_error, "deny must surface as error result");
    assert!(
        result.for_llm.contains("SECURITY BLOCKED"),
        "explicit blocked prefix expected, got: {}",
        result.for_llm
    );
    // The wrapped tool must NOT have run.
    assert!(
        !tmp_path.exists(),
        "denied write must not touch the filesystem"
    );
    let _ = std::fs::remove_file(&tmp_path);
}

/// BUG-20261119-199 回归锁：工作流工具适配器的审批等待必须发生在 blocking
/// 线程（spawn_blocking），不得占 tokio worker——模块文档自证 ask 臂
/// 「block until a verdict or timeout」是既定审批语义（有意等裁决），但
/// 占 worker 阻塞是 dispatch 主路径（tool_dispatch.rs）修过的缺陷形态：
/// 并行批 N 张卡 = N 个 worker 饿死；current_thread runtime 上
/// WebApprovalManager 的 block_in_place 还会直接 panic。红证：裸调形态下
/// 管理器在 worker 线程被调（tokio 默认 worker 名 tokio-rt-worker，本仓库 tokio 版本实测），
/// spawn_blocking 池线程无名 → 断言通过。
#[cfg(feature = "security")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_adapter_security_approval_wait_runs_off_worker_thread() {
    use nemesis_security::auditor::{ApprovalContext, ApprovalManager, ApprovalVerdict};
    use nemesis_security::pipeline::{SecurityPlugin, SecurityPluginConfig};
    use std::sync::Mutex;

    #[derive(Default)]
    struct ThreadRecordingManager {
        called_task_id: Mutex<Option<tokio::task::Id>>,
    }
    impl ApprovalManager for ThreadRecordingManager {
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
        ) -> Result<ApprovalVerdict, String> {
            Err("ctx-less path must not be taken".to_string())
        }
        fn request_approval_sync_ctx(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
            _ctx: &ApprovalContext,
        ) -> Result<ApprovalVerdict, String> {
            // 判别 = 任务 id 对比：裸调在调用方（gate_and_write / adapter /
            // hook 任务）上下文里同步执行 → mock 的 try_id == 调用方任务 id；
            // spawn_blocking 在独立 blocking 任务里跑（try_id 虽也 Some——
            // blocking 任务同样有 TaskId，线程名亦同名 tokio-rt-worker，
            // 均区分不了）→ id 必不相同。
            *self.called_task_id.lock().unwrap() = tokio::task::try_id();
            Ok(ApprovalVerdict::approved())
        }
    }

    let plugin = Arc::new(SecurityPlugin::new(SecurityPluginConfig {
        enabled: true,
        command_guard_enabled: false,
        injection_enabled: false,
        credential_enabled: false,
        dlp_enabled: false,
        ssrf_enabled: false,
        default_action: "ask".to_string(),
        ..Default::default()
    }));
    let recording = Arc::new(ThreadRecordingManager::default());
    plugin.auditor().set_approval_manager(recording.clone());

    let tmp_path =
        std::env::temp_dir().join(format!("nemesis_adapter_ask_{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&tmp_path);

    let inner: Arc<dyn AgentTool> = Arc::new(crate::loop_tools::WriteFileTool::default());
    let adapter = AgentToolAdapter::new("write_file".to_string(), inner, Some(plugin));

    let args = serde_json::json!({
        "path": tmp_path.display().to_string(),
        "content": "written after recorded approval"
    });
    // 必须经 tokio::spawn 进 worker 上下文：#[tokio::test] 测试体由
    // block_on 在测试主线程驱动（非 worker），裸调断言天然通过（红证力
    // 为零）。生产 gateway 里工具调度正是 worker 上下文。
    let gaw_handle = tokio::spawn(async move { adapter.execute(&args).await });
    let gaw_id = gaw_handle.id();
    let result = gaw_handle.await.expect("spawned task must not panic");
    assert!(
        !result.is_error,
        "recorded approval must allow the write: {}",
        result.for_llm
    );
    assert!(tmp_path.exists(), "approved write must land");

    let called_id = (*recording.called_task_id.lock().unwrap())
        .expect("ask policy must reach the approval manager");
    assert_ne!(
        gaw_id, called_id,
        "BUG-20261119-199: 审批等待在调用方任务上下文内同步执行（裸调 security.execute 占死 tokio worker）— 必须走 spawn_blocking 与 dispatch 主路径对齐"
    );
    let _ = std::fs::remove_file(&tmp_path);
}
