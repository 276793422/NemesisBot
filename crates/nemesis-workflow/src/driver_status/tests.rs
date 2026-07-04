use super::*;

#[test]
fn cron_and_webhook_are_driven() {
    assert!(driver_status_for("cron").driven);
    assert!(driver_status_for("webhook").driven);
}

#[test]
fn all_known_trigger_types_are_driven() {
    // After P3 wired EventDispatcher + inbound-bus subscription, all four
    // trigger types have runtime drivers. This test pins that invariant.
    assert!(driver_status_for("cron").driven);
    assert!(driver_status_for("webhook").driven);
    assert!(driver_status_for("event").driven);
    assert!(driver_status_for("message").driven);
}

#[test]
fn unknown_type_is_undriven_with_unknown_in_reason() {
    let s = driver_status_for("not_a_real_trigger");
    assert!(!s.driven);
    assert!(s.reason.as_ref().unwrap().contains("unknown"));
}

#[test]
fn all_driver_statuses_covers_known_types() {
    let m = all_driver_statuses();
    assert!(m.contains_key("cron"));
    assert!(m.contains_key("webhook"));
    assert!(m.contains_key("event"));
    assert!(m.contains_key("message"));
    assert_eq!(m.len(), 4);
}

#[test]
fn driven_status_omits_reason_field_when_serialized() {
    let s = driver_status_for("cron");
    let json = serde_json::to_string(&s).unwrap();
    assert!(!json.contains("reason"), "got: {}", json);
}

#[test]
fn undriven_status_includes_reason_field_when_serialized() {
    let s = driver_status_for("not_a_real_trigger");
    let json = serde_json::to_string(&s).unwrap();
    assert!(json.contains("reason"));
}
