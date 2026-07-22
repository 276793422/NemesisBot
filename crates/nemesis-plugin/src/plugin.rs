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
    fn version(&self) -> &str {
        "0.1.0"
    }

    /// Initialize the plugin with configuration.
    fn init(&mut self, _config: &serde_json::Value) -> Result<(), String> {
        Ok(())
    }

    /// Execute intercepts a tool execution.
    /// Returns (allowed, error_message, modified).
    /// Mirrors Go Plugin.Execute.
    fn execute(&self, _invocation: &mut ToolInvocation) -> (bool, Option<String>, bool) {
        (true, None, false)
    }

    /// Check if plugin is running.
    fn is_running(&self) -> bool {
        false
    }

    /// Cast to Any for downcasting.
    fn as_any(&self) -> &dyn Any;

    /// Cleanup when unloading.
    fn cleanup(&self) -> Result<(), String> {
        Ok(())
    }
}

/// Base plugin with default implementations.
pub struct BasePlugin {
    name: String,
    version: String,
}

impl BasePlugin {
    pub fn new(name: &str, version: &str) -> Self {
        Self {
            name: name.to_string(),
            version: version.to_string(),
        }
    }
}

impl Plugin for BasePlugin {
    fn name(&self) -> &str {
        &self.name
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
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
        let idx = self
            .plugins
            .iter()
            .position(|p| p.name() == name)
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
        self.plugins
            .iter()
            .find(|p| p.name() == name && self.is_enabled(name))
            .map(|p| p.as_ref())
    }

    /// List all enabled plugins.
    pub fn list_plugins(&self) -> Vec<&dyn Plugin> {
        self.plugins
            .iter()
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
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
