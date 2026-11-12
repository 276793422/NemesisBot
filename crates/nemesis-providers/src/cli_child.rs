//! CLI 型 provider 的子进程公共件：spawn → stdin 写入 → EOF → 收集输出。
//!
//! 2026-09-23 BUG 修复（cli-delegation-prompt-never-written-to-stdin）：
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
    // 超时弃 future = drop Child → 必须 kill_on_drop（BUG-20261111-082）：
    // agent loop 首捕统一包 call_timeout（chat_call_escalating →
    // chat_call_bounded 的 tokio::time::timeout），超时即取消本 future——
    // 无此标志时 CLI 子进程（claude/codex）成孤儿继续跑：拿着对话内容
    // 在后台烧 API token，stdout 管道无人读、缓冲写满后子进程自己阻塞
    // 永不退出；transient 重试还会逐次叠加新孤儿。正常完成路径不受影响
    // （wait_with_output 已收尸，drop 对已退出进程是 no-op）。同族先例：
    // cc_hooks.rs / middleware.rs run_shell_bounded / exec.rs。
    cmd.kill_on_drop(true);
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
mod cov_tests;
#[cfg(test)]
mod tests;
