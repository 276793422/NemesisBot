//! Device management service.

pub mod events;
pub mod service;
pub mod source;

pub use service::{
    Device, DeviceService, DeviceServiceConfig, LastChannelProvider, OutboundSender,
    ServiceDeviceEvent, is_internal_channel, parse_last_channel,
};
pub use source::{Action, DeviceEvent, EventSource, Kind, UsbEventSource};
