//! 聚合器测试：事件计数 / 审批配对与裁决归类 / 映射 FIFO 淘汰 /
//! 节流边界 / 读盘一致性。会话键全程纳秒唯一化（chat_log 测试同款纪律，
//! 规避并行测试撞键），起止各一次 `delete_chat_log` 清理。

use super::*;
use nemesis_types::agent::AgentEvent;

fn uniq_key(tag: &str) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("test:stats:{tag}:{nanos}")
}

fn cleanup(key: &str) {
    nemesis_agent::chat_log::delete_chat_log(key);
}

fn tool_started(key: &str, call: &str) -> AgentEvent {
    AgentEvent::ToolStarted {
        session_key: key.to_string(),
        chat_id: "chat-1".to_string(),
        call_id: call.to_string(),
        tool: "exec".to_string(),
        args_preview: "{}".to_string(),
    }
}

fn tool_finished(key: &str, call: &str, ok: bool) -> AgentEvent {
    AgentEvent::ToolFinished {
        session_key: key.to_string(),
        chat_id: "chat-1".to_string(),
        call_id: call.to_string(),
        tool: "exec".to_string(),
        duration_ms: 1,
        ok,
        result_preview: String::new(),
    }
}

fn approval_requested(key: &str, request_id: &str) -> AgentEvent {
    AgentEvent::ApprovalRequested {
        session_key: key.to_string(),
        chat_id: "chat-1".to_string(),
        request_id: request_id.to_string(),
        operation: "process_exec".to_string(),
        target: "cmd".to_string(),
        risk_level: "HIGH".to_string(),
        reason: "test".to_string(),
        timeout_secs: 30,
        pattern: "cmd *".to_string(),
        why: None,
        impact: None,
        reject_note: None,
        preview: None,
        preview_format: None,
        batch_id: None,
    }
}

fn approval_resolved(request_id: &str, decision: &str) -> AgentEvent {
    AgentEvent::ApprovalResolved {
        request_id: request_id.to_string(),
        decision: decision.to_string(),
    }
}

#[test]
fn test_tool_events_count_and_persist() {
    let key = uniq_key("tools");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    agg.observe(&tool_started(&key, "c1"));
    agg.observe(&tool_started(&key, "c2"));
    agg.observe(&tool_started(&key, "c3"));
    agg.observe(&tool_finished(&key, "c1", true));
    agg.observe(&tool_finished(&key, "c2", false));
    agg.observe(&tool_finished(&key, "c3", false));
    let s = agg.flush_and_read(&key);
    assert_eq!(s.tool_calls, 3);
    assert_eq!(s.tool_failures, 2);
    // 重启不归零验收的进程内替身：全新聚合器（内存空）读同一会话，
    // 读数来自持久层——不变。
    let fresh = SessionStatsAggregator::default();
    let s2 = fresh.flush_and_read(&key);
    assert_eq!(s2.tool_calls, 3);
    assert_eq!(s2.tool_failures, 2);
    cleanup(&key);
}

#[test]
fn test_sink_counters_and_read() {
    let key = uniq_key("sink");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    agg.bump(&key, StatCounter::Retries);
    agg.bump(&key, StatCounter::Retries);
    agg.bump(&key, StatCounter::ValidationFailures);
    agg.bump(&key, StatCounter::Compactions);
    let s = agg.flush_and_read(&key);
    assert_eq!(s.retries, 2);
    assert_eq!(s.validation_failures, 1);
    assert_eq!(s.compactions, 1);
    cleanup(&key);
}

#[test]
fn test_approval_pairing_and_decisions() {
    let key = uniq_key("approvals");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    agg.observe(&approval_requested(&key, "r1"));
    agg.observe(&approval_requested(&key, "r2"));
    agg.observe(&approval_requested(&key, "r3"));
    agg.observe(&approval_resolved("r1", "approved"));
    agg.observe(&approval_resolved("r2", "denied"));
    agg.observe(&approval_resolved("r3", "timeout"));
    // 未知 request_id（映射 miss：重启丢窗口/上限淘汰）= 诚实放弃归账，
    // 不炸不计数。
    agg.observe(&approval_resolved("r-missing", "approved"));
    let s = agg.flush_and_read(&key);
    assert_eq!(s.approvals_total, 3);
    assert_eq!(s.approvals_approved, 1);
    assert_eq!(s.approvals_denied, 2);
    cleanup(&key);
}

#[test]
fn test_approval_map_fifo_eviction() {
    let key = uniq_key("fifo");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    // 装满上限再溢出一个：最老映射按 FIFO 淘汰。
    for i in 0..APPROVAL_MAP_CAP {
        agg.observe(&approval_requested(&key, &format!("old-{i}")));
    }
    agg.observe(&approval_requested(&key, "new"));
    // 被淘汰的最老映射：裁决到达 = miss，诚实放弃（approved 不计数）。
    agg.observe(&approval_resolved("old-0", "approved"));
    // 未被淘汰的最新映射：正常归账。
    agg.observe(&approval_resolved("new", "approved"));
    let s = agg.flush_and_read(&key);
    assert_eq!(s.approvals_total, (APPROVAL_MAP_CAP as u64) + 1);
    assert_eq!(s.approvals_approved, 1);
    assert_eq!(s.approvals_denied, 0);
    cleanup(&key);
}

#[test]
fn test_throttle_first_flush_then_batch() {
    let key = uniq_key("throttle");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    // 首个事件：last_flush=None → 立即落盘（首个增量即满足时间维）。
    agg.observe(&tool_started(&key, "c1"));
    let disk = nemesis_agent::chat_log::read_session_stats(&key);
    assert_eq!(disk.tool_calls, 1);
    // 5s 窗口内的后续事件：内存累计，不落盘（裸读盘读不到）。
    agg.observe(&tool_started(&key, "c2"));
    agg.observe(&tool_started(&key, "c3"));
    let disk2 = nemesis_agent::chat_log::read_session_stats(&key);
    assert_eq!(disk2.tool_calls, 1, "节流窗口内的增量不应落盘");
    // 读路径强制 flush：读到的恒为最新。
    let s = agg.flush_and_read(&key);
    assert_eq!(s.tool_calls, 3);
    cleanup(&key);
}

#[test]
fn test_session_accum_throttle_semantics() {
    let mut acc = SessionAccum::default();
    // 从未落盘 + 有增量 = 立即到期（首个增量即落盘，模块文档口径）。
    acc.bump(|s| &mut s.tool_calls);
    assert!(acc.due());
    assert_eq!(acc.take_pending().tool_calls, 1);
    // 刚落盘 + 少量增量 = 不到期（时间维未到、次数维未满）。
    acc.bump(|s| &mut s.tool_calls);
    assert!(!acc.due());
    // 次数维到期：凑满 FLUSH_PENDING_LIMIT。
    for _ in 1..FLUSH_PENDING_LIMIT {
        acc.bump(|s| &mut s.tool_calls);
    }
    assert_eq!(acc.pending_count, FLUSH_PENDING_LIMIT);
    assert!(acc.due());
    let flushed = acc.take_pending();
    assert_eq!(flushed.tool_calls, u64::from(FLUSH_PENDING_LIMIT));
    // take 后 pending 清零、节流锚点重置。
    assert_eq!(acc.pending.tool_calls, 0);
    assert_eq!(acc.pending_count, 0);
    assert!(acc.last_flush.is_some());
}

#[test]
fn test_untouched_session_reads_zero() {
    let key = uniq_key("untouched");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    let s = agg.flush_and_read(&key);
    assert!(s.is_zero());
    cleanup(&key);
}

#[test]
fn test_all_fields_combined() {
    let key = uniq_key("combo");
    cleanup(&key);
    let agg = SessionStatsAggregator::default();
    // 事件路：工具 2 调 1 败 + 审 请求 1 拒。
    agg.observe(&tool_started(&key, "c1"));
    agg.observe(&tool_finished(&key, "c1", true));
    agg.observe(&tool_started(&key, "c2"));
    agg.observe(&tool_finished(&key, "c2", false));
    agg.observe(&approval_requested(&key, "r1"));
    agg.observe(&approval_resolved("r1", "approved"));
    agg.observe(&approval_requested(&key, "r2"));
    agg.observe(&approval_resolved("r2", "denied"));
    // sink 路：三环重试 / 校验失败 / 压缩各 1。
    agg.bump(&key, StatCounter::Retries);
    agg.bump(&key, StatCounter::ValidationFailures);
    agg.bump(&key, StatCounter::Compactions);
    let s = agg.flush_and_read(&key);
    assert_eq!(s.tool_calls, 2);
    assert_eq!(s.tool_failures, 1);
    assert_eq!(s.retries, 1);
    assert_eq!(s.validation_failures, 1);
    assert_eq!(s.compactions, 1);
    assert_eq!(s.approvals_total, 2);
    assert_eq!(s.approvals_approved, 1);
    assert_eq!(s.approvals_denied, 1);
    cleanup(&key);
}
