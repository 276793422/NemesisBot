//! Plugin trait and lifecycle management.

use std::any::Any;

/// Tool invocation represents a tool execution request.
/// Mirrors Go ToolInvocation — used by plugins to intercept and modify tool calls.
#[derive(Debug, Clone)]
pub struct ToolInvocation {
    /// Tool name being invoked.
    pub tool_name: String,
    /// Method being called (e.g., "Execute", "Stream").
    pub method: String,
    /// Original arguments.
    pub args: serde_json::Map<String, serde_json::Value>,
    /// User information.
    pub user: String,
    /// Source channel.
    pub source: String,
    /// Workspace path.
    pub workspace: String,
    /// Result (can be modified by plugins).
    pub result: Option<serde_json::Value>,
    /// Error (set by plugins to block execution).
    pub blocking_error: Option<String>,
    /// Metadata for plugins to pass information.
    pub metadata: serde_json::Map<String, serde_json::Value>,
}

impl ToolInvocation {
    /// Create a new tool invocation.
    pub fn new(tool_name: &str, args: serde_json::Map<String, serde_json::Value>) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            method: "Execute".to_string(),
            args,
            user: String::new(),
            source: String::new(),
            workspace: String::new(),
            result: None,
            blocking_error: None,
            metadata: serde_json::Map::new(),
        }
    }
}

/// Plugin interface.
pub trait Plugin: Send + Sync {
    /// Plugin name.
    fn name(&self) -> &str;

    /// Plugin version.
    fn version(&self) -> &str { "0.1.0" }

    /// Initialize the plugin with configuration.
    fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> { Ok(()) }

    /// Execute intercepts a tool execution.
    /// Returns (allowed, error_message, modified).
    /// Mirrors Go Plugin.Execute.
    fn execute(&self, _invocation: &mut ToolInvocation) -> (bool, Option<String>, bool) {
        (true, None, false)
    }

    /// Check if plugin is running.
    fn is_running(&self) -> bool { false }

    /// Cast to Any for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// Cleanup when unloading.
    fn cleanup(&self) -> Result<(), String> { Ok(()) }
}

/// Base plugin with default implementations.
pub struct BasePlugin {
    name: String,
    version: String,
}

impl BasePlugin {
    pub fn new(name: &str, version: &str) -> Self {
        Self { name: name.to_string(), version: version.to_string() }
    }
}

impl Plugin for BasePlugin {
    fn name(&self) -> &str { &self.name }
    fn version(&self) -> &str { &self.version }
    fn as_any(&self) -> &dyn Any { self }
}

/// Plugin manager: register, enable/disable, execute lifecycle.
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
    enabled: std::collections::HashMap<String, bool>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            enabled: std::collections::HashMap::new(),
        }
    }

    /// Register a plugin.
    pub fn register(&mut self, plugin: Box<dyn Plugin>) -> Result<(), String> {
        let name = plugin.name().to_string();
        if self.plugins.iter().any(|p| p.name() == name) {
            return Err(format!("plugin {} already registered", name));
        }
        self.enabled.insert(name, true);
        self.plugins.push(plugin);
        Ok(())
    }

    /// Unregister a plugin by name.
    pub fn unregister(&mut self, name: &str) -> Result<(), String> {
        let idx = self.plugins.iter().position(|p| p.name() == name)
            .ok_or_else(|| format!("plugin {} not found", name))?;
        self.plugins[idx].cleanup()?;
        self.plugins.remove(idx);
        self.enabled.remove(name);
        Ok(())
    }

    /// Enable a plugin.
    pub fn enable(&mut self, name: &str) {
        self.enabled.insert(name.to_string(), true);
    }

    /// Disable a plugin.
    pub fn disable(&mut self, name: &str) {
        if let Some(e) = self.enabled.get_mut(name) {
            *e = false;
        }
    }

    /// Check if a plugin is enabled.
    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.get(name).copied().unwrap_or(false)
    }

    /// Get a plugin by name.
    pub fn get_plugin(&self, name: &str) -> Option<&dyn Plugin> {
        self.plugins.iter()
            .find(|p| p.name() == name && self.is_enabled(name))
            .map(|p| p.as_ref())
    }

    /// List all enabled plugins.
    pub fn list_plugins(&self) -> Vec<&dyn Plugin> {
        self.plugins.iter()
            .filter(|p| self.is_enabled(p.name()))
            .map(|p| p.as_ref())
            .collect()
    }

    /// Cleanup all plugins.
    pub fn cleanup_all(&mut self) {
        for plugin in &self.plugins {
            if let Err(e) = plugin.cleanup() {
                tracing::warn!("Error cleaning up plugin {}: {}", plugin.name(), e);
            }
        }
        self.plugins.clear();
        self.enabled.clear();
    }

    /// Execute all enabled plugins for a tool invocation.
    /// Mirrors Go Manager.Execute — runs plugin chain, stops on denial.
    /// Returns (allowed, error_message).
    pub fn execute(&self, invocation: &mut ToolInvocation) -> (bool, Option<String>) {
        for plugin in &self.plugins {
            if !self.is_enabled(plugin.name()) {
                continue;
            }

            let (allowed, err, _modified) = plugin.execute(invocation);

            if !allowed {
                let msg = err.unwrap_or_else(|| "operation denied".to_string());
                return (false, Some(format!("[{}] {}", plugin.name(), msg)));
            }

            if invocation.blocking_error.is_some() {
                return (false, invocation.blocking_error.clone());
            }
        }

        (true, None)
    }
}

impl Default for PluginManager {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestPlugin { running: bool }
    impl Plugin for TestPlugin {
        fn name(&self) -> &str { "test" }
        fn version(&self) -> &str { "1.0.0" }
        fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> { self.running = true; Ok(()) }
        fn is_running(&self) -> bool { self.running }
        fn as_any(&self) -> &dyn Any { self }
        fn cleanup(&self) -> Result<(), String> { Ok(()) }
    }

    #[test]
    fn test_plugin_register() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        assert!(mgr.is_enabled("test"));
        assert_eq!(mgr.list_plugins().len(), 1);
    }

    #[test]
    fn test_plugin_duplicate() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        assert!(mgr.register(Box::new(TestPlugin { running: false })).is_err());
    }

    #[test]
    fn test_plugin_enable_disable() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        mgr.disable("test");
        assert!(!mgr.is_enabled("test"));
        assert!(mgr.get_plugin("test").is_none());
        mgr.enable("test");
        assert!(mgr.is_enabled("test"));
    }

    #[test]
    fn test_plugin_unregister() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        mgr.unregister("test").unwrap();
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn test_plugin_cleanup_all() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        mgr.cleanup_all();
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn test_base_plugin() {
        let bp = BasePlugin::new("base", "2.0.0");
        assert_eq!(bp.name(), "base");
        assert_eq!(bp.version(), "2.0.0");
    }

    #[test]
    fn test_tool_invocation_new() {
        let mut args = serde_json::Map::new();
        args.insert("path".to_string(), serde_json::json!("/tmp/test.txt"));
        let inv = ToolInvocation::new("file_read", args.clone());
        assert_eq!(inv.tool_name, "file_read");
        assert_eq!(inv.method, "Execute");
        assert_eq!(inv.args, args);
        assert!(inv.user.is_empty());
        assert!(inv.source.is_empty());
        assert!(inv.workspace.is_empty());
        assert!(inv.result.is_none());
        assert!(inv.blocking_error.is_none());
        assert!(inv.metadata.is_empty());
    }

    #[test]
    fn test_plugin_default_version() {
        struct NoVersionPlugin;
        impl Plugin for NoVersionPlugin {
            fn name(&self) -> &str { "noversion" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let p = NoVersionPlugin;
        assert_eq!(p.version(), "0.1.0");
    }

    #[test]
    fn test_plugin_default_init() {
        struct NoInitPlugin;
        impl Plugin for NoInitPlugin {
            fn name(&self) -> &str { "noinit" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut p = NoInitPlugin;
        assert!(p.init(&serde_json::Value::Null).is_ok());
    }

    #[test]
    fn test_plugin_default_is_running() {
        struct NoRunningPlugin;
        impl Plugin for NoRunningPlugin {
            fn name(&self) -> &str { "norunning" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let p = NoRunningPlugin;
        assert!(!p.is_running());
    }

    #[test]
    fn test_plugin_default_cleanup() {
        struct MinimalPlugin;
        impl Plugin for MinimalPlugin {
            fn name(&self) -> &str { "minimal" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let p = MinimalPlugin;
        assert!(p.cleanup().is_ok());
    }

    #[test]
    fn test_plugin_default_execute_allows() {
        struct MinimalPlugin;
        impl Plugin for MinimalPlugin {
            fn name(&self) -> &str { "minimal" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let p = MinimalPlugin;
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err, modified) = p.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
        assert!(!modified);
    }

    #[test]
    fn test_plugin_manager_default() {
        let mgr = PluginManager::default();
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn test_execute_with_blocking_error() {
        struct BlockingPlugin;
        impl Plugin for BlockingPlugin {
            fn name(&self) -> &str { "blocker" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                // Allows execution but sets a blocking error
                inv.blocking_error = Some("post-check failed".to_string());
                (true, None, false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(BlockingPlugin)).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        assert!(err.unwrap().contains("post-check failed"));
    }

    #[test]
    fn test_execute_denied_returns_formatted_error() {
        struct DenyAllPlugin;
        impl Plugin for DenyAllPlugin {
            fn name(&self) -> &str { "denyall" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, Some("forbidden".to_string()), false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(DenyAllPlugin)).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        let err_msg = err.unwrap();
        assert!(err_msg.contains("[denyall]"));
        assert!(err_msg.contains("forbidden"));
    }

    #[test]
    fn test_unregister_nonexistent() {
        let mut mgr = PluginManager::new();
        assert!(mgr.unregister("nonexistent").is_err());
    }

    #[test]
    fn test_disable_nonexistent() {
        let mut mgr = PluginManager::new();
        // Should not panic
        mgr.disable("nonexistent");
        assert!(!mgr.is_enabled("nonexistent"));
    }

    #[test]
    fn test_get_plugin_disabled() {
        struct SimplePlugin;
        impl Plugin for SimplePlugin {
            fn name(&self) -> &str { "simple" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(SimplePlugin)).unwrap();
        mgr.disable("simple");
        assert!(mgr.get_plugin("simple").is_none());
    }

    #[test]
    fn test_execute_chain_stops_on_denial() {
        struct AllowPlugin;
        impl Plugin for AllowPlugin {
            fn name(&self) -> &str { "allow" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (true, None, true)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        struct DenyPlugin;
        impl Plugin for DenyPlugin {
            fn name(&self) -> &str { "deny" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, Some("blocked".to_string()), false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(AllowPlugin)).unwrap();
        mgr.register(Box::new(DenyPlugin)).unwrap();

        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        assert!(err.unwrap().contains("blocked"));
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_execute_no_plugins_allows() {
        let mgr = PluginManager::new();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
    }

    #[test]
    fn test_execute_skips_disabled_plugin() {
        struct DenyPlugin;
        impl Plugin for DenyPlugin {
            fn name(&self) -> &str { "deny_disabled" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, Some("should not reach".to_string()), false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(DenyPlugin)).unwrap();
        mgr.disable("deny_disabled");

        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, _) = mgr.execute(&mut inv);
        assert!(allowed); // disabled plugin should be skipped
    }

    #[test]
    fn test_execute_denied_no_error_msg() {
        struct DenyNoMsgPlugin;
        impl Plugin for DenyNoMsgPlugin {
            fn name(&self) -> &str { "denynMsg" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, None, false) // denied but no message
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(DenyNoMsgPlugin)).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        assert!(err.unwrap().contains("operation denied"));
    }

    #[test]
    fn test_tool_invocation_all_fields() {
        let mut args = serde_json::Map::new();
        args.insert("key".to_string(), serde_json::json!("value"));

        let mut inv = ToolInvocation::new("my_tool", args);
        inv.user = "testuser".to_string();
        inv.source = "cli".to_string();
        inv.workspace = "/home".to_string();
        inv.result = Some(serde_json::json!({"ok": true}));
        inv.blocking_error = Some("blocked".to_string());
        inv.metadata.insert("meta_key".to_string(), serde_json::json!("meta_val"));

        assert_eq!(inv.tool_name, "my_tool");
        assert_eq!(inv.method, "Execute");
        assert_eq!(inv.user, "testuser");
        assert_eq!(inv.source, "cli");
        assert_eq!(inv.workspace, "/home");
        assert!(inv.result.is_some());
        assert!(inv.blocking_error.is_some());
        assert_eq!(inv.metadata.len(), 1);
    }

    #[test]
    fn test_plugin_manager_list_after_unregister() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        mgr.unregister("test").unwrap();
        assert!(mgr.list_plugins().is_empty());
        assert!(!mgr.is_enabled("test"));
    }

    #[test]
    fn test_plugin_init_with_config() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        let result = mgr.plugins[0].init(&serde_json::json!({"enabled": true}));
        assert!(result.is_ok());
    }

    // ---- Additional coverage for 95%+ target ----

    #[test]
    fn test_get_plugin_enabled() {
        struct SimplePlugin;
        impl Plugin for SimplePlugin {
            fn name(&self) -> &str { "simple" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(SimplePlugin)).unwrap();
        let p = mgr.get_plugin("simple");
        assert!(p.is_some());
        assert_eq!(p.unwrap().name(), "simple");
    }

    #[test]
    fn test_get_plugin_nonexistent() {
        let mgr = PluginManager::new();
        assert!(mgr.get_plugin("nonexistent").is_none());
    }

    #[test]
    fn test_list_plugins_multiple() {
        struct PluginA;
        impl Plugin for PluginA {
            fn name(&self) -> &str { "a" }
            fn as_any(&self) -> &dyn Any { self }
        }
        struct PluginB;
        impl Plugin for PluginB {
            fn name(&self) -> &str { "b" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(PluginA)).unwrap();
        mgr.register(Box::new(PluginB)).unwrap();
        let list = mgr.list_plugins();
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn test_list_plugins_skips_disabled() {
        struct PluginA;
        impl Plugin for PluginA {
            fn name(&self) -> &str { "a" }
            fn as_any(&self) -> &dyn Any { self }
        }
        struct PluginB;
        impl Plugin for PluginB {
            fn name(&self) -> &str { "b" }
            fn as_any(&self) -> &dyn Any { self }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(PluginA)).unwrap();
        mgr.register(Box::new(PluginB)).unwrap();
        mgr.disable("a");
        let list = mgr.list_plugins();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].name(), "b");
    }

    #[test]
    fn test_cleanup_all_with_cleanup_error() {
        struct ErrorCleanupPlugin;
        impl Plugin for ErrorCleanupPlugin {
            fn name(&self) -> &str { "error_cleanup" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Err("cleanup error".to_string()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(ErrorCleanupPlugin)).unwrap();
        // Should not panic, just log warning
        mgr.cleanup_all();
        assert!(mgr.list_plugins().is_empty());
        assert!(!mgr.is_enabled("error_cleanup"));
    }

    #[test]
    fn test_unregister_removes_from_enabled() {
        struct PluginA;
        impl Plugin for PluginA {
            fn name(&self) -> &str { "a" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct PluginB;
        impl Plugin for PluginB {
            fn name(&self) -> &str { "b" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(PluginA)).unwrap();
        mgr.register(Box::new(PluginB)).unwrap();
        mgr.unregister("a").unwrap();
        assert!(!mgr.is_enabled("a"));
        assert!(mgr.is_enabled("b"));
        assert_eq!(mgr.list_plugins().len(), 1);
    }

    #[test]
    fn test_unregister_cleanup_error() {
        struct FailCleanupPlugin;
        impl Plugin for FailCleanupPlugin {
            fn name(&self) -> &str { "fail_cleanup" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Err("cleanup failed".to_string()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(FailCleanupPlugin)).unwrap();
        let result = mgr.unregister("fail_cleanup");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("cleanup failed"));
    }

    #[test]
    fn test_execute_multiple_plugins_all_allow() {
        struct AllowPlugin { n: &'static str }
        impl Plugin for AllowPlugin {
            fn name(&self) -> &str { self.n }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (true, None, false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(AllowPlugin { n: "p1" })).unwrap();
        mgr.register(Box::new(AllowPlugin { n: "p2" })).unwrap();
        mgr.register(Box::new(AllowPlugin { n: "p3" })).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
    }

    #[test]
    fn test_execute_second_plugin_denies() {
        struct AllowPlugin;
        impl Plugin for AllowPlugin {
            fn name(&self) -> &str { "allow" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (true, None, false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct DenyPlugin;
        impl Plugin for DenyPlugin {
            fn name(&self) -> &str { "deny_second" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, Some("denied by second".to_string()), false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(AllowPlugin)).unwrap();
        mgr.register(Box::new(DenyPlugin)).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        assert!(err.unwrap().contains("deny_second"));
    }

    #[test]
    fn test_base_plugin_as_any_downcast() {
        let bp = BasePlugin::new("base", "1.0.0");
        let any_ref = bp.as_any();
        let downcast = any_ref.downcast_ref::<BasePlugin>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap().name(), "base");
    }

    #[test]
    fn test_base_plugin_cleanup() {
        let bp = BasePlugin::new("base", "1.0.0");
        assert!(bp.cleanup().is_ok());
    }

    #[test]
    fn test_base_plugin_is_running() {
        let bp = BasePlugin::new("base", "1.0.0");
        assert!(!bp.is_running());
    }

    #[test]
    fn test_base_plugin_execute() {
        let bp = BasePlugin::new("base", "1.0.0");
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err, modified) = bp.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
        assert!(!modified);
    }

    #[test]
    fn test_base_plugin_init() {
        let mut bp = BasePlugin::new("base", "1.0.0");
        assert!(bp.init(&serde_json::Value::Null).is_ok());
    }

    #[test]
    fn test_enable_nonexistent_then_register() {
        let mut mgr = PluginManager::new();
        mgr.enable("pre_enable");
        // Should be enabled in the map
        assert!(mgr.is_enabled("pre_enable"));
        // But get_plugin still returns None since no plugin registered
        assert!(mgr.get_plugin("pre_enable").is_none());
    }

    #[test]
    fn test_execute_with_blocking_error_uses_invocation_error() {
        struct BlockPlugin;
        impl Plugin for BlockPlugin {
            fn name(&self) -> &str { "blocker" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                inv.blocking_error = Some("blocked via invocation".to_string());
                (true, None, false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(BlockPlugin)).unwrap();
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(!allowed);
        assert_eq!(err.unwrap(), "blocked via invocation");
    }

    #[test]
    fn test_tool_invocation_debug() {
        let inv = ToolInvocation::new("test_tool", serde_json::Map::new());
        let debug = format!("{:?}", inv);
        assert!(debug.contains("test_tool"));
    }

    #[test]
    fn test_tool_invocation_clone() {
        let mut inv = ToolInvocation::new("cloned_tool", serde_json::Map::new());
        inv.user = "test_user".to_string();
        inv.result = Some(serde_json::json!(42));
        let cloned = inv.clone();
        assert_eq!(cloned.tool_name, "cloned_tool");
        assert_eq!(cloned.user, "test_user");
        assert_eq!(cloned.result, Some(serde_json::json!(42)));
    }

    #[test]
    fn test_base_plugin_as_any() {
        let bp = BasePlugin::new("base_test", "1.0");
        let any_ref = bp.as_any();
        let downcast = any_ref.downcast_ref::<BasePlugin>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap().name(), "base_test");
    }

    #[test]
    fn test_plugin_manager_multiple_plugins_execute_chain() {
        struct LogPlugin;
        impl Plugin for LogPlugin {
            fn name(&self) -> &str { "logger" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                inv.metadata.insert("logged".to_string(), serde_json::json!(true));
                (true, None, true)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        struct AuditPlugin;
        impl Plugin for AuditPlugin {
            fn name(&self) -> &str { "auditor" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                inv.metadata.insert("audited".to_string(), serde_json::json!(true));
                (true, None, true)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(LogPlugin)).unwrap();
        mgr.register(Box::new(AuditPlugin)).unwrap();

        let mut inv = ToolInvocation::new("file_read", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
        assert_eq!(inv.metadata.len(), 2);
        assert!(inv.metadata.contains_key("logged"));
        assert!(inv.metadata.contains_key("audited"));
    }

    #[test]
    fn test_plugin_manager_unregister_one_of_two() {
        struct PluginA;
        impl Plugin for PluginA {
            fn name(&self) -> &str { "a" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct PluginB;
        impl Plugin for PluginB {
            fn name(&self) -> &str { "b" }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(PluginA)).unwrap();
        mgr.register(Box::new(PluginB)).unwrap();
        assert_eq!(mgr.list_plugins().len(), 2);

        mgr.unregister("a").unwrap();
        assert_eq!(mgr.list_plugins().len(), 1);
        assert!(mgr.get_plugin("a").is_none());
        assert!(mgr.get_plugin("b").is_some());
    }

    #[test]
    fn test_cleanup_all_clears_enabled() {
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        assert!(mgr.is_enabled("test"));
        mgr.cleanup_all();
        assert!(!mgr.is_enabled("test"));
        assert!(mgr.list_plugins().is_empty());
    }

    #[test]
    fn test_tool_invocation_empty_args() {
        let inv = ToolInvocation::new("empty_args", serde_json::Map::new());
        assert!(inv.args.is_empty());
        assert_eq!(inv.tool_name, "empty_args");
    }

    #[test]
    fn test_plugin_manager_is_enabled_unknown() {
        let mgr = PluginManager::new();
        assert!(!mgr.is_enabled("unknown_plugin"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target (round 2)
    // ============================================================

    #[test]
    fn test_base_plugin_new_and_fields() {
        let bp = BasePlugin::new("coverage_plugin", "3.5.0");
        assert_eq!(bp.name(), "coverage_plugin");
        assert_eq!(bp.version(), "3.5.0");
        // as_any downcast
        let any_ref = bp.as_any();
        let downcast = any_ref.downcast_ref::<BasePlugin>();
        assert!(downcast.is_some());
        assert_eq!(downcast.unwrap().name(), "coverage_plugin");
        assert_eq!(downcast.unwrap().version(), "3.5.0");
    }

    #[test]
    fn test_base_plugin_default_trait_impls() {
        let bp = BasePlugin::new("default_test", "0.0.1");
        // Default is_running returns false
        assert!(!bp.is_running());
        // Default init returns Ok
        let mut bp_mut = bp;
        assert!(bp_mut.init(&serde_json::json!({"key": "val"})).is_ok());
        // Default cleanup returns Ok
        assert!(bp_mut.cleanup().is_ok());
        // Default execute allows
        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err, modified) = bp_mut.execute(&mut inv);
        assert!(allowed);
        assert!(err.is_none());
        assert!(!modified);
    }

    #[test]
    fn test_plugin_manager_register_unregister_reregister() {
        struct SimplePlugin(&'static str);
        impl Plugin for SimplePlugin {
            fn name(&self) -> &str { self.0 }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        let mut mgr = PluginManager::new();
        mgr.register(Box::new(SimplePlugin("simple_plugin"))).unwrap();
        assert!(mgr.is_enabled("simple_plugin"));

        // Unregister
        mgr.unregister("simple_plugin").unwrap();
        assert!(!mgr.is_enabled("simple_plugin"));

        // Re-register
        mgr.register(Box::new(SimplePlugin("simple_plugin"))).unwrap();
        assert!(mgr.is_enabled("simple_plugin"));
    }

    #[test]
    fn test_execute_chain_with_mixed_enabled_disabled() {
        struct LogPlugin;
        impl Plugin for LogPlugin {
            fn name(&self) -> &str { "log_enabled" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                inv.metadata.insert("logged".into(), serde_json::json!(true));
                (true, None, true)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct DenyPlugin;
        impl Plugin for DenyPlugin {
            fn name(&self) -> &str { "deny_disabled" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, _inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                (false, Some("should not reach".into()), false)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct AuditPlugin;
        impl Plugin for AuditPlugin {
            fn name(&self) -> &str { "audit_enabled" }
            fn as_any(&self) -> &dyn Any { self }
            fn execute(&self, inv: &mut ToolInvocation) -> (bool, Option<String>, bool) {
                inv.metadata.insert("audited".into(), serde_json::json!(true));
                (true, None, true)
            }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(LogPlugin)).unwrap();
        mgr.register(Box::new(DenyPlugin)).unwrap();
        mgr.register(Box::new(AuditPlugin)).unwrap();
        // Disable the deny plugin
        mgr.disable("deny_disabled");

        let mut inv = ToolInvocation::new("test", serde_json::Map::new());
        let (allowed, err) = mgr.execute(&mut inv);
        assert!(allowed, "Should be allowed since deny plugin is disabled");
        assert!(err.is_none());
        assert!(inv.metadata.contains_key("logged"));
        assert!(inv.metadata.contains_key("audited"));
        // deny plugin was skipped
    }

    #[test]
    fn test_tool_invocation_new_defaults() {
        let inv = ToolInvocation::new("my_tool", serde_json::Map::new());
        assert_eq!(inv.tool_name, "my_tool");
        assert_eq!(inv.method, "Execute");
        assert!(inv.args.is_empty());
        assert!(inv.user.is_empty());
        assert!(inv.source.is_empty());
        assert!(inv.workspace.is_empty());
        assert!(inv.result.is_none());
        assert!(inv.blocking_error.is_none());
        assert!(inv.metadata.is_empty());
    }

    #[test]
    fn test_tool_invocation_modify_fields() {
        let mut inv = ToolInvocation::new("tool", serde_json::Map::new());
        inv.method = "Stream".to_string();
        inv.user = "admin".to_string();
        inv.source = "rpc".to_string();
        inv.workspace = "/home/user".to_string();
        inv.result = Some(serde_json::json!({"ok": true}));
        inv.blocking_error = Some("denied".to_string());
        inv.metadata.insert("key".to_string(), serde_json::json!("val"));

        assert_eq!(inv.method, "Stream");
        assert_eq!(inv.user, "admin");
        assert_eq!(inv.source, "rpc");
        assert_eq!(inv.workspace, "/home/user");
        assert_eq!(inv.result, Some(serde_json::json!({"ok": true})));
        assert_eq!(inv.blocking_error, Some("denied".to_string()));
        assert_eq!(inv.metadata.get("key").unwrap(), "val");
    }

    #[test]
    fn test_cleanup_all_multiple_with_mixed_errors() {
        struct OkPlugin(&'static str);
        impl Plugin for OkPlugin {
            fn name(&self) -> &str { self.0 }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Ok(()) }
        }
        struct ErrPlugin(&'static str);
        impl Plugin for ErrPlugin {
            fn name(&self) -> &str { self.0 }
            fn as_any(&self) -> &dyn Any { self }
            fn cleanup(&self) -> Result<(), String> { Err("error".to_string()) }
        }

        let mut mgr = PluginManager::new();
        mgr.register(Box::new(OkPlugin("ok1"))).unwrap();
        mgr.register(Box::new(ErrPlugin("err1"))).unwrap();
        mgr.register(Box::new(OkPlugin("ok2"))).unwrap();

        mgr.cleanup_all();
        assert!(mgr.list_plugins().is_empty());
        assert!(!mgr.is_enabled("ok1"));
        assert!(!mgr.is_enabled("err1"));
        assert!(!mgr.is_enabled("ok2"));
    }

    // ============================================================
    // Additional coverage for 95%+ target (round 3)
    // ============================================================

    #[test]
    fn test_testplugin_all_trait_methods() {
        let p = TestPlugin { running: true };
        assert_eq!(p.name(), "test");
        assert_eq!(p.version(), "1.0.0");
        assert!(p.is_running());
        let any_ref: &dyn Any = p.as_any();
        let downcast = any_ref.downcast_ref::<TestPlugin>();
        assert!(downcast.is_some());
    }

    #[test]
    fn test_plugin_manager_full_lifecycle() {
        let mut mgr = PluginManager::default();
        mgr.register(Box::new(TestPlugin { running: false })).unwrap();
        // get_plugin when enabled
        assert!(mgr.get_plugin("test").is_some());
        // disable and get_plugin returns None
        mgr.disable("test");
        assert!(mgr.get_plugin("test").is_none());
        // enable and get_plugin works again
        mgr.enable("test");
        assert!(mgr.get_plugin("test").is_some());
        // unregister calls cleanup
        mgr.unregister("test").unwrap();
        assert!(mgr.get_plugin("test").is_none());
        assert_eq!(mgr.list_plugins().len(), 0);
    }
}
