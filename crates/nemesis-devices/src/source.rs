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
    /// The spawned `udevadm monitor` child, if running. Shared with the reader
    /// thread so either exit path (`stop()` or receiver drop) can reap it —
    /// udevadm monitor never exits on its own, so dropping the handle here
    /// would leak one thread + one process per start() (2026-09-02: 3 orphans
    /// hung the remote Linux nightly's stdout pipe; root-fixed here).
    child: std::sync::Arc<std::sync::Mutex<Option<std::process::Child>>>,
}

impl UsbEventSource {
    pub fn new() -> Self {
        Self {
            child: std::sync::Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Kill and reap the child process if present. Idempotent; the reader
    /// thread exits once stdout closes (EOF) after the kill.
    fn kill_child(child: &std::sync::Mutex<Option<std::process::Child>>) {
        if let Some(mut c) = child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for UsbEventSource {
    fn drop(&mut self) {
        // Receiver drop alone cannot wake the reader thread (it blocks in
        // read() until stdout closes) — kill the child here so a started
        // source dropped without stop() doesn't leak the process/thread.
        Self::kill_child(&self.child);
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

            // Spawn udevadm monitor
            let spawned = std::process::Command::new("udevadm")
                .args(["monitor", "--property", "--subsystem-match=usb"])
                .stdout(std::process::Stdio::piped())
                .spawn();

            if let Ok(mut child) = spawned {
                let stdout = child.stdout.take();
                // Hand the child to the shared slot BEFORE moving stdout into
                // the reader thread, so stop() can kill it at any time.
                *self.child.lock().unwrap() = Some(child);
                let child_slot = std::sync::Arc::clone(&self.child);
                std::thread::spawn(move || {
                    if let Some(stdout) = stdout {
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
                                            let action = match current_props
                                                .get("ACTION")
                                                .map(|s| s.as_str())
                                            {
                                                Some("add") => Action::Add,
                                                Some("remove") => Action::Remove,
                                                Some("change") => Action::Change,
                                                _ => Action::Add,
                                            };
                                            let event = DeviceEvent {
                                                action,
                                                kind: Kind::Usb,
                                                device_id: current_props
                                                    .get("DEVPATH")
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                vendor: current_props
                                                    .get("ID_VENDOR_FROM_DATABASE")
                                                    .or_else(|| current_props.get("ID_VENDOR"))
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                product: current_props
                                                    .get("ID_MODEL_FROM_DATABASE")
                                                    .or_else(|| current_props.get("ID_MODEL"))
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                serial: current_props
                                                    .get("ID_SERIAL_SHORT")
                                                    .cloned()
                                                    .unwrap_or_default(),
                                                capabilities: current_props
                                                    .get("ID_USB_INTERFACES")
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
                    // Reader done (stdout EOF after stop()/kill, or receiver
                    // dropped): reap the child so neither the thread nor the
                    // udevadm process leaks.
                    Self::kill_child(&child_slot);
                });
            }

            Ok(rx)
        }

        #[cfg(not(target_os = "linux"))]
        {
            Err("USB monitoring is only supported on Linux (requires udevadm)".to_string())
        }
    }

    fn stop(&self) -> Result<(), String> {
        // Kill the udevadm child (if any); the reader thread exits on stdout
        // EOF and reaps whatever it holds. Idempotent (no-op when not started).
        Self::kill_child(&self.child);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
