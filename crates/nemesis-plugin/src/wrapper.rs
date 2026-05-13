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
mod tests {
    use super::*;
    use std::any::Any;

    struct TestPlugin { running: bool }

    impl Plugin for TestPlugin {
        fn name(&self) -> &str { "test" }
        fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> { self.running = true; Ok(()) }
        fn is_running(&self) -> bool { self.running }
        fn as_any(&self) -> &dyn Any { self }
        fn cleanup(&self) -> Result<(), String> { Ok(()) }
    }

    #[test]
    fn test_plugin_lifecycle() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        assert!(!wrapper.is_initialized());
        wrapper.init().unwrap();
        assert!(wrapper.is_initialized());
        assert_eq!(wrapper.name(), "test");
        assert!(wrapper.is_running());
        wrapper.shutdown().unwrap();
        assert!(!wrapper.is_initialized());
    }

    #[test]
    fn test_plugin_with_config() {
        let config = serde_json::json!({"key": "value"});
        let wrapper = PluginWrapper::with_config(Box::new(TestPlugin { running: false }), config);
        wrapper.init().unwrap();
        assert!(wrapper.is_initialized());
    }

    #[test]
    fn test_double_init() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        wrapper.init().unwrap();
        wrapper.init().unwrap(); // Should be idempotent
        assert!(wrapper.is_initialized());
    }

    #[test]
    fn test_wrapper_version() {
        // TestPlugin doesn't override version(), so it returns the default "0.1.0"
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        assert_eq!(wrapper.version(), "0.1.0");
    }

    #[test]
    fn test_wrapper_is_running_false() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        assert!(!wrapper.is_running());
    }

    #[test]
    fn test_wrapper_is_running_true_after_init() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        wrapper.init().unwrap();
        // TestPlugin sets running = true in init()
        assert!(wrapper.is_running());
    }

    #[test]
    fn test_wrapper_name() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        assert_eq!(wrapper.name(), "test");
    }

    #[test]
    fn test_wrapper_plugin_ref() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        let guard = wrapper.plugin_ref();
        assert_eq!(guard.name(), "test");
    }

    #[test]
    fn test_wrapper_double_shutdown() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        wrapper.init().unwrap();
        wrapper.shutdown().unwrap();
        assert!(!wrapper.is_initialized());
        // Second shutdown should be idempotent
        wrapper.shutdown().unwrap();
        assert!(!wrapper.is_initialized());
    }

    #[test]
    fn test_wrapper_not_initialized_shutdown() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        // Shutdown without init should be ok
        wrapper.shutdown().unwrap();
        assert!(!wrapper.is_initialized());
    }

    // ---- Additional coverage for 95%+ target ----

    #[test]
    fn test_init_fails_propagates_error() {
        struct FailInitPlugin;
        impl Plugin for FailInitPlugin {
            fn name(&self) -> &str { "fail_init" }
            fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> {
                Err("init failed".to_string())
            }
            fn as_any(&self) -> &dyn Any { self }
        }

        let wrapper = PluginWrapper::new(Box::new(FailInitPlugin));
        let result = wrapper.init();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("init failed"));
        assert!(!wrapper.is_initialized());
    }

    #[test]
    fn test_shutdown_fails_propagates_error() {
        struct FailCleanupPlugin;
        impl Plugin for FailCleanupPlugin {
            fn name(&self) -> &str { "fail_cleanup" }
            fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> { Ok(()) }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Err("cleanup failed".to_string()) }
        }

        let wrapper = PluginWrapper::new(Box::new(FailCleanupPlugin));
        wrapper.init().unwrap();
        assert!(wrapper.is_initialized());
        let result = wrapper.shutdown();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cleanup failed"));
        // initialized flag is NOT reset when cleanup fails (error return happens before store)
        assert!(wrapper.is_initialized());
    }

    #[test]
    fn test_with_config_null() {
        let wrapper = PluginWrapper::with_config(
            Box::new(TestPlugin { running: false }),
            serde_json::Value::Null,
        );
        assert!(!wrapper.is_initialized());
        wrapper.init().unwrap();
        assert!(wrapper.is_initialized());
    }

    #[test]
    fn test_version_custom() {
        struct VersionedPlugin;
        impl Plugin for VersionedPlugin {
            fn name(&self) -> &str { "versioned" }
            fn version(&self) -> &str { "2.5.0" }
            fn as_any(&self) -> &dyn Any { self }
        }

        let wrapper = PluginWrapper::new(Box::new(VersionedPlugin));
        assert_eq!(wrapper.version(), "2.5.0");
    }

    #[test]
    fn test_plugin_ref_allows_method_call() {
        let wrapper = PluginWrapper::new(Box::new(TestPlugin { running: false }));
        let guard = wrapper.plugin_ref();
        assert_eq!(guard.name(), "test");
        assert_eq!(guard.version(), "0.1.0");
        assert!(!guard.is_running());
    }
}
