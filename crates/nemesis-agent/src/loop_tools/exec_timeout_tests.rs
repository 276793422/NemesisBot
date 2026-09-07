//! B2（2026-09-04 devtool-upgrade 阶段 1）：exec 超时收尸保留残余输出 +
//! 默认/上限 cap 的单元与集成测试。

use crate::context::RequestContext;
use crate::r#loop::Tool;
use crate::loop_tools::ExecTool;

fn tool() -> ExecTool {
    ExecTool::new(".", false)
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "chat1", "user1", "sess1")
}

// ---------------------------------------------------------------------------
// exec_timeout_secs 纯函数层（cap 语义的编译器锚点）
// ---------------------------------------------------------------------------

#[test]
fn timeout_defaults_to_30() {
    assert_eq!(super::exec_timeout_secs(None), 30);
}

#[test]
fn timeout_passthrough_under_cap() {
    assert_eq!(super::exec_timeout_secs(Some(5)), 5);
    assert_eq!(super::exec_timeout_secs(Some(600)), 600);
}

#[test]
fn timeout_capped_at_600() {
    // 模型传 999999 不再被照单全收。
    assert_eq!(super::exec_timeout_secs(Some(999_999)), 600);
    assert_eq!(super::exec_timeout_secs(Some(u64::MAX)), 600);
}

// ---------------------------------------------------------------------------
// tail_chars 纯函数层（多字节安全尾部截取）
// ---------------------------------------------------------------------------

#[test]
fn tail_chars_short_input_passthrough() {
    assert_eq!(super::tail_chars("hello", 10), "hello");
}

#[test]
fn tail_chars_keeps_last_n_chars() {
    assert_eq!(super::tail_chars("abcdefgh", 3), "fgh");
}

#[test]
fn tail_chars_multibyte_safe() {
    // 中文字符按 char 切，不按字节切（str 字节切片会 panic）。
    let s = "一二三四五";
    assert_eq!(super::tail_chars(s, 2), "四五");
}

// ---------------------------------------------------------------------------
// execute 集成层：超时路径
// ---------------------------------------------------------------------------

/// 平台各自的「先输出一行再死循环」命令——验证超时后残余 stdout 可读回。
///
/// 【为什么不能用 ping/sleep 等外部命令】exec 在 Windows 经 `cmd /C` 包裹、
/// Linux 经 `sh -c` 包裹，外部命令是 shell 的**孙进程**且以 bInheritHandles
/// 复制了包括测试 harness stdout 捕获管道在内的全部可继承句柄（`>nul` 重定向
/// 只改 std 句柄不影响句柄表继承）。kill 只杀 shell 不杀孙进程——孙进程活
/// 多久，harness 就等多久（本机 127.0.0.1 ICMP 限流时 `ping -n 30` 实测
/// ~148s，测试看似挂死）。纯 shell 内建死循环无孙进程：kill shell 即全部
/// 句柄关闭，无孤儿。
#[cfg(windows)]
const PARTIAL_CMD: &str = "echo before-sleep & for /l %i in (0,0,1) do @rem";
#[cfg(not(windows))]
const PARTIAL_CMD: &str = "echo before-sleep; while :; do :; done";

/// 纯内建死循环、无任何输出——验证超时零残余的诚实文案。
#[cfg(windows)]
const SPIN_CMD: &str = "for /l %i in (0,0,1) do @rem";
#[cfg(not(windows))]
const SPIN_CMD: &str = "while :; do :; done";

#[tokio::test]
async fn timeout_returns_partial_output_as_ok() {
    let out = tool()
        .execute(
            &serde_json::json!({"command": PARTIAL_CMD, "timeout": 1}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok (model-repairable), not Err");
    assert!(
        out.contains("timed out after 1"),
        "should report timeout: {out}"
    );
    // 非空洞断言：该序列只可能来自 stdout 残余段（「Command was:」回显在
    // 消息末尾，构不成这个前缀序列）。
    assert!(
        out.contains("Partial output:\nstdout (tail):\nbefore-sleep"),
        "residual stdout before the hang must survive: {out}"
    );
    assert!(out.contains("Tip:"), "should carry repair tip: {out}");
    assert!(out.contains("Command was:"), "should echo command: {out}");
}

#[tokio::test]
async fn timeout_with_no_output_reports_none() {
    let out = tool()
        .execute(
            &serde_json::json!({"command": SPIN_CMD, "timeout": 1}).to_string(),
            &ctx(),
        )
        .await
        .expect("timeout must be Ok");
    assert!(
        out.contains("(no output before timeout)"),
        "empty residual must say so: {out}"
    );
}

// ---------------------------------------------------------------------------
// execute 集成层：正常路径回归（消息字节不变）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn normal_success_path_unchanged() {
    #[cfg(windows)]
    let cmd = "echo hello-b2";
    #[cfg(not(windows))]
    let cmd = "echo hello-b2";
    let out = tool()
        .execute(&serde_json::json!({"command": cmd}).to_string(), &ctx())
        .await
        .expect("must succeed");
    assert_eq!(out.trim(), "hello-b2", "success message unchanged: {out}");
}

#[tokio::test]
async fn normal_failure_path_unchanged() {
    #[cfg(windows)]
    let cmd = "cmd /c exit 7";
    #[cfg(not(windows))]
    let cmd = "exit 7";
    let out = tool()
        .execute(&serde_json::json!({"command": cmd}).to_string(), &ctx())
        .await
        .expect("failure is Ok-shaped (exit code report)");
    assert!(
        out.starts_with("Exit code: 7"),
        "failure format unchanged: {out}"
    );
}

#[tokio::test]
async fn large_output_pipe_not_deadlocked() {
    // B2 设计点回归：并发排干管道——输出超 ~64KB 管道缓冲时子进程不能被
    // 阻塞成假超时（只 wait 不读的老毛病）。
    #[cfg(windows)]
    let cmd = "powershell -NoProfile -Command \"1..20000 | ForEach-Object { \\\"line-{0:00000}\\\" -f $_ }\"";
    #[cfg(not(windows))]
    let cmd = "seq 1 20000 | sed 's/^/line-/'";
    let out = tool()
        .execute(
            &serde_json::json!({"command": cmd, "timeout": 60}).to_string(),
            &ctx(),
        )
        .await
        .expect("large output must complete, not deadlock into timeout");
    assert!(
        !out.contains("timed out"),
        "must NOT degrade into a timeout: {}..",
        &out.chars().take(200).collect::<String>()
    );
    assert!(out.contains("line-20000"), "tail of large output present");
}
