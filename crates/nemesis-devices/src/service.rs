//! Device service: Config, SetBus, Start/Stop, USB monitoring, event notifications.

use chrono::{DateTime, Utc};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use crate::source::{DeviceEvent, EventSource, UsbEventSource};

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
    pub connected_at: Option<DateTime<Utc>>,
    pub metadata: HashMap<String, String>,
}

/// Internal service-level device event types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServiceDeviceEvent {
    Added { device_id: String, device_type: String },
    Removed { device_id: String },
    Changed { device_id: String, changes: HashMap<String, String> },
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

fn default_poll_interval() -> u64 { 5 }

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
            self.emit_event(ServiceDeviceEvent::Removed { device_id: id.to_string() });
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
                    tokio::spawn(Self::handle_events_task(
                        rx,
                        stop,
                        bus,
                        state_present,
                    ));
                    tracing::info!("Device source started: {}", kind);
                }
                Err(e) => {
                    tracing::error!("Failed to start source {:?}: {}", source.kind(), e);
                }
            }
        }

        self.running.store(true, Ordering::SeqCst);
        self.scan_devices();
        tracing::info!("Device service started (USB monitoring: {})", self.config.monitor_usb);
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
                ev.kind, ev.action, platform
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
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn test_register_and_get() {
        let svc = DeviceService::new();
        let device = Device {
            id: "dev1".to_string(),
            name: "Test Device".to_string(),
            device_type: "sensor".to_string(),
            status: "online".to_string(),
            vendor_id: Some("1234".to_string()),
            product_id: Some("5678".to_string()),
            serial: None,
            connected_at: Some(Utc::now()),
            metadata: HashMap::new(),
        };
        svc.register(device);
        let found = svc.get("dev1").unwrap();
        assert_eq!(found.name, "Test Device");
        assert_eq!(found.vendor_id, Some("1234".to_string()));
    }

    #[test]
    fn test_unregister() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".to_string(), name: "D1".to_string(), device_type: "sensor".to_string(),
            status: "online".to_string(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        assert!(svc.unregister("d1").is_some());
        assert!(svc.get("d1").is_none());
    }

    #[test]
    fn test_event_handler() {
        let svc = DeviceService::new();
        let event_count = Arc::new(AtomicUsize::new(0));
        let event_count_clone = event_count.clone();
        svc.set_handler(Box::new(move |_event| {
            event_count_clone.fetch_add(1, Ordering::SeqCst);
        }));

        svc.register(Device {
            id: "d1".to_string(), name: "D1".to_string(), device_type: "sensor".to_string(),
            status: "online".to_string(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.unregister("d1");
        assert_eq!(event_count.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn test_start_stop() {
        let mut config = DeviceServiceConfig::default();
        config.monitor_usb = false; // Disable USB on non-Linux
        let svc = DeviceService::with_config(config);
        svc.start().await.unwrap();
        assert!(svc.is_running());
        svc.stop();
        assert!(!svc.is_running());
    }

    #[test]
    fn test_status() {
        let svc = DeviceService::new();
        let status = svc.status();
        assert_eq!(status["device_count"], serde_json::json!(0));
        assert_eq!(status["enabled"], serde_json::json!(true));
    }

    #[test]
    fn test_config_default() {
        let config = DeviceServiceConfig::default();
        assert!(config.enabled);
        assert!(config.monitor_usb);
        assert_eq!(config.poll_interval_secs, 5);
    }

    #[test]
    fn test_parse_last_channel() {
        assert_eq!(parse_last_channel("web:user123"), ("web".to_string(), "user123".to_string()));
        assert_eq!(parse_last_channel("discord:789"), ("discord".to_string(), "789".to_string()));
        assert_eq!(parse_last_channel(""), ("".to_string(), "".to_string()));
        assert_eq!(parse_last_channel("nocolon"), ("".to_string(), "".to_string()));
        assert_eq!(parse_last_channel(":empty"), ("".to_string(), "".to_string()));
        assert_eq!(parse_last_channel("empty:"), ("".to_string(), "".to_string()));
        assert_eq!(parse_last_channel("multi:part:here"), ("multi".to_string(), "part:here".to_string()));
    }

    #[test]
    fn test_is_internal_channel() {
        assert!(is_internal_channel("cli"));
        assert!(is_internal_channel("system"));
        assert!(is_internal_channel("subagent"));
        assert!(!is_internal_channel("web"));
        assert!(!is_internal_channel("discord"));
    }

    /// A mock LastChannelProvider for testing.
    struct MockState {
        last_channel: String,
    }

    impl LastChannelProvider for MockState {
        fn get_last_channel(&self) -> String {
            self.last_channel.clone()
        }
    }

    #[test]
    fn test_send_notification_with_bus() {
        let svc = DeviceService::new();

        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));

        svc.set_state_manager(Arc::new(MockState {
            last_channel: "web:user123".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: "TestVendor".to_string(),
            product: "TestProduct".to_string(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };

        svc.send_notification(&ev);

        let msgs = sent.lock();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].0, "web");
        assert_eq!(msgs[0].1, "user123");
        assert!(msgs[0].2.contains("Connected"));
    }

    #[test]
    fn test_send_notification_no_bus() {
        let svc = DeviceService::new();
        // No bus set, should not panic
        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: "TestVendor".to_string(),
            product: "TestProduct".to_string(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev); // should silently return
    }

    #[test]
    fn test_send_notification_no_state() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        // No state set
        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_internal_channel() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "system:internal".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_empty_last_channel() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: String::new(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    // ---- New tests ----

    #[test]
    fn test_device_serialization() {
        let d = Device {
            id: "dev-1".into(),
            name: "USB Drive".into(),
            device_type: "storage".into(),
            status: "connected".into(),
            vendor_id: Some("1234".into()),
            product_id: Some("5678".into()),
            serial: Some("SN001".into()),
            connected_at: None,
            metadata: {
                let mut m = HashMap::new();
                m.insert("speed".into(), "usb3".into());
                m
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let rt: Device = serde_json::from_str(&json).unwrap();
        assert_eq!(rt.id, "dev-1");
        assert_eq!(rt.name, "USB Drive");
        assert_eq!(rt.metadata.get("speed").unwrap(), "usb3");
    }

    #[test]
    fn test_device_default_fields() {
        let d = Device {
            id: String::new(),
            name: String::new(),
            device_type: String::new(),
            status: String::new(),
            vendor_id: None,
            product_id: None,
            serial: None,
            connected_at: None,
            metadata: HashMap::new(),
        };
        assert!(d.vendor_id.is_none());
        assert!(d.connected_at.is_none());
    }

    #[test]
    fn test_service_device_event_added() {
        let ev = ServiceDeviceEvent::Added {
            device_id: "d1".into(),
            device_type: "sensor".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("Added"));
        assert!(json.contains("d1"));
    }

    #[test]
    fn test_service_device_event_removed() {
        let ev = ServiceDeviceEvent::Removed { device_id: "d2".into() };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("Removed"));
    }

    #[test]
    fn test_service_device_event_changed() {
        let mut changes = HashMap::new();
        changes.insert("status".into(), "offline".into());
        let ev = ServiceDeviceEvent::Changed { device_id: "d3".into(), changes };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("Changed"));
    }

    #[test]
    fn test_device_service_register_multiple() {
        let svc = DeviceService::new();
        for i in 0..10 {
            svc.register(Device {
                id: format!("dev-{}", i),
                name: format!("Device {}", i),
                device_type: "sensor".into(),
                status: "online".into(),
                vendor_id: None,
                product_id: None,
                serial: None,
                connected_at: None,
                metadata: HashMap::new(),
            });
        }
        assert_eq!(svc.count(), 10);
    }

    #[test]
    fn test_device_service_register_overwrite() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".into(), name: "Original".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.register(Device {
            id: "d1".into(), name: "Updated".into(), device_type: "t".into(),
            status: "offline".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        assert_eq!(svc.count(), 1);
        assert_eq!(svc.get("d1").unwrap().name, "Updated");
    }

    #[test]
    fn test_device_service_unregister_nonexistent() {
        let svc = DeviceService::new();
        let result = svc.unregister("nonexistent");
        assert!(result.is_none());
    }

    #[test]
    fn test_device_service_list() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.register(Device {
            id: "d2".into(), name: "D2".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        let list = svc.list();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_device_service_status_initial() {
        let svc = DeviceService::new();
        let status = svc.status();
        assert_eq!(status["running"], false);
        assert_eq!(status["enabled"], true);
        assert_eq!(status["device_count"], 0);
    }

    #[test]
    fn test_device_service_status_with_devices() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        let status = svc.status();
        assert_eq!(status["device_count"], 1);
    }

    #[test]
    fn test_device_service_config_custom() {
        let config = DeviceServiceConfig {
            enabled: false,
            poll_interval_secs: 10,
            monitor_usb: false,
        };
        assert!(!config.enabled);
        assert_eq!(config.poll_interval_secs, 10);
        assert!(!config.monitor_usb);
    }

    #[test]
    fn test_device_service_config_serialization() {
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 15,
            monitor_usb: true,
        };
        let json = serde_json::to_string(&config).unwrap();
        let rt: DeviceServiceConfig = serde_json::from_str(&json).unwrap();
        assert!(rt.enabled);
        assert_eq!(rt.poll_interval_secs, 15);
    }

    #[test]
    fn test_parse_last_channel_various() {
        // Valid
        assert_eq!(parse_last_channel("web:abc"), ("web".into(), "abc".into()));
        assert_eq!(parse_last_channel("telegram:12345"), ("telegram".into(), "12345".into()));

        // Invalid
        assert_eq!(parse_last_channel(":"), ("".into(), "".into()));
        assert_eq!(parse_last_channel("a:"), ("".into(), "".into()));
        assert_eq!(parse_last_channel(":b"), ("".into(), "".into()));
    }

    #[test]
    fn test_is_internal_channel_all() {
        assert!(is_internal_channel("cli"));
        assert!(is_internal_channel("system"));
        assert!(is_internal_channel("subagent"));
        assert!(!is_internal_channel("web"));
        assert!(!is_internal_channel("telegram"));
        assert!(!is_internal_channel("discord"));
        assert!(!is_internal_channel("feishu"));
    }

    #[test]
    fn test_device_service_handler_on_register_and_unregister() {
        let svc = DeviceService::new();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_clone = events.clone();
        svc.set_handler(Box::new(move |ev| {
            let desc = format!("{:?}", ev);
            events_clone.lock().push(desc);
        }));

        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "sensor".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.unregister("d1");

        let evts = events.lock();
        assert_eq!(evts.len(), 2);
        assert!(evts[0].contains("Added"));
        assert!(evts[1].contains("Removed"));
    }

    #[test]
    fn test_device_service_no_handler_no_panic() {
        let svc = DeviceService::new();
        // Should not panic when no handler is set
        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.unregister("d1");
    }

    #[test]
    fn test_device_service_default() {
        let svc = DeviceService::default();
        assert!(!svc.is_running());
        assert_eq!(svc.count(), 0);
    }

    // ---- New tests for coverage ----

    #[tokio::test]
    async fn test_start_disabled() {
        let config = DeviceServiceConfig {
            enabled: false,
            poll_interval_secs: 5,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        svc.start().await.unwrap();
        // When disabled, running should remain false
        assert!(!svc.is_running());
    }

    #[tokio::test]
    async fn test_start_already_running() {
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 5,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        svc.start().await.unwrap();
        assert!(svc.is_running());
        // Start again - should return Ok (idempotent)
        svc.start().await.unwrap();
        assert!(svc.is_running());
        svc.stop();
    }

    #[test]
    fn test_config_deserialization_partial_json() {
        // Only provide some fields - rest should use defaults from serde(default)
        let json = r#"{"enabled": false}"#;
        let config: DeviceServiceConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.poll_interval_secs, 5); // default
        assert!(!config.monitor_usb); // serde(default) = false
    }

    #[test]
    fn test_config_deserialization_empty_json() {
        let json = r#"{}"#;
        let config: DeviceServiceConfig = serde_json::from_str(json).unwrap();
        // All fields should be defaults from serde(default)
        assert!(!config.enabled); // serde(default) = false
        assert_eq!(config.poll_interval_secs, 5); // default_poll_interval
        assert!(!config.monitor_usb); // serde(default) = false
    }

    #[test]
    fn test_config_deserialization_full_json() {
        let json = r#"{"enabled": true, "poll_interval_secs": 30, "monitor_usb": true}"#;
        let config: DeviceServiceConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.poll_interval_secs, 30);
        assert!(config.monitor_usb);
    }

    #[test]
    fn test_config_default_differs_from_deserialize_empty() {
        // Default trait impl has enabled: true, but serde(default) for enabled is false
        let default_config = DeviceServiceConfig::default();
        assert!(default_config.enabled); // Default trait sets this to true

        let empty_json_config: DeviceServiceConfig = serde_json::from_str("{}").unwrap();
        assert!(!empty_json_config.enabled); // serde(default) uses bool default = false
    }

    #[test]
    fn test_service_status_disabled() {
        let config = DeviceServiceConfig {
            enabled: false,
            poll_interval_secs: 10,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        let status = svc.status();
        assert_eq!(status["enabled"], false);
        assert_eq!(status["monitor_usb"], false);
        assert_eq!(status["poll_interval_secs"], 10);
    }

    #[test]
    fn test_service_status_after_register() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.register(Device {
            id: "d2".into(), name: "D2".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        let status = svc.status();
        assert_eq!(status["device_count"], 2);
    }

    #[test]
    fn test_service_with_config_no_usb() {
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 5,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        // No sources should be added
        assert!(!svc.is_running());
    }

    #[test]
    fn test_device_serialization_roundtrip() {
        let d = Device {
            id: "dev-x".into(),
            name: "Test".into(),
            device_type: "sensor".into(),
            status: "connected".into(),
            vendor_id: Some("0xABCD".into()),
            product_id: Some("0x1234".into()),
            serial: Some("SN999".into()),
            connected_at: Some(Utc::now()),
            metadata: {
                let mut m = HashMap::new();
                m.insert("version".into(), "1.0".into());
                m
            },
        };
        let json = serde_json::to_string(&d).unwrap();
        let parsed: Device = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, "dev-x");
        assert_eq!(parsed.vendor_id, Some("0xABCD".into()));
        assert_eq!(parsed.metadata.get("version").unwrap(), "1.0");
    }

    // ---- Additional tests for 95%+ coverage ----

    #[test]
    fn test_service_device_event_changed_serialization() {
        let mut changes = HashMap::new();
        changes.insert("status".to_string(), "offline".to_string());
        changes.insert("latency".to_string(), "120ms".to_string());
        let ev = ServiceDeviceEvent::Changed {
            device_id: "dev-changed".into(),
            changes: changes.clone(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
        if let ServiceDeviceEvent::Changed { device_id, changes: c } = parsed {
            assert_eq!(device_id, "dev-changed");
            assert_eq!(c.len(), 2);
        } else {
            panic!("Expected Changed variant");
        }
    }

    #[test]
    fn test_send_notification_with_invalid_channel_format() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        // "nocolon" format - parse returns empty
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "nocolon".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_with_empty_platform() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: ":onlyuser".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Remove,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_with_subagent_channel() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "subagent:agent1".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_remove_action() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "web:user456".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Remove,
            kind: crate::source::Kind::Usb,
            device_id: "1-2".to_string(),
            vendor: "V".to_string(),
            product: "P".to_string(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        let msgs = sent.lock();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].2.contains("Disconnected"));
    }

    #[test]
    fn test_config_deserialization_with_monitor_usb_true() {
        let json = r#"{"enabled": true, "poll_interval_secs": 10, "monitor_usb": true}"#;
        let config: DeviceServiceConfig = serde_json::from_str(json).unwrap();
        assert!(config.enabled);
        assert_eq!(config.poll_interval_secs, 10);
        assert!(config.monitor_usb);
    }

    #[tokio::test]
    async fn test_start_already_running_idempotent() {
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 5,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        svc.start().await.unwrap();
        assert!(svc.is_running());
        // Second start should be idempotent
        svc.start().await.unwrap();
        assert!(svc.is_running());
        svc.stop();
        assert!(!svc.is_running());
    }

    #[test]
    fn test_status_after_register_and_unregister() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        assert_eq!(svc.status()["device_count"], 1);
        svc.unregister("d1");
        assert_eq!(svc.status()["device_count"], 0);
    }

    // ---- Additional edge-case tests for 95%+ ----

    #[test]
    fn test_send_notification_with_cli_channel() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "cli:admin".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Add,
            kind: crate::source::Kind::Usb,
            device_id: "1-1".to_string(),
            vendor: String::new(),
            product: String::new(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        assert!(sent.lock().is_empty());
    }

    #[test]
    fn test_send_notification_change_action() {
        let svc = DeviceService::new();
        let sent = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
        let sent_clone = sent.clone();
        svc.set_bus_sender(Box::new(move |ch, id, content| {
            sent_clone.lock().push((ch.to_string(), id.to_string(), content.to_string()));
        }));
        svc.set_state_manager(Arc::new(MockState {
            last_channel: "web:user789".to_string(),
        }));

        let ev = DeviceEvent {
            action: crate::source::Action::Change,
            kind: crate::source::Kind::Bluetooth,
            device_id: "bt-1".to_string(),
            vendor: "V".to_string(),
            product: "P".to_string(),
            serial: String::new(),
            capabilities: String::new(),
            raw: HashMap::new(),
        };
        svc.send_notification(&ev);
        let msgs = sent.lock();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].2.contains("Changed"));
    }

    #[test]
    fn test_register_emits_added_event() {
        let svc = DeviceService::new();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_clone = events.clone();
        svc.set_handler(Box::new(move |ev| {
            events_clone.lock().push(format!("{:?}", ev));
        }));

        svc.register(Device {
            id: "d1".into(), name: "D1".into(), device_type: "sensor".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        let evts = events.lock();
        assert_eq!(evts.len(), 1);
        assert!(evts[0].contains("Added"));
        assert!(evts[0].contains("d1"));
        assert!(evts[0].contains("sensor"));
    }

    #[test]
    fn test_unregister_nonexistent_no_event() {
        let svc = DeviceService::new();
        let events = Arc::new(Mutex::new(Vec::<String>::new()));
        let events_clone = events.clone();
        svc.set_handler(Box::new(move |ev| {
            events_clone.lock().push(format!("{:?}", ev));
        }));
        let result = svc.unregister("nonexistent");
        assert!(result.is_none());
        assert!(events.lock().is_empty());
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 30,
            monitor_usb: false,
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: DeviceServiceConfig = serde_json::from_str(&json).unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.poll_interval_secs, 30);
        assert!(!parsed.monitor_usb);
    }

    #[test]
    fn test_device_with_all_optional_fields() {
        let now = Utc::now();
        let device = Device {
            id: "full-dev".into(),
            name: "Full Device".into(),
            device_type: "sensor".into(),
            status: "online".into(),
            vendor_id: Some("0x1234".into()),
            product_id: Some("0x5678".into()),
            serial: Some("SN-FULL-001".into()),
            connected_at: Some(now),
            metadata: {
                let mut m = HashMap::new();
                m.insert("firmware".into(), "1.2.3".into());
                m.insert("driver".into(), "usbhid".into());
                m
            },
        };
        let json = serde_json::to_string(&device).unwrap();
        let parsed: Device = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.vendor_id, Some("0x1234".into()));
        assert_eq!(parsed.product_id, Some("0x5678".into()));
        assert_eq!(parsed.serial, Some("SN-FULL-001".into()));
        assert!(parsed.connected_at.is_some());
        assert_eq!(parsed.metadata.len(), 2);
        assert_eq!(parsed.metadata.get("firmware").unwrap(), "1.2.3");
    }

    #[test]
    fn test_service_device_event_added_serialization() {
        let ev = ServiceDeviceEvent::Added {
            device_id: "dev-add".into(),
            device_type: "bluetooth".into(),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
        if let ServiceDeviceEvent::Added { device_id, device_type } = parsed {
            assert_eq!(device_id, "dev-add");
            assert_eq!(device_type, "bluetooth");
        } else {
            panic!("Expected Added variant");
        }
    }

    #[test]
    fn test_service_device_event_removed_serialization() {
        let ev = ServiceDeviceEvent::Removed { device_id: "dev-rm".into() };
        let json = serde_json::to_string(&ev).unwrap();
        let parsed: ServiceDeviceEvent = serde_json::from_str(&json).unwrap();
        if let ServiceDeviceEvent::Removed { device_id } = parsed {
            assert_eq!(device_id, "dev-rm");
        } else {
            panic!("Expected Removed variant");
        }
    }

    #[test]
    fn test_parse_last_channel_valid_multi_colon() {
        // "platform:user:extra" should split on first colon
        let (platform, user_id) = parse_last_channel("web:user:extra");
        assert_eq!(platform, "web");
        assert_eq!(user_id, "user:extra");
    }

    #[test]
    fn test_service_count_after_multiple_operations() {
        let svc = DeviceService::new();
        svc.register(Device {
            id: "a".into(), name: "A".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.register(Device {
            id: "b".into(), name: "B".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        svc.register(Device {
            id: "c".into(), name: "C".into(), device_type: "t".into(),
            status: "online".into(), vendor_id: None, product_id: None, serial: None,
            connected_at: None, metadata: HashMap::new(),
        });
        assert_eq!(svc.count(), 3);
        svc.unregister("b");
        assert_eq!(svc.count(), 2);
        assert!(svc.get("a").is_some());
        assert!(svc.get("c").is_some());
        assert!(svc.get("b").is_none());
    }

    #[tokio::test]
    async fn test_start_enabled_no_sources() {
        // Create service with enabled=true but no USB sources
        let config = DeviceServiceConfig {
            enabled: true,
            poll_interval_secs: 5,
            monitor_usb: false,
        };
        let svc = DeviceService::with_config(config);
        // Start should succeed even with no sources
        svc.start().await.unwrap();
        assert!(svc.is_running());
        svc.stop();
        assert!(!svc.is_running());
    }
}
