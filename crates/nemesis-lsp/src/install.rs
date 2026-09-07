//! C6（devtool-upgrade 阶段 6）：LSP 服务器自举——安装命令目录 + 后台安装执行器。
//!
//! 两条安装通路，信任级不同：
//! - **一键安装**（dashboard `coding.lsp_install`）走 AgentLoop 的 exec
//!   dispatch（安全 8 层 + 审批卡 + executor 隔离），本模块只供命令目录；
//! - **静默自举**（`agents.lsp_tool.auto_install=true` 时网关启动期后台
//!   执行 [`auto_install_missing`]）——用户显式配置 = standing consent，
//!   与 A6 formatter / LSP 服务器 spawn 同信任级（基础设施装配，不走 8 层）。
//!
//! 命令形态：目录存 program+args（不进 shell——目录是常量，无需引号解析；
//! Windows 上 npm 是 .cmd 不是 PE，须 `cmd /C` 包装）。rust-analyzer 的
//! 官方二进制下载形态后置（rustup 组件安装已覆盖主流场景）。

use std::time::Duration;

use tracing::{info, warn};

use crate::registry::{Lang, SERVERS, server_available};

/// 一个语言的安装命令（平台相关，调用时解析）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallCommand {
    /// 用户可读/可复制的完整命令串（与 `program`/`args` 等价的 shell 形态）。
    pub display: String,
    /// 实际 spawn 的程序。Windows 上 npm 走 `cmd /C` 包装（.cmd 不是 PE，
    /// CreateProcess 无法直接执行）；其余平台直呼。
    pub program: String,
    pub args: Vec<String>,
    /// true = 需要交互终端（sudo 密码等）——一键执行诚实拒绝，只给复制。
    pub needs_interactive: bool,
}

/// 平台相关安装命令目录。五个注册语言全覆盖；返回的命令都是官方安装
/// 通道（rustup / go install / npm / winget / apt），无第三方脚本。
pub fn install_command(lang: Lang) -> InstallCommand {
    // (display, program, args, needs_interactive) 四元组按平台展开。
    let (display, program, args, needs_interactive): (&str, &str, Vec<&str>, bool) = match lang {
        Lang::Rust => (
            "rustup component add rust-analyzer",
            "rustup",
            vec!["component", "add", "rust-analyzer"],
            false,
        ),
        Lang::Go => (
            "go install golang.org/x/tools/gopls@latest",
            "go",
            vec!["install", "golang.org/x/tools/gopls@latest"],
            false,
        ),
        Lang::TypeScript if cfg!(windows) => (
            "npm install -g typescript typescript-language-server",
            "cmd",
            vec![
                "/C",
                "npm",
                "install",
                "-g",
                "typescript",
                "typescript-language-server",
            ],
            false,
        ),
        Lang::TypeScript => (
            "npm install -g typescript typescript-language-server",
            "npm",
            vec!["install", "-g", "typescript", "typescript-language-server"],
            false,
        ),
        // pyright-langserver 由 npm 包 `pyright` 提供（官方发行通道）。
        Lang::Python if cfg!(windows) => (
            "npm install -g pyright",
            "cmd",
            vec!["/C", "npm", "install", "-g", "pyright"],
            false,
        ),
        Lang::Python => (
            "npm install -g pyright",
            "npm",
            vec!["install", "-g", "pyright"],
            false,
        ),
        // clangd 无跨平台包管理命令：Windows 走 winget（非交互），
        // Unix 需 sudo（交互终端）——一键执行诚实拒绝。
        Lang::C if cfg!(windows) => (
            "winget install --id LLVM.LLVM -e --accept-source-agreements --accept-package-agreements",
            "winget",
            vec![
                "install",
                "--id",
                "LLVM.LLVM",
                "-e",
                "--accept-source-agreements",
                "--accept-package-agreements",
            ],
            false,
        ),
        Lang::C => (
            "sudo apt-get install -y clangd",
            "sudo",
            vec!["apt-get", "install", "-y", "clangd"],
            true,
        ),
    };
    InstallCommand {
        display: display.to_string(),
        program: program.to_string(),
        args: args.into_iter().map(str::to_string).collect(),
        needs_interactive,
    }
}

/// 静默自举的待安装清单：probe `SERVERS` → 缺失 ∧ 非交互。
/// 纯函数（probe 是即时的 PATH 查找），消费方与测试共用。
pub fn pending_auto_installs() -> Vec<(Lang, InstallCommand)> {
    SERVERS
        .iter()
        .map(|s| s.lang)
        .filter(|l| !server_available(*l))
        .map(|l| (l, install_command(l)))
        .filter(|(_, c)| !c.needs_interactive)
        .collect()
}

/// 单次命令执行的原始输出（截尾保留）。
#[derive(Debug, Clone)]
struct RunOutput {
    success: bool,
    exit_code: Option<i32>,
    /// stdout+stderr 合并尾部（≤4KB，char 边界安全截断）。
    output_tail: String,
}

/// spawn 并等待一个进程（piped 捕获 + 超时 + kill_on_drop）。
///
/// 管道排水与 wait 并发（loop_tools ExecTool ② 注记同款纪律：不能把管道
/// 移进被 timeout 包住的 future——超时取消时管道随 future 死、子进程写满
/// 管道缓冲即互相等死锁）。
async fn run_process(
    program: &str,
    args: &[String],
    timeout: Duration,
) -> Result<RunOutput, String> {
    use tokio::io::AsyncReadExt;

    let mut child = tokio::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("program not found: {program}（未安装或不在 PATH）")
            } else {
                format!("spawn {program} failed: {e}")
            }
        })?;

    let mut stdout = child.stdout.take().ok_or("stdout not captured")?;
    let mut stderr = child.stderr.take().ok_or("stderr not captured")?;
    // 独立任务排水（join 句柄在 wait 之后收——超时分支里任务随进程死，
    // read 返 Err 被吞掉即可，不影响诚实报错）。
    let out_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf).await;
        buf
    });
    let err_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf).await;
        buf
    });

    let status = tokio::time::timeout(timeout, child.wait())
        .await
        .map_err(|_| {
            format!(
                "install timed out after {}s（命令仍在后台会被杀掉；网络慢可重试）",
                timeout.as_secs()
            )
        })?
        .map_err(|e| format!("wait failed: {e}"))?;

    let mut combined = out_task.await.unwrap_or_default();
    combined.extend_from_slice(&err_task.await.unwrap_or_default());

    Ok(RunOutput {
        success: status.success(),
        exit_code: status.code(),
        output_tail: tail_string(&String::from_utf8_lossy(&combined), 4096),
    })
}

/// 保留尾部的 char 边界安全截断（str-slice 多字节 panic 家族防御）。
fn tail_string(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    // 从候选截断点向后扫到 char 边界——直接 `s[start..]` 在多字节字符
    // 中间会 panic（本函数要防的正是它，首版实现自己踩了，测试抓出）。
    let mut start = s.len() - max_bytes;
    while !s.is_char_boundary(start) {
        start -= 1;
    }
    format!("…{}", &s[start..])
}

/// 一键安装 / 静默自举共用的单语言安装执行（piped、600s 默认超时）。
pub async fn run_install(cmd: &InstallCommand, timeout: Duration) -> Result<String, String> {
    let out = run_process(&cmd.program, &cmd.args, timeout).await?;
    if out.success {
        Ok(format!(
            "installed via `{}`\n{}",
            cmd.display,
            out.output_tail.trim()
        ))
    } else {
        Err(format!(
            "`{}` exited with {}:\n{}",
            cmd.display,
            out.exit_code
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into()),
            out.output_tail.trim()
        ))
    }
}

/// 静默自举入口（`agents.lsp_tool.auto_install=true` 时网关启动 spawn）：
/// 串行安装全部缺失的非交互语言服务器，逐条 info/warn 日志。装完后需
/// 重启 Agent 重新探测注册（既有的 documented 流程）。
pub async fn auto_install_missing(timeout: Duration) -> Vec<(Lang, Result<String, String>)> {
    let mut results = Vec::new();
    for (lang, cmd) in pending_auto_installs() {
        info!("auto-install: {} -> `{}`", lang.label(), cmd.display);
        let r = run_install(&cmd, timeout).await;
        match &r {
            Ok(out) => info!("auto-install: {} ok\n{}", lang.label(), out),
            Err(e) => warn!("auto-install: {} failed: {}", lang.label(), e),
        }
        results.push((lang, r));
    }
    results
}

#[cfg(test)]
mod tests;
