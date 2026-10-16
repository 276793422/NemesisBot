//! E5（2026-10-15 收尾保障）：预算暂停前收尾编译探针。
//!
//! 首轮产品验证实验 3 个 FAIL 中 2 个同族：预算暂停把「编辑」与「复验」
//! 切开，破损代码留在工作树（CKPTGC：必红测试写了没跑；HTTPARSE r3：最后
//! 一轮编辑打坏宏，库不编译）。本器官在暂停事件推送前跑一次快速编译检查，
//! 把树状态诚实注记进暂停消息——**只报告不回滚**（用户拍板 2026-10-15）。
//!
//! 触发闸（全部满足才跑，任一不满足零副作用直通）：
//! 1. `prepare_round` 判定预算暂停（reason = budget_exhausted / max_turns，
//!    cron / detached / 常驻三条路都算——树状态对三者同等重要）；
//! 2. 本轮发生过写类工具成功（[`crate::turn_guard::TurnGuard::had_writes`]）；
//! 3. config `agents.defaults.pre_pause_probe.enabled`（默认开，新鲜读）；
//! 4. workspace_root 已装配且命中已知生态标记（无标记=非编码工作区，静默）。
//!
//! 诚实边界：探针只验「编译可过」，不等价「验收会过」（能编译的语义错误
//! 抓不到）；探针在暂停事件推送前同步执行，Done 晚到最多 timeout_secs。
#[cfg(not(target_os = "linux"))]
use super::prelude::*;
use super::*;
use std::path::Path;

/// 探针摘录上限（字符）——暂停消息里的首个错误行，超长截断。
pub(crate) const PROBE_EXCERPT_MAX_CHARS: usize = 240;

/// 探针结果分类（[`run_probe`] 产出，[`probe_note`] 消费）。
#[derive(Debug)]
pub(crate) enum ProbeOutcome {
    /// 编译检查通过（exit 0）。
    Passed {
        marker: &'static str,
        command: &'static str,
        secs: u64,
    },
    /// 编译检查失败（exit 非零）——`excerpt` 为首个错误行摘录（可为空：
    /// exit 非零但无 error 签名行，注记退化为通用文案）。
    Failed {
        marker: &'static str,
        command: &'static str,
        excerpt: String,
    },
    /// 超时中止——树状态未验证（诚实：不知好坏）。
    TimedOut {
        marker: &'static str,
        command: &'static str,
        timeout_secs: u64,
    },
    /// 检查命令无法启动（工具链不在场等）——诚实跳过，不装检查过。
    ToolMissing {
        marker: &'static str,
        command: &'static str,
    },
}

/// 按工作区根的标记文件选探针命令（首个命中决定，与 loop_tools
/// `CHECK_ECOSYSTEMS` 同序）。返回 `(marker, command)`。
///
/// 故意**不做**的生态：node（JS 无通用编译闸；tsc 只覆盖 TS 且 npx 形态
/// 有网络拉取副作用）/ python（无编译期）——与 run_checks 的「不做映射就
/// 诚实回『未映射』」同纪律，不硬猜命令。
///
/// cargo 用 `--all-targets` 连测试一起编（CKPTGC 失败形态=测试文件红，纯
/// `cargo check` 抓不到）+ `--message-format=short`（错误行可机读摘录）。
pub(crate) fn select_probe_command(root: &Path) -> Option<(&'static str, &'static str)> {
    if root.join("Cargo.toml").is_file() {
        Some((
            "Cargo.toml",
            "cargo check --all-targets --message-format=short",
        ))
    } else if root.join("go.mod").is_file() {
        Some(("go.mod", "go build ./..."))
    } else if root.join("pom.xml").is_file() {
        Some(("pom.xml", "mvn -q -DskipTests compile"))
    } else {
        None
    }
}

/// 跑探针：复用 [`crate::loop_tools::run_one_stage`] 执行内核（含 2026-10-15
/// 超时树杀语义——收尸连 cargo/rustc 孙树一起收）。无生态标记 → None（静默）。
pub(crate) async fn run_probe(root: &Path, timeout_secs: u64) -> Option<ProbeOutcome> {
    let (marker, command) = select_probe_command(root)?;
    let started = std::time::Instant::now();
    let (code, stdout, stderr, timed_out) =
        crate::loop_tools::run_one_stage(command, Some(root), timeout_secs).await;
    Some(match (code, timed_out) {
        (_, true) => ProbeOutcome::TimedOut {
            marker,
            command,
            timeout_secs,
        },
        (Some(0), false) => ProbeOutcome::Passed {
            marker,
            command,
            secs: started.elapsed().as_secs(),
        },
        (Some(_), false) => ProbeOutcome::Failed {
            marker,
            command,
            excerpt: first_error_excerpt(&stdout, &stderr),
        },
        (None, false) => ProbeOutcome::ToolMissing { marker, command },
    })
}

/// 首个错误行摘录（stderr 优先，宽匹配 "error" 大小写不敏感——cargo short
/// / go build / mvn / tsc 的错误行都含该词），截 [`PROBE_EXCERPT_MAX_CHARS`]
/// 字符。无命中 → 空串（调用方退化通用文案）。
pub(crate) fn first_error_excerpt(stdout: &str, stderr: &str) -> String {
    for hay in [stderr, stdout] {
        if let Some(line) = hay
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty() && l.to_ascii_lowercase().contains("error"))
        {
            return line.chars().take(PROBE_EXCERPT_MAX_CHARS).collect();
        }
    }
    String::new()
}

/// 探针结果 → 暂停消息追加文本（每条都以 `\n\n` 起头；只报告不回滚——
/// 不改判终局、不动工作树）。
pub(crate) fn probe_note(outcome: &ProbeOutcome) -> String {
    match outcome {
        ProbeOutcome::Passed { command, secs, .. } => {
            format!("\n\n📋 收尾探针：编译检查通过（{command}，{secs}s），工作树处于可编译状态。")
        }
        ProbeOutcome::Failed {
            command, excerpt, ..
        } => {
            if excerpt.is_empty() {
                format!(
                    "\n\n⚠️ 收尾探针：编译检查失败（{command}，exit 非零），工作树当前不可编译，续跑时请先修复编译错误。"
                )
            } else {
                format!(
                    "\n\n⚠️ 收尾探针：编译检查失败（{command}），工作树当前不可编译。首个错误：{excerpt} —— 续跑时请先修复。"
                )
            }
        }
        ProbeOutcome::TimedOut {
            command,
            timeout_secs,
            ..
        } => format!(
            "\n\n⚠️ 收尾探针：编译检查超过 {timeout_secs}s 未完成（{command}），已中止；工作树状态未验证。"
        ),
        ProbeOutcome::ToolMissing { marker, command } => format!(
            "\n\n📋 收尾探针：检测到 {marker} 但无法运行检查命令（{command}），已跳过编译检查。"
        ),
    }
}

impl AgentLoop {
    /// 预算暂停 Done 事件的收尾探针注记（接线器官；四道触发闸任一不满足
    /// 即零副作用直通，暂停消息一字不动）。
    pub(crate) async fn attach_pre_pause_probe_note(&self, ev: &mut AgentEvent, st: &TurnState) {
        let AgentEvent::Done(text) = ev else {
            return;
        };
        if !st.turn_guard.had_writes() {
            return;
        }
        let settings = self.current_pre_pause_probe();
        if !settings.enabled {
            return;
        }
        let Some(root) = self.workspace_root() else {
            return;
        };
        let Some(outcome) = run_probe(&root, settings.timeout_secs).await else {
            return;
        };
        info!(
            "[AgentLoop] pre-pause probe done (marker={}), attaching note",
            match &outcome {
                ProbeOutcome::Passed { marker, .. }
                | ProbeOutcome::Failed { marker, .. }
                | ProbeOutcome::TimedOut { marker, .. }
                | ProbeOutcome::ToolMissing { marker, .. } => marker,
            }
        );
        text.push_str(&probe_note(&outcome));
    }
}
