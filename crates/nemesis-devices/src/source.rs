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
mod tests;
