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
use std::time::Duration;

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

/// Resolve the sandbox home to the ACTIVELY-configured box, mirroring the
/// gateway's resolution — NOT a hardcoded `~/.nemesisbot` (which on a machine
/// that runs the packaged `bin/bin_windows/nemesisbot.exe` is the wrong dir:
/// the box + Start.exe live in the exe-relative home). Order:
///   1. `NEMESISBOT_HOME` env (explicit override),
///   2. the workspace's packaged home `bin/bin_windows/.nemesisbot` (where
///      `nemesisbot sandbox start` is typically run on Windows — auto-detected
///      so the tests hit the real box without env hand-holding),
///   3. `~/.nemesisbot` (default-home fallback).
fn sandbox_home() -> PathBuf {
    if let Ok(h) = std::env::var("NEMESISBOT_HOME") {
        return PathBuf::from(h);
    }
    // Use `.parent()` (NOT `.join("..")`) so the path stays canonical —
    // pending_workspace's `real_path.starts_with(workspace)` filter is a lexical
    // component match, and a `..` segment would mismatch the box's canonical
    // real_path (every file filtered out → 0 pending).
    let dev_home = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("CARGO_MANIFEST_DIR has no parent")
        .join("bin")
        .join("bin_windows")
        .join(".nemesisbot");
    if dev_home.is_dir() {
        return dev_home;
    }
    dirs::home_dir()
        .expect("home dir")
        .join(".nemesisbot")
}

/// Wait for the Sandboxie box session to quiesce. Sandboxie serializes box
/// session init and flushes the virtual FS to `FileRootPath` when the session
/// ends — so (a) a Start.exe spawn right after a previous box session timed
/// out connecting to the pipe, and (b) a `pending` read right after a boxed
/// write sees a not-yet-flushed box. Both race the session teardown. In
/// production the LLM latency between tool calls masks this; the L2.2 tests
/// run back-to-back, so they must let the box settle.
async fn settle_sandbox_box() {
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
}

#[tokio::test]
async fn spawn_and_call_sleep_round_trips() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| false)));
    let res = ch
        .spawn_and_call("sleep", r#"{"seconds":1}"#, &ctx())
        .await
        .expect("sleep should succeed");
    assert!(res.contains("Slept"), "unexpected sleep result: {res}");
}

#[tokio::test]
async fn spawn_and_call_exec_runs_command() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| false)));
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
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| false)));
    let err = ch
        .spawn_and_call("definitely_not_a_tool", "{}", &ctx())
        .await
        .expect_err("unknown tool should error");
    assert!(err.contains("unknown tool"), "unexpected error: {err}");
}

#[tokio::test]
async fn spawn_and_call_file_write_then_read_round_trips() {
    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| false)));
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

#[cfg(windows)]
#[tokio::test]
async fn spawn_and_call_via_pipe_round_trips() {
    // sandbox=true → named-pipe transport. L2.1: start_exe=None → direct spawn
    // (no box), so this validates the pipe transport independently of Sandboxie.
    let ch = Arc::new(
        ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| true))
            .with_timeout(Duration::from_secs(15)),
    );
    let res = ch
        .spawn_and_call("sleep", r#"{"seconds":1}"#, &ctx())
        .await
        .expect("pipe round-trip should succeed");
    assert!(res.contains("Slept"), "unexpected sleep result via pipe: {res}");
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_and_call_via_startexe_crosses_box() {
    // L2.2 headline test: spawn via Start.exe into NemesisBox, exchange over the
    // named pipe. Verifies the box's OpenPipePath lets the pipe through (the
    // make-or-break Layer 2 risk). Requires `nemesisbot sandbox install` to have
    // been run (driver + Start.exe present).
    let home = sandbox_home();
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    let start_exe = paths.start_exe();
    assert!(
        start_exe.exists(),
        "Start.exe not found at {} — run `nemesisbot sandbox install` first",
        start_exe.display()
    );

    let ch = Arc::new(
        ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| true))
            .with_start_exe(start_exe)
            .with_timeout(Duration::from_secs(30)),
    );
    let res = ch
        .spawn_and_call("sleep", r#"{"seconds":1}"#, &ctx())
        .await
        .expect("Start.exe + pipe round-trip should succeed (box must let the pipe through)");
    assert!(
        res.contains("Slept"),
        "unexpected sleep result via Start.exe+pipe: {res}"
    );
}

#[cfg(windows)]
#[tokio::test]
async fn spawn_and_call_via_startexe_isolates_outside_workspace_write() {
    // L2.2 isolation: a boxed write_file to a path OUTSIDE the workspace must
    // NOT touch the real disk — the write is contained in the box's virtual FS.
    // (write_file is a unit tool with no self-restrict, so it will happily write
    // wherever asked; the box must contain it.) Requires sandbox install.
    let home = sandbox_home();
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    let start_exe = paths.start_exe();
    assert!(
        start_exe.exists(),
        "Start.exe not found at {} — run `nemesisbot sandbox install` first",
        start_exe.display()
    );

    // EXPERIMENT-CONFIRMED: consecutive Start.exe spawns into the same box need
    // the previous box session to quiesce (Sandboxie serializes session init).
    // In production LLM latency between tool calls provides this naturally;
    // back-to-back tests do not.
    settle_sandbox_box().await;

    // A path OUTSIDE the workspace, unique per test run, clean slate.
    let outside = std::env::temp_dir().join(format!(
        "nemesis_isolation_{}.txt",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&outside);

    let ch = Arc::new(
        ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| true))
            .with_start_exe(start_exe)
            .with_timeout(Duration::from_secs(30)),
    );
    let args = format!(
        r#"{{"path":{:?},"content":"isolation-marker"}}"#,
        outside.to_string_lossy()
    );
    let res = ch.spawn_and_call("write_file", &args, &ctx()).await;
    // The boxed tool should report success (it wrote to the box's virtual FS)...
    assert!(
        res.as_ref().map(|r| r.contains("wrote")).unwrap_or(false),
        "write_file should succeed inside the box: {:?}",
        res
    );
    // ...but the REAL disk must NOT have the file (containment held).
    assert!(
        !outside.exists(),
        "ISOLATION FAILED: real file exists at {} — the box did not contain the write",
        outside.display()
    );
}

#[cfg(windows)]
#[tokio::test]
async fn l23_pending_commit_brings_boxed_workspace_write_to_real_disk() {
    // L2.3: a sandboxed write to the workspace lands in the box's virtual FS;
    // pending lists it; commit copies it to real disk. (write_file is a unit tool
    // — no self-restrict — so it writes wherever asked; the box contains it until
    // commit.) Requires `nemesisbot sandbox install`.
    let home = sandbox_home();
    let paths = nemesis_sandbox::SandboxPaths::new(&home);
    let start_exe = paths.start_exe();
    assert!(
        start_exe.exists(),
        "Start.exe not found at {} — run `nemesisbot sandbox install` first",
        start_exe.display()
    );

    // Workspace under %USERPROFILE% (so it maps via the box's user/current layout).
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).ok();
    let target = workspace.join("l23_test.txt");
    let _ = std::fs::remove_file(&target); // clean slate on real disk

    let ch = Arc::new(
        ExecutorChannel::new(nemesisbot_exe(), workspace.to_string_lossy().to_string(), Arc::new(|| true))
            .with_start_exe(start_exe.clone())
            .with_timeout(Duration::from_secs(30)),
    );
    let args = format!(
        r#"{{"path":{:?},"content":"l23-commit-marker"}}"#,
        target.to_string_lossy()
    );
    let res = ch.spawn_and_call("write_file", &args, &ctx()).await;
    assert!(
        res.as_ref().map(|r| r.contains("wrote")).unwrap_or(false),
        "boxed write_file should succeed: {:?}",
        res
    );
    // The write is contained — real disk must NOT have it yet.
    assert!(
        !target.exists(),
        "real target must not exist before commit (write should be in the box)"
    );

    // Let the box session flush the write to the on-disk virtual FS before
    // pending enumerates it (otherwise pending races the flush and sees 0).
    settle_sandbox_box().await;

    // pending must list the workspace file.
    let up = dirs::home_dir().expect("home dir");
    let pending =
        nemesis_sandbox::pending::pending_workspace(&paths.box_root, &workspace, &up).unwrap();
    let found = pending.iter().find(|p| p.real_path.ends_with("l23_test.txt"));
    assert!(
        found.is_some(),
        "pending should list l23_test.txt; got {} entries: {:?}",
        pending.len(),
        pending
    );

    // commit brings it to real disk.
    let n = nemesis_sandbox::pending::commit_file(found.unwrap()).unwrap();
    assert!(n > 0, "commit copied 0 bytes");
    assert!(target.exists(), "real target must exist after commit");
    let content = std::fs::read_to_string(&target).unwrap();
    assert!(
        content.contains("l23-commit-marker"),
        "committed content mismatch: {content}"
    );

    // cleanup: clear the box + remove the real test file.
    let _ = nemesis_sandbox::pending::delete_box_contents(
        &paths.start_exe(),
        nemesis_sandbox::DEFAULT_BOX_NAME,
    );
    let _ = std::fs::remove_file(&target);
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

    let ch = Arc::new(ExecutorChannel::new(nemesisbot_exe(), workspace(), Arc::new(|| false)));
    let remote = RemoteExecutorTool::new("sleep".to_string(), local, ch);

    assert_eq!(remote.parameters(), local_params, "parameters must delegate verbatim");
    assert_eq!(remote.description(), local_desc, "description must delegate verbatim");
}
