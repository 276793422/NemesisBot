//! 人工触点状态机 + 审计事件（看板人工触点体系，v25）。
//!
//! 触点有**自己的状态机**（[`can_transition`]），不污染 issue 通用状态机
//! （对齐审计回滚 `rollback_decision` 旁路先例——done 是 issue 的终态，
//! 触点语义放 `board_touchpoints` 表）。触点单本体 = 普通 issue：决定史
//! 评论、状态转移、任务树全部复用既有单据面，本模块只管触点语义字段。
//!
//! 设计文档：`docs/PLAN/2026-11-07_board-human-touchpoints.md` §5。
//! 三条公理：无人值守是默认态（机器永不死锁）；关门不锁门（超时=收口
//! 不是丢弃，迟到输入=变更管理走评论）；否决=注入方向（同范围连续
//! veto_max_rounds 次否决且无机械进展 → parked_sync 熔断停车）。

use serde::{Deserialize, Serialize};

use crate::models::{TouchpointKind, TouchpointStatus};

/// 超时默认动作（sweep 收口方向；挂卡方未给 payload 时的机器兜底）。
pub fn default_action_for(kind: TouchpointKind) -> &'static str {
    match kind {
        // 规格按草案继续拆解（关门不锁门：迟到意见走评论进变更管理）。
        TouchpointKind::Spec => "proceed_with_draft",
        // 原型按当前版继续（方向校验缺席 = 记录沉默，机器不停）。
        TouchpointKind::Prototype => "proceed_current_version",
    }
}

/// 触点状态机：合法转移表（纯函数；存储层写前裁决）。
///
/// - `pending`：唯一等人工的活态 → 六出路。
/// - `vetoed_local`：局部否决（返工）→ 机器闸复验通过回 pending /
///   同范围熔断停车 / 人工取消。
/// - `vetoed_spec`：改规格否决 → 重拆完成后 v2 重挂回 pending /
///   熔断停车 / 人工取消。
/// - `confirmed` / `timeout_defaulted` / `cancelled` / `parked_sync`：
///   终态无出路（恢复只经显式回滚路径，不在状态机内——迟到输入走
///   评论进变更管理，不重开触点）。
pub fn can_transition(from: TouchpointStatus, to: TouchpointStatus) -> bool {
    use TouchpointStatus as S;
    match from {
        S::Pending => matches!(
            to,
            S::Confirmed
                | S::TimeoutDefaulted
                | S::VetoedLocal
                | S::VetoedSpec
                | S::Cancelled
                | S::ParkedSync
        ),
        S::VetoedLocal | S::VetoedSpec => {
            matches!(to, S::Pending | S::ParkedSync | S::Cancelled)
        }
        S::Confirmed | S::TimeoutDefaulted | S::Cancelled | S::ParkedSync => false,
    }
}

/// 审计事件（`board_touchpoints.audit` 列的数组元素；append-only——
/// 人的沉默与迟到输入都是显式事件，超时 sweep / 提醒三拍也落这里）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TouchpointAuditEvent {
    /// 事件时刻（epoch ms）。
    pub ts_ms: i64,
    /// 触发者：`human:<id>` / `system:<组件>` / `timeout-sweep`。
    pub actor: String,
    /// 动作词（开放词表）：confirm / veto_local / veto_spec / cancel /
    /// mute / timeout_default / rehang / late_input / parked_sync / remind …
    pub action: String,
    /// 人读细节（修订摘要/否决原因/默认动作等；board 只存取不解释）。
    #[serde(default)]
    pub detail: String,
}

/// 构造审计事件（时刻由调用方给：web/gateway 各有自己的时钟口径）。
impl TouchpointAuditEvent {
    pub fn new(
        ts_ms: i64,
        actor: impl Into<String>,
        action: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            ts_ms,
            actor: actor.into(),
            action: action.into(),
            detail: detail.into(),
        }
    }
}

/// 追加审计事件到 audit JSON 数组（纯函数）。损坏的既有数组 = 诚实
/// 拒绝（对齐 `append_pending_merge` 先例——静默重置会把既有审计史
/// 整条毁掉）。
pub fn append_audit_event(audit_json: &str, event: TouchpointAuditEvent) -> Result<String, String> {
    let mut events: Vec<TouchpointAuditEvent> =
        serde_json::from_str(if audit_json.trim().is_empty() {
            "[]"
        } else {
            audit_json
        })
        .map_err(|e| format!("append_audit_event: audit 解析失败（原数组保留）: {e}"))?;
    events.push(event);
    serde_json::to_string(&events).map_err(|e| format!("append_audit_event: {e}"))
}

#[cfg(test)]
mod tests;
