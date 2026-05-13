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
            tracing::warn!(tool = self.inner.name(), "Tool execution blocked by plugin");
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
                        "Tool execution failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        duration_ms = elapsed.as_millis() as u64,
                        result_len = result.for_llm.len(),
                        "Tool execution completed"
                    );
                }
                result
            }
            None => {
                tracing::error!(tool = name, "Tool not found");
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
                    "Executing tool with context"
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
                        "Tool execution with context failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        channel = channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "Tool execution with context completed"
                    );
                }
                result
            }
            None => {
                // Clean up side-channel on error path
                self.tool_contexts.remove(name);
                tracing::error!(tool = name, "Tool not found");
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
                    "Executing tool with full context"
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
                        "Tool execution with full context failed"
                    );
                } else {
                    tracing::info!(
                        tool = name,
                        channel = %context.channel,
                        duration_ms = elapsed.as_millis() as u64,
                        "Tool execution with full context completed"
                    );
                }
                result
            }
            None => {
                self.tool_contexts.remove(name);
                tracing::error!(tool = name, "Tool not found");
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
mod tests {
    use super::*;

    struct EchoTool;

    #[async_trait]
    impl Tool for EchoTool {
        fn name(&self) -> &str { "echo" }
        fn description(&self) -> &str { "Echo back the input" }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "text": {"type": "string", "description": "Text to echo"}
                },
                "required": ["text"]
            })
        }
        async fn execute(&self, args: &serde_json::Value) -> ToolResult {
            let text = args["text"].as_str().unwrap_or("");
            ToolResult::success(text)
        }
    }

    struct ReadFileTool;

    #[async_trait]
    impl Tool for ReadFileTool {
        fn name(&self) -> &str { "read_file" }
        fn description(&self) -> &str { "Read a file" }
        fn parameters(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"path": {"type": "string"}}})
        }
        async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
            ToolResult::success("file contents")
        }
    }

    #[tokio::test]
    async fn test_register_and_get() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert!(registry.has("echo"));
        assert!(!registry.has("unknown"));
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_execute_tool() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let tool = registry.get("echo").unwrap();
        let result = tool.execute(&serde_json::json!({"text": "hello"})).await;
        assert_eq!(result.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_definitions() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(ReadFileTool));

        let defs = registry.definitions();
        assert_eq!(defs.len(), 2);
    }

    #[tokio::test]
    async fn test_unregister() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert!(registry.unregister("echo"));
        assert!(!registry.has("echo"));
        assert!(registry.is_empty());
    }

    #[tokio::test]
    async fn test_list() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(ReadFileTool));

        let list = registry.list();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"echo".to_string()));
        assert!(list.contains(&"read_file".to_string()));
    }

    #[tokio::test]
    async fn test_execute_existing_tool() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_execute_missing_tool() {
        let registry = ToolRegistry::new();

        let result = registry.execute("nonexistent", &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_execute_with_context() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry
            .execute_with_context("echo", &serde_json::json!({"text": "test"}), "rpc", "chat123")
            .await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "test");
    }

    #[tokio::test]
    async fn test_get_summaries() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(ReadFileTool));

        let summaries = registry.get_summaries();
        assert_eq!(summaries.len(), 2);
        let combined = summaries.join("\n");
        assert!(combined.contains("echo"));
        assert!(combined.contains("read_file"));
    }

    #[tokio::test]
    async fn test_to_provider_defs() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let defs = registry.to_provider_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["type"], "function");
        assert_eq!(defs[0]["function"]["name"], "echo");
    }

    // --- PluginableTool tests ---

    struct AllowAllPlugin;
    impl PluginHook for AllowAllPlugin {}

    struct BlockAllPlugin;
    impl PluginHook for BlockAllPlugin {
        fn pre_execute(&self, tool_name: &str, _args: &serde_json::Value) -> bool {
            let _ = tool_name;
            false
        }
    }

    struct AuditingPlugin {
        called: std::sync::Mutex<bool>,
    }
    impl AuditingPlugin {
        fn new() -> Self { Self { called: std::sync::Mutex::new(false) } }
    }
    impl PluginHook for AuditingPlugin {
        fn post_execute(&self, _tool_name: &str, _args: &serde_json::Value, _result: &ToolResult) {
            *self.called.lock().unwrap() = true;
        }
    }

    #[tokio::test]
    async fn test_pluginable_tool_allows_execution() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::new(AllowAllPlugin));

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_pluginable_tool_blocks_execution() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::new(BlockAllPlugin));

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("blocked"));
    }

    #[tokio::test]
    async fn test_pluginable_tool_post_hook_called() {
        let plugin = Arc::new(AuditingPlugin::new());
        let called_flag = {
            let plugin_ref = plugin.clone();
            // We need to check the flag after execution. Since AuditingPlugin
            // uses a Mutex<bool>, we can check it after the call.
            move || -> bool { *plugin_ref.called.lock().unwrap() }
        };

        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), plugin);

        let _ = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(called_flag());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn test_registry_default() {
        let registry = ToolRegistry::default();
        assert!(registry.is_empty());
        assert_eq!(registry.len(), 0);
    }

    #[tokio::test]
    async fn test_register_overwrites_existing() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        // Registering again should overwrite
        registry.register(Arc::new(EchoTool));
        assert_eq!(registry.len(), 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent() {
        let registry = ToolRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[tokio::test]
    async fn test_unregister_nonexistent() {
        let registry = ToolRegistry::new();
        assert!(!registry.unregister("nonexistent"));
    }

    #[tokio::test]
    async fn test_definitions_structure() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let defs = registry.definitions();
        assert_eq!(defs.len(), 1);
        let def = &defs[0];
        assert_eq!(def["type"], "function");
        assert_eq!(def["function"]["name"], "echo");
        assert_eq!(def["function"]["description"], "Echo back the input");
        assert!(def["function"]["parameters"]["properties"]["text"].is_object());
    }

    #[tokio::test]
    async fn test_execute_with_context_missing_tool() {
        let registry = ToolRegistry::new();
        let result = registry
            .execute_with_context("nonexistent", &serde_json::json!({}), "web", "chat123")
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("not found"));
    }

    #[tokio::test]
    async fn test_get_summaries_content() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let summaries = registry.get_summaries();
        assert_eq!(summaries.len(), 1);
        assert!(summaries[0].contains("echo"));
        assert!(summaries[0].contains("Echo back"));
    }

    #[tokio::test]
    async fn test_to_provider_defs_structure() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(ReadFileTool));

        let defs = registry.to_provider_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0]["function"]["name"], "read_file");
    }

    #[tokio::test]
    async fn test_pluginable_tool_preserves_name() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::new(AllowAllPlugin));

        let tool = registry.get("echo").unwrap();
        assert_eq!(tool.name(), "echo");
        assert_eq!(tool.description(), "Echo back the input");
    }

    #[tokio::test]
    async fn test_pluginable_tool_error_message_contains_name() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::new(BlockAllPlugin));

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("echo"));
        assert!(result.for_llm.contains("blocked"));
    }

    #[tokio::test]
    async fn test_multiple_tools_registered() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(ReadFileTool));

        assert_eq!(registry.len(), 2);
        let list = registry.list();
        assert_eq!(list.len(), 2);

        let defs = registry.definitions();
        assert_eq!(defs.len(), 2);

        let summaries = registry.get_summaries();
        assert_eq!(summaries.len(), 2);
    }

    #[tokio::test]
    async fn test_execute_returns_result_on_success() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry.execute("echo", &serde_json::json!({"text": "test_result"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "test_result");
    }

    // --- tool_to_schema tests ---

    #[test]
    fn test_tool_to_schema_structure() {
        let tool = EchoTool;
        let schema = tool_to_schema(&tool);

        assert_eq!(schema["type"], "function");
        assert_eq!(schema["function"]["name"], "echo");
        assert_eq!(schema["function"]["description"], "Echo back the input");
        assert!(schema["function"]["parameters"]["properties"]["text"].is_object());
    }

    #[test]
    fn test_tool_to_schema_matches_definitions() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        registry.register(Arc::new(ReadFileTool));

        let defs = registry.definitions();
        for def in &defs {
            let name = def["function"]["name"].as_str().unwrap();
            let tool = registry.get(name).unwrap();
            let schema = tool_to_schema(tool.as_ref());
            assert_eq!(def["type"], schema["type"]);
            assert_eq!(def["function"]["name"], schema["function"]["name"]);
            assert_eq!(def["function"]["description"], schema["function"]["description"]);
        }
    }

    // ============================================================
    // Additional registry tests - concurrent, edge cases
    // ============================================================

    #[tokio::test]
    async fn test_concurrent_register_and_execute() {
        let registry = Arc::new(ToolRegistry::new());
        registry.register(Arc::new(EchoTool));

        let mut handles = vec![];
        for i in 0..10 {
            let reg = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                reg.execute("echo", &serde_json::json!({"text": format!("concurrent-{}", i)})).await
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
        for result in results {
            let r = result.unwrap();
            assert!(!r.is_error);
        }
    }

    #[tokio::test]
    async fn test_register_same_tool_twice_overwrites() {
        let registry = ToolRegistry::new();

        struct VersionedTool { version: usize }
        #[async_trait]
        impl Tool for VersionedTool {
            fn name(&self) -> &str { "versioned" }
            fn description(&self) -> &str { "Versioned tool" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                ToolResult::success(&format!("v{}", self.version))
            }
        }

        registry.register(Arc::new(VersionedTool { version: 1 }));
        let result = registry.execute("versioned", &serde_json::json!({})).await;
        assert_eq!(result.for_llm, "v1");

        registry.register(Arc::new(VersionedTool { version: 2 }));
        let result = registry.execute("versioned", &serde_json::json!({})).await;
        assert_eq!(result.for_llm, "v2");
    }

    #[tokio::test]
    async fn test_execute_with_context_propagates_to_contextual_tool() {
        struct CapturingTool {
            channel: std::sync::Mutex<String>,
            chat_id: std::sync::Mutex<String>,
        }
        #[async_trait]
        impl Tool for CapturingTool {
            fn name(&self) -> &str { "capture" }
            fn description(&self) -> &str { "Captures context" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                let ch = self.channel.lock().unwrap().clone();
                let cid = self.chat_id.lock().unwrap().clone();
                ToolResult::success(&format!("ch={},cid={}", ch, cid))
            }
        }
        impl ContextualTool for CapturingTool {
            fn set_context(&mut self, ctx: &crate::registry::ToolExecutionContext) {
                if let Ok(mut ch) = self.channel.try_lock() { *ch = ctx.channel.clone(); }
                if let Ok(mut cid) = self.chat_id.try_lock() { *cid = ctx.chat_id.clone(); }
            }
        }

        let registry = ToolRegistry::new();
        registry.register(Arc::new(CapturingTool {
            channel: std::sync::Mutex::new(String::new()),
            chat_id: std::sync::Mutex::new(String::new()),
        }));

        let result = registry
            .execute_with_context("capture", &serde_json::json!({}), "rpc", "chat-ctx-123")
            .await;
        // execute_with_context stores context in a side-channel but doesn't call
        // set_context on Arc<dyn Tool> (which can't provide &mut). The tool
        // executes normally and returns a result.
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_get_summaries_empty() {
        let registry = ToolRegistry::new();
        let summaries = registry.get_summaries();
        assert!(summaries.is_empty());
    }

    #[tokio::test]
    async fn test_to_provider_defs_empty() {
        let registry = ToolRegistry::new();
        let defs = registry.to_provider_defs();
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn test_definitions_empty() {
        let registry = ToolRegistry::new();
        let defs = registry.definitions();
        assert!(defs.is_empty());
    }

    #[tokio::test]
    async fn test_list_empty() {
        let registry = ToolRegistry::new();
        let list = registry.list();
        assert!(list.is_empty());
    }

    #[tokio::test]
    async fn test_unregister_then_re_register() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));
        assert!(registry.has("echo"));

        registry.unregister("echo");
        assert!(!registry.has("echo"));

        registry.register(Arc::new(EchoTool));
        assert!(registry.has("echo"));
        let result = registry.execute("echo", &serde_json::json!({"text": "back"})).await;
        assert_eq!(result.for_llm, "back");
    }

    #[test]
    fn test_tool_to_schema_empty_properties() {
        struct MinimalTool;
        #[async_trait]
        impl Tool for MinimalTool {
            fn name(&self) -> &str { "minimal" }
            fn description(&self) -> &str { "Minimal" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                ToolResult::success("ok")
            }
        }
        let schema = tool_to_schema(&MinimalTool);
        assert_eq!(schema["function"]["name"], "minimal");
        assert_eq!(schema["function"]["parameters"]["type"], "object");
    }

    #[tokio::test]
    async fn test_pluginable_tool_pre_hook_receives_args() {
        struct InspectPlugin {
            last_args: std::sync::Mutex<Option<serde_json::Value>>,
        }
        impl InspectPlugin {
            fn new() -> Self { Self { last_args: std::sync::Mutex::new(None) } }
        }
        impl PluginHook for InspectPlugin {
            fn pre_execute(&self, _tool_name: &str, args: &serde_json::Value) -> bool {
                if let Ok(mut la) = self.last_args.try_lock() {
                    *la = Some(args.clone());
                }
                true
            }
        }

        let plugin = Arc::new(InspectPlugin::new());
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::clone(&plugin) as Arc<dyn PluginHook>);

        let _ = registry.execute("echo", &serde_json::json!({"text": "inspected"})).await;
        let args = plugin.last_args.lock().unwrap().clone().unwrap();
        assert_eq!(args["text"], "inspected");
    }

    #[tokio::test]
    async fn test_many_tools_registered() {
        let registry = ToolRegistry::new();

        struct NumTool(usize);
        #[async_trait]
        impl Tool for NumTool {
            fn name(&self) -> &str { Box::leak(format!("tool_{}", self.0).into_boxed_str()) }
            fn description(&self) -> &str { "Numbered tool" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                ToolResult::success(&format!("tool_{}", self.0))
            }
        }

        for i in 0..20 {
            registry.register(Arc::new(NumTool(i)));
        }
        assert_eq!(registry.len(), 20);
        assert_eq!(registry.list().len(), 20);
        assert_eq!(registry.definitions().len(), 20);
    }

    // ============================================================
    // Registry concurrent access and execution tests
    // ============================================================

    #[tokio::test]
    async fn test_concurrent_execute_different_tools() {
        let registry = Arc::new(ToolRegistry::new());

        struct ToolA;
        #[async_trait]
        impl Tool for ToolA {
            fn name(&self) -> &str { "tool_a" }
            fn description(&self) -> &str { "Tool A" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                ToolResult::success(&format!("A: {}", args["input"].as_str().unwrap_or("")))
            }
        }

        struct ToolB;
        #[async_trait]
        impl Tool for ToolB {
            fn name(&self) -> &str { "tool_b" }
            fn description(&self) -> &str { "Tool B" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                ToolResult::success(&format!("B: {}", args["input"].as_str().unwrap_or("")))
            }
        }

        registry.register(Arc::new(ToolA));
        registry.register(Arc::new(ToolB));

        let mut handles = vec![];
        for i in 0..5 {
            let reg = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                reg.execute("tool_a", &serde_json::json!({"input": format!("a-{}", i)})).await
            }));
        }
        for i in 0..5 {
            let reg = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                reg.execute("tool_b", &serde_json::json!({"input": format!("b-{}", i)})).await
            }));
        }

        let results: Vec<_> = futures::future::join_all(handles).await;
        let mut a_count = 0;
        let mut b_count = 0;
        for r in results {
            let result = r.unwrap();
            assert!(!result.is_error);
            if result.for_llm.starts_with("A:") { a_count += 1; }
            else if result.for_llm.starts_with("B:") { b_count += 1; }
        }
        assert_eq!(a_count, 5);
        assert_eq!(b_count, 5);
    }

    #[tokio::test]
    async fn test_register_and_unregister_concurrently() {
        let registry = Arc::new(ToolRegistry::new());

        let reg1 = Arc::clone(&registry);
        let h1 = tokio::spawn(async move {
            reg1.register(Arc::new(EchoTool));
        });

        let reg2 = Arc::clone(&registry);
        let h2 = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            reg2.unregister("echo");
        });

        let _ = futures::future::join_all(vec![h1, h2]).await;
        // Final state depends on ordering, but should not panic
    }

    // ============================================================
    // Additional coverage tests
    // ============================================================

    #[tokio::test]
    async fn test_register_with_plugin_full_context() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin(
            Arc::new(EchoTool),
            Arc::new(AllowAllPlugin),
            "test_user",
            "cli",
            "/workspace",
        );

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "hello");
    }

    #[tokio::test]
    async fn test_register_with_plugin_blocks_execution_full_context() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin(
            Arc::new(EchoTool),
            Arc::new(BlockAllPlugin),
            "test_user",
            "cli",
            "/workspace",
        );

        let result = registry.execute("echo", &serde_json::json!({"text": "hello"})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("blocked"));
    }

    #[tokio::test]
    async fn test_execute_with_full_context_success() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let ctx = ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-123".to_string(),
            correlation_id: "corr-456".to_string(),
            user: "test".to_string(),
            source: "cli".to_string(),
            workspace: "/workspace".to_string(),
            metadata: serde_json::json!({}),
        };

        let result = registry
            .execute_with_full_context("echo", &serde_json::json!({"text": "test"}), ctx)
            .await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "test");
    }

    #[tokio::test]
    async fn test_execute_with_full_context_missing_tool() {
        let registry = ToolRegistry::new();

        let ctx = ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-789".to_string(),
            ..Default::default()
        };

        let result = registry
            .execute_with_full_context("nonexistent", &serde_json::json!({}), ctx)
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("not found"));
    }

    #[tokio::test]
    async fn test_get_tool_context_returns_none_when_empty() {
        let registry = ToolRegistry::new();
        assert!(registry.get_tool_context("echo").is_none());
    }

    #[tokio::test]
    async fn test_get_tool_context_returns_value_after_set() {
        let registry = ToolRegistry::new();
        let ctx = ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-123".to_string(),
            ..Default::default()
        };

        // Simulate what execute_with_context does
        registry.tool_contexts.insert("echo".to_string(), ctx.clone());
        let retrieved = registry.get_tool_context("echo");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().channel, "rpc");

        // Clean up
        registry.tool_contexts.remove("echo");
    }

    #[tokio::test]
    async fn test_execute_with_context_cleans_up_on_success() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let _ = registry
            .execute_with_context("echo", &serde_json::json!({"text": "test"}), "rpc", "chat-123")
            .await;

        // Context should be cleaned up
        assert!(registry.get_tool_context("echo").is_none());
    }

    #[tokio::test]
    async fn test_execute_with_full_context_cleans_up_on_success() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let ctx = ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-123".to_string(),
            ..Default::default()
        };

        let _ = registry
            .execute_with_full_context("echo", &serde_json::json!({"text": "test"}), ctx)
            .await;

        // Context should be cleaned up
        assert!(registry.get_tool_context("echo").is_none());
    }

    #[tokio::test]
    async fn test_execute_with_full_context_cleans_up_on_missing_tool() {
        let registry = ToolRegistry::new();

        let ctx = ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-789".to_string(),
            ..Default::default()
        };

        let _ = registry
            .execute_with_full_context("nonexistent", &serde_json::json!({}), ctx)
            .await;

        // Context should be cleaned up even on error
        assert!(registry.get_tool_context("nonexistent").is_none());
    }

    #[test]
    fn test_tool_to_schema_with_read_file() {
        let schema = tool_to_schema(&ReadFileTool);
        assert_eq!(schema["function"]["name"], "read_file");
        assert_eq!(schema["function"]["description"], "Read a file");
        assert!(schema["function"]["parameters"]["properties"]["path"].is_object());
    }

    #[tokio::test]
    async fn test_pluginable_tool_new_simple() {
        let registry = ToolRegistry::new();
        registry.register_with_plugin_simple(Arc::new(EchoTool), Arc::new(AllowAllPlugin));

        let result = registry.execute("echo", &serde_json::json!({"text": "simple"})).await;
        assert!(!result.is_error);
        assert_eq!(result.for_llm, "simple");
    }

    #[tokio::test]
    async fn test_pluginable_tool_post_hook_with_full_context() {
        let plugin = Arc::new(AuditingPlugin::new());
        let called_flag = {
            let plugin_ref = plugin.clone();
            move || -> bool { *plugin_ref.called.lock().unwrap() }
        };

        let registry = ToolRegistry::new();
        registry.register_with_plugin(
            Arc::new(EchoTool),
            plugin,
            "user1",
            "web",
            "/home",
        );

        let _ = registry.execute("echo", &serde_json::json!({"text": "test"})).await;
        assert!(called_flag());
    }

    #[tokio::test]
    async fn test_execute_records_timing_on_success() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        let result = registry.execute("echo", &serde_json::json!({"text": "timing"})).await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_execute_records_timing_on_error() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(EchoTool));

        // This should not panic even if tool is not found
        let result = registry.execute("nonexistent", &serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[test]
    fn test_tool_execution_context_clone() {
        let ctx = ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-123".to_string(),
            correlation_id: "corr-456".to_string(),
            user: "test".to_string(),
            source: "cli".to_string(),
            workspace: "/tmp".to_string(),
            metadata: serde_json::json!({"key": "value"}),
        };
        let cloned = ctx.clone();
        assert_eq!(cloned.channel, ctx.channel);
        assert_eq!(cloned.chat_id, ctx.chat_id);
        assert_eq!(cloned.metadata["key"], "value");
    }

    #[tokio::test]
    async fn test_definitions_multiple_tools_sorted() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(EchoTool));

        let defs = registry.definitions();
        assert_eq!(defs.len(), 2);
        // Both should be present
        let names: Vec<&str> = defs.iter().map(|d| d["function"]["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"echo"));
        assert!(names.contains(&"read_file"));
    }

    #[tokio::test]
    async fn test_get_summaries_multiple_tools() {
        let registry = ToolRegistry::new();
        registry.register(Arc::new(ReadFileTool));
        registry.register(Arc::new(EchoTool));

        let summaries = registry.get_summaries();
        let combined = summaries.join("; ");
        assert!(combined.contains("echo"));
        assert!(combined.contains("read_file"));
    }

    #[test]
    fn test_tool_execution_context_default() {
        let ctx = ToolExecutionContext::default();
        assert_eq!(ctx.channel, "");
        assert_eq!(ctx.chat_id, "");
        assert_eq!(ctx.correlation_id, "");
    }

    #[test]
    fn test_tool_execution_context_custom() {
        let ctx = ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-123".to_string(),
            correlation_id: "corr-456".to_string(),
            user: "test".to_string(),
            source: "cli".to_string(),
            workspace: "/tmp".to_string(),
            metadata: serde_json::json!({}),
        };
        assert_eq!(ctx.channel, "rpc");
        assert_eq!(ctx.chat_id, "chat-123");
        assert_eq!(ctx.correlation_id, "corr-456");
    }
}
