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
