use super::*;

#[test]
fn test_device_event_format() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "1-2".to_string(),
        vendor: "TestVendor".to_string(),
        product: "TestProduct".to_string(),
        serial: "SN123".to_string(),
        capabilities: "input".to_string(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Connected"));
    assert!(msg.contains("TestVendor"));
    assert!(msg.contains("SN123"));
}

#[test]
fn test_usb_source_kind() {
    let source = UsbEventSource::new();
    assert_eq!(source.kind(), Kind::Usb);
}

#[test]
fn test_usb_source_default() {
    let source = UsbEventSource::default();
    assert_eq!(source.kind(), Kind::Usb);
}

#[test]
fn test_action_serialization() {
    assert_eq!(serde_json::to_string(&Action::Add).unwrap(), "\"add\"");
    assert_eq!(
        serde_json::to_string(&Action::Remove).unwrap(),
        "\"remove\""
    );
    assert_eq!(
        serde_json::to_string(&Action::Change).unwrap(),
        "\"change\""
    );
}

#[test]
fn test_action_deserialization() {
    assert_eq!(
        serde_json::from_str::<Action>("\"add\"").unwrap(),
        Action::Add
    );
    assert_eq!(
        serde_json::from_str::<Action>("\"remove\"").unwrap(),
        Action::Remove
    );
    assert_eq!(
        serde_json::from_str::<Action>("\"change\"").unwrap(),
        Action::Change
    );
}

#[test]
fn test_kind_serialization() {
    assert_eq!(serde_json::to_string(&Kind::Usb).unwrap(), "\"usb\"");
    assert_eq!(
        serde_json::to_string(&Kind::Bluetooth).unwrap(),
        "\"bluetooth\""
    );
    assert_eq!(serde_json::to_string(&Kind::Pci).unwrap(), "\"pci\"");
    assert_eq!(
        serde_json::to_string(&Kind::Generic).unwrap(),
        "\"generic\""
    );
}

#[test]
fn test_kind_deserialization() {
    assert_eq!(serde_json::from_str::<Kind>("\"usb\"").unwrap(), Kind::Usb);
    assert_eq!(
        serde_json::from_str::<Kind>("\"bluetooth\"").unwrap(),
        Kind::Bluetooth
    );
    assert_eq!(serde_json::from_str::<Kind>("\"pci\"").unwrap(), Kind::Pci);
    assert_eq!(
        serde_json::from_str::<Kind>("\"generic\"").unwrap(),
        Kind::Generic
    );
}

#[test]
fn test_action_equality() {
    assert_eq!(Action::Add, Action::Add);
    assert_ne!(Action::Add, Action::Remove);
}

#[test]
fn test_kind_equality() {
    assert_eq!(Kind::Usb, Kind::Usb);
    assert_ne!(Kind::Usb, Kind::Bluetooth);
}

#[test]
fn test_device_event_format_remove() {
    let event = DeviceEvent {
        action: Action::Remove,
        kind: Kind::Bluetooth,
        device_id: "bt-1".to_string(),
        vendor: "BTVendor".to_string(),
        product: "Headphones".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Disconnected"));
    assert!(msg.contains("Bluetooth"));
    assert!(!msg.contains("Serial:"));
    assert!(!msg.contains("Capabilities:"));
}

#[test]
fn test_device_event_format_change() {
    let event = DeviceEvent {
        action: Action::Change,
        kind: Kind::Pci,
        device_id: "pci-0".to_string(),
        vendor: "Intel".to_string(),
        product: "NIC".to_string(),
        serial: String::new(),
        capabilities: "network".to_string(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Changed"));
    assert!(msg.contains("network"));
}

#[test]
fn test_device_event_serialization() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "usb-1".to_string(),
        vendor: "Vendor".to_string(),
        product: "Product".to_string(),
        serial: "SN456".to_string(),
        capabilities: "storage".to_string(),
        raw: {
            let mut m = HashMap::new();
            m.insert("key".to_string(), "value".to_string());
            m
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"action\":\"add\""));
    assert!(json.contains("\"kind\":\"usb\""));
    assert!(json.contains("usb-1"));
    assert!(json.contains("SN456"));

    let parsed: DeviceEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.action, Action::Add);
    assert_eq!(parsed.kind, Kind::Usb);
    assert_eq!(parsed.device_id, "usb-1");
}

#[test]
fn test_device_event_debug() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Generic,
        device_id: "dev-1".to_string(),
        vendor: String::new(),
        product: String::new(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let debug = format!("{:?}", event);
    assert!(debug.contains("Add"));
    assert!(debug.contains("Generic"));
}

#[test]
fn test_usb_source_stop() {
    let source = UsbEventSource::new();
    assert!(source.stop().is_ok());
}

#[test]
fn test_device_event_empty_fields() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: String::new(),
        vendor: String::new(),
        product: String::new(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Connected"));
}

#[test]
fn test_device_event_with_raw_data() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "1-1".to_string(),
        vendor: "Vendor".to_string(),
        product: "Product".to_string(),
        serial: "SN".to_string(),
        capabilities: "input".to_string(),
        raw: {
            let mut m = HashMap::new();
            m.insert("ID_PATH".to_string(), "/devices/pci0000:00".to_string());
            m.insert("ACTION".to_string(), "add".to_string());
            m
        },
    };
    assert_eq!(event.raw.get("ID_PATH").unwrap(), "/devices/pci0000:00");
}

// ---- New tests ----

#[test]
fn test_action_clone() {
    let a = Action::Add;
    let b = a.clone();
    assert_eq!(a, b);
}

#[test]
fn test_kind_clone() {
    let k = Kind::Bluetooth;
    let k2 = k.clone();
    assert_eq!(k, k2);
}

#[test]
fn test_device_event_clone() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "test".into(),
        vendor: "V".into(),
        product: "P".into(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let cloned = event.clone();
    assert_eq!(cloned.device_id, "test");
}

#[test]
fn test_device_event_format_all_actions() {
    let base_event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "id".into(),
        vendor: "V".into(),
        product: "P".into(),
        serial: "SN".into(),
        capabilities: "cap".into(),
        raw: HashMap::new(),
    };

    let add_msg = DeviceEvent {
        action: Action::Add,
        ..base_event.clone()
    }
    .format_message();
    assert!(add_msg.contains("Connected"));

    let remove_msg = DeviceEvent {
        action: Action::Remove,
        ..base_event.clone()
    }
    .format_message();
    assert!(remove_msg.contains("Disconnected"));

    let change_msg = DeviceEvent {
        action: Action::Change,
        ..base_event
    }
    .format_message();
    assert!(change_msg.contains("Changed"));
}

#[test]
fn test_device_event_format_no_serial_no_caps() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Generic,
        device_id: "id".into(),
        vendor: "V".into(),
        product: "P".into(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(!msg.contains("Serial:"));
    assert!(!msg.contains("Capabilities:"));
    assert!(msg.contains("Connected"));
}

#[test]
fn test_device_event_format_with_serial_and_caps() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Usb,
        device_id: "id".into(),
        vendor: "V".into(),
        product: "P".into(),
        serial: "ABC123".into(),
        capabilities: "storage, input".into(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Serial: ABC123"));
    assert!(msg.contains("Capabilities: storage, input"));
}

#[test]
fn test_action_serde_roundtrip_all() {
    for action in [Action::Add, Action::Remove, Action::Change] {
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(action, parsed);
    }
}

#[test]
fn test_kind_serde_roundtrip_all() {
    for kind in [Kind::Usb, Kind::Bluetooth, Kind::Pci, Kind::Generic] {
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: Kind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, parsed);
    }
}

#[test]
fn test_device_event_roundtrip() {
    let event = DeviceEvent {
        action: Action::Remove,
        kind: Kind::Bluetooth,
        device_id: "bt-0".into(),
        vendor: "Vendor".into(),
        product: "Product".into(),
        serial: "SN".into(),
        capabilities: "audio".into(),
        raw: {
            let mut m = HashMap::new();
            m.insert("k".into(), "v".into());
            m
        },
    };
    let json = serde_json::to_string(&event).unwrap();
    let parsed: DeviceEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.action, Action::Remove);
    assert_eq!(parsed.kind, Kind::Bluetooth);
    assert_eq!(parsed.raw.get("k").unwrap(), "v");
}

#[test]
fn test_usb_event_source_default() {
    let source = UsbEventSource::default();
    assert_eq!(source.kind(), Kind::Usb);
}

#[test]
fn test_usb_source_stop_multiple_times() {
    let source = UsbEventSource::new();
    assert!(source.stop().is_ok());
    assert!(source.stop().is_ok());
    assert!(source.stop().is_ok());
}

#[test]
fn test_kind_debug_format() {
    assert!(format!("{:?}", Kind::Usb).contains("Usb"));
    assert!(format!("{:?}", Kind::Bluetooth).contains("Bluetooth"));
    assert!(format!("{:?}", Kind::Pci).contains("Pci"));
    assert!(format!("{:?}", Kind::Generic).contains("Generic"));
}

#[test]
fn test_usb_source_start_non_linux() {
    // On non-Linux (like Windows), start() should return an error
    let source = UsbEventSource::new();
    let result = source.start();
    // On Windows this should fail; on Linux it would succeed
    #[cfg(not(target_os = "linux"))]
    {
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Linux"));
    }
    #[cfg(target_os = "linux")]
    {
        assert!(result.is_ok());
    }
}

#[test]
fn test_action_debug_format() {
    assert!(format!("{:?}", Action::Add).contains("Add"));
    assert!(format!("{:?}", Action::Remove).contains("Remove"));
    assert!(format!("{:?}", Action::Change).contains("Change"));
}

#[test]
fn test_device_event_format_change_with_caps_and_serial() {
    let event = DeviceEvent {
        action: Action::Change,
        kind: Kind::Pci,
        device_id: "pci-0".to_string(),
        vendor: "Intel".to_string(),
        product: "NIC".to_string(),
        serial: "SN-PCI-001".to_string(),
        capabilities: "network,gigabit".to_string(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Changed"));
    assert!(msg.contains("Capabilities: network,gigabit"));
    assert!(msg.contains("Serial: SN-PCI-001"));
    assert!(msg.contains("Pci"));
}

#[test]
fn test_device_event_format_generic_kind() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Generic,
        device_id: "gen-0".to_string(),
        vendor: "V".to_string(),
        product: "P".to_string(),
        serial: String::new(),
        capabilities: String::new(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Connected"));
    assert!(msg.contains("Generic"));
}

#[test]
fn test_action_serialization_roundtrip() {
    for action in [Action::Add, Action::Remove, Action::Change] {
        let json = serde_json::to_string(&action).unwrap();
        let parsed: Action = serde_json::from_str(&json).unwrap();
        assert_eq!(action, parsed);
    }
}

#[test]
fn test_kind_serialization_roundtrip() {
    for kind in [Kind::Usb, Kind::Bluetooth, Kind::Pci, Kind::Generic] {
        let json = serde_json::to_string(&kind).unwrap();
        let parsed: Kind = serde_json::from_str(&json).unwrap();
        assert_eq!(kind, parsed);
    }
}

#[test]
fn test_device_event_format_bluetooth_add() {
    let event = DeviceEvent {
        action: Action::Add,
        kind: Kind::Bluetooth,
        device_id: "bt-0".to_string(),
        vendor: "BT Corp".to_string(),
        product: "Headset".to_string(),
        serial: "BT-SN-001".to_string(),
        capabilities: "audio".to_string(),
        raw: HashMap::new(),
    };
    let msg = event.format_message();
    assert!(msg.contains("Connected"));
    assert!(msg.contains("Bluetooth"));
    assert!(msg.contains("BT Corp"));
    assert!(msg.contains("Headset"));
    assert!(msg.contains("Serial: BT-SN-001"));
    assert!(msg.contains("Capabilities: audio"));
}

// ----: KERNEL/UDEV double-block duplicate events ----
//
// `udevadm monitor --property` prints every physical event twice: the raw
// kernel uevent (KERNEL block, sparse properties — no hwdb lookups) and the
// rule-processed udev event (UDEV block, full property set). Block-level
// event emission therefore duplicated every notification (and the KERNEL
// block surfaced as an "Unknown Device" because it lacks the ID_*_FROM_
// DATABASE enrichment).

/// Realistic `udevadm monitor --property` capture for one USB stick plug-in
/// (banner + KERNEL block + UDEV block, blank-line terminated).
const KERNEL_UDEV_ADD_SAMPLE: &str = "\
monitor will print the received events for:
UDEV - the event which udev sends out after rule processing
KERNEL - the kernel uevent

KERNEL[7341.176970] add      /devices/pci0000:00/0000:00:14.0/usb1/1-2 (usb)
ACTION=add
DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-2
SUBSYSTEM=usb
SEQNUM=2244
DEVNAME=/dev/bus/usb/001/004
DEVTYPE=usb_device
PRODUCT=781/9200/100

UDEV  [7341.191233] add      /devices/pci0000:00/0000:00:14.0/usb1/1-2 (usb)
ACTION=add
DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-2
SUBSYSTEM=usb
DEVNAME=/dev/bus/usb/001/004
DEVTYPE=usb_device
PRODUCT=781/9200/100
ID_VENDOR=SanDisk
ID_VENDOR_FROM_DATABASE=SanDisk Corp.
ID_MODEL=Cruzer_Blade
ID_MODEL_FROM_DATABASE=Cruzer Blade
ID_SERIAL_SHORT=4C531001590211109394
ID_USB_INTERFACES=:080650:

";

/// UDEV block for ACTION=bind (driver attachment — not a physical
/// connect/disconnect transition).
const UDEV_BIND_SAMPLE: &str = "\
UDEV  [7342.011002] bind      /devices/pci0000:00/0000:00:14.0/usb1/1-2 (usb)
ACTION=bind
DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-2
SUBSYSTEM=usb
DEVNAME=/dev/bus/usb/001/004
DRIVER=usb
ID_VENDOR_FROM_DATABASE=SanDisk Corp.

";

fn feed_sample(parser: &mut MonitorParser, sample: &str) -> Vec<DeviceEvent> {
    let mut events = Vec::new();
    for line in sample.lines() {
        if let Some(ev) = parser.feed_line(line) {
            events.push(ev);
        }
    }
    events
}

#[test]
fn bug194_kernel_and_udev_blocks_yield_single_event() {
    let mut parser = MonitorParser::default();
    let events = feed_sample(&mut parser, KERNEL_UDEV_ADD_SAMPLE);
    assert_eq!(
        events.len(),
        1,
        "one physical plug-in (KERNEL+UDEV double block) must surface exactly one event"
    );
    // The surviving event is the UDEV block with the enriched property set
    // (not the sparse KERNEL block that would render as "Unknown Device").
    assert_eq!(events[0].action, Action::Add);
    assert_eq!(events[0].vendor, "SanDisk Corp.");
    assert_eq!(events[0].product, "Cruzer Blade");
    assert_eq!(events[0].serial, "4C531001590211109394");
    assert_eq!(
        events[0].device_id,
        "/devices/pci0000:00/0000:00:14.0/usb1/1-2"
    );
}

#[test]
fn bug194_udev_remove_block_yields_remove_event() {
    let sample = "\
UDEV  [7350.400112] remove   /devices/pci0000:00/0000:00:14.0/usb1/1-2 (usb)
ACTION=remove
DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-2
SUBSYSTEM=usb
ID_VENDOR_FROM_DATABASE=SanDisk Corp.
ID_MODEL_FROM_DATABASE=Cruzer Blade

";
    let mut parser = MonitorParser::default();
    let events = feed_sample(&mut parser, sample);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].action, Action::Remove);
    assert!(events[0].format_message().contains("Disconnected"));
}

#[test]
fn bug194_bind_action_not_surfaced_as_connected() {
    let mut parser = MonitorParser::default();
    let events = feed_sample(&mut parser, UDEV_BIND_SAMPLE);
    assert!(
        events.is_empty(),
        "driver bind/unbind must not surface as a device notification, got {:?}",
        events
    );
}

#[test]
fn bug194_kernel_only_block_surfaces_nothing() {
    // A monitor configured to only print kernel uevents (or a udev rule
    // failure swallowing the UDEV block) must not fake an event out of the
    // sparse KERNEL block either.
    let sample = "\
KERNEL[7341.176970] add      /devices/pci0000:00/0000:00:14.0/usb1/1-2 (usb)
ACTION=add
DEVPATH=/devices/pci0000:00/0000:00:14.0/usb1/1-2
SUBSYSTEM=usb

";
    let mut parser = MonitorParser::default();
    let events = feed_sample(&mut parser, sample);
    assert!(events.is_empty());
}

// ---------- BUG-20261113-135：udevadm stdout 行读带帽 + 非 UTF-8 不杀监控 ----------

/// 经 pump_monitor_lines 把字节流泵进 parser，收集事件与全部投递行。
fn pump_bytes(input: &[u8]) -> (Vec<DeviceEvent>, Vec<String>) {
    let mut reader = std::io::BufReader::new(std::io::Cursor::new(input));
    let mut parser = MonitorParser::default();
    let mut events = Vec::new();
    let mut lines = Vec::new();
    let outcome = pump_monitor_lines(&mut reader, &mut |line: &str| {
        lines.push(line.to_string());
        if let Some(event) = parser.feed_line(line) {
            events.push(event);
        }
        true
    });
    assert!(
        outcome.is_ok(),
        "pump must not die on poison lines: {outcome:?}"
    );
    (events, lines)
}

#[test]
fn bug135_non_utf8_line_does_not_kill_monitor() {
    // 旧形态：非 UTF-8 一行 → lines() Err → Err(_) => break → 监控静默
    // 永久停摆（零日志）。新形态：整行收齐一次 lossy 收敛，监控存活，
    // 后续块照常解析出事件。
    let mut input = b"UDEV  [100.0] add      /devices/usb1/1-1 (usb)\n".to_vec();
    input.extend_from_slice(b"ACTION=add\n");
    input.extend_from_slice(b"ID_VENDOR=\xff\xfeBad\xfdName\n"); // 非 UTF-8 字节
    input.extend_from_slice(b"\n"); // 块结束 → add 事件
    input.extend_from_slice(b"UDEV  [200.0] remove   /devices/usb1/1-1 (usb)\n");
    input.extend_from_slice(b"ACTION=remove\n");
    input.extend_from_slice(b"\n");
    let (events, lines) = pump_bytes(&input);
    assert_eq!(events.len(), 2, "both blocks must surface: {events:?}");
    assert_eq!(events[0].action, Action::Add);
    assert_eq!(events[1].action, Action::Remove);
    // mojibake 行 lossy 投递（替换符），vendor 值降级但事件仍在。
    let poisoned = lines
        .iter()
        .find(|l| l.starts_with("ID_VENDOR="))
        .expect("poisoned property line must be delivered");
    assert!(
        poisoned.contains('\u{FFFD}'),
        "non-UTF-8 bytes must become replacement chars: {poisoned:?}"
    );
    assert!(events[0].vendor.contains('\u{FFFD}'));
}

#[test]
fn bug135_oversized_line_truncated_and_monitor_survives() {
    // 超 64KB 帽的毒丸行：截断丢弃至行尾，游标对齐下一行行首，
    // 监控存活、后续块照常出事件。
    let mut input = b"UDEV  [300.0] add      /devices/usb1/1-2 (usb)\n".to_vec();
    input.extend(vec![b'A'; MAX_MONITOR_LINE_BYTES + 16]);
    input.push(b'\n');
    input.extend_from_slice(b"ACTION=add\nID_MODEL=GoodDevice\n\n");
    let (events, lines) = pump_bytes(&input);
    assert_eq!(
        events.len(),
        1,
        "poison oversized line must not kill the monitor: {events:?}"
    );
    assert_eq!(events[0].action, Action::Add);
    assert_eq!(events[0].product, "GoodDevice");
    assert_eq!(
        lines[1].len(),
        MAX_MONITOR_LINE_BYTES,
        "truncated line delivers exactly cap bytes"
    );
}

#[test]
fn bug135_emit_false_stops_pump_early() {
    // 下游关闭（receiver drop）：emit 返回 false → 泵诚实提前收尾，
    // 不再消费后续字节。
    let input = b"line1\nline2\nline3\n";
    let mut reader = std::io::BufReader::new(std::io::Cursor::new(input));
    let mut count = 0;
    let outcome = pump_monitor_lines(&mut reader, &mut |_line| {
        count += 1;
        false
    });
    assert!(outcome.is_ok());
    assert_eq!(count, 1, "pump must stop at first false emit");
}
