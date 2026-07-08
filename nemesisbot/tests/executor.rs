//! Integration tests for executor separation (Layer 1).
//!
//! These spawn the real `nemesisbot` binary in executor role via
//! [`ExecutorChannel`] — exactly the dispatch path the gateway uses — and
//! exercise the per-call stdio IPC end-to-end. They cover the gateway-side path
//! that the manual child-side smoke tests (piping JSON directly) did not.
//!
//! Run: `cargo test -p nemesisbot --test executor`
//!
//! See `docs/PLAN/2026-07-08_executor-separation.md`.

use std::path::PathBuf;
use std::sync::Arc;

use nemesis_agent::context::RequestContext;
use nemesis_agent::r#loop::Tool;
use nemesis_agent::{register_default_tools, ExecutorChannel, RemoteExecutorTool};

/// Path to the built nemesisbot binary. `CARGO_BIN_EXE_nemesisbot` is injected
/// by cargo for integration tests in this package and always points at the
/// freshly-built binary regardless of profile / target dir.
fn nemesisbot_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_nemesisbot"))
}

/// A writable workspace path passed to the executor child. Must be an EXISTING
/// dir: ExecTool uses it as the command's cwd. The nemesisbot package dir
/// (CARGO_MANIFEST_DIR) always exists and is writable. (Cargo's target dir lives
/// at the workspace root, so `<manifest>/target` would NOT exist.)
fn workspace() -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .to_string_lossy()
        .to_string()
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "executor-test", "tester", "executor-test-session")
}

#[tokio::test]
async fn spawn_and_call_sleep_round_trips() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), false));
    let res = ch
        .spawn_and_call("sleep", r#"{"seconds":1}"#, &ctx())
        .await
        .expect("sleep should succeed");
    assert!(res.contains("Slept"), "unexpected sleep result: {res}");
}

#[tokio::test]
async fn spawn_and_call_exec_runs_command() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), false));
    let res = ch
        .spawn_and_call("exec", r#"{"command":"echo executor-integration"}"#, &ctx())
        .await
        .expect("exec should succeed");
    assert!(
        res.contains("executor-integration"),
        "exec result should contain echo output: {res}"
    );
}

#[tokio::test]
async fn spawn_and_call_unknown_tool_errors() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), false));
    let err = ch
        .spawn_and_call("definitely_not_a_tool", "{}", &ctx())
        .await
        .expect_err("unknown tool should error");
    assert!(err.contains("unknown tool"), "unexpected error: {err}");
}

#[tokio::test]
async fn spawn_and_call_file_write_then_read_round_trips() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), false));
    // Unique temp file to avoid parallel-test collisions.
    let path = std::env::temp_dir().join(format!("executor_test_{}.txt", std::process::id()));
    let write_args = format!(
        r#"{{"path":{:?},"content":"executor-file-round-trip"}}"#,
        path.to_string_lossy()
    );
    ch.spawn_and_call("write_file", &write_args, &ctx())
        .await
        .expect("write_file should succeed");

    let read_args = format!(r#"{{"path":{:?}}}"#, path.to_string_lossy());
    let content = ch
        .spawn_and_call("read_file", &read_args, &ctx())
        .await
        .expect("read_file should succeed");
    assert!(
        content.contains("executor-file-round-trip"),
        "read should return what we wrote: {content}"
    );

    // Clean up via the tool itself (also exercises delete_file remotely).
    let del_args = format!(r#"{{"path":{:?}}}"#, path.to_string_lossy());
    let _ = ch.spawn_and_call("delete_file", &del_args, &ctx()).await;
}

#[tokio::test]
async fn spawn_and_call_sandbox_without_start_exe_errors_clearly() {
    // Layer 2 (sandbox) is not integrated yet. sandbox=true must fail with a
    // clear message rather than silently running unsandboxed.
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), true));
    let err = ch
        .spawn_and_call("sleep", r#"{"seconds":1}"#, &ctx())
        .await
        .expect_err("sandbox without Start.exe must error");
    assert!(
        err.contains("Sandboxie"),
        "error should mention Sandboxie not being integrated: {err}"
    );
}

#[test]
fn remote_executor_tool_delegates_schema_byte_identically() {
    // RemoteExecutorTool must surface the SAME description/parameters as the
    // local impl it wraps — the LLM sees the local schema (never crosses the
    // wire), so prompt cache is preserved and the checkpoint safety net still
    // snapshots file writes. Captured before the local tool is moved in.
    let mut tools = register_default_tools();
    let local = tools.remove("sleep").expect("sleep tool should be registered");
    let local_params = local.parameters();
    let local_desc = local.description();

    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), false));
    let remote = RemoteExecutorTool::new("sleep".to_string(), local, ch);

    assert_eq!(remote.parameters(), local_params, "parameters must delegate verbatim");
    assert_eq!(remote.description(), local_desc, "description must delegate verbatim");
}
