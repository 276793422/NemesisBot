use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

#[tokio::test]
async fn test_signal_channel_new_validates() {
    let config = SignalConfig {
        api_url: String::new(),
        phone_number: String::new(),
        allow_from: Vec::new(),
    };
    assert!(SignalChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_signal_channel_lifecycle() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "signal");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_receive_url() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();
    assert_eq!(
        ch.receive_url(),
        "http://localhost:8080/v1/receive/+1234567890"
    );
}

#[test]
fn test_process_envelope_direct() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();

    let envelope = SignalEnvelopeInner {
        source: Some("Alice".to_string()),
        source_number: Some("+9876543210".to_string()),
        source_uuid: None,
        timestamp: Some(1234567890),
        data_message: Some(SignalDataMessage {
            timestamp: Some(1234567890),
            message: Some("Hello".to_string()),
            group_info: None,
        }),
        sync_message: None,
    };

    let (sender, chat, content) = ch.process_envelope(&envelope).unwrap();
    assert_eq!(sender, "+9876543210");
    assert_eq!(chat, "+9876543210");
    assert_eq!(content, "Hello");
}

#[test]
fn test_is_duplicate() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();

    assert!(!ch.is_duplicate(100));
    assert!(ch.is_duplicate(100));
    assert!(!ch.is_duplicate(200));
}

// ---- New tests ----

#[test]
fn test_signal_config_fields() {
    let config = SignalConfig {
        api_url: "http://localhost:9090".into(),
        phone_number: "+1112223333".into(),
        allow_from: vec!["+999".into()],
    };
    assert_eq!(config.api_url, "http://localhost:9090");
    assert_eq!(config.phone_number, "+1112223333");
}

#[test]
fn test_process_envelope_group_message() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".into(),
        phone_number: "+1234567890".into(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();

    let envelope = SignalEnvelopeInner {
        source: Some("Bob".into()),
        source_number: Some("+555".into()),
        source_uuid: None,
        timestamp: Some(999),
        data_message: Some(SignalDataMessage {
            timestamp: Some(999),
            message: Some("Group hello".into()),
            group_info: Some(SignalGroupInfo {
                group_id: Some("group-1".into()),
                name: Some("Test Group".into()),
            }),
        }),
        sync_message: None,
    };
    let (sender, chat, content) = ch.process_envelope(&envelope).unwrap();
    assert_eq!(sender, "+555");
    assert_eq!(chat, "group-1");
    assert_eq!(content, "Group hello");
}

#[test]
fn test_process_envelope_no_data_message() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".into(),
        phone_number: "+1234567890".into(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();

    let envelope = SignalEnvelopeInner {
        source: None,
        source_number: None,
        source_uuid: None,
        timestamp: None,
        data_message: None,
        sync_message: None,
    };
    assert!(ch.process_envelope(&envelope).is_none());
}

#[test]
fn test_is_duplicate_many() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".into(),
        phone_number: "+1234567890".into(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();

    for i in 0..100 {
        assert!(!ch.is_duplicate(i));
    }
    for i in 0..100 {
        assert!(ch.is_duplicate(i));
    }
}

#[tokio::test]
async fn test_signal_double_stop() {
    let config = SignalConfig {
        api_url: "http://localhost:8080".into(),
        phone_number: "+1234567890".into(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.stop().await.unwrap();
    ch.stop().await.unwrap();
}
