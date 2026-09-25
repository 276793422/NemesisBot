// cli_child.rs 覆盖率补充测试（无 piped stdin 的回落臂 28）。

use super::*;
use std::process::Stdio;

/// 未配置 piped stdin（child.stdin = None）→ 跳过 stdin 写入块直取输出
/// （28 的回落路径）。
#[tokio::test]
async fn run_without_piped_stdin_still_collects_output() {
    let mut cmd = tokio::process::Command::new("cmd");
    cmd.args(["/C", "echo", "hi-from-cov"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = run_with_stdin(cmd, "ignored-prompt").await.unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("hi-from-cov"), "{stdout}");
}
