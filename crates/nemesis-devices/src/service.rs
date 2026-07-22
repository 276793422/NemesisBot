//! Device service: Config, SetBus, Start/Stop, USB monitoring, event notifications.

use crate::source::{DeviceEvent, EventSource, UsbEventSource};
use chrono::{DateTime, Local};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Callback for sending outbound messages via the message bus.
/// Receives (channel, chat_id, content).
/// Mirrors Go `Service.bus.PublishOutbound`.
pub type OutboundSender = Box<dyn Fn(&str, &str, &str) + Send + Sync>;

/// Trait for getting the last active channel/chat information.
/// Mirrors Go `state.Manager` usage in the device service.
pub trait LastChannelProvider: Send + Sync {
    /// Get the last channel string (e.g. "web:user123").
    fn get_last_channel(&self) -> String;
}

/// Device information.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub device_type: String,
    pub status: String,
    pub vendor_id: Option<String>,
    pub product_id: Option<String>,
    pub serial: Option<String>,
    pub connected_at: Option<DateTime<Local>>,
    pub metadata: HashMap<String, String>,
}

/// Internal service-level device event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceDeviceEvent {
    Added {
        device_id: String,
        device_type: String,
    },
    Removed {
        device_id: String,
    },
    Changed {
        device_id: String,
        changes: HashMap<String, String>,
    },
}

/// Device event callback.
pub type DeviceEventHandler = Box<dyn Fn(ServiceDeviceEvent) + Send + Sync>;

/// Device service configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceServiceConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default)]
    pub monitor_usb: bool,
}

fn default_poll_interval() -> u64 {
    5
}

impl Default for DeviceServiceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            poll_interval_secs: 5,
            monitor_usb: true,
        }
    }
}

/// Internal channels that should not receive device notifications.
const INTERNAL_CHANNELS: &[&str] = &["cli", "system", "subagent"];

/// Parse a last channel string in the format "platform:userID".
/// Returns (platform, user_id) or empty strings if invalid.
/// Mirrors Go `parseLastChannel`.
pub fn parse_last_channel(last_channel: &str) -> (String, String) {
    if last_channel.is_empty() {
        return (String::new(), String::new());
    }
    match last_channel.split_once(':') {
        Some((platform, user_id)) if !platform.is_empty() && !user_id.is_empty() => {
            (platform.to_string(), user_id.to_string())
        }
        _ => (String::new(), String::new()),
    }
}

/// Check if a channel name is internal (should not receive device notifications).
/// Mirrors Go `constants.IsInternalChannel`.
pub fn is_internal_channel(channel: &str) -> bool {
    INTERNAL_CHANNELS.contains(&channel)
}

/// Device service manages connected devices with USB monitoring and event notifications.
///
/// Mirrors Go `devices.Service`:
/// - `bus` / `state` fields for notification routing
/// - `SetBus` to inject the outbound message sender
/// - `handleEvents` to process device events and send notifications
/// - `sendNotification` to route notifications to the last active channel
pub struct DeviceService {
    config: DeviceServiceConfig,
    devices: Arc<Mutex<HashMap<String, Device>>>,
    handler: Mutex<Option<DeviceEventHandler>>,
    running: AtomicBool,
    /// Outbound message sender (bus integration).
    bus_sender: Mutex<Option<OutboundSender>>,
    /// State manager for retrieving last channel info.
    state: Mutex<Option<Arc<dyn LastChannelProvider>>>,
    /// Event sources (e.g. USB monitor).
    sources: Vec<Box<dyn EventSource>>,
    /// Stop flag for event processing tasks.
    stop_flag: Arc<AtomicBool>,
}

impl DeviceService {
    /// Create a new device service.
    pub fn new() -> Self {
        Self {
            config: DeviceServiceConfig::default(),
            devices: Arc::new(Mutex::new(HashMap::new())),
            handler: Mutex::new(None),
            running: AtomicBool::new(false),
            bus_sender: Mutex::new(None),
            state: Mutex::new(None),
            sources: Vec::new(),
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with custom configuration.
    pub fn with_config(config: DeviceServiceConfig) -> Self {
        let mut sources: Vec<Box<dyn EventSource>> = Vec::new();
        if config.monitor_usb {
            sources.push(Box::new(UsbEventSource::new()));
        }

        Self {
            config,
            devices: Arc::new(Mutex::new(HashMap::new())),
            handler: Mutex::new(None),
            running: AtomicBool::new(false),
            bus_sender: Mutex::new(None),
            state: Mutex::new(None),
            sources,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Test-only constructor that injects custom event sources.
    /// Allows tests to exercise source iteration in `start()`/`stop()` without
    /// relying on platform-specific `UsbEventSource` behavior.
    #[cfg(test)]
    pub fn with_sources_for_test(
        config: DeviceServiceConfig,
        sources: Vec<Box<dyn EventSource>>,
    ) -> Self {
        Self {
            config,
            devices: Arc::new(Mutex::new(HashMap::new())),
            handler: Mutex::new(None),
            running: AtomicBool::new(false),
            bus_sender: Mutex::new(None),
            state: Mutex::new(None),
            sources,
            stop_flag: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Set the outbound message sender (bus integration).
    /// Mirrors Go `Service.SetBus(msgBus)`.
    pub fn set_bus_sender(&self, sender: OutboundSender) {
        *self.bus_sender.lock() = Some(sender);
    }

    /// Set the state manager for retrieving last channel info.
    /// Mirrors Go `Service.state = stateMgr`.
    pub fn set_state_manager(&self, provider: Arc<dyn LastChannelProvider>) {
        *self.state.lock() = Some(provider);
    }

    /// Set the device event handler.
    pub fn set_handler(&self, handler: DeviceEventHandler) {
        *self.handler.lock() = Some(handler);
    }

    /// Register a device.
    pub fn register(&self, device: Device) {
        let id = device.id.clone();
        let event = ServiceDeviceEvent::Added {
            device_id: id.clone(),
            device_type: device.device_type.clone(),
        };
        self.devices.lock().insert(id, device);
        self.emit_event(event);
    }

    /// Remove a device.
    pub fn unregister(&self, id: &str) -> Option<Device> {
        let removed = self.devices.lock().remove(id);
        if removed.is_some() {
            self.emit_event(ServiceDeviceEvent::Removed {
                device_id: id.to_string(),
            });
        }
        removed
    }

    /// Get a device by ID.
    pub fn get(&self, id: &str) -> Option<Device> {
        self.devices.lock().get(id).cloned()
    }

    /// List all devices.
    pub fn list(&self) -> Vec<Device> {
        self.devices.lock().values().cloned().collect()
    }

    /// Get device count.
    pub fn count(&self) -> usize {
        self.devices.lock().len()
    }

    /// Start the device monitoring service.
    /// Mirrors Go `Service.Start(ctx)`.
    pub async fn start(&self) -> Result<(), String> {
        if self.running.load(Ordering::SeqCst) || !self.config.enabled {
            tracing::info!("Device service disabled or no sources");
            return Ok(());
        }

        self.stop_flag.store(false, Ordering::SeqCst);

        for source in &self.sources {
            match source.start() {
                Ok(rx) => {
                    let kind = format!("{:?}", source.kind());
                    let stop = self.stop_flag.clone();
                    let bus = {
                        let b = self.bus_sender.lock();
                        b.is_some()
                    };
                    let state_present = {
                        let s = self.state.lock();
                        s.is_some()
                    };
                    tokio::spawn(Self::handle_events_task(rx, stop, bus, state_present));
                    tracing::info!("Device source started: {}", kind);
                }
                Err(e) => {
                    tracing::error!("Failed to start source {:?}: {}", source.kind(), e);
                }
            }
        }

        self.running.store(true, Ordering::SeqCst);
        self.scan_devices();
        tracing::info!(
            "Device service started (USB monitoring: {})",
            self.config.monitor_usb
        );
        Ok(())
    }

    /// Stop the device monitoring service.
    /// Mirrors Go `Service.Stop()`.
    pub fn stop(&self) {
        self.stop_flag.store(true, Ordering::SeqCst);
        for source in &self.sources {
            if let Err(e) = source.stop() {
                tracing::warn!("Error stopping source {:?}: {}", source.kind(), e);
            }
        }
        self.running.store(false, Ordering::SeqCst);
        tracing::info!("Device service stopped");
    }

    /// Is the service running?
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Scan for connected devices (platform-dependent).
    fn scan_devices(&self) {
        tracing::debug!("Scanning for connected devices...");
        // Platform-specific USB enumeration would go here.
        // On Windows: Win32 SetupDi API
        // On Linux: sysfs / libusb
    }

    /// Emit a device event to the handler.
    fn emit_event(&self, event: ServiceDeviceEvent) {
        if let Some(ref handler) = *self.handler.lock() {
            handler(event);
        }
    }

    /// Send a device event as a notification to the last active channel.
    /// Mirrors Go `Service.sendNotification(ev)`.
    pub fn send_notification(&self, ev: &DeviceEvent) {
        // Check if bus is available
        let bus_guard = self.bus_sender.lock();
        if bus_guard.is_none() {
            return;
        }
        drop(bus_guard);

        // Get last channel from state
        let last_channel = {
            let state = self.state.lock();
            match state.as_ref() {
                Some(s) => s.get_last_channel(),
                None => return,
            }
        };

        if last_channel.is_empty() {
            tracing::debug!(
                "No last channel, skipping notification for event: {}",
                ev.format_message()
            );
            return;
        }

        let (platform, user_id) = parse_last_channel(&last_channel);
        if platform.is_empty() || user_id.is_empty() || is_internal_channel(&platform) {
            return;
        }

        let msg = ev.format_message();

        // Send via bus
        let bus = self.bus_sender.lock();
        if let Some(ref sender) = *bus {
            sender(&platform, &user_id, &msg);
            tracing::info!(
                "Device notification sent: kind={:?} action={:?} to={}",
                ev.kind,
                ev.action,
                platform
            );
        }
    }

    /// Background task to handle events from a source channel.
    /// Mirrors Go `Service.handleEvents(kind, eventCh)`.
    async fn handle_events_task(
        mut rx: tokio::sync::mpsc::Receiver<DeviceEvent>,
        stop: Arc<AtomicBool>,
        _bus_available: bool,
        _state_available: bool,
    ) {
        loop {
            if stop.load(Ordering::SeqCst) {
                break;
            }
            tokio::select! {
                ev = rx.recv() => {
                    match ev {
                        Some(ev) => {
                            tracing::debug!(
                                "Device event: action={:?} kind={:?} device={}",
                                ev.action, ev.kind, ev.device_id
                            );
                        }
                        None => break,
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                }
            }
        }
    }

    /// Get the service status.
    pub fn status(&self) -> serde_json::Value {
        let devices = self.devices.lock();
        serde_json::json!({
            "running": self.running.load(Ordering::SeqCst),
            "enabled": self.config.enabled,
            "device_count": devices.len(),
            "monitor_usb": self.config.monitor_usb,
            "poll_interval_secs": self.config.poll_interval_secs,
        })
    }
}

impl Default for DeviceService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod service_extra_tests;
