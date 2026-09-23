//! cli_child 真子进程测试：stdin 送达 + EOF 收尾 + 早退容错。
//!
//! 回显命令跨平台选择（不引入外部依赖、不碰 .cmd）：
//! - unix: `sh -c 'cat'`（cat 原样回显，EOF 退出——若 stdin 未关闭则
//!   永远不退，timeout 守卫会让测试红而不是挂）；
//! - windows: `cmd /c findstr NEMESIS_STDIN_PROBE`（findstr 从 stdin 逐行
//!   读、回显含模式的行，同样 EOF 退出）。CRLF/模式过滤导致字节级回显
//!   不可比，故 windows 只断言哨兵行全部送达。

use super::*;
use std::process::Stdio;

const PROBE_A: &str = "NEMESIS_STDIN_PROBE_ALPHA_7f3a";
const PROBE_B: &str = "NEMESIS_STDIN_PROBE_BETA_c21e";

fn echo_command() -> tokio::process::Command {
    let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        cmd.args(["/c", "findstr", "NEMESIS_STDIN_PROBE"]);
    } else {
        cmd.args(["-c", "cat"]);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd
}

/// stdin 全量送达 + 写完关管道触发 EOF：子进程在 timeout 内自然退出。
/// （修复前等价形态：stdin 只 pipe 不写 → 子进程等 EOF 到超时 → 这里红。）
#[tokio::test]
async fn stdin_is_delivered_and_eof_terminates_child() {
    let prompt = format!("{PROBE_A}\n{PROBE_B}\n");
    let fut = run_with_stdin(echo_command(), &prompt);
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
        .await
        .expect("child must terminate after stdin EOF (timeout = EOF never delivered)")
        .expect("spawn failed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains(PROBE_A),
        "probe A missing from stdout: {stdout:?}"
    );
    assert!(
        stdout.contains(PROBE_B),
        "probe B missing from stdout: {stdout:?}"
    );
    if cfg!(not(windows)) {
        // cat 是字节级回显：非 windows 平台钉死逐字节一致（含换行形态）。
        assert_eq!(stdout, prompt);
    }
}

/// 子进程提前退出（根本不读 stdin）：写端断裂是子进程死亡的表征，
/// 必须被忽略——函数照常返回输出，退出码诚实上报，不挂不 panic。
#[tokio::test]
async fn early_exiting_child_does_not_hang_or_error() {
    let mut cmd = tokio::process::Command::new(if cfg!(windows) { "cmd" } else { "sh" });
    if cfg!(windows) {
        cmd.args(["/c", "exit", "3"]);
    } else {
        cmd.args(["-c", "exit 3"]);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let fut = run_with_stdin(cmd, "nobody will read this");
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), fut)
        .await
        .expect("early-exiting child must not hang")
        .expect("wait_with_output failed");

    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(3));
}
