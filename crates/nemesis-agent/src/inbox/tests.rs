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
    }
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
    assert_eq!(
        inbox.enqueue("s", msg("normal one")),
        EnqueueOutcome::QueuedForNextTurn
    );
    assert_eq!(
        inbox.enqueue("s", msg("!urgent")),
        EnqueueOutcome::QueuedForNextStep
    );
    assert_eq!(
        inbox.enqueue("s", msg("normal two")),
        EnqueueOutcome::QueuedForNextTurn
    );
    // Capacity 3 reached → rejected.
    assert_eq!(inbox.enqueue("s", msg("overflow")), EnqueueOutcome::Rejected);
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
