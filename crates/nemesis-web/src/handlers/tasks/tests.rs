//! Tests for the tasks handler — P1-2 (2026-08-24 UI entry gap) three-state
//! `max_rounds` parsing. Declared from `tasks.rs` so the private
//! `parse_max_rounds_patch` is reachable without constructing an AppState
//! (same pattern as `models/tests.rs`). The service-level three-state patch
//! semantics (absent/null/set on disk) are covered by
//! `nemesis-cron/src/service/tests.rs`; these tests pin the handler-side
//! parsing contract, in particular the loud rejection of present-but-invalid
//! values that would otherwise degrade to "clear" and silently wipe a job's
//! budget.

use super::*;

fn payload(v: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "max_rounds": v })
}

#[test]
fn max_rounds_absent_means_unchanged() {
    let data = serde_json::json!({ "id": "j1" });
    assert_eq!(parse_max_rounds_patch(&data).unwrap(), None);
}

#[test]
fn max_rounds_null_means_clear() {
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::Value::Null)).unwrap(),
        Some(None)
    );
}

#[test]
fn max_rounds_positive_int_sets() {
    for n in [1u64, 5, 20, u32::MAX as u64] {
        assert_eq!(
            parse_max_rounds_patch(&payload(serde_json::json!(n))).unwrap(),
            Some(Some(n as u32)),
            "n={n}"
        );
    }
}

#[test]
fn max_rounds_invalid_values_rejected_loudly() {
    // 0 is filtered to "no budget" downstream (loop.rs `*v > 0`), so
    // accepting it would mean unlimited-while-looking-like-zero.
    // Negative/fractional/string/bool/huge all degrade to "absent"/"clear"
    // under the old silent parse — all must error instead.
    let bad = [
        serde_json::json!(0),
        serde_json::json!(-5),
        serde_json::json!(5.5),
        serde_json::json!("10"),
        serde_json::json!(true),
        serde_json::json!((u32::MAX as u64) + 1),
        serde_json::json!([5]),
        serde_json::json!({ "v": 5 }),
    ];
    for v in bad {
        let err = parse_max_rounds_patch(&payload(v.clone())).unwrap_err();
        assert!(
            err.contains("max_rounds"),
            "value {v}: error should name the field, got '{err}'"
        );
    }
}

/// cron.add collapses the three states into Option<u32> (None = global
/// default): absent and null both land on None via flatten.
#[test]
fn max_rounds_flatten_gives_add_semantics() {
    assert_eq!(
        parse_max_rounds_patch(&serde_json::json!({})).unwrap().flatten(),
        None
    );
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::Value::Null))
            .unwrap()
            .flatten(),
        None
    );
    assert_eq!(
        parse_max_rounds_patch(&payload(serde_json::json!(7)))
            .unwrap()
            .flatten(),
        Some(7)
    );
}
