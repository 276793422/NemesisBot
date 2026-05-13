//! Device event source, types, and EventSource trait.
//!
//! Mirrors Go devices/events/events.go and devices/source.go.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Device action type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Add,
    Remove,
    Change,
}

/// Device kind.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Usb,
    Bluetooth,
    Pci,
    Generic,
}

/// A device event with full metadata.
/// Mirrors Go DeviceEvent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceEvent {
    pub action: Action,
    pub kind: Kind,
    pub device_id: String,
    pub vendor: String,
    pub product: String,
    pub serial: String,
    pub capabilities: String,
    pub raw: HashMap<String, String>,
}

impl DeviceEvent {
    /// Format the event as a human-readable message.
    /// Mirrors Go DeviceEvent.FormatMessage.
    pub fn format_message(&self) -> String {
        let action_text = match self.action {
            Action::Add => "Connected",
            Action::Remove => "Disconnected",
            Action::Change => "Changed",
        };

        let mut msg = format!("Device {}\n", action_text);
        msg += &format!("Type: {:?}\n", self.kind);
        msg += &format!("Device: {} {}\n", self.vendor, self.product);
        if !self.capabilities.is_empty() {
            msg += &format!("Capabilities: {}\n", self.capabilities);
        }
        if !self.serial.is_empty() {
            msg += &format!("Serial: {}\n", self.serial);
        }
        msg
    }
}

/// EventSource trait for device monitoring.
/// Mirrors Go EventSource interface.
pub trait EventSource: Send + Sync {
    /// Return the kind of events this source produces.
    fn kind(&self) -> Kind;

    /// Start producing events. Returns a receiver channel.
    fn start(&self) -> Result<tokio::sync::mpsc::Receiver<DeviceEvent>, String>;

    /// Stop producing events.
    fn stop(&self) -> Result<(), String>;
}

/// USB event source using udevadm (Linux only).
/// On non-Linux platforms, start() returns an error.
pub struct UsbEventSource {
    running: std::sync::Mutex<bool>,
}

impl UsbEventSource {
    pub fn new() -> Self {
        Self {
            running: std::sync::Mutex::new(false),
        }
    }
}

impl Default for UsbEventSource {
    fn default() -> Self {
        Self::new()
    }
}

impl EventSource for UsbEventSource {
    fn kind(&self) -> Kind {
        Kind::Usb
    }

    fn start(&self) -> Result<tokio::sync::mpsc::Receiver<DeviceEvent>, String> {
        #[cfg(target_os = "linux")]
        {
            let (tx, rx) = tokio::sync::mpsc::channel(100);
            *self.running.lock().unwrap() = true;

            // Spawn udevadm monitor
            std::thread::spawn(move || {
                let output = std::process::Command::new("udevadm")
                    .args(["monitor", "--property", "--subsystem-match=usb"])
                    .stdout(std::process::Stdio::piped())
                    .spawn();

                if let Ok(mut child) = output {
                    if let Some(stdout) = child.stdout.take() {
                        use std::io::{BufRead, BufReader};
                        let reader = BufReader::new(stdout);
                        let mut current_props: HashMap<String, String> = HashMap::new();

                        for line in reader.lines() {
                            match line {
                                Ok(l) => {
                                    let l = l.trim().to_string();
                                    if l.is_empty() {
                                        // End of block - process accumulated properties
                                        if !current_props.is_empty() {
                                            let action = match current_props.get("ACTION").map(|s| s.as_str()) {
                                                Some("add") => Action::Add,
                                                Some("remove") => Action::Remove,
                                                Some("change") => Action::Change,
                                                _ => Action::Add,
                                            };
                                            let event = DeviceEvent {
                                                action,
                                                kind: Kind::Usb,
                                                device_id: current_props.get("DEVPATH")
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                vendor: current_props.get("ID_VENDOR_FROM_DATABASE")
                                                    .or_else(|| current_props.get("ID_VENDOR"))
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                product: current_props.get("ID_MODEL_FROM_DATABASE")
                                                    .or_else(|| current_props.get("ID_MODEL"))
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                serial: current_props.get("ID_SERIAL_SHORT")
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                capabilities: current_props.get("ID_USB_INTERFACES")
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                raw: current_props.clone(),
                                            };
                                            if tx.blocking_send(event).is_err() {
                                                break;
                                            }
                                        }
                                        current_props.clear();
                                    } else if let Some((k, v)) = l.split_once('=') {
                                        current_props.insert(k.to_string(), v.to_string());
                                    }
                                }
                                Err(_) => break,
                            }
                        }
                    }
                    let _ = child.wait();
                }
            });

            Ok(rx)
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err("USB monitoring is only supported on Linux (requires udevadm)".to_string())
        }
    }

    fn stop(&self) -> Result<(), String> {
        *self.running.lock().unwrap() = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(serde_json::to_string(&Action::Remove).unwrap(), "\"remove\"");
        assert_eq!(serde_json::to_string(&Action::Change).unwrap(), "\"change\"");
    }

    #[test]
    fn test_action_deserialization() {
        assert_eq!(serde_json::from_str::<Action>("\"add\"").unwrap(), Action::Add);
        assert_eq!(serde_json::from_str::<Action>("\"remove\"").unwrap(), Action::Remove);
        assert_eq!(serde_json::from_str::<Action>("\"change\"").unwrap(), Action::Change);
    }

    #[test]
    fn test_kind_serialization() {
        assert_eq!(serde_json::to_string(&Kind::Usb).unwrap(), "\"usb\"");
        assert_eq!(serde_json::to_string(&Kind::Bluetooth).unwrap(), "\"bluetooth\"");
        assert_eq!(serde_json::to_string(&Kind::Pci).unwrap(), "\"pci\"");
        assert_eq!(serde_json::to_string(&Kind::Generic).unwrap(), "\"generic\"");
    }

    #[test]
    fn test_kind_deserialization() {
        assert_eq!(serde_json::from_str::<Kind>("\"usb\"").unwrap(), Kind::Usb);
        assert_eq!(serde_json::from_str::<Kind>("\"bluetooth\"").unwrap(), Kind::Bluetooth);
        assert_eq!(serde_json::from_str::<Kind>("\"pci\"").unwrap(), Kind::Pci);
        assert_eq!(serde_json::from_str::<Kind>("\"generic\"").unwrap(), Kind::Generic);
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

        let add_msg = DeviceEvent { action: Action::Add, ..base_event.clone() }.format_message();
        assert!(add_msg.contains("Connected"));

        let remove_msg = DeviceEvent { action: Action::Remove, ..base_event.clone() }.format_message();
        assert!(remove_msg.contains("Disconnected"));

        let change_msg = DeviceEvent { action: Action::Change, ..base_event }.format_message();
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
}
