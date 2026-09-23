//! CLI 型 provider 的子进程公共件：spawn → stdin 写入 → EOF → 收集输出。
//!
//! 2026-09-23 BUG 修复（docs/BUG/2026-09-03_cli-delegation-prompt-never-written-to-stdin.md）：
//! 此前 claude/codex 两个 CLI provider 用 `.output()` 一步到位——stdin 只
//! `Stdio::piped()` 却**从不写入**（prompt 构建后 `let _prompt = …` 死绑定
//! 丢弃），而命令行又传了 `-`（从 stdin 读 prompt）→ 子进程永远等不到
//! EOF，挂到外层超时。本 helper 是修复后的唯一路径：spawn、写 prompt、
//! 关 stdin（EOF 触发 CLI 开始处理）、收集 stdout/stderr。

/// Spawn `cmd`（调用方预配 `Stdio::piped()` 等管道），把 `prompt` 写进
/// stdin 后关闭管道（EOF），收集输出。
///
/// 子进程提前退出（没读完 stdin 就死了）时 `write_all` 可能因管道断裂
/// 失败——那是子进程崩溃的**表征**不是本函数的错误：忽略写失败，让
/// 退出码/stderr 诚实上报。
pub async fn run_with_stdin(
    mut cmd: tokio::process::Command,
    prompt: &str,
) -> std::io::Result<std::process::Output> {
    let mut child = cmd.spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        // 写完 shutdown + drop = 关闭写端，CLI 的「从 stdin 读 prompt」
        // 协议依赖这个 EOF 才开始处理。
        let _ = stdin.write_all(prompt.as_bytes()).await;
        let _ = stdin.shutdown().await;
        drop(stdin);
    }
    child.wait_with_output().await
}

#[cfg(test)]
mod tests;
