//! #16 提示词安全常态化触发 helper（W2 批三，2026-11-20 拍板）：人格资产
//! 变更（onboard 完成 / 人格编辑保存）后 detached spawn
//! `nemesisbot eval prompt <text>`（Sandboxie 沙盒内自评）。
//!
//! fire-and-forget：调用方（onboard / persona 保存链）不等待不感知结果
//! ——评估报告由子进程落 `<home>/workspace/logs/eval/<ts>_prompt/`，
//! 触发流水记 `<home>/workspace/logs/prompt_eval.log`（含诚实 SKIP 臂）。
//! 非 Windows = 恒 no-op + SKIP 落日志（eval 沙盒链是 Windows-only）。
//!
//! in-flight 去重：进程级 [`AtomicBool`] + 后台 wait 线程自动回臂——
//! 上一个自动评估还在跑时不重复 spawn（eval 全局互斥锁本就拒并发，这里
//! 提前挡掉是为了不制造必败子进程噪音）。手动 `nemesisbot eval prompt`
//! 不经本 helper，不受此限。

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};

/// 触发文本上限（28000 字符）：人格四文件（IDENTITY/SOUL/USER/AGENT）
/// 拼接远小于此；超限截断是多字节安全的（floor char boundary）。
pub const PROMPT_EVAL_MAX_CHARS: usize = 28_000;

/// 触发流水文件（`<home>` 相对段）。
const LOG_REL: &str = "workspace/logs/prompt_eval.log";

static EVAL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// 触发结果（调用方只做日志/统计投影，不做流程分支）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptEvalTrigger {
    /// 已 detached spawn（子进程自管生命周期与报告落盘）。
    Spawned,
    /// 诚实跳过（原因已投影进 prompt_eval.log）。
    Skipped(&'static str),
}

/// 截断点（多字节安全；与 eval_assessor 原语同实现——nemesis-utils 不
/// 依赖 nemesis-config，本地一份，单测钉死语义）。
fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else {
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}

/// 触发文本的规范形态（截断到 [`PROMPT_EVAL_MAX_CHARS`]）。
pub fn normalize_trigger_text(prompt: &str) -> &str {
    &prompt[..floor_char_boundary(prompt, PROMPT_EVAL_MAX_CHARS)]
}

/// 触发一次人格文本的沙盒安全评估（detached fire-and-forget）。
///
/// `home`：当前实例 home——原样经 `NEMESISBOT_HOME` env 传给子进程
/// （子进程按同一 home 解析 workspace/config 与报告目录；不用 `--local`：
/// env 注入是 exec 链的既有约定）。
pub fn trigger_prompt_eval(home: &Path, prompt: &str) -> PromptEvalTrigger {
    trigger_prompt_eval_inner(home, prompt, cfg!(target_os = "windows"))
}

fn trigger_prompt_eval_inner(
    home: &Path,
    prompt: &str,
    platform_supported: bool,
) -> PromptEvalTrigger {
    if !platform_supported {
        append_log(home, "SKIP platform: eval sandbox chain is Windows-only");
        return PromptEvalTrigger::Skipped("platform-unsupported");
    }
    if EVAL_IN_FLIGHT.swap(true, Ordering::AcqRel) {
        append_log(home, "SKIP busy: auto prompt-eval already in flight");
        return PromptEvalTrigger::Skipped("already-in-flight");
    }
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => {
            EVAL_IN_FLIGHT.store(false, Ordering::Release);
            append_log(home, &format!("SKIP current-exe-unresolved: {e}"));
            return PromptEvalTrigger::Skipped("current-exe-unresolved");
        }
    };
    let text = normalize_trigger_text(prompt);
    let mut cmd = std::process::Command::new(&exe);
    // stdout 只承载最终回复、日志一律 stderr——自动触发面两者都不接管道
    // （detached：不拖住调用方进程的管道，子进程报告自落盘）。
    cmd.args(["eval", "prompt", text])
        .env("NEMESISBOT_HOME", home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：绝不弹新控制台窗口（CLAUDE.md 后台进程纪律）。
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    match cmd.spawn() {
        Ok(mut child) => {
            append_log(
                home,
                &format!("START pid={} chars={}", child.id(), text.chars().count()),
            );
            // wait 线程：回收子进程 + 到期回臂（下次人格变更可再触发）。
            // 线程寿命 ≤ eval 单跑上限（默认 1800s 硬熔断），可接受。
            std::thread::spawn(move || {
                let _ = child.wait();
                EVAL_IN_FLIGHT.store(false, Ordering::Release);
            });
            PromptEvalTrigger::Spawned
        }
        Err(e) => {
            EVAL_IN_FLIGHT.store(false, Ordering::Release);
            append_log(home, &format!("SKIP spawn-failed: {e}"));
            PromptEvalTrigger::Skipped("spawn-failed")
        }
    }
}

/// 追加一行触发流水（`<home>/workspace/logs/prompt_eval.log`）。
/// 失败静默（触发流水是可观测性辅助，不是闸门——写日志失败不该打扰
/// onboard/persona 主流程）。
fn append_log(home: &Path, line: &str) {
    let path = home.join(LOG_REL);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ts = chrono::Utc::now().to_rfc3339();
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, format!("[{ts}] {line}\n").as_bytes()));
}

#[cfg(test)]
mod tests;
