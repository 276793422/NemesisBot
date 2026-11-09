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

/// BUG-20261109-034 回归锁：api_url 尾斜杠归一（构造期写回 = 3 个拼接
/// 消费点单一漏斗，与 discord/telegram/qq 的 BUG-20261109-012 同族）。
/// 用户从 signal-cli REST 文档复制带尾斜杠 base 时不得产生 `//v1/`
/// 双斜杠（signal-cli-rest-api 路由精确匹配 404，收发全挂）。
#[test]
fn signal_api_url_trailing_slash_normalized() {
    let config = SignalConfig {
        api_url: "http://127.0.0.1:8080/".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();
    assert_eq!(
        ch.config.api_url, "http://127.0.0.1:8080",
        "api_url 尾斜杠必须在构造期归一"
    );
    assert!(
        !ch.receive_url().contains("//v1"),
        "receive_url 不得残留双斜杠: {}",
        ch.receive_url()
    );
    assert_eq!(
        ch.receive_url(),
        "http://127.0.0.1:8080/v1/receive/+1234567890"
    );

    // 无尾斜杠形态行为不变（幂等）
    let cfg2 = SignalConfig {
        api_url: "http://127.0.0.1:8080".to_string(),
        phone_number: "+1234567890".to_string(),
        allow_from: Vec::new(),
    };
    let ch2 = SignalChannel::new(cfg2, test_bus()).unwrap();
    assert_eq!(ch2.receive_url(), ch.receive_url());
}
