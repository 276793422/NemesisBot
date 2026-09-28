//! P33（2026-09-25 能力扩展计划 WS7）：中断安全重放——nanobot 相位机思想移植。
//!
//! 核心不变量：**checkpoint/快照恢复时，未完成工具调用永不静默重放**。
//! 恢复时检测快照里挂起的 tool_use（assistant 带工具调用、其后无对应
//! tool result），合成显式 tool 结果
//! `{status:"interrupted", note:"进程中断，工具未完成，效果未知，请勿假设其效果"}`
//! 回灌对话——provider 消息对完整性优先，模型永远看到显式记录而不是
//! 凭空假设工具效果。
//!
//! 与既有设施的关系（单一真相源纪律）：
//! - 前缀对齐 [`TOOL_OUTCOME_UNKNOWN`]：合成内容以 `[TOOL_OUTCOME_UNKNOWN]`
//!   开头，使 [`crate::loop_continuation::merge_real_tool_result`] 的占位
//!   识别纪律原样生效——真实结果后来到达时**替换**折算（而非追加成同 id
//!   双 tool 消息被严格 provider 400 拒绝）；真实结果不可达时，快照带上
//!   诚实 interrupted 注记等待用户确认继续。
//! - 相位机：快照恢复状态机 `checkpoint → restored → confirmed`，非法
//!   转移 loud 拒绝（[`validate_restore_transition`]）。唯一例外：
//!   从未中断的存活快照允许 `checkpoint → confirmed` 直达续行（没有
//!   中断就没有需要确认的折算）。
//!
//! v1 简化（诚实记录）：恢复路径折算注入后**不自动续行 LLM**——续行仍由
//! 集群回调/恢复轮询驱动；若真实结果在案则替换折算后照常续行（工具实际
//! 完成，不存在未知效果重放），若不可达则快照保持 interrupted 注记等下
//! 一条用户消息自然继续。完整审批确认环（恢复后先发审批卡再放行）挂账
//! 后续版本。

use crate::r#loop::LlmMessage;
use crate::types::TOOL_OUTCOME_UNKNOWN;

/// 折算结果里的 status 字段值（机器可读；配合 note 供模型/前端双消费）。
pub const INTERRUPTED_STATUS: &str = "interrupted";

/// 折算结果的诚实注记原文（计划钦定文案，勿改——模型行为依赖此语义）。
pub const INTERRUPTED_NOTE: &str = "进程中断，工具未完成，效果未知，请勿假设其效果";

/// 构造一条挂起 tool_use 的显式 interrupted 折算结果内容。
///
/// `[TOOL_OUTCOME_UNKNOWN]` 前缀是**契约**不是装饰：`merge_real_tool_result`
/// 只替换以该前缀开头的占位（发现 G 纪律），折算必须可被后到的真实结果
/// 安全替换。payload 用 serde_json 构造（工具名/id 含引号时不产生坏 JSON）。
pub fn interrupted_tool_result_content(tool_call_id: &str, tool_name: &str) -> String {
    let payload = serde_json::json!({
        "status": INTERRUPTED_STATUS,
        "note": INTERRUPTED_NOTE,
        "tool": tool_name,
        "tool_call_id": tool_call_id,
    });
    format!("[{TOOL_OUTCOME_UNKNOWN}] {payload}")
}

/// 检测快照里挂起的 tool_use（有 tool call、其后无对应 tool result）。
///
/// 返回挂起的 tool_call_id（按出现序）。判定口径与
/// `repair_tool_message_pairs` Pass 2 一致：该 id 在**本 assistant 之后**
/// 的任意位置有 role="tool" 应答即视为已完成（Pass 0 保证至多一条）。
pub fn find_pending_tool_calls(messages: &[LlmMessage]) -> Vec<String> {
    let mut pending = Vec::new();
    for (i, m) in messages.iter().enumerate() {
        let Some(calls) = m.tool_calls.as_ref() else {
            continue;
        };
        for tc in calls {
            let answered = messages[i + 1..]
                .iter()
                .any(|r| r.role == "tool" && r.tool_call_id.as_deref() == Some(tc.id.as_str()));
            if !answered && !pending.contains(&tc.id) {
                pending.push(tc.id.clone());
            }
        }
    }
    pending
}

/// 为快照里所有挂起 tool_use 合成显式 interrupted 结果（原地返回新 vec）。
///
/// 返回 `(折算后的消息, 折算的 tool_call_id 列表)`。插入位置 = 携带调用的
/// assistant 行之后；同一 assistant 多个挂起调用按模型调用序排列（与
/// `repair_tool_message_pairs` 的反序回填手法同源，测试钉死）。
pub fn synthesize_interrupted_results(messages: Vec<LlmMessage>) -> (Vec<LlmMessage>, Vec<String>) {
    let n = messages.len();
    let mut insertions: Vec<(usize, LlmMessage)> = Vec::new();
    let mut synthesized_ids: Vec<String> = Vec::new();
    for i in 0..n {
        let Some(calls) = messages[i].tool_calls.as_ref() else {
            continue;
        };
        if calls.is_empty() {
            continue;
        }
        for tc in calls {
            let answered = messages[i + 1..]
                .iter()
                .any(|r| r.role == "tool" && r.tool_call_id.as_deref() == Some(tc.id.as_str()));
            if answered {
                continue;
            }
            insertions.push((
                i + 1,
                LlmMessage {
                    role: "tool".to_string(),
                    content: interrupted_tool_result_content(&tc.id, &tc.name),
                    tool_calls: None,
                    tool_call_id: Some(tc.id.clone()),
                    reasoning_content: None,
                    images: Vec::new(),
                },
            ));
            synthesized_ids.push(tc.id.clone());
        }
    }
    let mut messages = messages;
    // 反序回填：同位插入时后收集的先插、先收集的落其前——最终顺序 = 调用序
    // （repair_tool_message_pairs 同款已验证手法）。
    for (at, m) in insertions.into_iter().rev() {
        messages.insert(at, m);
    }
    (messages, synthesized_ids)
}

/// 快照恢复相位（状态机 `checkpoint → restored → confirmed`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestorePhase {
    /// 快照已保存（checkpoint 边界）——出厂相位。
    Checkpoint,
    /// 已从中断中恢复（磁盘快照回载 + interrupted 折算完成）。
    Restored,
    /// 已确认续行（回调/恢复轮询/用户消息驱动）。
    Confirmed,
}

/// 相位转移严格校验：非法转移 loud 拒绝（计划钦定语义）。
///
/// 合法转移：
/// - `Checkpoint → Restored`（中断恢复）
/// - `Restored → Confirmed`（恢复后确认续行）
/// - `Checkpoint → Confirmed`（**仅**存活快照——从未经历恢复，无折算
///   需要确认，回调直达续行）
///
/// 其余一切（重确认、回退、Restored 直达 Checkpoint 之外的自环等）一律
/// `Err`，调用方诚实停车，不静默续行。
pub fn validate_restore_transition(from: RestorePhase, to: RestorePhase) -> Result<(), String> {
    let legal = matches!(
        (from, to),
        (RestorePhase::Checkpoint, RestorePhase::Restored)
            | (RestorePhase::Restored, RestorePhase::Confirmed)
            | (RestorePhase::Checkpoint, RestorePhase::Confirmed)
    );
    if legal {
        Ok(())
    } else {
        Err(format!(
            "非法快照恢复相位转移 {from:?} → {to:?}（合法链 checkpoint→restored→confirmed；仅未中断的存活快照允许 checkpoint→confirmed 直达）"
        ))
    }
}

#[cfg(test)]
mod interrupt_replay_tests;
