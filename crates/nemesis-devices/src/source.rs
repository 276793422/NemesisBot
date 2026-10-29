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
            // W1（BUG-20261028-126 伴生）：spawn 失败此前被 `if let Ok` 吞掉，
            // udevadm 缺失时返回 Ok(rx) 假装在监控（事件永不到达、无错误面）。
            // 诚实传播 Err，由 DeviceService::start 的 source 循环记 error。
            let mut child = std::process::Command::new("udevadm")
                .args(["monitor", "--property", "--subsystem-match=usb"])
                .stdout(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| format!("udevadm spawn failed (USB monitoring unavailable): {e}"))?;

            {
                let stdout = child.stdout.take();
                // Hand the child to the shared slot BEFORE moving stdout into
                // the reader thread, so stop() can kill it at any time.
                *self.child.lock().unwrap() = Some(child);
                let child_slot = std::sync::Arc::clone(&self.child);
                std::thread::spawn(move || {
                    if let Some(stdout) = stdout {
                        use std::io::{BufRead, BufReader};
                        let reader = BufReader::new(stdout);
                        let mut parser = MonitorParser::default();

                        for line in reader.lines() {
                            match line {
                                Ok(l) => {
                                    if let Some(event) = parser.feed_line(&l) {
                                        if tx.blocking_send(event).is_err() {
                                            break;
                                        }
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

/// Emitter of a `udevadm monitor` block, named by the block header line
/// (`KERNEL[ts] add /devices/...` vs `UDEV  [ts] add /devices/...`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg(any(target_os = "linux", test))]
enum MonitorBlockSource {
    Kernel,
    Udev,
}

/// Classify a `udevadm monitor --property` line as a block header.
/// Header lines carry the emitter tag followed by a `[` (`KERNEL[ts] ...`,
/// `UDEV  [ts] ...`); property lines are KEY=value and banner lines lack
/// the bracket — all three must not match.
#[cfg(any(target_os = "linux", test))]
fn parse_block_header(line: &str) -> Option<MonitorBlockSource> {
    if line.starts_with("KERNEL[") {
        Some(MonitorBlockSource::Kernel)
    } else if line.starts_with("UDEV") && line.contains('[') {
        Some(MonitorBlockSource::Udev)
    } else {
        None
    }
}

/// Streaming parser for `udevadm monitor --property` stdout. Feed lines as
/// they arrive; each completed block (blank-line terminated) surfaces as at
/// most one DeviceEvent. Kept as a plain struct so the parse rules stay
/// testable off-Linux (the spawn path is cfg(linux)-only).
#[derive(Default)]
#[cfg(any(target_os = "linux", test))]
struct MonitorParser {
    props: HashMap<String, String>,
    source: Option<MonitorBlockSource>,
}

#[cfg(any(target_os = "linux", test))]
impl MonitorParser {
    /// Feed one raw stdout line; returns the event for a completed block.
    fn feed_line(&mut self, raw: &str) -> Option<DeviceEvent> {
        let line = raw.trim();
        if line.is_empty() {
            let event = if self.props.is_empty() {
                None
            } else {
                block_to_event(self.source, &self.props)
            };
            self.props.clear();
            self.source = None;
            return event;
        }
        if let Some(source) = parse_block_header(line) {
            self.source = Some(source);
        } else if let Some((k, v)) = line.split_once('=') {
            self.props.insert(k.to_string(), v.to_string());
        }
        None
    }
}

/// Map one accumulated property block to a DeviceEvent.
///
/// Only UDEV-source blocks surface as events: `udevadm monitor --property`
/// prints every physical transition twice — the raw kernel uevent (KERNEL
/// block, sparse properties, no hwdb lookups) and the rule-processed udev
/// event (UDEV block, full property set). Emitting both duplicated every
/// notification (and the KERNEL block rendered as an "Unknown Device").
/// Skipping KERNEL loses no property — the UDEV block is the same physical
/// event with the enriched set (BUG-20261030-194).
///
/// Actions other than add/remove/change (bind/unbind driver attachment,
/// online/offline power transitions) are not physical connect/disconnect
/// transitions — surfacing them would fabricate "Device Connected"
/// notifications, so they produce no event.
#[cfg(any(target_os = "linux", test))]
fn block_to_event(
    source: Option<MonitorBlockSource>,
    props: &HashMap<String, String>,
) -> Option<DeviceEvent> {
    if source != Some(MonitorBlockSource::Udev) {
        return None;
    }
    let action = match props.get("ACTION").map(|s| s.as_str()) {
        Some("add") => Action::Add,
        Some("remove") => Action::Remove,
        Some("change") => Action::Change,
        _ => return None,
    };
    Some(DeviceEvent {
        action,
        kind: Kind::Usb,
        device_id: props.get("DEVPATH").cloned().unwrap_or_default(),
        vendor: props
            .get("ID_VENDOR_FROM_DATABASE")
            .or_else(|| props.get("ID_VENDOR"))
            .cloned()
            .unwrap_or_default(),
        product: props
            .get("ID_MODEL_FROM_DATABASE")
            .or_else(|| props.get("ID_MODEL"))
            .cloned()
            .unwrap_or_default(),
        serial: props.get("ID_SERIAL_SHORT").cloned().unwrap_or_default(),
        capabilities: props.get("ID_USB_INTERFACES").cloned().unwrap_or_default(),
        raw: props.clone(),
    })
}

#[cfg(test)]
mod tests;
