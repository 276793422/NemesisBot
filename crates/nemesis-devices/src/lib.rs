//! Device management service.

pub mod service;
pub mod source;
pub mod events;

pub use service::{
    DeviceService, Device, ServiceDeviceEvent, DeviceServiceConfig,
    OutboundSender, LastChannelProvider, parse_last_channel, is_internal_channel,
};
pub use source::{DeviceEvent, EventSource, UsbEventSource, Action, Kind};
