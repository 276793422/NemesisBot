//! Discovery sub-module for peer discovery over UDP multicast.

mod crypto;
mod discovery;
mod listener;
mod message;

pub use crypto::{CryptoService, derive_key, encrypt_data, decrypt_data};
pub use discovery::{DiscoveryConfig, DiscoveryService, DiscoveryError, ClusterCallbacks};
pub use listener::{handle_discovery_message, DiscoveryAction, UdpListener};
pub use message::{DiscoveryMessage, DiscoveryMessageType, MessageValidationError};
