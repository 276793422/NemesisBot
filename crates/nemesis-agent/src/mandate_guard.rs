//! b（2026-11-03 三期）：任务 mandate 执行面守卫——类型 + marker + 纯
//! 裁决函数（顶层 pub 模块，与 token_budget 同布局：loop 侧接线在
//! `r#loop:mandate_guard`）。
//!
//! B 端 PeerChatHandler 前置闸（nemesis-cluster）验签通过后，把授权投影
//! （[`nemesis_types:cluster:MandateEnforcement`]）随 `ClusterTask.mandate`
//! 投递；nemesisbot cluster_agent 装配本守卫进 AgentLoop（`set_mandate_guard`），
//! run 结束摘除（`take_mandate_guard`），违规记录随回调 response 追加块
//! 交回 master。干预面两处：
//!
//! 1. **dispatch 调用闸**（tool_dispatch，estop 之后）：工具面/时限/调用
//!    次数违规 → 拒该次调用（结构化错误回灌，任务继续）；
//! 2. **轮顶检**（run_loop，check_session_budget 块之后）：时限过期 /
//!    token 超预算 / 调用次数超限 → 终局 Error（[`MANDATE_VIOLATION_MARKER`]
//!    开头——worker `classify_terminal_failure` 识别 `mandate_violation`，
//!    master board_review 按能力类失败处理：同目标重派前必须换节点或人工
//!    介入）。
//!
//! mandate 缺失 = 无守卫 = 旧行为（旧节点兼容；语义与 B 端 verify posture
//! 同方向）。**语义分层**（设计文档）：超工具面 = 拒该次调用（任务
//! 继续）；时限 / token 预算 / 调用次数超限 = 任务终局 + marker。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

/// 任务终局 marker（Error 事件文案前缀）。nemesisbot
/// `classify_terminal_failure` 单一真相源：识别为 `mandate_violation`。
pub const MANDATE_VIOLATION_MARKER: &str = "⛔ mandate 违规";

/// dispatch 调用级拒绝前缀（完整文案 = "Error: " + 本前缀 + 细节）。
pub const MANDATE_CALL_DENIED_PREFIX: &str = "mandate denied";

/// 违规记录上限（防异常任务把回调追加块撑爆——超限截断并注记）。
pub const MAX_RECORDED_VIOLATIONS: usize = 32;

/// 当前 unix 秒（与 nemesis-cluster mandate 模块同一时钟假设：NTP，与
/// announce 过期 同口径）。
pub fn now_unix() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

/// 执行面守卫（B 端验签产物的运行期限定器）。授权面字段只读；计数器与
/// 违规记录是运行时状态（AtomicU64 / Mutex）。
#[derive(Debug)]
pub struct MandateGuard {
    /// 授权投影（B 端 `enforcement_from_body` 产物原样；task_id 绑定）。
    pub enforcement: nemesis_types::cluster::MandateEnforcement,
    /// 接线时刻 session 用量快照（input+output 合计；token 预算按 delta
    /// 口径——基线前消耗不计入本任务预算）。
    pub tokens_baseline: u64,
    /// 已派发工具调用计数（含被拒尝试——单调，超限后恒拒；拒绝不计入
    /// 「执行了」的语义但占预算判定槽，防重试风暴烧穿限额）。
    pub tool_calls: AtomicU64,
    /// 违规记录（时间序；回调 response 追加块素材；上限截断）。
    pub violations: Mutex<Vec<String>>,
}

impl MandateGuard {
    pub fn new(
        enforcement: nemesis_types::cluster::MandateEnforcement,
        tokens_baseline: u64,
    ) -> Self {
        Self {
            enforcement,
            tokens_baseline,
            tool_calls: AtomicU64::new(0),
            violations: Mutex::new(Vec::new()),
        }
    }

    /// 调用级裁决（dispatch 消费）。`Ok` = 放行；`Err(reason)` = 拒该
    /// 次调用（任务继续——终局归轮顶检）。
    ///
    /// 判定序：时限 → 工具面 → 调用次数（预算类排最后——不合法调用不烧
    /// 预算槽）。注意时限/工具面拒绝**不**递增调用计数（没执行不花预算），
    /// 次数超限的拒绝**会**递增（占位防重试风暴）。
    pub fn check_tool_call(&self, tool: &str, now: u64) -> Result<(), String> {
        if self.enforcement.expires_at > 0 && now >= self.enforcement.expires_at {
            let detail = format!(
                "时间窗已过期（expires_at={} <= now={now}），拒绝工具 `{tool}`",
                self.enforcement.expires_at
            );
            self.record_violation(detail.clone());
            return Err(detail);
        }
        if let Some(list) = &self.enforcement.tool_allowlist
            && !list.iter().any(|t| t == tool)
        {
            let detail = format!("工具 `{tool}` 不在 mandate 授权面");
            self.record_violation(detail.clone());
            return Err(detail);
        }
        if self.enforcement.max_tool_calls > 0 {
            let prev = self.tool_calls.fetch_add(1, Ordering::SeqCst);
            if prev + 1 > self.enforcement.max_tool_calls {
                let detail = format!(
                    "工具调用次数超限（{}/{}），拒绝 `{tool}`",
                    prev + 1,
                    self.enforcement.max_tool_calls
                );
                self.record_violation(detail.clone());
                return Err(detail);
            }
        }
        Ok(())
    }

    /// 时间窗判定（轮顶检消费）。`expires_at == 0` = 不限。
    pub fn check_expiry(&self, now: u64) -> Result<(), String> {
        if self.enforcement.expires_at > 0 && now >= self.enforcement.expires_at {
            let detail = format!(
                "时间窗已过期（expires_at={} <= now={now}）",
                self.enforcement.expires_at
            );
            self.record_violation(detail.clone());
            return Err(detail);
        }
        Ok(())
    }

    /// token 预算判定（轮顶检消费）。`max_tokens == 0` = 不限；delta 口径
    /// （接线时快照基线）。`used_total` = 本时刻 session 用量合计。
    pub fn check_token_budget(&self, used_total: u64) -> Result<(), String> {
        if self.enforcement.max_tokens == 0 {
            return Ok(());
        }
        let delta = used_total.saturating_sub(self.tokens_baseline);
        if delta >= self.enforcement.max_tokens {
            let detail = format!(
                "token 预算耗尽（{}/{}）",
                delta, self.enforcement.max_tokens
            );
            self.record_violation(detail.clone());
            return Err(detail);
        }
        Ok(())
    }

    /// 调用次数判定（轮顶检消费）。`max_tool_calls == 0` = 不限。
    pub fn check_tool_call_budget(&self) -> Result<(), String> {
        if self.enforcement.max_tool_calls == 0 {
            return Ok(());
        }
        let done = self.tool_calls.load(Ordering::SeqCst);
        if done >= self.enforcement.max_tool_calls {
            let detail = format!(
                "工具调用次数已达上限（{}/{}）",
                done, self.enforcement.max_tool_calls
            );
            self.record_violation(detail.clone());
            return Err(detail);
        }
        Ok(())
    }

    /// 记违规（上限截断 + 注记）。
    fn record_violation(&self, detail: String) {
        let mut v = self.violations.lock().unwrap_or_else(|e| e.into_inner());
        if v.len() < MAX_RECORDED_VIOLATIONS {
            v.push(detail);
        } else if v.len() == MAX_RECORDED_VIOLATIONS {
            v.push("……（后续违规记录因上限截断）".into());
        }
    }

    /// 违规记录追加块（回调 response 素材；无违规 = None）。
    pub fn violation_report(&self) -> Option<String> {
        let v = self.violations.lock().unwrap_or_else(|e| e.into_inner());
        if v.is_empty() {
            return None;
        }
        let mut out = format!("\n\n---\n{MANDATE_VIOLATION_MARKER}（{} 条）:", v.len());
        for (i, d) in v.iter().enumerate() {
            out.push_str(&format!("\n{}. {d}", i + 1));
        }
        Some(out)
    }
}

/// 终局 Error 文案（轮顶检消费；marker 开头供 classify 识别，尾部「已保存」
/// 句式对齐 token_budget hard-stop 形态）。
pub fn terminal_notice(detail: &str) -> String {
    format!("{MANDATE_VIOLATION_MARKER}：{detail}。任务已停止；已完成的工作已保存。")
}

#[cfg(test)]
mod tests;
