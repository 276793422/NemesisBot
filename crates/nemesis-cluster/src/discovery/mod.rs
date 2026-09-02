//! Discovery sub-module for peer discovery over UDP multicast.

mod crypto;
mod discovery;
mod listener;
mod message;

pub use crypto::{CryptoService, decrypt_data, derive_key, encrypt_data};
pub use discovery::{
    AnnounceWarnGate, ClusterCallbacks, DRIFT_WARN_THRESHOLD_SECS, DiscoveryConfig, DiscoveryError,
    DiscoveryService,
};
pub use listener::{DiscoveryAction, UdpListener, handle_discovery_message};
pub use message::{
    DEFAULT_EXPIRY_THRESHOLD_SECS, DiscoveryMessage, DiscoveryMessageType, MessageValidationError,
};
