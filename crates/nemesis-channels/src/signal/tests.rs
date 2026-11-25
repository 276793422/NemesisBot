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

    let (sender, chat, content, ts) = process_envelope(&envelope).unwrap();
    assert_eq!(sender, "+9876543210");
    assert_eq!(chat, "+9876543210");
    assert_eq!(content, "Hello");
    assert_eq!(ts, 1234567890);
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
                group_name: Some("Test Group".into()),
            }),
        }),
        sync_message: None,
    };
    let (sender, chat, content, ts) = process_envelope(&envelope).unwrap();
    assert_eq!(sender, "+555");
    assert_eq!(chat, "group-1");
    assert_eq!(content, "Group hello");
    assert_eq!(ts, 999);
}

#[test]
fn test_process_envelope_no_data_message() {
    let envelope = SignalEnvelopeInner {
        source: None,
        source_number: None,
        source_uuid: None,
        timestamp: None,
        data_message: None,
        sync_message: None,
    };
    assert!(process_envelope(&envelope).is_none());
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

/// BUG-20261125-019 回归锁：start/stop 必须维护 base running 旗标 + 幂等双启
/// 守卫（BUG-20261121-050 同构家族横向扫漏网补齐——manager 的 start_all 守卫 /
/// any_channel_running / status 上报三消费点都读 trait is_running）。
/// 服务端点指 loopback 死端口（后台 receive loop 连接失败零外网，start 本体
/// 不阻塞）；二次 start 走幂等守卫诚实跳过（不 spawn 第二个 receive loop）。
#[tokio::test]
async fn b019_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = SignalConfig {
        api_url: "http://127.0.0.1:1".to_string(),
        phone_number: "+10000000000".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _) = tokio::sync::broadcast::channel(16);
    let ch = SignalChannel::new(config, tx).unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 后 trait is_running 必须为 true（BUG-20261125-019）"
    );
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
}

/// BUG-20261125-025 回归锁：真实线上 wire 形态（signal-cli-rest-api swagger
/// `receive.MessageEnvelope`，camelCase 字段名）必须可反序列化并产出消息。
/// 修复前：结构体 snake_case 无 rename_all → serde 精确匹配，线上
/// `dataMessage`/`sourceNumber`/`groupInfo` 键静默忽略 → `data_message` 恒
/// None → receive loop 每条 continue = 100% 静默丢消息；旧测试 fixture 用
/// 结构体构造器自建值（与 wire 脱锚）解释了假绿。
#[test]
fn b025_wire_camelcase_envelope_deserializes_and_processes() {
    let raw = r#"{
        "envelope": {
            "source": "+4912345678",
            "sourceDevice": 2,
            "sourceName": "Alice",
            "sourceNumber": "+4912345678",
            "sourceUuid": "0c6d2654-4c1a-4a06-a797-9c1dcd1e7c99",
            "timestamp": 1700000000000,
            "dataMessage": {
                "timestamp": 1700000000000,
                "message": "Hello from wire",
                "expiresInSeconds": 0,
                "viewOnce": false
            }
        }
    }"#;
    let env: SignalEnvelope =
        serde_json::from_str(raw).expect("线上 swagger wire 形态必须可反序列化");
    let inner = env.envelope.expect("envelope 必须解出");
    assert_eq!(
        inner.source_number.as_deref(),
        Some("+4912345678"),
        "sourceNumber 必须落 source_number 字段（修复前恒 None）"
    );
    assert!(
        inner.data_message.is_some(),
        "dataMessage 必须落 data_message 字段（修复前恒 None = 100% 丢消息）"
    );

    let (sender, chat, content, ts) = process_envelope(&inner).unwrap();
    assert_eq!(sender, "+4912345678");
    assert_eq!(chat, "+4912345678");
    assert_eq!(content, "Hello from wire");
    assert_eq!(ts, 1700000000000);
}

/// BUG-20261125-025 回归锁（群消息 + sender 回落链）：wire `groupInfo.groupId`
/// 作为群 chat_id；sender 走 sourceNumber → source → sourceUuid 回落链
/// （隐藏号码账户只有 UUID 形态身份，修复前 sender 恒 "unknown"）。
#[test]
fn b025_wire_group_message_and_sender_fallback_chain() {
    let raw = r#"{
        "envelope": {
            "sourceUuid": "aaaabbbb-cccc-dddd-eeee-ffff00001111",
            "timestamp": 1700000000001,
            "dataMessage": {
                "timestamp": 1700000000001,
                "message": "group hi",
                "groupInfo": {
                    "groupId": "c2FmZQ==",
                    "groupName": "Test Group",
                    "revision": 3
                }
            }
        }
    }"#;
    let env: SignalEnvelope = serde_json::from_str(raw).expect("群消息 wire 形态必须可反序列化");
    let (sender, chat, content, _ts) =
        process_envelope(&env.envelope.expect("envelope 必须解出")).unwrap();
    assert_eq!(
        sender, "aaaabbbb-cccc-dddd-eeee-ffff00001111",
        "无 sourceNumber/source 时必须回落 sourceUuid"
    );
    assert_eq!(chat, "c2FmZQ==", "群 chat_id 必须取 groupId");
    assert_eq!(content, "group hi");
}

/// BUG-20261125-025 回归锁（sender 回落链全形态）：三字段齐备取优先级最高
/// 的 sourceNumber；全缺 = "unknown"（诚实兜底，与修复前行为一致）。
#[test]
fn b025_sender_chain_priority_and_unknown_fallback() {
    let mk = |sn: Option<&str>, s: Option<&str>, su: Option<&str>| SignalEnvelopeInner {
        source: s.map(|v| v.to_string()),
        source_number: sn.map(|v| v.to_string()),
        source_uuid: su.map(|v| v.to_string()),
        timestamp: None,
        data_message: Some(SignalDataMessage {
            timestamp: Some(1),
            message: Some("m".into()),
            group_info: None,
        }),
        sync_message: None,
    };
    assert_eq!(
        envelope_sender(&mk(Some("+111"), Some("+222"), Some("u1"))),
        "+111",
        "sourceNumber 优先级最高"
    );
    assert_eq!(
        envelope_sender(&mk(None, Some("+222"), Some("u1"))),
        "+222",
        "source 次之"
    );
    assert_eq!(
        envelope_sender(&mk(None, None, Some("u1"))),
        "u1",
        "sourceUuid 兜底"
    );
    let empty = SignalEnvelopeInner {
        source: None,
        source_number: None,
        source_uuid: None,
        timestamp: None,
        data_message: None,
        sync_message: None,
    };
    assert_eq!(
        envelope_sender(&empty),
        "unknown",
        "全缺 = unknown 诚实兜底"
    );
}

/// BUG-20261125-038 回归锁：去重表接线（修复前表存在但 receive loop 零调用，
/// signal-cli 长轮询重复投递的 envelope 原样喂 agent）。
#[test]
fn b038_should_publish_first_seen_then_duplicate() {
    let map = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    assert!(
        SignalChannel::should_publish(&map, 1700000000000),
        "首见放行"
    );
    assert!(
        !SignalChannel::should_publish(&map, 1700000000000),
        "重复丢弃"
    );
    assert!(
        SignalChannel::should_publish(&map, 1700000000001),
        "另一条放行"
    );
    // 表内容：两条登记
    assert_eq!(map.read().len(), 2);
}

/// BUG-20261125-038 回归锁：timestamp 0（process_envelope 对缺失 timestamp
/// 落 0）恒放行且不登记——若登记 0，全部缺时间戳消息折叠成一条。
#[test]
fn b038_should_publish_missing_timestamp_always_passes() {
    let map = Arc::new(parking_lot::RwLock::new(HashMap::new()));
    assert!(SignalChannel::should_publish(&map, 0));
    assert!(SignalChannel::should_publish(&map, 0));
    assert!(SignalChannel::should_publish(&map, 0));
    // 0 不得污染表
    assert_eq!(map.read().len(), 0, "缺时间戳不登记");
}

/// BUG-20261125-038 回归锁：实例方法与静态 helper 必须共享同一张表
/// （Arc 槽位接线语义——闭包登记实例方法可见）。
#[test]
fn b038_is_duplicate_delegates_to_shared_table() {
    let config = SignalConfig {
        api_url: "http://127.0.0.1:8080".to_string(),
        phone_number: "+10000000000".to_string(),
        allow_from: Vec::new(),
    };
    let ch = SignalChannel::new(config, test_bus()).unwrap();
    assert!(SignalChannel::should_publish(&ch.processed_timestamps, 42));
    assert!(ch.is_duplicate(42), "静态 helper 登记后实例方法必须可见");
    assert!(!ch.is_duplicate(43));
}
