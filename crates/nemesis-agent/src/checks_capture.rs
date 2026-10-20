//! 检查类工具调用的交付证据捕获（诚实边界收口 ②，2026-10-19）。
//!
//! F5 证据闸只能证「交付点存在」；typed `checks` 块补「检查真跑过、过了没
//! 有」的客观记录。捕获 = **管线侧确定性记账**（不是 agent 自述）：
//! `AgentLoop` 的工具分发漏斗（`handle_tool_call_at_depth`）在检查类工具
//! 真实执行后，把「完整命令 + 成败 + 结果摘要」记进会话键对应的槽；集群
//! worker 在任务 run 前后 begin/take，随交付 payload 上行。
//!
//! 归属模型：槽表按 **session_key 索引**（并发任务各归各账，互不串窗口）；
//! begin 即建槽、take 即摘槽，任务结束槽必被摘（漏摘被同会话下一次 begin
//! 覆盖自愈，内存占用有界）。**子代理调用不入主任务窗口**：detached 轮次
//! 的 dispatch context 用独立会话键（`subagent:{uuid}`，turn.rs
//! `detached_session_key`），槽表按主任务键查必然 miss——子代理跑的检查
//! 不进 checks 块（**欠报 = 诚实方向**；主任务级的检查以主循环调用为准，
//! 会话键穿透归属属后续增强，不在本步）。
//!
//! 诚实边界：
//! - **被闸拒绝的调用不入账**（急停/hidden/allow/min-tier/计划模式/限额
//!   拒绝/安全拦截）——记进去会谎称「检查跑了且失败」；只有真实执行过的
//!   调用才进证据（[`classify_result`] 单点分类）。
//! - `ok` 按工具类选**精确判别器**（R4 修复，2026-10-20）：`exec` = 结果
//!   前缀判据（`Exit code:`/`Command timed out` = 失败，与
//!   `exec_output_passed` 单一真相源）；`run_checks` = 阶段状态行判据
//!   （行首 `✗ `/`⏱ ` = 有失败阶段）；其余检查类工具无结构化退码才落
//!   `Tool error:` 前缀兜底。旧「结果不含 `Tool error:` 即成功」的
//!   contains 启发式已废——非零退出（`Exit code: 7` 形态）被记成成功、
//!   成功 stdout 中途打印 `Tool error:` 被记成失败，两类误判都出自它；
//!   评审侧把 `ok` 当信号不当承诺的纪律不变。
//! - args 解析失败 / `command` 字段缺失 = 调用根本没执行成功，不造值（不
//!   入账）。
//! - `exec_async`（异步 shell）**不在捕获名单**：异步结果未到账即捕获 =
//!   谎称已验；任务用它跑检查时的诚实形态是等价同步 exec 或人工核对。

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

/// 检查类工具名单（捕获过滤单一真相源）。`exec` = 自由命令；`run_checks`
/// = 固定命令表（scope/filter 渲染）。`exec_async` 不在名单（见模块头诚实
/// 边界）。
pub const CHECKS_CLASS_TOOLS: [&str; 2] = ["exec", "run_checks"];

/// summary 预览字符上限（证据 payload 防膨胀；结果全文已在 spill/审计链，
/// 这里只需「过了没有 + 失败长什么样」）。
pub const SUMMARY_CAP: usize = 500;

/// 单条检查证据记录（wire 形态 = `$.delivery.checks[]` 条目）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct CheckRecord {
    /// 工具名（`exec` / `run_checks`）。
    pub tool: String,
    /// 完整命令（exec = `command` 字段原文未截断；run_checks = scope/filter
    /// 渲染的调用摘要——固定命令表的内部展开是实现细节，调用本身是证据）。
    pub cmd: String,
    /// 成败判别（按工具类选精确判别器，见模块头诚实边界；`false` = 跑了且
    /// 失败——失败也是证据）。
    pub ok: bool,
    /// 结果摘要（≤[`SUMMARY_CAP`] 字符，chars 安全截断）。
    pub summary: String,
}

/// 捕获槽（单会话的记录列表，begin→take 生命周期）。
pub type ChecksSink = Arc<Mutex<Vec<CheckRecord>>>;

/// 会话键 → 捕获槽表（AgentLoop 持有；并发任务按 session_key 精确归属）。
pub type ChecksCaptureMap = RwLock<HashMap<String, ChecksSink>>;

/// 工具是否在捕获名单。
pub fn is_checks_class(tool: &str) -> bool {
    CHECKS_CLASS_TOOLS.as_slice().contains(&tool)
}

/// 从工具 args（JSON 字符串）提取命令证据。解析失败 / 必要字段缺失 =
/// `None`（调用没执行成功，不造值）。
pub fn extract_cmd(tool: &str, arguments: &str) -> Option<String> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(arguments) else {
        return None;
    };
    match tool {
        "exec" => val
            .get("command")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        "run_checks" => {
            let scope = val
                .get("scope")
                .and_then(|v| v.as_str())
                .unwrap_or("all")
                .to_lowercase();
            let mut cmd = format!("run_checks scope={scope}");
            if let Some(f) = val.get("filter").and_then(|v| v.as_str()) {
                cmd.push_str(&format!(" filter={f}"));
            }
            Some(cmd)
        }
        _ => None,
    }
}

/// run_checks 成败判据：回复的阶段状态行行首 `✗ `（exit 非零 / spawn
/// 失败）或 `⏱ `（超时）任一在场 = 有失败阶段。只认**行首**——聚焦摘录
/// 区缩进回显的 stdout 内容（`  ` 前缀）不会误判。判定器与 RunChecksTool
/// 的状态行形态（`▶ `/`✗ `/`⏱ `）单点对齐。
pub(crate) fn run_checks_passed(result: &str) -> bool {
    !result
        .lines()
        .any(|l| l.starts_with("✗ ") || l.starts_with("⏱ "))
}

/// 按工具类选成败判别器（前置条件：拒绝族已排除、`Tool error:` 前缀已
/// 判负）。`exec` = [`crate::loop_tools::exec_output_passed`]（前缀精确
/// 判据，单一真相源直用）；`run_checks` = [`run_checks_passed`]；其余
/// 工具无结构化退码，默认成功。
pub(crate) fn classify_by_tool(tool: &str, result: &str) -> bool {
    match tool {
        "exec" => crate::loop_tools::exec_output_passed(result),
        "run_checks" => run_checks_passed(result),
        _ => true,
    }
}

/// 结果分类（拒绝/执行单点判定）：`Some(ok)` = 工具真实执行过（ok 按工具
/// 类选精确判别器）；`None` = 被闸拒绝未执行（不入账）。
///
/// 拒绝文案族（tool_dispatch.rs 各闸的稳定形态，2026-10-19 逐点核对）：
/// `⛔` 前缀族（ESTOP / RATE LIMIT 拒绝）、`Plan mode:` 写闸、
/// `Error: Tool '<名>' ...`（hidden / allow / min-tier 三闸同前缀——工具
/// 执行结果不会长这样，工具失败是 `Tool error: ...`）、`SECURITY BLOCKED`
/// （安全层/guardian 拦截，contained 形态）。
///
/// R4（2026-10-20）：`Tool error:` 从 contains 改 **starts_with**——工具
/// 返回 Err 时 dispatch 层恒以该前缀开头（执行过但失败的形态），而成功
/// 输出**中途**出现该字样（如测试断言里引用错误文案）不应误判失败。
pub fn classify_result(tool: &str, result: &str) -> Option<bool> {
    if result.starts_with('⛔') || result.starts_with("Plan mode:") {
        return None;
    }
    if result.contains("SECURITY BLOCKED") {
        return None;
    }
    if result.starts_with("Error: Tool '") {
        return None;
    }
    if result.starts_with("Tool error:") {
        return Some(false);
    }
    Some(classify_by_tool(tool, result))
}

/// 按字符数截断（byte 切片会在多字节字符边界 panic；与 tool_event_hook 的
/// truncate_chars 同款形态——crate 边界各自持有，不跨模块引私有 fn）。
pub fn truncate_chars(s: &str, cap: usize) -> String {
    if s.chars().count() <= cap {
        return s.to_string();
    }
    let mut out: String = s.chars().take(cap).collect();
    out.push('…');
    out
}

/// 漏斗侧记账入口（`handle_tool_call_at_depth` 包装层调用）：检查类工具 +
/// 会话槽在场 + 命令可提取 + 结果非拒绝 → 入账。任一不满足 = no-op（诚实
/// 缺席）。注意此函数在**每次**检查类工具调用时都会走：查表 miss（绝大多数
/// 非 worker 任务）只有一次 RwLock 读 + 名单比对，零分配。
pub fn record_check(
    map: &ChecksCaptureMap,
    session_key: &str,
    tool: &str,
    arguments: &str,
    result: &str,
) {
    if !is_checks_class(tool) {
        return;
    }
    let sink = map.read().get(session_key).cloned();
    let Some(sink) = sink else {
        return;
    };
    let Some(cmd) = extract_cmd(tool, arguments) else {
        return;
    };
    let Some(ok) = classify_result(tool, result) else {
        return;
    };
    sink.lock().push(CheckRecord {
        tool: tool.to_string(),
        cmd,
        ok,
        summary: truncate_chars(result, SUMMARY_CAP),
    });
}

#[cfg(test)]
mod tests;
