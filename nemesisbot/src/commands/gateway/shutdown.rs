// ---------------------------------------------------------------------------
// Global shutdown state
// ---------------------------------------------------------------------------

use std::sync::atomic::{AtomicBool, Ordering};

/// Global shutdown flag (replaces Go's globalShutdownChan).
/// pub(crate)：tests.rs 直控复位（隔离），经根 cfg(test) 再导出暴露。
pub(crate) static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request global shutdown from any component.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)] // only called from the tray's on_quit (desktop feature); kept as a general API.
pub(crate) fn trigger_global_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if global shutdown has been requested.
#[allow(dead_code)]
pub(crate) fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}
