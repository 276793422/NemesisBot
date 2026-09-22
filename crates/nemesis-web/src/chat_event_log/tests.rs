//! L2（devtool-upgrade 阶段 6）— chat_event_log 单元测试。
//!
//! ⚠️ 进程级全局状态（SESSION_LOGS OnceLock）——并行测试共用一张表。
//! 隔离手段：每测试用唯一 session id（pid + 单调计数），互不可见；
//! 全局表有 128 会话上界，测试条目自然被逐出，无泄漏。

use super::*;

/// 每个触碰全局表的测试整段互斥（session_cap sweep 会逐别人的会话）。
/// 纯同步 #[test] 无运行时 → blocking_lock 合法。
macro_rules! table_guard {
    () => {
        super::test_support::GLOBAL_TABLE_LOCK.blocking_lock()
    };
}

fn unique_session(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "l2-{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

#[test]
fn record_seqs_are_monotonic_per_session() {
    let _guard = table_guard!();
    let sid = unique_session("mono");
    let s1 = record(&sid, "assistant", "first", None);
    let s2 = record(&sid, "assistant", "second", Some("m1"));
    let s3 = record(&sid, "user", "third", None);
    assert_eq!((s1, s2, s3), (1, 2, 3));
}

#[test]
fn independent_sessions_have_independent_counters() {
    let a = unique_session("a");
    let b = unique_session("b");
    assert_eq!(record(&a, "assistant", "x", None), 1);
    assert_eq!(record(&b, "assistant", "y", None), 1);
    assert_eq!(record(&a, "assistant", "z", None), 2);
}

#[test]
fn replay_after_returns_window_in_order() {
    let _guard = table_guard!();
    let sid = unique_session("replay");
    for i in 1..=5 {
        record(&sid, "assistant", &format!("m{i}"), None);
    }
    let (events, gap) = replay_after(&sid, 2);
    assert!(!gap);
    assert_eq!(events.len(), 3);
    assert_eq!(events[0].seq, 3);
    assert_eq!(events[2].content, "m5");
    // 追平 → 空、无 gap
    let (events, gap) = replay_after(&sid, 5);
    assert!(events.is_empty() && !gap);
    // after=0 → 窗口内全量
    let (events, gap) = replay_after(&sid, 0);
    assert_eq!(events.len(), 5);
    assert!(!gap);
}

#[test]
fn replay_unknown_session_honest_gap_for_stale_clients() {
    let _guard = table_guard!();
    let sid = unique_session("unknown");
    // from-scratch 客户端（after=0）：无可补，非缺口
    let (events, gap) = replay_after(&sid, 0);
    assert!(events.is_empty() && !gap);
    // 持旧 seq（网关重启后会话环形为空）：gap=true → 前端全量刷新兜底
    let (events, gap) = replay_after(&sid, 42);
    assert!(events.is_empty() && gap);
}

#[test]
fn ring_cap_eviction_produces_gap() {
    let _guard = table_guard!();
    let sid = unique_session("cap");
    // 填满 + 溢出：推 SESSION_REPLAY_CAP + 50 条
    let total = SESSION_REPLAY_CAP + 50;
    for i in 1..=total {
        record(&sid, "assistant", &format!("m{i}"), None);
    }
    // after=0：最老缓冲 seq = 51 > 1 → 缺口滑出窗口
    let (events, gap) = replay_after(&sid, 0);
    assert!(gap);
    assert_eq!(events.len(), SESSION_REPLAY_CAP);
    assert_eq!(events[0].seq, 51);
    // 窗口内起点 → 无 gap
    let (events, gap) = replay_after(&sid, 50);
    assert!(!gap);
    assert_eq!(events.len(), total - 50);
    assert_eq!(events[0].content, "m51");
}

#[test]
fn session_cap_eviction_does_not_evict_current_session() {
    // 超 MAX_SESSIONS 后：新会话仍能记录且自身不被逐出（逐出别人）。
    // 此测试会制造 129+ 会话（逐出旧行）——上界保证无泄漏。
    let _guard = table_guard!();
    for i in 0..(MAX_SESSIONS + 5) {
        let sid = format!("l2-sweep-{}-{i}", std::process::id());
        assert_eq!(record(&sid, "assistant", "x", None), 1);
        let (events, _) = replay_after(&sid, 0);
        assert_eq!(events.len(), 1, "session {sid} must survive its own record");
    }
}

// --- P1（2026-09-21）：工具事件持久化（record_tool 同 seq 序列回放） ---

#[test]
fn tool_events_share_seq_space_and_replay_interleaved() {
    let _guard = table_guard!();
    let sid = unique_session("tool");
    let s1 = record(&sid, "user", "hi", None);
    let t1 = record_tool(
        &sid,
        serde_json::json!({"kind": "ToolStarted", "data": {"call_id": "c1", "tool": "exec"}}),
    );
    let t2 = record_tool(
        &sid,
        serde_json::json!({"kind": "ToolFinished", "data": {"call_id": "c1", "ok": true}}),
    );
    let s2 = record(&sid, "assistant", "done", None);
    // 同键空间、会话内单调：chat 行与 tool 条目交错编号。
    assert_eq!((s1, t1, t2, s2), (1, 2, 3, 4));
    // 回放按 seq 还原真实时序；tool 条目 kind="tool"、载荷原样、文本字段空。
    let (events, gap) = replay_after(&sid, 0);
    assert!(!gap);
    assert_eq!(events.len(), 4);
    assert_eq!(events[1].kind.as_deref(), Some("tool"));
    assert_eq!(events[1].tool.as_ref().unwrap()["kind"], "ToolStarted");
    assert_eq!(events[1].role, "");
    assert!(events[1].content.is_empty());
    assert_eq!(events[2].tool.as_ref().unwrap()["data"]["call_id"], "c1");
    // 普通 chat 行 kind/tool 恒空（旧条目形态兼容）。
    assert_eq!(events[0].kind, None);
    assert_eq!(events[0].tool, None);
    // after 游标按同一序列推进（前端 lastChatSeq 对 chat 行与 tool 条目
    // 统一推进——这里验证 seq=3 之后的增量只含 assistant 行）。
    let (events, gap) = replay_after(&sid, 3);
    assert!(!gap);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].role, "assistant");
}

#[test]
fn record_tool_respects_replay_window_cap() {
    let _guard = table_guard!();
    let sid = unique_session("tool-cap");
    // 全 tool 条目灌满环形窗口：与 chat 行同窗约束。
    for i in 1..=(SESSION_REPLAY_CAP + 10) {
        record_tool(
            &sid,
            serde_json::json!({"kind": "ToolStarted", "data": {"n": i}}),
        );
    }
    let (events, gap) = replay_after(&sid, 0);
    assert!(gap, "滑出窗口 → gap 诚实上报");
    assert_eq!(events.len(), SESSION_REPLAY_CAP);
    assert_eq!(events[0].seq, 11);
}

// --- A1（2026-09-22 聊天切会话竞态）：latest_seq——历史响应 last_seq
// --- 的环尾采样（read_chat_log 之后调用，前端据此剔除先于快照渲染的
// --- assistant 实时帧）。

#[test]
fn latest_seq_unknown_session_is_zero() {
    let _guard = table_guard!();
    let sid = unique_session("latest-unknown");
    // 无记录 → 0 → 序列化省略 → 前端按缺省跳过剔除（A2 兜底）。
    assert_eq!(latest_seq(&sid), 0);
}

#[test]
fn latest_seq_tracks_chat_and_tool_entries() {
    let _guard = table_guard!();
    let sid = unique_session("latest");
    // 环内已分配最大 seq：chat 行与 tool 条目同一序列，逐条推进。
    assert_eq!(latest_seq(&sid), 0);
    assert_eq!(record(&sid, "user", "hi", None), 1);
    assert_eq!(latest_seq(&sid), 1);
    assert_eq!(
        record_tool(&sid, serde_json::json!({"kind": "ToolStarted"})),
        2
    );
    assert_eq!(latest_seq(&sid), 2);
    assert_eq!(record(&sid, "assistant", "done", None), 3);
    assert_eq!(latest_seq(&sid), 3);
}

// --- P8（2026-09-21）：record 即广播 chat.activity（多端感知信号） ---

/// 并行污染免疫的信号等待：同进程其他测试（无 table_guard 的计数测试、
/// server 层入站测试等）的 record 也会广播到已 install 的全局 hub——丢弃
/// 异己信号，只认目标 sid 的 chat.activity（broadcast 有界，溢出丢弃亦不
/// 致卡死：超次即 panic 带现场）。
fn wait_own_activity(
    rx: &mut tokio::sync::broadcast::Receiver<crate::events::Event>,
    sid: &str,
) -> crate::events::Event {
    for _ in 0..200 {
        match rx.try_recv() {
            Ok(ev) if ev.event_type == "chat.activity" && ev.data["session_id"] == *sid => {
                return ev;
            }
            Ok(_) => continue, // 他测信号，丢弃
            Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    panic!("等待 sid={sid} 的 chat.activity 超时（200 轮）");
}

#[test]
fn record_and_record_tool_publish_activity_with_bare_sid() {
    // OnceLock 全局：整个测试进程只此一处 install——先订阅后记录，断言
    // 信号内容。并行他测的 record（无 guard 计数测试、server 层入站测试）
    // 也会广播到本 hub：wait_own_activity 丢弃异己信号免疫污染。
    let _guard = table_guard!();
    let hub = std::sync::Arc::new(EventHub::new());
    install_event_hub(hub.clone());
    let mut rx = hub.subscribe();
    // 键为完整会话键（server.rs send_to_session/pump 的 record 形态）——
    // 信号里必须还原裸 sid 与前端 currentId 同域比对。
    let sid = "p8-bare-sid";
    let key = format!("agent:main:session:{sid}");
    let seq = record(&key, "assistant", "hello", None);
    let ev = wait_own_activity(&mut rx, sid);
    assert_eq!(ev.event_type, "chat.activity");
    assert_eq!(ev.data["session_id"], sid);
    assert_eq!(ev.data["seq"], seq);
    assert_eq!(ev.data["kind"], "chat");
    // record_tool 同样广播（kind="tool"，落后端据此走增量 sync 而非全量）；
    // 还原不了的键原样发（前端匹配不上自然忽略）。
    let seq2 = record_tool(&key, serde_json::json!({"kind": "ToolStarted"}));
    let ev2 = wait_own_activity(&mut rx, sid);
    assert_eq!(ev2.data["session_id"], sid);
    assert_eq!(ev2.data["seq"], seq2);
    assert_eq!(ev2.data["kind"], "tool");
    let web_key = "web:conn-42";
    let seq3 = record(web_key, "assistant", "x", None);
    let ev3 = wait_own_activity(&mut rx, "web:conn-42");
    assert_eq!(ev3.data["session_id"], "web:conn-42");
    assert_eq!(ev3.data["seq"], seq3);
    assert_eq!(ev3.data["kind"], "chat");
}
