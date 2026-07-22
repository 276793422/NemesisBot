//! Discovery sub-module for peer discovery over UDP multicast.

mod crypto;
mod discovery;
mod listener;
mod message;

pub use crypto::{CryptoService, decrypt_data, derive_key, encrypt_data};
pub use discovery::{ClusterCallbacks, DiscoveryConfig, DiscoveryError, DiscoveryService};
pub use listener::{DiscoveryAction, UdpListener, handle_discovery_message};
pub use message::{DiscoveryMessage, DiscoveryMessageType, MessageValidationError};
