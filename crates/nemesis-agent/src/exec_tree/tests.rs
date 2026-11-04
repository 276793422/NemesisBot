//! 修6 守卫单测：终止原因单调、真实进程树收杀、drop RAII 兜底、会话级 sweep。

use super::*;
#[cfg(target_os = "windows")]
use std::sync::atomic::AtomicIsize;
#[cfg(not(target_os = "windows"))]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicBool, AtomicU8};
use std::time::Duration;
use tokio::time::timeout;

/// 无子进程的守卫（job=0 / pgid=0：kill_tree_now 诚实返回 false）——
/// 原因单调逻辑纯内存可测。
fn detached_inner() -> Arc<ExecTreeInner> {
    Arc::new(ExecTreeInner {
        #[cfg(target_os = "windows")]
        job: AtomicIsize::new(0),
        #[cfg(not(target_os = "windows"))]
        pgid: AtomicU32::new(0),
        completed: AtomicBool::new(false),
        reason: AtomicU8::new(R_NONE),
    })
}

/// 长命子进程（60s；测试只活到收杀为止）。形态对齐生产内核：
/// kill_on_drop + unix process_group(0)。
fn long_command() -> tokio::process::Command {
    #[cfg(target_os = "windows")]
    {
        let mut c = tokio::process::Command::new("cmd");
        c.args(["/C", "ping", "-n", "60", "127.0.0.1"]);
        c.kill_on_drop(true);
        c
    }
    #[cfg(not(target_os = "windows"))]
    {
        let mut c = tokio::process::Command::new("sleep");
        c.arg("60");
        c.kill_on_drop(true);
        c.process_group(0);
        c
    }
}

#[test]
fn reason_precedence_monotonic() {
    let guard = ExecTreeGuard {
        inner: detached_inner(),
    };
    assert_eq!(guard.reason(), None);
    guard.terminate(TerminationReason::Drop);
    assert_eq!(guard.reason(), Some(TerminationReason::Drop));
    guard.terminate(TerminationReason::Timeout);
    assert_eq!(guard.reason(), Some(TerminationReason::Timeout));
    // 低优先级后写不得降级（estop sweep 之后到来的 drop 不改标注）。
    guard.terminate(TerminationReason::Drop);
    assert_eq!(guard.reason(), Some(TerminationReason::Timeout));
    guard.terminate(TerminationReason::Estop);
    assert_eq!(guard.reason(), Some(TerminationReason::Estop));
    guard.terminate(TerminationReason::Timeout);
    assert_eq!(guard.reason(), Some(TerminationReason::Estop));
}

#[tokio::test]
async fn terminate_kills_whole_tree() {
    let mut cmd = long_command();
    let (guard, mut child) =
        ExecTreeGuard::attach_and_register(&mut cmd, None).expect("spawn long child");
    guard.terminate(TerminationReason::Timeout);
    // 收杀是即时内核调用：wait 必须秒级返回（30s 上限只防极端环境）。
    let waited = timeout(Duration::from_secs(30), child.wait()).await;
    let status = waited
        .expect("terminate 后 wait 应即时返回")
        .expect("wait ok");
    assert!(!status.success(), "被收杀的进程不应报成功退出");
    assert_eq!(guard.reason(), Some(TerminationReason::Timeout));
}

#[tokio::test]
async fn drop_guard_kills_child() {
    let mut cmd = long_command();
    let (guard, mut child) =
        ExecTreeGuard::attach_and_register(&mut cmd, None).expect("spawn long child");
    // RAII 兜底：未 mark_complete 即 drop → 树被收（未来取消路径的语义）。
    drop(guard);
    let waited = timeout(Duration::from_secs(30), child.wait()).await;
    let status = waited.expect("drop 兜底收杀应即时返回").expect("wait ok");
    assert!(!status.success(), "被 drop 收杀的进程不应报成功退出");
}

#[tokio::test]
async fn registry_sweep_kills_session_trees() {
    // 唯一键（nanos）防并行测试互扰；注册表是进程级全局。
    let key = format!(
        "exec-tree-sweep-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    );
    let mut cmd = long_command();
    let (guard, mut child) =
        ExecTreeGuard::attach_and_register(&mut cmd, Some(&key)).expect("spawn long child");
    assert_eq!(kill_session_trees(&key), 1, "sweep 应命中本测试注册的树");
    let waited = timeout(Duration::from_secs(30), child.wait()).await;
    let status = waited.expect("sweep 收杀应即时返回").expect("wait ok");
    assert!(!status.success());
    // estop 标注判定源：sweep 后 reason == Estop（run_checks 诚实标注消费）。
    assert_eq!(guard.reason(), Some(TerminationReason::Estop));
    // 快照语义：条目已移除，第二次 sweep 无目标。
    assert_eq!(kill_session_trees(&key), 0);
}
