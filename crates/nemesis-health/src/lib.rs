//! Health check server.

pub mod disk;
pub mod server;

pub use disk::{WatermarkLevel, evaluate, volume_free_bytes};
pub use server::HealthServer;
