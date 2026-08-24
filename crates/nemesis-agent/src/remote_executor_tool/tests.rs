use super::*;

#[test]
fn build_command_no_start_exe_is_direct_spawn() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| false),
    );
    assert!(ch.start_exe.is_none());
    // No error — direct spawn command is built.
    let _ = ch.build_command();
}

#[test]
fn build_command_with_start_exe_wraps() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/x/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_start_exe(PathBuf::from("/x/Start.exe"));
    assert!(ch.start_exe.is_some());
    // No error — Start.exe wrap command is built (L2.2 form).
    let _ = ch.build_command();
}

#[test]
fn move_tools_is_the_expected_set() {
    assert_eq!(
        MOVE_TOOLS,
        &[
            "exec",
            "run_script",
            "read_file",
            "write_file",
            "list_dir",
            "edit_file",
            "append_file",
            "delete_file",
            "create_dir",
            "delete_dir",
            "grep",
            "git",
        ]
    );
}

// ---------------------------------------------------------------------------
// P5-2 严格模式闸门：fail-closed 拒绝发生在 spawn 之前
// ---------------------------------------------------------------------------

use std::time::Duration;

use crate::context::RequestContext;

/// 通道带 sandbox_probe=true + 严格闸门。exe 指向不存在的路径——若闸门
/// 失效放行到 spawn，错误信息是 "failed to spawn"（可据此区分拒绝对顺序）。
fn strict_test_channel(gate_result: Result<(), String>) -> ExecutorChannel {
    ExecutorChannel::new(
        PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_timeout(Duration::from_secs(5))
    .with_strict_gate(Arc::new(move || gate_result.clone()))
}

fn req_ctx() -> RequestContext {
    RequestContext::new("web", "test-chat", "test-session", "/ws")
}

#[tokio::test]
async fn strict_gate_refuses_sandboxed_call_before_spawn() {
    let ch = strict_test_channel(Err("Sandboxie engine not ready".into()));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("gate must refuse");
    assert!(err.contains("strict mode"), "err: {err}");
    assert!(err.contains("Sandboxie engine not ready"), "err: {err}");
    // 拒绝发生在 spawn 之前：不是子进程 spawn 失败的错误。
    assert!(!err.contains("failed to spawn"), "refusal must precede spawn: {err}");
}

#[tokio::test]
async fn strict_gate_pass_proceeds_to_spawn() {
    // 闸门 Ok → 放行到正常 spawn 路径（exe 不存在 → spawn 失败，证明顺序）。
    let ch = strict_test_channel(Ok(()));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail");
    assert!(!err.contains("strict mode"), "gate passed: {err}");
}

/// 默认（未注入闸门）行为与改动前逐字节一致：probe=true 直接进 spawn 路径。
#[tokio::test]
async fn no_gate_keeps_default_fail_open_path() {
    let ch = ExecutorChannel::new(
        PathBuf::from("/definitely/not/a/real/nemesisbot.exe"),
        "/ws".into(),
        Arc::new(|| true),
    )
    .with_timeout(Duration::from_secs(5));
    assert!(ch.strict_gate.is_none(), "default construction has no gate");
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail");
    assert!(!err.contains("strict mode"), "no gate = no refusal: {err}");
}
