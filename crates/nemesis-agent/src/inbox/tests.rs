//! Inbox unit tests (I1 / U7).

use super::*;

fn msg(content: &str) -> QueuedMessage {
    QueuedMessage {
        msg: nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: String::new(),
            chat_id: "c1".to_string(),
            content: content.to_string(),
            media: vec![],
            session_key: "web:c1".to_string(),
            correlation_id: String::new(),
            metadata: Default::default(),
            voice_playback: None,
        },
        timestamp: "2026-08-22T00:00:00Z".to_string(),
        priority: MessagePriority::Normal,
    }
}

/// A5：带 metadata 的消息构造器（priority_of / 高优先级用例）。
fn msg_with_meta(content: &str, metadata: Vec<(&str, &str)>) -> QueuedMessage {
    let mut m = msg(content);
    m.msg.metadata = metadata
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let priority = priority_of(&m.msg);
    m.priority = priority;
    m
}

// A5（2026-10-21 引入）：优先级解析单点——metadata["priority"]
// trim + 大小写不敏感 == "high" 才是 High；缺键/空/未知值一律 Normal。
#[test]
fn test_priority_of_metadata() {
    let mut m = msg("x");
    assert_eq!(priority_of(&m.msg), MessagePriority::Normal);
    m.msg.metadata.insert("priority".into(), "high".into());
    assert_eq!(priority_of(&m.msg), MessagePriority::High);
    m.msg.metadata.insert("priority".into(), " HIGH ".into());
    assert_eq!(priority_of(&m.msg), MessagePriority::High);
    m.msg.metadata.insert("priority".into(), "High".into());
    assert_eq!(priority_of(&m.msg), MessagePriority::High);
    // 未知值不猜 → Normal。
    m.msg.metadata.insert("priority".into(), "urgent".into());
    assert_eq!(priority_of(&m.msg), MessagePriority::Normal);
    m.msg.metadata.insert("priority".into(), String::new());
    assert_eq!(priority_of(&m.msg), MessagePriority::Normal);
}

// A5：High 插队语义——插到现有 High 连续段之后、第一个 Normal 之前
// （High 之间 FIFO；Normal 保持原有相对顺序）。
#[test]
fn test_high_priority_insertion_order() {
    let inbox = Inbox::new(16);
    let drain = |inbox: &Inbox, key: &str| -> Vec<String> {
        let mut out = Vec::new();
        while let Some(m) = inbox.claim_next_turn_head(key) {
            out.push(m.msg.content);
        }
        out
    };

    // Normal x2 后到 High → 插到队首（两个 Normal 之前）。
    inbox.enqueue_for_mode("a", msg("n1"), false);
    inbox.enqueue_for_mode("a", msg("n2"), false);
    inbox.enqueue_for_mode("a", msg_with_meta("h1", vec![("priority", "high")]), false);
    assert_eq!(drain(&inbox, "a"), vec!["h1", "n1", "n2"]);

    // High x2 后到 High → 排在现有 High 之后（High 之间 FIFO）。
    inbox.enqueue_for_mode("b", msg_with_meta("h1", vec![("priority", "HIGH")]), false);
    inbox.enqueue_for_mode(
        "b",
        msg_with_meta("h2", vec![("priority", " High ")]),
        false,
    );
    inbox.enqueue_for_mode("b", msg_with_meta("h3", vec![("priority", "high")]), false);
    assert_eq!(drain(&inbox, "b"), vec!["h1", "h2", "h3"]);

    // 混合段：n1, h1, n2 后到 h2 → 插到 h1 之后、n1 之前（High 恒在
    // 现有 Normal 之前；两个 High 保持 FIFO）。
    inbox.enqueue_for_mode("c", msg("n1"), false);
    inbox.enqueue_for_mode("c", msg_with_meta("h1", vec![("priority", "high")]), false);
    inbox.enqueue_for_mode("c", msg("n2"), false);
    inbox.enqueue_for_mode("c", msg_with_meta("h2", vec![("priority", "high")]), false);
    assert_eq!(drain(&inbox, "c"), vec!["h1", "h2", "n1", "n2"]);
}

// A5：软水位边界——容量 10，软水位 9。第 10 条 Normal（total=9 = 软水位）
// RejectedSoft（非 Rejected——硬上限未到）；同态下 High 与 steer 插话仍
// 放行；硬上限（total=10）一律 Rejected。
#[test]
fn test_soft_watermark_boundary() {
    let inbox = Inbox::new(10);
    use EnqueueOutcome::{QueuedForNextStep, QueuedForNextTurn, Rejected, RejectedSoft};

    // 填 9 条（到达时 total=0..=8 → 未到软水位，全部放行）。
    for i in 0..9 {
        assert_eq!(
            inbox.enqueue_for_mode("s", msg(&format!("n{i}")), false),
            QueuedForNextTurn
        );
    }
    // total=9 = 软水位：Normal → RejectedSoft。
    assert_eq!(inbox.enqueue_for_mode("s", msg("n9"), false), RejectedSoft);
    // 同态下 steer 插话不受软水位约束（进 next_step，total → 10）。
    assert_eq!(
        inbox.enqueue_for_mode("s", msg("!urgent"), true),
        QueuedForNextStep
    );
    // total=10 = 硬上限：连 High 也拒。
    assert_eq!(
        inbox.enqueue_for_mode("s", msg_with_meta("h2", vec![("priority", "high")]), false),
        Rejected
    );
    assert_eq!(inbox.enqueue_for_mode("s", msg("x"), false), Rejected);

    // High 旁路单独验证（不撞硬上限）：total=9 = 软水位时 High 仍放行。
    let inbox2 = Inbox::new(10);
    for i in 0..9 {
        let _ = inbox2.enqueue_for_mode("s2", msg(&format!("n{i}")), false);
    }
    assert_eq!(
        inbox2.enqueue_for_mode("s2", msg_with_meta("h", vec![("priority", "high")]), false),
        QueuedForNextTurn
    );

    // capacity=1：软水位 .max(1) 与硬上限同值 → 语义退化为纯容量上限
    // （第 1 条入队后 total=1，后续一律硬拒），不产生「软水位 0 全拒死」
    // 的病理——这正是 .max(1) 存在的意义。
    let tiny = Inbox::new(1);
    assert_eq!(
        tiny.enqueue_for_mode("t", msg("n1"), false),
        QueuedForNextTurn
    );
    assert_eq!(tiny.enqueue_for_mode("t", msg("n2"), false), Rejected);
    assert_eq!(
        tiny.enqueue_for_mode("t", msg_with_meta("h", vec![("priority", "high")]), false),
        Rejected
    );
}

#[test]
fn test_steer_detection() {
    assert!(is_steer_message("!stop"));
    assert!(is_steer_message("！别删"));
    assert!(is_steer_message("  ! trim first"));
    assert!(!is_steer_message("hello"));
    assert!(!is_steer_message("what?!")); // mid-text ! is not a prefix
    assert!(!is_steer_message(""));
}

// Round-5 fix: the marker-strip rule lives in ONE place beside the
// detection rule — classification and stripping cannot drift.
#[test]
fn test_strip_steer_marker() {
    assert_eq!(strip_steer_marker("!stop"), "stop");
    assert_eq!(strip_steer_marker("！别删"), "别删");
    assert_eq!(strip_steer_marker("  !  spaced"), "spaced");
    // Non-steer content comes back unchanged (including leading spaces).
    assert_eq!(strip_steer_marker("hello"), "hello");
    assert_eq!(strip_steer_marker("what?!"), "what?!");
    assert_eq!(strip_steer_marker(""), "");
    // Every message classified steer strips exactly one marker CHAR; every
    // non-steer message is a fixed point.
    for c in ["!x", "！中文", "plain", "?q", "!!double"] {
        let stripped = strip_steer_marker(c);
        if is_steer_message(c) {
            assert_eq!(
                stripped.chars().count() + 1,
                c.trim_start().chars().count(),
                "{c} -> {stripped}"
            );
        } else {
            assert_eq!(stripped, c);
        }
    }
}

#[test]
fn test_enqueue_routing_and_capacity() {
    let inbox = Inbox::new(3);
    // A5 软水位（2026-10-21）：容量 3 → soft = max(2,1) = 2。
    assert_eq!(
        inbox.enqueue("s", msg("normal one")),
        EnqueueOutcome::QueuedForNextTurn
    );
    assert_eq!(
        inbox.enqueue("s", msg("!urgent")),
        EnqueueOutcome::QueuedForNextStep
    );
    // total=2 已达软水位：普通消息软拒（不是硬拒——硬上限未到）。
    assert_eq!(
        inbox.enqueue("s", msg("normal two")),
        EnqueueOutcome::RejectedSoft
    );
    // High 不受软水位约束（补进第 3 格 = 硬上限）。
    assert_eq!(
        inbox.enqueue("s", msg_with_meta("high", vec![("priority", "high")])),
        EnqueueOutcome::QueuedForNextTurn
    );
    // 硬上限 3 已满 → 一律硬拒（steer 与 High 的软水位豁免不豁免硬上限）。
    assert_eq!(
        inbox.enqueue("s", msg("!another")),
        EnqueueOutcome::Rejected
    );
    assert_eq!(
        inbox.enqueue("s", msg("overflow")),
        EnqueueOutcome::Rejected
    );
    assert_eq!(inbox.pending("s"), (2, 1));
}

#[test]
fn test_claim_order_next_step_all_then_turn_head() {
    let inbox = Inbox::new(8);
    inbox.enqueue("s", msg("turn A"));
    inbox.enqueue("s", msg("!steer 1"));
    inbox.enqueue("s", msg("!steer 2"));
    inbox.enqueue("s", msg("turn B"));

    // claim_next_step takes ALL interjections in order.
    let stepped = inbox.claim_next_step("s");
    assert_eq!(stepped.len(), 2);
    assert_eq!(stepped[0].msg.content, "!steer 1");
    assert_eq!(stepped[1].msg.content, "!steer 2");
    assert!(!inbox.has_next_step("s"));

    // claim_next_turn_head takes exactly the HEAD.
    let head = inbox.claim_next_turn_head("s").unwrap();
    assert_eq!(head.msg.content, "turn A");
    let head2 = inbox.claim_next_turn_head("s").unwrap();
    assert_eq!(head2.msg.content, "turn B");
    assert!(inbox.claim_next_turn_head("s").is_none());
}

#[test]
fn test_abort_transfers_next_step() {
    let inbox = Inbox::new(8);
    inbox.enqueue("s", msg("turn A"));
    inbox.enqueue("s", msg("!steer"));
    inbox.transfer_next_step_to_next_turn("s");
    // Steer message survives, appended after the earlier turn message.
    let (turns, steps) = inbox.pending("s");
    assert_eq!((turns, steps), (2, 0));
    let h1 = inbox.claim_next_turn_head("s").unwrap();
    assert_eq!(h1.msg.content, "turn A");
    // Round-5 fix: the marker is STRIPPED on transfer — a late steer must
    // replay marker-free exactly like an in-turn claim, not leak the
    // routing `!` into the model's content.
    let h2 = inbox.claim_next_turn_head("s").unwrap();
    assert_eq!(h2.msg.content, "steer");
}

#[test]
fn test_per_session_isolation() {
    let inbox = Inbox::new(4);
    inbox.enqueue("a", msg("for a"));
    inbox.enqueue("b", msg("for b"));
    assert_eq!(inbox.pending("a"), (1, 0));
    assert_eq!(inbox.pending("b"), (1, 0));
    inbox.clear("a");
    assert_eq!(inbox.pending("a"), (0, 0));
    assert_eq!(inbox.pending("b"), (1, 0));
}

// Second-pass review fixes:
#[test]
fn test_queue_mode_routes_all_to_next_turn() {
    let inbox = Inbox::new(4);
    // steer disabled (Queue mode): '!' message still goes to next_turn.
    assert_eq!(
        inbox.enqueue_for_mode("s", msg("!urgent"), false),
        EnqueueOutcome::QueuedForNextTurn
    );
    assert_eq!(inbox.pending("s"), (1, 0));
    // steer enabled (Steer mode): same message goes to next_step.
    let inbox2 = Inbox::new(4);
    assert_eq!(
        inbox2.enqueue_for_mode("s", msg("!urgent"), true),
        EnqueueOutcome::QueuedForNextStep
    );
    assert_eq!(inbox2.pending("s"), (0, 1));
}

#[test]
fn test_late_steer_transfers_on_normal_turn_end() {
    // The post-turn handler now transfers unconditionally (was
    // cancelled-only) — a steer that missed the escape hatch becomes the
    // next turn's input instead of a stale out-of-context injection later.
    let inbox = Inbox::new(4);
    inbox.enqueue_for_mode("s", msg("!late"), true);
    // Normal end: transfer (same call the post-turn handler makes now).
    inbox.transfer_next_step_to_next_turn("s");
    assert_eq!(inbox.pending("s"), (1, 0));
}
