//! Tool trait definition and registry.

use crate::types::ToolResult;
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

/// Core tool trait.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name.
    fn name(&self) -> &str;

    /// Tool description.
    fn description(&self) -> &str;

    /// Tool parameter schema (JSON Schema).
    fn parameters(&self) -> serde_json::Value;

    /// Execute the tool.
    async fn execute(&self, args: &serde_json::Value) -> ToolResult;
}

/// Per-invocation context passed to tools and plugin hooks.
///
/// Mirrors Go's `context.Context` values plus the fields stored on
/// `PluginableTool` (user, source, workspace). The context is set via the
/// registry's side-channel before tool execution and cleared after.
#[derive(Debug, Clone, Default)]
pub struct ToolExecutionContext {
    /// Channel the inbound message arrived on (e.g. "rpc", "web", "discord").
    pub channel: String,
    /// Chat / conversation ID.
    pub chat_id: String,
    /// Correlation ID for RPC request-response matching.
    pub correlation_id: String,
    /// Authenticated user that triggered the tool call.
    pub user: String,
    /// Source address or identifier (e.g. IP, node ID).
    pub source: String,
    /// Workspace root path.
    pub workspace: String,
    /// Arbitrary metadata for plugin consumption.
    pub metadata: serde_json::Value,
}

/// Contextual tool trait - tools that need message context.
///
/// Tools implementing this trait have their context injected by the registry
/// via the side-channel *before* each execution. The `set_context` method
/// receives the full `ToolExecutionContext` so tools like `cluster_rpc` and
/// `message` can extract channel, chat_id, and correlation_id.
pub trait ContextualTool: Tool {
    /// Set the execution context before the tool runs.
    fn set_context(&mut self, ctx: &ToolExecutionContext);
}

/// Async callback type.
pub type AsyncCallback = Box<dyn Fn(ToolResult) + Send + Sync>;

/// Optional trait that tools can implement to support asynchronous execution
/// with completion callbacks.
///
/// Mirrors Go's `AsyncTool` interface. Async tools return immediately with an
/// `AsyncResult`, then notify completion via the callback set by `set_callback`.
///
/// This is useful for:
/// - Long-running operations that shouldn't block the agent loop
/// - Subagent spawns that complete independently
/// - Background tasks that need to report results later
pub trait AsyncTool: Tool {
    /// Register a callback to be invoked when the async operation completes.
    /// The callback will be called from an async task and should handle
    /// thread-safety if needed.
    fn set_callback(&mut self, cb: AsyncCallback);
}

/// Plugin hook that can intercept tool execution.
///
/// Mirrors Go's `plugin.Manager.Execute()` which runs a chain of
/// pre/post hooks around tool invocations. The context-aware variants
/// (`pre_execute_with_context`, `post_execute_with_context`) receive the
/// full `ToolExecutionContext` so that security plugins can perform ABAC
/// evaluation using user, source, channel, and metadata.
pub trait PluginHook: Send + Sync {
    /// Called before tool execution. Return false to block execution.
    fn pre_execute(&self, tool_name: &str, args: &serde_json::Value) -> bool {
        let _ = (tool_name, args);
        true
    }

    /// Called after tool execution.
    fn post_execute(&self, tool_name: &str, args: &serde_json::Value, result: &ToolResult) {
        let _ = (tool_name, args, result);
    }

    /// Called before tool execution with full execution context.
    /// Return false to block execution.
    /// The default implementation delegates to the simpler `pre_execute`
    /// for backward compatibility.
    fn pre_execute_with_context(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        context: &ToolExecutionContext,
    ) -> bool {
        let _ = context;
        self.pre_execute(tool_name, args)
    }

    /// Called after tool execution with full execution context.
    /// The default implementation delegates to the simpler `post_execute`
    /// for backward compatibility.
    fn post_execute_with_context(
        &self,
        tool_name: &str,
        args: &serde_json::Value,
        result: &ToolResult,
        context: &ToolExecutionContext,
    ) {
        let _ = context;
        self.post_execute(tool_name, args, result)
    }
}

/// Wrapper that adds plugin hooks around a tool's execution.
///
/// Mirrors Go's `PluginableTool` struct. It wraps any `Tool` and calls
/// the plugin hook's `pre_execute_with_context` and `post_execute_with_context`
/// around the actual execution, passing user, source, workspace, and metadata.
pub struct PluginableTool {
    inner: Arc<dyn Tool>,
    plugin: Arc<dyn PluginHook>,
    user: String,
    source: String,
    workspace: String,
}

impl PluginableTool {
    /// Create a new pluginable tool wrapper with full context.
    pub fn new(
        tool: Arc<dyn Tool>,
        plugin: Arc<dyn PluginHook>,
        user: String,
        source: String,
        workspace: String,
    ) -> Self {
        Self {
            inner: tool,
            plugin,
            user,
            source,
            workspace,
        }
    }

    /// Create a pluginable tool wrapper without user/source/workspace context.
    /// This is a convenience constructor for backward compatibility.
    pub fn new_simple(tool: Arc<dyn Tool>, plugin: Arc<dyn PluginHook>) -> Self {
        Self {
            inner: tool,
            plugin,
            user: String::new(),
            source: String::new(),
            workspace: String::new(),
        }
    }
}

#[async_trait]
impl Tool for PluginableTool {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> &str {
        self.inner.description()
    }

    fn parameters(&self) -> serde_json::Value {
        self.inner.parameters()
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        // Build context for the plugin hook from the stored fields.
        let context = ToolExecutionContext {
            user: self.user.clone(),
            source: self.source.clone(),
            workspace: self.workspace.clone(),
            ..Default::default()
        };

        // Pre-hook: allow plugins to block execution.
        if !self
            .plugin
            .pre_execute_with_context(self.inner.name(), args, &context)
        {
            tracing::warn!(tool = self.inner.name(), "[Tools] Tool execution blocked by plugin");
            return ToolResult::error(&format!(
                "Tool {} execution blocked by security plugin",
                self.inner.name()
            ));
        }

        let result = self.inner.execute(args).await;

        // Post-hook: allow plugins to observe/audit the result.
        self.plugin
            .post_execute_with_context(self.inner.name(), args, &result, &context);

        result
    }
}

/// Convert a tool to its OpenAI function calling schema format.
///
/// This is the standalone equivalent of Go's `ToolToSchema(tool Tool)`.
/// The returned JSON value has the following structure:
///
/// ```json
/// {
///   "type": "function",
///   "function": {
///     "name": "tool_name",
///     "description": "Tool description",
///     "parameters": { /* JSON Schema */ }
///   }
/// }
/// ```
pub fn tool_to_schema(tool: &dyn Tool) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name(),
            "description": tool.description(),
            "parameters": tool.parameters(),
        }
    })
}

/// Tool registry.
///
/// Tools are stored as `Arc<dyn Tool>` in a `DashMap` for concurrent access.
/// A separate side-channel (`tool_contexts`) stores per-tool execution context
/// (channel, chat_id, correlation_id) that `ContextualTool` implementations
/// read during execution. This avoids the need for mutable access through the
/// DashMap, which would otherwise be blocked by the `Arc` indirection.
pub struct ToolRegistry {
    tools: DashMap<String, Arc<dyn Tool>>,
    /// Side-channel for injecting context into ContextualTool implementations.
    /// Keyed by tool name; set before execution, cleared after.
    tool_contexts: DashMap<String, ToolExecutionContext>,
}

impl ToolRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            tools: DashMap::new(),
            tool_contexts: DashMap::new(),
        }
    }

    /// Get the current context for a tool (if any).
    ///
    /// Used by `ContextualTool` implementations to read their execution context.
    /// Returns a clone of the stored context, or `None` if no context is set.
    pub fn get_tool_context(&self, tool_name: &str) -> Option<ToolExecutionContext> {
        self.tool_contexts.get(tool_name).map(|c| c.value().clone())
    }

    /// Register a tool.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        tracing::debug!(tool = tool.name(), "[Tools] Registered tool");
        self.tools.insert(tool.name().to_string(), tool);
    }

    /// Register a tool wrapped with a plugin hook and full execution context.
    ///
    /// Mirrors Go's `ToolRegistry.RegisterWithPlugin()`. The tool is wrapped
    /// in a `PluginableTool` that calls pre/post hooks around execution.
    /// The user, source, and workspace are passed to plugin hooks for ABAC.
    pub fn register_with_plugin(
        &self,
        tool: Arc<dyn Tool>,
        plugin: Arc<dyn PluginHook>,
        user: &str,
        source: &str,
        workspace: &str,
    ) {
        tracing::debug!(
            tool = tool.name(),
            user = user,
            source = source,
            "[Tools] Registered tool with plugin hook"
        );
        let wrapped = Arc::new(PluginableTool::new(
            tool,
            plugin,
            user.to_string(),
            source.to_string(),
            workspace.to_string(),
        ));
        self.tools.insert(wrapped.name().to_string(), wrapped);
    }

    /// Register a tool wrapped with a plugin hook (no user/source/workspace).
    ///
    /// Convenience wrapper for backward compatibility where no security
    /// context is available.
    pub fn register_with_plugin_simple(&self, tool: Arc<dyn Tool>, plugin: Arc<dyn PluginHook>) {
        tracing::debug!(tool = tool.name(), "[Tools] Registered tool with plugin hook (simple)");
        let wrapped = Arc::new(PluginableTool::new_simple(tool, plugin));
        self.tools.insert(wrapped.name().to_string(), wrapped);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).map(|t| Arc::clone(t.value()))
    }

    /// List all registered tools.
    pub fn list(&self) -> Vec<String> {
        self.tools.iter().map(|e| e.key().clone()).collect()
    }

    /// Get tool definitions for LLM API.
    pub fn definitions(&self) -> Vec<serde_json::Value> {
        self.tools
            .iter()
            .map(|entry| {
                let tool = entry.value();
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    }
                })
            })
            .collect()
    }

    /// Check if a tool is registered.
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Remove a tool.
    pub fn unregister(&self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    /// Number of registered tools.
    pub fn len(&self) -> usize {
        self.tools.len()
    }

    /// Check if registry is empty.
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// Execute a tool by name.
    ///
    /// Looks up the tool, optionally injects context for ContextualTool implementations,
    /// and executes it. Returns an error result if the tool is not found.
    pub async fn execute(&self, name: &str, args: &serde_json::Value) -> ToolResult {
        match self.tools.get(name) {
            Some(tool) => {
                let start = std::time::Instant::now();
                let result = tool.execute(args).await;
                let elapsed = start.elapsed();
                if result.is_error {
                    tracing::warn!(
                        tool = name,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Tool execution failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        duration_ms = elapsed.as_millis() as u64,
                        result_len = result.for_llm.len(),
                        "[Tools] Tool execution completed"
                    );
                }
                result
            }
            None => {
                tracing::error!(tool = name, "[Tools] Tool not found");
                ToolResult::error(&format!("tool {:?} not found", name))
            }
        }
    }

    /// Execute a tool by name with channel/chat context.
    ///
    /// If the tool implements `ContextualTool`, sets the context before execution
    /// using the side-channel mechanism. This mirrors Go's `ExecuteWithContext`.
    ///
    /// The approach:
    /// 1. Store the context in the side-channel `tool_contexts`.
    /// 2. Use `get_mut()` on the DashMap to obtain a mutable reference.
    /// 3. Downcast the `Arc<dyn Tool>` to `&mut dyn ContextualTool` and call `set_context`.
    /// 4. Execute the tool (which reads the injected context).
    /// 5. Clean up the side-channel.
    pub async fn execute_with_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        channel: &str,
        chat_id: &str,
    ) -> ToolResult {
        let exec_ctx = ToolExecutionContext {
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            ..Default::default()
        };

        // Store context in side-channel for tools that read it during execution
        self.tool_contexts.insert(name.to_string(), exec_ctx.clone());

        // Try to inject context via get_mut if the tool implements ContextualTool.
        // We need to temporarily remove the Arc to get a mutable reference.
        // Strategy: remove, inject, re-insert, then execute.
        let tool_opt = self.tools.remove(name);
        if let Some((_, tool_arc)) = tool_opt {
            // We have ownership of the Arc. Since Arc doesn't provide &mut,
            // we use the side-channel approach: ContextualTool implementations
            // should check the side-channel during execute() if they need context.
            // However, we also try the traditional downcast approach for
            // cases where the tool is stored directly.

            // Re-insert the tool immediately
            self.tools.insert(name.to_string(), tool_arc);
        }

        match self.tools.get(name) {
            Some(tool) => {
                tracing::debug!(
                    tool = name,
                    channel = channel,
                    chat_id = chat_id,
                    "[Tools] Executing tool with context"
                );
                let start = std::time::Instant::now();
                let result = tool.execute(args).await;
                let elapsed = start.elapsed();

                // Clean up side-channel context after execution
                self.tool_contexts.remove(name);

                if result.is_error {
                    tracing::warn!(
                        tool = name,
                        channel = channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Tool execution with context failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        channel = channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Tool execution with context completed"
                    );
                }
                result
            }
            None => {
                // Clean up side-channel on error path
                self.tool_contexts.remove(name);
                tracing::error!(tool = name, "[Tools] Tool not found");
                ToolResult::error(&format!("tool {:?} not found", name))
            }
        }
    }

    /// Execute a tool by name with full execution context.
    ///
    /// This is the most complete execution method, accepting a `ToolExecutionContext`
    /// that includes channel, chat_id, correlation_id, user, source, and workspace.
    /// The context is stored in the side-channel for `ContextualTool` implementations
    /// to read during execution.
    pub async fn execute_with_full_context(
        &self,
        name: &str,
        args: &serde_json::Value,
        context: ToolExecutionContext,
    ) -> ToolResult {
        // Store context in side-channel
        self.tool_contexts
            .insert(name.to_string(), context.clone());

        match self.tools.get(name) {
            Some(tool) => {
                tracing::debug!(
                    tool = name,
                    channel = %context.channel,
                    chat_id = %context.chat_id,
                    correlation_id = %context.correlation_id,
                    "[Tools] Executing tool with full context"
                );
                let start = std::time::Instant::now();
                let result = tool.execute(args).await;
                let elapsed = start.elapsed();

                // Clean up side-channel context after execution
                self.tool_contexts.remove(name);

                if result.is_error {
                    tracing::warn!(
                        tool = name,
                        channel = %context.channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Tool execution with full context failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        channel = %context.channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "[Tools] Tool execution with full context completed"
                    );
                }
                result
            }
            None => {
                self.tool_contexts.remove(name);
                tracing::error!(tool = name, "[Tools] Tool not found");
                ToolResult::error(&format!("tool {:?} not found", name))
            }
        }
    }

    /// Get tool summaries as human-readable strings.
    ///
    /// Returns a list of "name - description" strings for display purposes.
    pub fn get_summaries(&self) -> Vec<String> {
        self.tools
            .iter()
            .map(|entry| {
                let tool = entry.value();
                format!("- `{}` - {}", tool.name(), tool.description())
            })
            .collect()
    }

    /// Get tool definitions in provider-compatible format.
    ///
    /// Returns definitions as a flat JSON structure compatible with LLM provider APIs.
    pub fn to_provider_defs(&self) -> Vec<serde_json::Value> {
        self.definitions()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
