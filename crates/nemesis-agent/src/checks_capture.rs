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
//! - `ok` 是**启发式**（与 `ToolEventHook` 同款：结果不含 `Tool error:` /
//!   `SECURITY BLOCKED` 即视为成功）——工具账本层没有结构化退码，这是该层
//!   能拿到的全部信息；评审侧把 `ok` 当信号不当承诺。
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
    /// 成败启发式（见模块头诚实边界；`false` = 跑了且失败——失败也是证据）。
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

/// 结果分类（拒绝/执行单点判定）：`Some(ok)` = 工具真实执行过（ok = 与
/// `ToolEventHook` 同款启发式）；`None` = 被闸拒绝未执行（不入账）。
///
/// 拒绝文案族（tool_dispatch.rs 各闸的稳定形态，2026-10-19 逐点核对）：
/// `⛔` 前缀族（ESTOP / RATE LIMIT 拒绝）、`Plan mode:` 写闸、
/// `Error: Tool '<名>' ...`（hidden / allow / min-tier 三闸同前缀——工具
/// 执行结果不会长这样，工具失败是 `Tool error: ...`）、`SECURITY BLOCKED`
/// （安全层/guardian 拦截，contained 形态）。
pub fn classify_result(result: &str) -> Option<bool> {
    if result.starts_with('⛔') || result.starts_with("Plan mode:") {
        return None;
    }
    if result.contains("SECURITY BLOCKED") {
        return None;
    }
    if result.starts_with("Error: Tool '") {
        return None;
    }
    Some(!result.contains("Tool error:"))
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
    let Some(ok) = classify_result(result) else {
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
