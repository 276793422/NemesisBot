use super::*;

#[tokio::test]
async fn test_maixcam_channel_lifecycle() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    assert_eq!(ch.name(), "maixcam");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_listen_addr() {
    let config = MaixCamConfig {
        host: "0.0.0.0".to_string(),
        port: 9999,
        allow_from: Vec::new(),
    };
    let ch = MaixCamChannel::new(config).unwrap();
    assert_eq!(ch.listen_addr(), "0.0.0.0:9999");
}

#[test]
fn test_process_person_detected() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("person"));
    data.insert("score".to_string(), serde_json::json!(0.95));
    data.insert("x".to_string(), serde_json::json!(100.0));
    data.insert("y".to_string(), serde_json::json!(200.0));
    data.insert("w".to_string(), serde_json::json!(50.0));
    data.insert("h".to_string(), serde_json::json!(80.0));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: Some(1234567890.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            assert!(content.contains("Person detected"));
            assert!(content.contains("95.00%"));
            assert_eq!(metadata.get("class_name").unwrap(), "person");
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_process_heartbeat() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("heartbeat".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    assert!(matches!(event, MaixCamEvent::Heartbeat));
}

#[test]
fn test_build_command() {
    let cmd = MaixCamChannel::build_command("default", "take photo");
    assert_eq!(cmd.cmd_type, "command");
    assert_eq!(cmd.message, "take photo");
    assert_eq!(cmd.chat_id, "default");
}

#[tokio::test]
async fn test_send_no_clients() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[tokio::test]
async fn test_send_with_clients() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    ch.connect_client();

    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.unwrap();

    let outbound = ch.drain_outbound();
    assert_eq!(outbound.len(), 1);
}

#[test]
fn test_deserialize_message() {
    let json = r#"{"type":"person_detected","timestamp":1234.5,"data":{"class_name":"person","score":0.9}}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type.as_deref(), Some("person_detected"));
    assert_eq!(msg.timestamp.unwrap(), 1234.5);
}

// -- Additional tests --

#[test]
fn test_maixcam_config_default() {
    let config = MaixCamConfig::default();
    assert_eq!(config.host, "0.0.0.0");
    assert_eq!(config.port, 8888);
    assert!(config.allow_from.is_empty());
}

#[test]
fn test_maixcam_config_custom() {
    let config = MaixCamConfig {
        host: "192.168.1.1".into(),
        port: 9999,
        allow_from: vec!["device-1".into()],
    };
    assert_eq!(config.host, "192.168.1.1");
    assert_eq!(config.port, 9999);
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_build_command_fields() {
    let cmd = MaixCamChannel::build_command("chat-1", "take photo");
    assert_eq!(cmd.cmd_type, "command");
    assert_eq!(cmd.timestamp, 0.0);
    assert_eq!(cmd.message, "take photo");
    assert_eq!(cmd.chat_id, "chat-1");
}

#[test]
fn test_build_command_serialization() {
    let cmd = MaixCamChannel::build_command("chat-1", "hello");
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"command\""));
    assert!(json.contains("\"message\":\"hello\""));
    assert!(json.contains("\"chat_id\":\"chat-1\""));
}

#[test]
fn test_process_status_message() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("cpu".to_string(), serde_json::json!(45.2));
    data.insert("mem".to_string(), serde_json::json!(1024));

    let msg = MaixCamMessage {
        msg_type: Some("status".to_string()),
        tips: None,
        timestamp: Some(1234567890.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::StatusUpdate(info) => {
            assert!(!info.is_empty());
        }
        _ => panic!("expected StatusUpdate event"),
    }
}

#[test]
fn test_process_status_message_no_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("status".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::StatusUpdate(info) => {
            assert!(info.is_empty() || info.contains("None"));
        }
        _ => panic!("expected StatusUpdate event"),
    }
}

#[test]
fn test_process_unknown_message_type() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("custom_event".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::Unknown(name) => {
            assert_eq!(name, "custom_event");
        }
        _ => panic!("expected Unknown event"),
    }
}

#[test]
fn test_process_message_no_type() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: None,
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::Unknown(name) => {
            assert_eq!(name, "");
        }
        _ => panic!("expected Unknown event with empty name"),
    }
}

#[test]
fn test_client_count_tracking() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    assert_eq!(ch.client_count(), 0);

    ch.connect_client();
    assert_eq!(ch.client_count(), 1);

    ch.connect_client();
    assert_eq!(ch.client_count(), 2);

    ch.disconnect_client();
    assert_eq!(ch.client_count(), 1);

    // Disconnect below zero should not go negative
    ch.disconnect_client();
    ch.disconnect_client();
    assert_eq!(ch.client_count(), 0);
}

#[test]
fn test_person_detected_without_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, sender_id, chat_id } => {
            assert!(content.contains("Person detected"));
            assert!(content.contains("person")); // default class_name
            assert_eq!(sender_id, "maixcam");
            assert_eq!(chat_id, "default");
            // No timestamp in metadata when timestamp is None
            assert!(metadata.get("timestamp").is_none());
            assert_eq!(metadata.get("class_name").unwrap(), "person");
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_deserialize_message_minimal() {
    let json = r#"{}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert!(msg.msg_type.is_none());
    assert!(msg.tips.is_none());
    assert!(msg.timestamp.is_none());
    assert!(msg.data.is_none());
}

#[test]
fn test_deserialize_message_with_tips() {
    let json = r#"{"type":"heartbeat","tips":"system ok","timestamp":999.0}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type.as_deref(), Some("heartbeat"));
    assert_eq!(msg.tips.as_deref(), Some("system ok"));
}

#[test]
fn test_drain_outbound_empty() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let outbound = ch.drain_outbound();
    assert!(outbound.is_empty());
}

// ---- Additional coverage tests ----

#[tokio::test]
async fn test_send_not_running() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    // Not started
    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not running"));
}

#[tokio::test]
async fn test_stop_clears_state() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    ch.connect_client();
    assert_eq!(ch.client_count(), 1);

    ch.stop().await.unwrap();
    assert_eq!(ch.client_count(), 0);
    assert!(ch.drain_outbound().is_empty());
}

#[test]
fn test_process_message_with_tips() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("person"));
    data.insert("score".to_string(), serde_json::json!(0.8));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: Some("Detection alert".to_string()),
        timestamp: Some(1234567890.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            assert!(content.contains("Person detected"));
            assert!(content.contains("80.00%"));
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_process_message_with_coordinates() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("person"));
    data.insert("score".to_string(), serde_json::json!(0.75));
    data.insert("x".to_string(), serde_json::json!(10.0));
    data.insert("y".to_string(), serde_json::json!(20.0));
    data.insert("w".to_string(), serde_json::json!(100.0));
    data.insert("h".to_string(), serde_json::json!(200.0));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: Some(999.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            assert!(content.contains("Person detected"));
            assert!(content.contains("75.00%"));
            assert!(metadata.contains_key("score"));
            assert!(metadata.contains_key("class_name"));
            assert!(metadata.contains_key("timestamp"));
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[tokio::test]
async fn test_start_stop_multiple_cycles() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    for _ in 0..3 {
        ch.start().await.unwrap();
        assert!(*ch.running.read());
        ch.stop().await.unwrap();
        assert!(!*ch.running.read());
    }
}

#[test]
fn test_deserialize_status_with_data() {
    let json = r#"{"type":"status","timestamp":1234.5,"data":{"cpu":50.0,"mem":2048}}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type.as_deref(), Some("status"));
    assert!(msg.data.is_some());
}

#[tokio::test]
async fn test_send_queues_when_no_writers_but_has_count() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    // Manually set client count but don't add writers
    *ch.client_count.write() = 1;

    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Should queue message since no writers match but count > 0
    ch.send(msg).await.unwrap();
    let outbound = ch.drain_outbound();
    assert_eq!(outbound.len(), 1);
}

#[test]
fn test_build_command_serialization_roundtrip() {
    let cmd = MaixCamChannel::build_command("chat-1", "hello world");
    let json = serde_json::to_string(&cmd).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "command");
    assert_eq!(parsed["message"], "hello world");
    assert_eq!(parsed["chat_id"], "chat-1");
}

// --- Additional coverage tests ---

#[tokio::test]
async fn test_send_when_not_started() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    // Not started
    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not running"));
}

#[test]
fn test_disconnect_client_decrements() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.connect_client();
    ch.connect_client();
    assert_eq!(ch.client_count(), 2);
    ch.disconnect_client();
    assert_eq!(ch.client_count(), 1);
    ch.disconnect_client();
    assert_eq!(ch.client_count(), 0);
}

#[test]
fn test_disconnect_client_never_goes_negative() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    // Disconnect without connect
    ch.disconnect_client();
    assert_eq!(ch.client_count(), 0);
}

#[test]
fn test_drain_outbound_when_empty() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let drained = ch.drain_outbound();
    assert!(drained.is_empty());
}

#[test]
fn test_drain_outbound_multiple() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.outbound_queue.write().push(OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "c1".to_string(),
        content: "msg1".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    });
    ch.outbound_queue.write().push(OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "c2".to_string(),
        content: "msg2".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    });
    let drained = ch.drain_outbound();
    assert_eq!(drained.len(), 2);
    // Queue should be empty after drain
    assert!(ch.drain_outbound().is_empty());
}

#[test]
fn test_process_person_detected_with_timestamp() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("vehicle"));
    data.insert("score".to_string(), serde_json::json!(0.75));
    data.insert("x".to_string(), serde_json::json!(10.0));
    data.insert("y".to_string(), serde_json::json!(20.0));
    data.insert("w".to_string(), serde_json::json!(30.0));
    data.insert("h".to_string(), serde_json::json!(40.0));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: Some("alert".to_string()),
        timestamp: Some(1700000000.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, sender_id, chat_id } => {
            assert!(content.contains("vehicle"));
            assert!(content.contains("75.00%"));
            assert!(metadata.contains_key("timestamp"));
            assert!(metadata.contains_key("score"));
            assert_eq!(sender_id, "maixcam");
            assert_eq!(chat_id, "default");
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_process_person_detected_defaults() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            // Defaults: class_name="person", score=0.0, x/y/w/h=0.0
            assert!(content.contains("person"));
            assert!(content.contains("0.00%"));
            assert!(!metadata.contains_key("timestamp"));
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_deserialize_message_with_tips_field() {
    let json = r#"{"type":"person_detected","tips":"high confidence","timestamp":1234.5,"data":{}}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.tips.as_deref(), Some("high confidence"));
}

#[test]
fn test_deserialize_minimal_message() {
    let json = r#"{}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert!(msg.msg_type.is_none());
    assert!(msg.tips.is_none());
    assert!(msg.timestamp.is_none());
    assert!(msg.data.is_none());
}

#[tokio::test]
async fn test_stop_clears_writers() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    assert!(*ch.running.read());
    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
    assert!(ch.client_writers.is_empty());
    assert_eq!(*ch.client_count.read(), 0);
}

#[test]
fn test_listen_addr_custom() {
    let config = MaixCamConfig {
        host: "127.0.0.1".into(),
        port: 7777,
        allow_from: Vec::new(),
    };
    let ch = MaixCamChannel::new(config).unwrap();
    assert_eq!(ch.listen_addr(), "127.0.0.1:7777");
}

#[tokio::test]
async fn test_send_no_clients_returns_error() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    // client_count = 0, client_writers empty -> should error
    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "default".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("no connected") || err.contains("not running"));
}

#[test]
fn test_process_message_person_detected_no_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            assert!(content.contains("Person detected"));
            assert!(content.contains("0.00%"));
        }
        _ => panic!("expected PersonDetected event"),
    }
}

#[test]
fn test_process_message_status_with_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let mut data = HashMap::new();
    data.insert("cpu".to_string(), serde_json::json!(80.0));
    let msg = MaixCamMessage {
        msg_type: Some("status".to_string()),
        tips: None,
        timestamp: Some(1234.0),
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::StatusUpdate(data_str) => {
            assert!(data_str.contains("cpu"));
        }
        _ => panic!("expected StatusUpdate event"),
    }
}

#[test]
fn test_process_message_unknown_type() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let msg = MaixCamMessage {
        msg_type: Some("custom_event".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };
    let event = ch.process_message(&msg);
    assert!(matches!(event, MaixCamEvent::Unknown(ref s) if s == "custom_event"));
}

#[test]
fn test_process_message_empty_type() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    let msg = MaixCamMessage {
        msg_type: None,
        tips: None,
        timestamp: None,
        data: None,
    };
    let event = ch.process_message(&msg);
    assert!(matches!(event, MaixCamEvent::Unknown(ref s) if s.is_empty()));
}

#[test]
fn test_build_command_timestamp_zero() {
    let cmd = MaixCamChannel::build_command("test-chat", "test msg");
    assert_eq!(cmd.timestamp, 0.0);
    assert_eq!(cmd.cmd_type, "command");
    assert_eq!(cmd.chat_id, "test-chat");
    assert_eq!(cmd.message, "test msg");
}

#[test]
fn test_deserialize_maixcam_message_with_all_fields() {
    let json = r#"{"type":"person_detected","tips":"Alert!","timestamp":123456.789,"data":{"class_name":"cat","score":0.92}}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type.as_deref(), Some("person_detected"));
    assert_eq!(msg.tips.as_deref(), Some("Alert!"));
    assert_eq!(msg.timestamp.unwrap(), 123456.789);
    assert!(msg.data.is_some());
    let data = msg.data.unwrap();
    assert_eq!(data.get("class_name").unwrap().as_str(), Some("cat"));
}

#[test]
fn test_maixcam_command_serialize() {
    let cmd = MaixCamChannel::build_command("c1", "hello");
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"type\":\"command\""));
    assert!(json.contains("\"chat_id\":\"c1\""));
    assert!(json.contains("\"message\":\"hello\""));
}

#[tokio::test]
async fn test_start_stop_clears_writers_and_queue() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    ch.connect_client();
    ch.outbound_queue.write().push(OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "c1".to_string(),
        content: "msg".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    });
    assert_eq!(ch.client_count(), 1);
    assert_eq!(ch.outbound_queue.read().len(), 1);

    ch.stop().await.unwrap();
    assert_eq!(ch.client_count(), 0);
    assert!(ch.outbound_queue.read().is_empty());
    assert!(ch.client_writers.is_empty());
}

#[test]
fn test_maixcam_event_debug_format() {
    let event = MaixCamEvent::Heartbeat;
    let debug = format!("{:?}", event);
    assert!(debug.contains("Heartbeat"));

    let event = MaixCamEvent::Unknown("test".to_string());
    let debug = format!("{:?}", event);
    assert!(debug.contains("test"));

    let event = MaixCamEvent::StatusUpdate("cpu=80".to_string());
    let debug = format!("{:?}", event);
    assert!(debug.contains("cpu=80"));
}

// ============================================================
// Additional coverage tests for 95%+ target (round 2)
// ============================================================

#[test]
fn test_maixcam_event_person_detected_debug() {
    let event = MaixCamEvent::PersonDetected {
        content: "Test".to_string(),
        metadata: HashMap::new(),
        sender_id: "cam1".to_string(),
        chat_id: "chat1".to_string(),
    };
    let debug = format!("{:?}", event);
    assert!(debug.contains("PersonDetected"));
}

#[test]
fn test_person_detected_partial_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    // Only class_name, no coordinates
    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("cat"));
    data.insert("score".to_string(), serde_json::json!(0.5));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            assert!(content.contains("cat"));
            assert!(content.contains("50.00%"));
            // x/y/w/h default to 0.0
            assert!(content.contains("0"));
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_with_non_standard_class() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("vehicle"));
    data.insert("score".to_string(), serde_json::json!(1.0));

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: Some("High confidence detection".to_string()),
        timestamp: Some(9999.0),
        data: Some(data),
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            assert!(content.contains("vehicle"));
            assert!(content.contains("100.00%"));
            assert_eq!(metadata.get("class_name").unwrap(), "vehicle");
            assert!(metadata.contains_key("timestamp"));
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_status_with_no_data() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("status".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };

    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::StatusUpdate(data) => {
            assert!(data.is_empty() || data.contains("None"));
        }
        _ => panic!("expected StatusUpdate"),
    }
}

#[tokio::test]
async fn test_send_with_queued_messages_and_no_writers() {
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();
    ch.start().await.unwrap();
    // Set client count > 0 but no writers
    *ch.client_count.write() = 2;

    let msg = OutboundMessage {
        channel: "maixcam".to_string(),
        chat_id: "device-1".to_string(),
        content: "command".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.unwrap();

    let drained = ch.drain_outbound();
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].chat_id, "device-1");
    assert_eq!(drained[0].content, "command");
}

#[test]
fn test_build_command_empty_message() {
    let cmd = MaixCamChannel::build_command("chat-1", "");
    assert_eq!(cmd.message, "");
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"message\":\"\""));
}

// ============================================================
// Data-coercion default coverage (as_f64 / as_str unwrap_or arms)
// ============================================================

#[test]
fn test_person_detected_score_as_integer_defaults_to_zero() {
    // score provided as a JSON integer (not float): as_f64() still works for
    // integers, so this confirms integer scores are accepted.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("score".to_string(), serde_json::json!(1)); // integer
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            // Integer 1 coerces to 1.0 -> 100.00%
            assert!(content.contains("100.00%"));
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_score_as_string_defaults_to_zero() {
    // score provided as a string cannot be coerced via as_f64 -> defaults 0.0.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("score".to_string(), serde_json::json!("0.9")); // string
    data.insert("class_name".to_string(), serde_json::json!("dog"));
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            // String score falls back to 0.0 -> 0.00%
            assert!(content.contains("0.00%"));
            assert_eq!(metadata.get("score").unwrap(), "0.00");
            assert_eq!(metadata.get("class_name").unwrap(), "dog");
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_class_name_as_non_string_defaults_to_person() {
    // class_name provided as a number cannot be coerced via as_str -> "person".
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!(42)); // number
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, metadata, .. } => {
            assert!(content.contains("person"));
            assert_eq!(metadata.get("class_name").unwrap(), "person");
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_coordinates_as_strings_default_to_zero() {
    // x/y/w/h as strings -> as_f64 None -> all default to 0.0.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("person"));
    data.insert("score".to_string(), serde_json::json!(0.5));
    data.insert("x".to_string(), serde_json::json!("100"));
    data.insert("y".to_string(), serde_json::json!("200"));
    data.insert("w".to_string(), serde_json::json!("50"));
    data.insert("h".to_string(), serde_json::json!("80"));
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            // Coordinates default to 0; position line shows (0, 0) and size 0x0.
            assert!(content.contains("Position: (0, 0)"));
            assert!(content.contains("Size: 0x0"));
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_timestamp_negative_excluded_from_metadata() {
    // timestamp of Some(...) is always inserted; None is always skipped.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: None,
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { metadata, .. } => {
            assert!(!metadata.contains_key("timestamp"));
            // score + class_name are always present regardless of timestamp.
            assert!(metadata.contains_key("score"));
            assert!(metadata.contains_key("class_name"));
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_person_detected_full_metadata_keys() {
    // When all fields are present, metadata has exactly timestamp/class_name/score.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("cat"));
    data.insert("score".to_string(), serde_json::json!(0.42));
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: Some(1700000123.0),
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { metadata, .. } => {
            assert_eq!(metadata.len(), 3);
            assert_eq!(metadata.get("class_name").unwrap(), "cat");
            assert_eq!(metadata.get("score").unwrap(), "0.42");
            assert_eq!(metadata.get("timestamp").unwrap(), "1700000123");
        }
        _ => panic!("expected PersonDetected"),
    }
}

#[test]
fn test_status_update_includes_data_debug_repr() {
    // StatusUpdate formats the data HashMap with {:?}; verify the key appears.
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("battery".to_string(), serde_json::json!(87));
    let msg = MaixCamMessage {
        msg_type: Some("status".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::StatusUpdate(s) => {
            assert!(s.contains("battery"));
        }
        _ => panic!("expected StatusUpdate"),
    }
}

#[test]
fn test_deserialize_message_unknown_extra_fields_ignored() {
    // Unknown JSON fields must be ignored (no #[serde(deny_unknown_fields)]).
    let json = r#"{"type":"heartbeat","unexpected_field":true,"another":[1,2,3]}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type.as_deref(), Some("heartbeat"));
}

#[test]
fn test_deserialize_message_null_type() {
    // Explicit null for "type" -> Option<String> = None -> Unknown("").
    let json = r#"{"type":null}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    assert!(msg.msg_type.is_none());
}

#[test]
fn test_deserialize_message_with_nested_data() {
    let json = r#"{"type":"status","data":{"obj":{"nested":true},"arr":[1,2]}}"#;
    let msg: MaixCamMessage = serde_json::from_str(json).unwrap();
    let data = msg.data.unwrap();
    assert!(data.get("obj").is_some());
    assert!(data.get("arr").unwrap().is_array());
}

#[test]
fn test_maixcam_config_default_eq_manual_construction() {
    // Default impl must match the documented defaults (0.0.0.0:8888, empty allow_from).
    let default = MaixCamConfig::default();
    let manual = MaixCamConfig {
        host: "0.0.0.0".to_string(),
        port: 8888,
        allow_from: Vec::new(),
    };
    assert_eq!(default.host, manual.host);
    assert_eq!(default.port, manual.port);
    assert_eq!(default.allow_from.len(), manual.allow_from.len());
}

#[test]
fn test_build_command_serialization_has_timestamp_field() {
    // The serialized command must include the timestamp key (even though it's 0.0).
    let cmd = MaixCamChannel::build_command("c1", "hi");
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"timestamp\":0"));
    // Round-trip via Value to confirm structural correctness.
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["type"], "command");
    assert_eq!(v["timestamp"].as_f64(), Some(0.0));
}

#[test]
fn test_listen_addr_with_ipv6_host() {
    let config = MaixCamConfig {
        host: "::1".to_string(),
        port: 1234,
        allow_from: Vec::new(),
    };
    let ch = MaixCamChannel::new(config).unwrap();
    assert_eq!(ch.listen_addr(), "::1:1234");
}

#[test]
fn test_process_message_person_detected_full_content_format() {
    // Verify the full content template (all six fields formatted).
    let config = MaixCamConfig::default();
    let ch = MaixCamChannel::new(config).unwrap();

    let mut data = HashMap::new();
    data.insert("class_name".to_string(), serde_json::json!("vehicle"));
    data.insert("score".to_string(), serde_json::json!(0.875));
    data.insert("x".to_string(), serde_json::json!(12.0));
    data.insert("y".to_string(), serde_json::json!(34.0));
    data.insert("w".to_string(), serde_json::json!(56.0));
    data.insert("h".to_string(), serde_json::json!(78.0));
    let msg = MaixCamMessage {
        msg_type: Some("person_detected".to_string()),
        tips: None,
        timestamp: None,
        data: Some(data),
    };
    let event = ch.process_message(&msg);
    match event {
        MaixCamEvent::PersonDetected { content, .. } => {
            assert!(content.contains("Class: vehicle"));
            assert!(content.contains("Confidence: 87.50%"));
            assert!(content.contains("Position: (12, 34)"));
            assert!(content.contains("Size: 56x78"));
        }
        _ => panic!("expected PersonDetected"),
    }
}
