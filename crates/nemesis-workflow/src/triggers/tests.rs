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
    let mgr = TriggerManager::new();    let trigger = make_trigger("event", HashMap::from([("type", "file_created")]));
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

// =========================================================================
// W4a coverage batch — triggers.rs gap closure
// =========================================================================

/// CronTimezone::label (triggers.rs ~59-64).
#[test]
fn w4a_cron_timezone_labels() {
    assert_eq!(CronTimezone::Local.label(), "local");
    assert_eq!(CronTimezone::Utc.label(), "utc");
}

/// match_trigger_event skips triggers that aren't type "event" — a cron
/// trigger whose config happens to carry event_type must not match
/// (triggers.rs ~192).
#[test]
fn w4a_match_trigger_event_ignores_non_event_triggers() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger(
        "cron",
        HashMap::from([("event_type", "workflow.completed"), ("schedule", "0 * * * *")]),
    );
    mgr.register_trigger("cron_wf", trigger).unwrap();

    let mut data = HashMap::new();
    data.insert("status".to_string(), serde_json::json!("completed"));
    let event = crate::event_dispatcher::TriggerEvent::new("workflow.completed", data);
    assert!(mgr.match_trigger_event(&event).is_empty());
}

/// match_message skips triggers that aren't type "message" — a webhook
/// trigger whose config looks message-shaped must not match
/// (triggers.rs ~233).
#[test]
fn w4a_match_message_ignores_non_message_triggers() {
    let mgr = TriggerManager::new();
    let trigger = make_trigger("webhook", HashMap::from([("content", "hello")]));
    mgr.register_trigger("hook_wf", trigger).unwrap();

    let msg = InboundMessageRef {
        channel: "web",
        sender_id: "u",
        chat_id: "c",
        content: "hello",
    };
    assert!(mgr.match_message(&msg).is_empty());
}

/// match_event_data: a config key missing from the event data fails the
/// match; a glob that doesn't match also fails (triggers.rs ~262 + ~267-269).
#[test]
fn w4a_match_trigger_event_data_missing_key_and_glob_miss() {
    let mgr = TriggerManager::new();

    // Missing key: trigger wants "level", event only carries "status".
    let missing = make_trigger(
        "event",
        HashMap::from([("event_type", "app.log"), ("level", "error")]),
    );
    mgr.register_trigger("missing_key_wf", missing).unwrap();

    // Glob mismatch: pattern "err*" vs actual "info".
    let glob = make_trigger(
        "event",
        HashMap::from([("event_type", "app.log"), ("level", "err*")]),
    );
    mgr.register_trigger("glob_miss_wf", glob).unwrap();

    let mut data = HashMap::new();
    data.insert("status".to_string(), serde_json::json!("ok"));
    data.insert("level".to_string(), serde_json::json!("info"));
    let event = crate::event_dispatcher::TriggerEvent::new("app.log", data);

    let matched = mgr.match_trigger_event(&event);
    assert!(!matched.contains(&"missing_key_wf".to_string()));
    assert!(!matched.contains(&"glob_miss_wf".to_string()));
}

/// match_message_data: chat_id key is honoured, unknown keys are ignored
/// (triggers.rs ~285-286).
#[test]
fn w4a_match_message_chat_id_and_unknown_key() {
    let mgr = TriggerManager::new();

    // chat_id filter matches only the right conversation.
    let by_chat = make_trigger("message", HashMap::from([("chat_id", "room-7")]));
    mgr.register_trigger("chat_wf", by_chat).unwrap();

    // Unknown config keys are ignored (permissive), so this still matches.
    let with_unknown = make_trigger(
        "message",
        HashMap::from([("mystery_key", "whatever"), ("channel", "web")]),
    );
    mgr.register_trigger("unknown_key_wf", with_unknown).unwrap();

    let in_room = InboundMessageRef {
        channel: "web",
        sender_id: "u",
        chat_id: "room-7",
        content: "hi",
    };
    let matched = mgr.match_message(&in_room);
    assert!(matched.contains(&"chat_wf".to_string()));
    assert!(matched.contains(&"unknown_key_wf".to_string()));

    let other_room = InboundMessageRef {
        channel: "web",
        sender_id: "u",
        chat_id: "room-9",
        content: "hi",
    };
    let matched2 = mgr.match_message(&other_room);
    assert!(!matched2.contains(&"chat_wf".to_string()));
    assert!(matched2.contains(&"unknown_key_wf".to_string()));
}

/// register_workflow_triggers: registers every trigger from a Workflow
/// definition and propagates the first error (triggers.rs ~325-338).
#[test]
fn w4a_register_workflow_triggers_registers_and_errors() {
    let mgr = TriggerManager::new();

    let wf = crate::types::Workflow {
        name: "multi_wf".to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![
            crate::types::TriggerConfig {
                trigger_type: "cron".to_string(),
                config: HashMap::from([(
                    "schedule".to_string(),
                    serde_json::json!("0 * * * *"),
                )]),
            },
            crate::types::TriggerConfig {
                trigger_type: "webhook".to_string(),
                config: HashMap::new(),
            },
        ],
        nodes: vec![],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };
    mgr.register_workflow_triggers("multi_wf", &wf.triggers).unwrap();
    assert_eq!(mgr.list_triggers("multi_wf").len(), 2);
    assert!(mgr.get_cron_workflows().contains_key("multi_wf"));
    assert_eq!(mgr.get_webhook_workflows(), vec!["multi_wf"]);

    // An invalid type surfaces the error.
    let bad = vec![crate::types::TriggerConfig {
        trigger_type: "nonsense".to_string(),
        config: HashMap::new(),
    }];
    let err = mgr.register_workflow_triggers("bad_wf", &bad);
    assert!(err.is_err());
}

/// match_event (legacy path): a config key absent from the data map fails;
/// a non-matching glob fails (triggers.rs ~367 + ~375). The legacy matcher
/// compares trigger_type against the event_type string, so a registered
/// "event"-typed trigger matches the literal event_type "event".
#[test]
fn w4a_match_event_legacy_missing_key_and_glob_miss() {
    let mgr = TriggerManager::new();

    let missing = make_trigger("event", HashMap::from([("env", "prod")]));
    mgr.register_trigger("missing_wf", missing).unwrap();

    let glob = make_trigger("event", HashMap::from([("env", "stg*")]));
    mgr.register_trigger("glob_wf", glob).unwrap();

    // Region-only data (no env at all) exercises the literal missing-key arm.
    let mut no_env = HashMap::new();
    no_env.insert("region".to_string(), serde_json::json!("eu"));
    let matched2 = mgr.match_event("event", &no_env);
    assert!(!matched2.contains(&"missing_wf".to_string()));
    assert!(!matched2.contains(&"glob_wf".to_string()));

    // env present but non-matching values: plain mismatch + glob mismatch.
    let mut data = HashMap::new();
    data.insert("region".to_string(), serde_json::json!("eu"));
    data.insert("env".to_string(), serde_json::json!("dev"));
    let matched = mgr.match_event("event", &data);
    assert!(!matched.contains(&"missing_wf".to_string()));
    assert!(!matched.contains(&"glob_wf".to_string()));

    // Matching data resolves both.
    let mut ok_data = HashMap::new();
    ok_data.insert("env".to_string(), serde_json::json!("stg-9"));
    let matched3 = mgr.match_event("event", &ok_data);
    assert!(matched3.contains(&"glob_wf".to_string()));
}

/// match_glob: a middle segment that never appears makes the match fail
/// (triggers.rs ~422).
#[test]
fn w4a_match_glob_middle_part_missing() {
    assert!(match_glob("a*b*c", "aXXbXXc"));
    assert!(!match_glob("a*b*c", "aXXc"));
    assert!(!match_glob("a*b*c", "bXXbXXc"));
}
