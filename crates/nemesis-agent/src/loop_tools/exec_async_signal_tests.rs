//! AsyncExecTool 启动确认臂的信号终止诚实上报（BUG-20261117-056）。
//!
//! `child.wait()` Ok 返回后进程必然已退出——`code()` None = 信号终止
//! （Unix 启动即崩的常见失败形态：SIGSEGV/SIGKILL），必须 Err
//! 「exited prematurely (terminated by signal)」，不得并入成功臂谎报
//! 「确认运行中」（timeout Err 臂才是 still running 语义）。
//!
//! 裁决逻辑抽成纯函数 [`startup_confirm_verdict`]（exec.rs），
//! 合成 `(success, code=None)` 输入即可在**任意平台**锁死 None 臂
//! （Windows `code()` 恒 Some、None 臂真实进程不可达——纯函数测试
//! 全平台跑，真进程 e2e 用例 unix 门控兜底 Linux nightly）。

use super::*;

// ---------------------------------------------------------------------------
// 纯函数裁决（全平台；合成输入覆盖三臂，None 臂在 Windows 上只能这样锁）
// ---------------------------------------------------------------------------

#[test]
fn verdict_success_is_running() {
    assert_eq!(
        startup_confirm_verdict(true, Some(0)),
        StartupVerdict::Running
    );
}

#[test]
fn verdict_signal_death_is_not_success() {
    // 核心：旧形态 `success() || code().is_none()` 把 (false, None)
    // 并进成功臂——修复后必须 SignalDeath。
    assert_eq!(
        startup_confirm_verdict(false, None),
        StartupVerdict::SignalDeath
    );
    assert_ne!(
        startup_confirm_verdict(false, None),
        StartupVerdict::Running
    );
}

#[test]
fn verdict_failed_code_is_failed() {
    assert_eq!(
        startup_confirm_verdict(false, Some(1)),
        StartupVerdict::Failed(1)
    );
    assert_eq!(
        startup_confirm_verdict(false, Some(137)),
        StartupVerdict::Failed(137)
    );
}

// ---------------------------------------------------------------------------
// 真进程 e2e（unix 门控：Windows code() 恒 Some，None 臂真实进程不可达）
// ---------------------------------------------------------------------------

#[cfg(unix)]
fn ctx() -> crate::context::RequestContext {
    crate::context::RequestContext::new("web", "chat1", "user1", "sess1")
}

#[cfg(unix)]
fn tool() -> AsyncExecTool {
    let dir = std::env::temp_dir().join(format!("nb-exec-async-sig-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    AsyncExecTool::new(&dir.to_string_lossy(), false)
}

#[cfg(unix)]
#[tokio::test]
async fn signal_death_within_startup_window_reports_premature_exit() {
    // `kill -SEGV $$`：sh 自杀（SIGSEGV）→ ExitStatus code None。
    // 旧形态 `|| code().is_none()` 把该结果当「started and confirmed
    // running」上报（假成功——agent 以为长驻进程在跑，实际启动即崩）；
    // 修复后必须 Err 且点名信号终止。不取 -9 是为绕开 deny 词表
    // `\bkill\s+-[9]\b`（`kill -SEGV` 不命中词表）。
    let t = tool();
    let err = t
        .execute(r#"{"command":"kill -SEGV $$","wait_seconds":1}"#, &ctx())
        .await
        .err()
        .expect("信号终止必须 Err，不得谎报运行中");
    assert!(
        err.contains("terminated by signal"),
        "错误消息必须点名信号终止: {err}"
    );
    assert!(
        !err.contains("confirmed running"),
        "不得保留假成功文案: {err}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn quick_clean_exit_within_window_keeps_ok_semantics() {
    // 对照：wait_seconds 内退出 0（真快退）保持合法完成语义——修复
    // 只收窄 None 臂，不动 success 臂与 timeout Err 臂。
    let t = tool();
    let ok = t
        .execute(r#"{"command":"true","wait_seconds":1}"#, &ctx())
        .await
        .expect("true 必须成功");
    assert!(ok.contains("confirmed running"), "快退 0 保持原消息: {ok}");
}
