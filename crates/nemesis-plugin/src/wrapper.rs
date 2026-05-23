//! Plugin wrapper for managed lifecycle.

use crate::plugin::Plugin;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Wrapper that manages plugin lifecycle.
pub struct PluginWrapper {
    plugin: Arc<Mutex<Box<dyn Plugin>>>,
    initialized: AtomicBool,
    config: serde_json::Value,
}

impl PluginWrapper {
    pub fn new(plugin: Box<dyn Plugin>) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            initialized: AtomicBool::new(false),
            config: serde_json::Value::Null,
        }
    }

    /// Create with initial configuration.
    pub fn with_config(plugin: Box<dyn Plugin>, config: serde_json::Value) -> Self {
        Self {
            plugin: Arc::new(Mutex::new(plugin)),
            initialized: AtomicBool::new(false),
            config,
        }
    }

    /// Initialize the plugin with configuration.
    pub fn init(&self) -> Result<(), String> {
        if self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }
        let mut plugin = self.plugin.lock();
        plugin.init(&self.config)?;
        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    /// Cleanup (shutdown) the plugin.
    pub fn shutdown(&self) -> Result<(), String> {
        if !self.initialized.load(Ordering::SeqCst) {
            return Ok(());
        }
        let plugin = self.plugin.lock();
        plugin.cleanup()?;
        self.initialized.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Get plugin name.
    pub fn name(&self) -> String {
        self.plugin.lock().name().to_string()
    }

    /// Get plugin version.
    pub fn version(&self) -> String {
        self.plugin.lock().version().to_string()
    }

    /// Check if initialized.
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::SeqCst)
    }

    /// Check if the wrapped plugin is running.
    pub fn is_running(&self) -> bool {
        self.plugin.lock().is_running()
    }

    /// Get a reference to the inner plugin for downcasting.
    pub fn plugin_ref(&self) -> parking_lot::MutexGuard<'_, Box<dyn Plugin>> {
        self.plugin.lock()
    }
}

#[cfg(test)]
mod tests;
