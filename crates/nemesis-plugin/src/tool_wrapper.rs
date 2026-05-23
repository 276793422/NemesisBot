//! Tool wrapper with plugin interception.
//!
//! Mirrors Go `module/plugin/wrapper.go` — wraps a tool executor with
//! three-phase plugin interception (pre-check, execute, post-check).

use crate::plugin::{PluginManager, ToolInvocation};
use std::sync::Arc;
use parking_lot::Mutex;

/// Trait for executing tools.
///
/// Mirrors Go `ToolExecutor` interface.
pub trait ToolExecutor: Send + Sync {
    /// Execute the tool with the given arguments.
    fn execute(&self, args: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String>;
}

/// Wraps a tool with plugin support.
///
/// Executes in three phases:
/// 1. **Pre-execution**: Ask plugins if the call should proceed.
/// 2. **Execute**: Run the original tool.
/// 3. **Post-execution**: Let plugins inspect/modify the result.
pub struct ToolWrapper {
    tool_name: String,
    plugin_mgr: Arc<Mutex<PluginManager>>,
    user: String,
    source: String,
    workspace: String,
    original_tool: Arc<dyn ToolExecutor>,
}

impl ToolWrapper {
    /// Create a new tool wrapper with plugin support.
    pub fn new(
        tool_name: &str,
        plugin_mgr: Arc<Mutex<PluginManager>>,
        user: &str,
        source: &str,
        workspace: &str,
        original_tool: Arc<dyn ToolExecutor>,
    ) -> Self {
        Self {
            tool_name: tool_name.to_string(),
            plugin_mgr,
            user: user.to_string(),
            source: source.to_string(),
            workspace: workspace.to_string(),
            original_tool,
        }
    }
}

impl ToolExecutor for ToolWrapper {
    fn execute(&self, args: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
        // Create tool invocation for plugin inspection
        let mut invocation = ToolInvocation {
            tool_name: self.tool_name.clone(),
            method: "Execute".to_string(),
            args: args.clone(),
            user: self.user.clone(),
            source: self.source.clone(),
            workspace: self.workspace.clone(),
            result: None,
            blocking_error: None,
            metadata: serde_json::Map::new(),
        };

        // Phase 1: Pre-execution — ask plugins if we should proceed
        {
            let mgr = self.plugin_mgr.lock();
            let (allowed, err) = mgr.execute(&mut invocation);
            if !allowed {
                return Err(err.unwrap_or_else(|| "operation denied by plugin".to_string()));
            }
        }

        // Phase 2: Execute the original tool
        let result = self.original_tool.execute(args);
        match &result {
            Ok(val) => {
                invocation.result = Some(val.clone());
            }
            Err(e) => {
                invocation.blocking_error = Some(e.clone());
            }
        }

        // Phase 3: Post-execution — let plugins inspect/modify result
        {
            let mgr = self.plugin_mgr.lock();
            let (allowed, err) = mgr.execute(&mut invocation);
            if !allowed {
                return Err(err.unwrap_or_else(|| "post-execution denied by plugin".to_string()));
            }
        }

        // Check if a plugin modified the result
        if let Some(modified) = invocation.result {
            Ok(modified)
        } else {
            result
        }
    }
}

/// Wraps an existing tool to make it plugin-aware.
///
/// A convenience struct that delegates to `ToolWrapper`.
/// Mirrors Go `PluginableTool`.
pub struct PluginableTool {
    name: String,
    plugin_mgr: Arc<Mutex<PluginManager>>,
    inner_tool: Arc<dyn ToolExecutor>,
    user: String,
    source: String,
    workspace: String,
}

impl PluginableTool {
    /// Create a new plugin-aware tool.
    pub fn new(
        name: &str,
        plugin_mgr: Arc<Mutex<PluginManager>>,
        inner_tool: Arc<dyn ToolExecutor>,
        user: &str,
        source: &str,
        workspace: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            plugin_mgr,
            inner_tool,
            user: user.to_string(),
            source: source.to_string(),
            workspace: workspace.to_string(),
        }
    }
}

impl ToolExecutor for PluginableTool {
    fn execute(&self, args: &serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Value, String> {
        let wrapper = ToolWrapper::new(
            &self.name,
            self.plugin_mgr.clone(),
            &self.user,
            &self.source,
            &self.workspace,
            self.inner_tool.clone(),
        );
        wrapper.execute(args)
    }
}

#[cfg(test)]
mod tests;
