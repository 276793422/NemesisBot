use super::*;

fn make_trigger(trigger_type: &str, config: HashMap<&str, &str>) -> TriggerConfig {
    let mut c = HashMap::new();
    for (k, v) in config {
        c.insert(k.to_string(), serde_json::json!(v));
    }
    TriggerConfig {
        trigger_type: trigger_type.to_string(),
        config: c,
    }
}

#[test]
fn test_register_cron_trigger() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("cron", HashMap::from([("schedule", "0 * * * *")]));
    mgr.register_trigger("test_wf", trigger).unwrap();

    let cron = mgr.get_cron_workflows();
    assert!(cron.contains_key("test_wf"));
    assert_eq!(cron["test_wf"], vec!["0 * * * *"]);
}

#[test]
fn test_register_cron_trigger_legacy_expression_field_still_tracked() {
    // Old YAML files may have written `expression:` instead of `schedule:`.
    // register_trigger accepts both so the cron cache stays populated even
    // when the wrong field name was used. The actual scheduler (engine.rs)
    // only honours `schedule`; this cache entry is informational.
    let mgr = TriggerManager::new();
    let trigger = make_trigger("cron", HashMap::from([("expression", "0 * * * *")]));
    mgr.register_trigger("test_wf", trigger).unwrap();

    let cron = mgr.get_cron_workflows();
    assert_eq!(cron["test_wf"], vec!["0 * * * *"]);
}

#[test]
fn test_register_unknown_trigger_type() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("unknown", HashMap::new());
    let result = mgr.register_trigger("test_wf", trigger);
    assert!(result.is_err());
}

#[test]
fn test_remove_trigger() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("webhook", HashMap::new());
    mgr.register_trigger("test_wf", trigger).unwrap();
    mgr.remove_trigger("test_wf");

    assert!(mgr.list_triggers("test_wf").is_empty());
    assert!(mgr.get_cron_workflows().is_empty());
}

#[test]
fn test_match_event() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
    mgr.register_trigger("file_processor", trigger).unwrap();

    let mut data = HashMap::new();
    data.insert("type".to_string(), serde_json::json!("file_created"));

    let matched = mgr.match_event("event", &data);
    assert_eq!(matched, vec!["file_processor"]);
}

#[test]
fn test_match_event_no_match() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
    mgr.register_trigger("file_processor", trigger).unwrap();

    let mut data = HashMap::new();
    data.insert("type".to_string(), serde_json::json!("file_deleted"));

    let matched = mgr.match_event("event", &data);
    assert!(matched.is_empty());
}

#[test]
fn test_match_event_no_filter() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::new());
    mgr.register_trigger("catch_all", trigger).unwrap();

    let data = HashMap::new();
    let matched = mgr.match_event("event", &data);
    assert_eq!(matched, vec!["catch_all"]);
}

#[test]
fn test_get_webhook_workflows() {
    let mgr = TriggerManager::new();
    mgr.register_trigger("wf1", make_trigger("webhook", HashMap::new()))
        .unwrap();
    mgr.register_trigger("wf2", make_trigger("cron", HashMap::new()))
        .unwrap();
    mgr.register_trigger("wf3", make_trigger("webhook", HashMap::new()))
        .unwrap();

    let mut webhooks = mgr.get_webhook_workflows();
    webhooks.sort();
    assert_eq!(webhooks, vec!["wf1", "wf3"]);
}

#[test]
fn test_list_all_triggers() {
    let mgr = TriggerManager::new();
    mgr.register_trigger("wf1", make_trigger("cron", HashMap::new()))
        .unwrap();
    mgr.register_trigger("wf2", make_trigger("webhook", HashMap::new()))
        .unwrap();

    let all = mgr.list_all_triggers();
    assert_eq!(all.len(), 2);
}

#[test]
fn test_glob_matching() {
    assert!(match_glob("foo*", "foobar"));
    assert!(match_glob("*bar", "foobar"));
    assert!(match_glob("foo*bar", "fooXbar"));
    assert!(!match_glob("foo*bar", "bazbar"));
    assert!(match_glob("exact", "exact"));
    assert!(!match_glob("exact", "other"));
}

// ============================================================
// Additional trigger tests: serialization, edge cases
// ============================================================

#[test]
fn test_trigger_config_serialization() {
    let config = TriggerConfig {
        trigger_type: "cron".to_string(),
        config: {
            let mut m = HashMap::new();
            m.insert("schedule".to_string(), serde_json::json!("0 * * * *"));
            m
        },
    };
    let json = serde_json::to_string(&config).unwrap();
    let restored: TriggerConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.trigger_type, "cron");
}

#[test]
fn test_trigger_manager_default() {
    let mgr = TriggerManager::default();
    assert!(mgr.list_all_triggers().is_empty());
}

#[test]
fn test_glob_matching_empty_pattern() {
    assert!(match_glob("", ""));
    assert!(!match_glob("", "something"));
}

#[test]
fn test_glob_matching_star_only() {
    assert!(match_glob("*", "anything"));
    assert!(match_glob("*", ""));
}

#[test]
fn test_glob_matching_multiple_stars() {
    assert!(match_glob("a*b*c", "aXbYc"));
    assert!(!match_glob("a*b*c", "aXbYd"));
}

#[test]
fn test_value_to_string() {
    assert_eq!(value_to_string(&serde_json::json!("hello")), "hello");
    assert_eq!(value_to_string(&serde_json::json!(42)), "42");
    assert_eq!(value_to_string(&serde_json::json!(true)), "true");
    assert_eq!(value_to_string(&serde_json::json!(null)), "null");
}

#[test]
fn test_register_multiple_triggers_same_workflow() {
    let mgr = TriggerManager::new();
    mgr.register_trigger(
        "wf1",
        make_trigger("cron", HashMap::from([("schedule", "0 * * * *")])),
    )
    .unwrap();
    // Re-registering should update
    mgr.register_trigger(
        "wf1",
        make_trigger("cron", HashMap::from([("schedule", "0 0 * * *")])),
    )
    .unwrap();
    let cron = mgr.get_cron_workflows();
    assert!(cron.contains_key("wf1"));
}

#[test]
fn test_remove_nonexistent_trigger() {
    let mgr = TriggerManager::new();
    // Should not panic
    mgr.remove_trigger("nonexistent");
}

#[test]
fn test_list_triggers_for_specific_workflow() {
    let mgr = TriggerManager::new();
    mgr.register_trigger("wf1", make_trigger("cron", HashMap::new()))
        .unwrap();
    mgr.register_trigger("wf2", make_trigger("webhook", HashMap::new()))
        .unwrap();

    let wf1_triggers = mgr.list_triggers("wf1");
    assert_eq!(wf1_triggers.len(), 1);
    let wf2_triggers = mgr.list_triggers("wf2");
    assert_eq!(wf2_triggers.len(), 1);
    let wf3_triggers = mgr.list_triggers("wf3");
    assert!(wf3_triggers.is_empty());
}

#[test]
fn test_match_event_with_glob_filter() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::from([("type", "file_*")]));
    mgr.register_trigger("glob_processor", trigger).unwrap();

    let mut data = HashMap::new();
    data.insert("type".to_string(), serde_json::json!("file_created"));
    let matched = mgr.match_event("event", &data);
    assert_eq!(matched, vec!["glob_processor"]);
}

#[test]
fn test_match_event_wrong_channel() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
    mgr.register_trigger("file_processor", trigger).unwrap();

    let mut data = HashMap::new();
    data.insert("type".to_string(), serde_json::json!("file_created"));
    // Matching against a different channel should not match
    let matched = mgr.match_event("webhook", &data);
    assert!(matched.is_empty());
}

// ============================================================
// match_trigger_event: typed TriggerEvent path (the "real" event matcher)
// ============================================================

use crate::event_dispatcher::TriggerEvent;
use std::collections::HashMap as StdHashMap;

fn make_typed_event(event_type: &str, data: &[(&str, serde_json::Value)]) -> TriggerEvent {
    let mut m = StdHashMap::new();
    for (k, v) in data {
        m.insert(k.to_string(), v.clone());
    }
    TriggerEvent::new(event_type, m)
}

#[test]
fn match_trigger_event_matches_when_event_type_filter_matches() {
    let mgr = TriggerManager::new();
    // event trigger filtering on event_type=workflow.completed
    let trigger = make_trigger(
        "event",
        HashMap::from([("event_type", "workflow.completed")]),
    );
    mgr.register_trigger("on_complete", trigger).unwrap();

    let ev = make_typed_event("workflow.completed", &[]);
    let matched = mgr.match_trigger_event(&ev);
    assert_eq!(matched, vec!["on_complete"]);
}

#[test]
fn match_trigger_event_supports_glob_event_type() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::from([("event_type", "workflow.*")]));
    mgr.register_trigger("any_workflow_event", trigger).unwrap();

    let ev = make_typed_event("workflow.failed", &[]);
    assert_eq!(mgr.match_trigger_event(&ev), vec!["any_workflow_event"]);

    let ev = make_typed_event("forge.pattern_created", &[]);
    assert!(mgr.match_trigger_event(&ev).is_empty());
}

#[test]
fn match_trigger_event_supports_additional_data_matchers() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger(
        "event",
        HashMap::from([("event_type", "workflow.completed"), ("status", "success")]),
    );
    mgr.register_trigger("on_success", trigger).unwrap();

    // status=success → matches
    let ev = make_typed_event(
        "workflow.completed",
        &[("status", serde_json::json!("success"))],
    );
    assert_eq!(mgr.match_trigger_event(&ev), vec!["on_success"]);

    // status=failed → no match
    let ev = make_typed_event(
        "workflow.completed",
        &[("status", serde_json::json!("failed"))],
    );
    assert!(mgr.match_trigger_event(&ev).is_empty());
}

#[test]
fn match_trigger_event_ignores_triggers_without_event_type_key() {
    // An event trigger without `event_type` in config is malformed and ignored.
    let mgr = TriggerManager::new();
    let trigger = make_trigger("event", HashMap::new());
    mgr.register_trigger("malformed", trigger).unwrap();

    let ev = make_typed_event("anything", &[]);
    assert!(mgr.match_trigger_event(&ev).is_empty());
}

// ============================================================
// match_message: inbound bus message path
// ============================================================

#[test]
fn match_message_matches_by_channel_only() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("message", HashMap::from([("channel", "web")]));
    mgr.register_trigger("web_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "user1",
        chat_id: "chat1",
        content: "anything",
    };
    assert_eq!(mgr.match_message(&msg), vec!["web_wf"]);
}

#[test]
fn match_message_supports_glob_channel() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("message", HashMap::from([("channel", "*")]));
    mgr.register_trigger("any_channel_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "telegram",
        sender_id: "u",
        chat_id: "c",
        content: "x",
    };
    assert_eq!(mgr.match_message(&msg), vec!["any_channel_wf"]);
}

#[test]
fn match_message_supports_content_glob() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger(
        "message",
        HashMap::from([("channel", "web"), ("content", "/cmd *")]),
    );
    mgr.register_trigger("slash_cmd_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "u",
        chat_id: "c",
        content: "/cmd arg1",
    };
    assert_eq!(mgr.match_message(&msg), vec!["slash_cmd_wf"]);

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "u",
        chat_id: "c",
        content: "not a slash command",
    };
    assert!(mgr.match_message(&msg).is_empty());
}

#[test]
fn match_message_filters_by_sender_id() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger(
        "message",
        HashMap::from([("channel", "web"), ("sender_id", "admin")]),
    );
    mgr.register_trigger("admin_only_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "admin",
        chat_id: "c",
        content: "x",
    };
    assert_eq!(mgr.match_message(&msg), vec!["admin_only_wf"]);

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "guest",
        chat_id: "c",
        content: "x",
    };
    assert!(mgr.match_message(&msg).is_empty());
}

#[test]
fn match_message_empty_config_matches_everything() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("message", HashMap::new());
    mgr.register_trigger("catchall_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "any",
        sender_id: "any",
        chat_id: "any",
        content: "any",
    };
    assert_eq!(mgr.match_message(&msg), vec!["catchall_wf"]);
}
