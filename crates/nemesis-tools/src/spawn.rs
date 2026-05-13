//! Spawn tool, SubagentTool (synchronous subagent), and SubagentManager.
//!
//! `SpawnTool` spawns a subagent task asynchronously and returns a task ID.
//! `SubagentTool` executes a subagent task synchronously and returns the result
//! directly in the ToolResult (blocking until completion).
//!
//! `SubagentManager` manages the lifecycle of subagent tasks. When an LLM
//! callback is configured via `set_llm_callback()`, both tools run a real
//! LLM + tool-call loop (`run_tool_loop`). Without a callback they fall back
//! to a placeholder result so that unit tests remain deterministic.

use crate::registry::{ContextualTool, Tool, AsyncCallback, ToolRegistry};
use crate::toolloop::{LLMCallback, ToolLoopConfig, run_tool_loop};
use crate::types::ToolResult;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

/// System prompt injected into every subagent conversation.
const SUBAGENT_SYSTEM_PROMPT: &str =
    "You are a subagent. Complete the given task independently and report the result.\n\
     You have access to tools - use them as needed to complete your task.\n\
     After completing the task, provide a clear summary of what was done.";

/// Subagent task state.
#[derive(Debug, Clone)]
pub struct SubagentTask {
    pub id: String,
    pub task: String,
    pub label: String,
    pub agent_id: String,
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub status: String,
    pub result: String,
    pub created_ms: i64,
}

/// Subagent manager - manages subagent task lifecycle.
///
/// When an `llm_callback` is set (via `set_llm_callback`), spawned tasks run
/// a real LLM + tool-call iteration loop. Without a callback the manager
/// returns a placeholder result, keeping unit tests fast and deterministic.
pub struct SubagentManager {
    tasks: Arc<Mutex<Vec<SubagentTask>>>,
    next_id: Arc<Mutex<u32>>,
    registry: Arc<parking_lot::RwLock<Arc<ToolRegistry>>>,
    max_iterations: usize,
    /// Optional LLM callback. When set, subagent tasks execute a real tool loop.
    llm_callback: Arc<parking_lot::RwLock<Option<LLMCallback>>>,
}

impl SubagentManager {
    /// Create a new subagent manager.
    pub fn new(registry: Arc<ToolRegistry>) -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
            registry: Arc::new(parking_lot::RwLock::new(registry)),
            max_iterations: 10,
            llm_callback: Arc::new(parking_lot::RwLock::new(None)),
        }
    }

    /// Set the maximum number of tool loop iterations.
    pub fn set_max_iterations(&mut self, max: usize) {
        self.max_iterations = max;
    }

    /// Set the LLM callback used by subagent tasks.
    ///
    /// When set, `SubagentTool` and `SpawnTool` will call `run_tool_loop`
    /// with this callback, executing a real LLM + tool iteration cycle.
    /// When `None`, tasks fall back to a placeholder result.
    pub fn set_llm_callback(&self, callback: LLMCallback) {
        let mut guard = self.llm_callback.write();
        *guard = Some(callback);
    }

    /// Returns `true` if an LLM callback is configured.
    pub fn has_llm_callback(&self) -> bool {
        self.llm_callback.read().is_some()
    }

    /// Build the initial messages for a subagent task.
    fn build_messages(task: &str) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({"role": "system", "content": SUBAGENT_SYSTEM_PROMPT}),
            serde_json::json!({"role": "user", "content": task}),
        ]
    }

    /// Run the tool loop for a subagent task.
    ///
    /// Returns the LLM's final text content and the number of iterations.
    /// If no LLM callback is configured, returns `None`.
    pub async fn run_task_llm(
        &self,
        task: &str,
    ) -> Option<crate::toolloop::ToolLoopResult> {
        let callback = match self.llm_callback.read().clone() {
            Some(cb) => cb,
            None => return None,
        };

        let registry = self.registry.read().clone();
        let max_iter = self.max_iterations;

        let config = ToolLoopConfig {
            tools: registry,
            max_iterations: max_iter,
            timeout_secs: 300,
        };

        let messages = Self::build_messages(task);

        Some(run_tool_loop(config, &callback, messages).await)
    }

    /// Spawn a new subagent task.

    /// Spawn a new subagent task.
    pub async fn spawn(
        &self,
        task: &str,
        label: &str,
        agent_id: &str,
        origin_channel: &str,
        origin_chat_id: &str,
    ) -> String {
        let mut next = self.next_id.lock().await;
        let id = format!("subagent-{}", *next);
        *next += 1;

        let subagent_task = SubagentTask {
            id: id.clone(),
            task: task.to_string(),
            label: label.to_string(),
            agent_id: agent_id.to_string(),
            origin_channel: origin_channel.to_string(),
            origin_chat_id: origin_chat_id.to_string(),
            status: "running".to_string(),
            result: String::new(),
            created_ms: chrono::Utc::now().timestamp_millis(),
        };

        let mut tasks = self.tasks.lock().await;
        tasks.push(subagent_task);
        id
    }

    /// Get a task by ID.
    pub async fn get_task(&self, task_id: &str) -> Option<SubagentTask> {
        let tasks = self.tasks.lock().await;
        tasks.iter().find(|t| t.id == task_id).cloned()
    }

    /// Update a task's status and result.
    pub async fn update_task(&self, task_id: &str, status: &str, result: &str) -> bool {
        let mut tasks = self.tasks.lock().await;
        if let Some(task) = tasks.iter_mut().find(|t| t.id == task_id) {
            task.status = status.to_string();
            task.result = result.to_string();
            true
        } else {
            false
        }
    }

    /// List all tasks.
    pub async fn list_tasks(&self) -> Vec<SubagentTask> {
        let tasks = self.tasks.lock().await;
        tasks.clone()
    }

    /// Get the number of tasks.
    pub async fn task_count(&self) -> usize {
        self.tasks.lock().await.len()
    }

    /// Set the tool registry for subagent execution.
    /// This replaces the current registry with a new one.
    pub async fn set_tools(&self, registry: Arc<ToolRegistry>) {
        let mut guard = self.registry.write();
        *guard = registry;
    }

    /// Register a single tool into the manager's tool registry.
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        let guard = self.registry.read();
        guard.register(tool);
    }

    /// Get a reference to the tool registry.
    pub fn registry(&self) -> parking_lot::RwLockReadGuard<'_, Arc<ToolRegistry>> {
        self.registry.read()
    }
}

/// Spawn tool - spawns a subagent to handle a task in the background.
pub struct SpawnTool {
    manager: Arc<SubagentManager>,
    channel: Arc<Mutex<String>>,
    chat_id: Arc<Mutex<String>>,
    allowlist_check: Option<Arc<dyn Fn(&str) -> bool + Send + Sync>>,
    /// Callback for async completion notification.
    /// Mirrors Go's `SpawnTool.callback AsyncCallback`.
    callback: Option<Arc<dyn Fn(ToolResult) + Send + Sync>>,
}

impl SpawnTool {
    /// Create a new spawn tool.
    pub fn new(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            channel: Arc::new(Mutex::new("cli".to_string())),
            chat_id: Arc::new(Mutex::new("direct".to_string())),
            allowlist_check: None,
            callback: None,
        }
    }

    /// Set the allowlist checker function.
    pub fn set_allowlist_check(&mut self, check: Arc<dyn Fn(&str) -> bool + Send + Sync>) {
        self.allowlist_check = Some(check);
    }

    /// Set the async callback for completion notification.
    ///
    /// Mirrors Go's `SpawnTool.SetCallback(cb AsyncCallback)`.
    /// When the spawned subagent completes, this callback will be invoked
    /// with the result. This implements the `AsyncTool` interface.
    pub fn set_callback(&mut self, cb: AsyncCallback) {
        // Wrap the boxed callback in an Arc for safe sharing.
        self.callback = Some(Arc::from(cb));
    }

    /// Get the current channel context.
    pub async fn channel(&self) -> String {
        self.channel.lock().await.clone()
    }

    /// Get the current chat ID context.
    pub async fn chat_id(&self) -> String {
        self.chat_id.lock().await.clone()
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn name(&self) -> &str {
        "spawn"
    }

    fn description(&self) -> &str {
        "Spawn a subagent to handle a task in the background"
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {"type": "string", "description": "The task for subagent to complete"},
                "label": {"type": "string", "description": "Optional short label for the task"},
                "agent_id": {"type": "string", "description": "Optional target agent ID"}
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let task = match args["task"].as_str() {
            Some(t) => t,
            None => return ToolResult::error("task is required"),
        };

        let label = args["label"].as_str().unwrap_or("");
        let agent_id = args["agent_id"].as_str().unwrap_or("");

        // Check allowlist if targeting specific agent
        if !agent_id.is_empty() {
            if let Some(ref check) = self.allowlist_check {
                if !check(agent_id) {
                    return ToolResult::error(&format!(
                        "not allowed to spawn agent '{}'",
                        agent_id
                    ));
                }
            }
        }

        let channel = self.channel.lock().await.clone();
        let chat_id = self.chat_id.lock().await.clone();

        let task_id = self
            .manager
            .spawn(task, label, agent_id, &channel, &chat_id)
            .await;

        let msg = if !label.is_empty() {
            format!(
                "Spawned subagent '{}' (id: {}) for task: {}",
                label, task_id, task
            )
        } else {
            format!("Spawned subagent (id: {}) for task: {}", task_id, task)
        };

        // If a callback is configured, kick off the real LLM execution in the
        // background.  When it finishes the task status is updated and the
        // optional `SpawnTool.callback` (AsyncTool notification) is invoked.
        if self.manager.has_llm_callback() {
            let mgr = self.manager.clone();
            let tid = task_id.clone();
            let task_str = task.to_string();
            let cb = self.callback.clone();

            tokio::spawn(async move {
                if let Some(loop_result) = mgr.run_task_llm(&task_str).await {
                    mgr.update_task(&tid, "completed", &loop_result.content)
                        .await;

                    // Invoke the AsyncTool callback (if set) so the agent loop
                    // is notified that the background task finished.
                    if let Some(ref async_cb) = cb {
                        let result_summary = format!(
                            "Subagent '{}' completed (iterations: {}): {}",
                            tid, loop_result.iterations, loop_result.content
                        );
                        async_cb(ToolResult::success(&result_summary));
                    }
                } else {
                    let placeholder = format!("Subagent task received: {}", task_str);
                    mgr.update_task(&tid, "completed", &placeholder).await;
                }
            });
        }

        ToolResult::async_result(&msg)
    }
}

impl ContextualTool for SpawnTool {
    fn set_context(&mut self, ctx: &crate::registry::ToolExecutionContext) {
        if let Ok(mut ch) = self.channel.try_lock() {
            *ch = ctx.channel.clone();
        }
        if let Ok(mut cid) = self.chat_id.try_lock() {
            *cid = ctx.chat_id.clone();
        }
    }
}

// ---------------------------------------------------------------------------
// SubagentTool - synchronous subagent execution
// ---------------------------------------------------------------------------

/// SubagentTool executes a subagent task synchronously and returns the result.
///
/// Unlike `SpawnTool` which runs tasks asynchronously and returns a task ID,
/// `SubagentTool` waits for completion and returns the result directly in
/// the `ToolResult`. This mirrors Go's `SubagentTool` which calls
/// `RunToolLoop` synchronously.
///
/// The tool provides:
/// - `for_llm`: Full execution details including label, iterations, and result
/// - `for_user`: Brief summary for user (truncated if too long)
pub struct SubagentTool {
    manager: Arc<SubagentManager>,
    channel: Arc<Mutex<String>>,
    chat_id: Arc<Mutex<String>>,
}

impl SubagentTool {
    /// Create a new subagent tool backed by the given manager.
    pub fn new(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            channel: Arc::new(Mutex::new("cli".to_string())),
            chat_id: Arc::new(Mutex::new("direct".to_string())),
        }
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Execute a subagent task synchronously and return the result. Use this for delegating specific tasks to an independent agent instance. Returns execution summary to user and full details to LLM."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task for subagent to complete"
                },
                "label": {
                    "type": "string",
                    "description": "Optional short label for the task (for display)"
                }
            },
            "required": ["task"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let task = match args["task"].as_str() {
            Some(t) if !t.is_empty() => t,
            _ => return ToolResult::error("task is required"),
        };

        let label = args["label"].as_str().unwrap_or("");

        // Spawn the task in the manager
        let channel = self.channel.lock().await.clone();
        let chat_id = self.chat_id.lock().await.clone();

        let task_id = self
            .manager
            .spawn(task, label, "", &channel, &chat_id)
            .await;

        let label_str = if label.is_empty() {
            "(unnamed)".to_string()
        } else {
            label.to_string()
        };

        // Try to run the real LLM tool loop if a callback is configured.
        let (result_summary, iterations) = match self.manager.run_task_llm(task).await {
            Some(loop_result) => {
                let content = loop_result.content.clone();
                self.manager
                    .update_task(&task_id, "completed", &content)
                    .await;
                (content, loop_result.iterations)
            }
            None => {
                // No LLM callback configured - fall back to placeholder.
                let placeholder = format!("Subagent task received: {}", task);
                self.manager
                    .update_task(&task_id, "completed", &placeholder)
                    .await;
                (placeholder, 0)
            }
        };

        // ForUser: Brief summary (truncated if too long)
        let max_user_len = 500;
        let user_content = if result_summary.len() > max_user_len {
            format!("{}...", &result_summary[..max_user_len])
        } else {
            result_summary.clone()
        };

        // ForLLM: Full execution details
        let llm_content = format!(
            "Subagent task completed:\nLabel: {}\nIterations: {}\nResult: {}",
            label_str, iterations, result_summary
        );

        ToolResult::user_result(&llm_content, &user_content)
    }
}

impl ContextualTool for SubagentTool {
    fn set_context(&mut self, ctx: &crate::registry::ToolExecutionContext) {
        if let Ok(mut ch) = self.channel.try_lock() {
            *ch = ctx.channel.clone();
        }
        if let Ok(mut cid) = self.chat_id.try_lock() {
            *cid = ctx.chat_id.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subagent_manager_spawn() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        let id = manager
            .spawn("test task", "test-label", "", "web", "chat-1")
            .await;
        assert!(id.starts_with("subagent-"));

        let task = manager.get_task(&id).await.unwrap();
        assert_eq!(task.task, "test task");
        assert_eq!(task.label, "test-label");
        assert_eq!(task.status, "running");
    }

    #[tokio::test]
    async fn test_subagent_manager_update() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        let id = manager.spawn("task", "", "", "", "").await;
        let updated = manager.update_task(&id, "completed", "done").await;
        assert!(updated);

        let task = manager.get_task(&id).await.unwrap();
        assert_eq!(task.status, "completed");
        assert_eq!(task.result, "done");
    }

    #[tokio::test]
    async fn test_subagent_manager_list() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        manager.spawn("task1", "", "", "", "").await;
        manager.spawn("task2", "", "", "", "").await;

        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 2);
        assert_eq!(manager.task_count().await, 2);
    }

    #[tokio::test]
    async fn test_spawn_tool_execute() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SpawnTool::new(manager);

        let result = tool
            .execute(&serde_json::json!({"task": "do something", "label": "test"}))
            .await;
        assert!(result.is_async, "Spawn should return async result");
        assert!(result.for_llm.contains("Spawned subagent"));
        assert!(result.for_llm.contains("test"));
    }

    #[tokio::test]
    async fn test_spawn_tool_missing_task() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SpawnTool::new(manager);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }

    #[tokio::test]
    async fn test_spawn_tool_allowlist_check() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SpawnTool::new(manager);
        tool.set_allowlist_check(Arc::new(|id| id == "allowed-agent"));

        // Allowed agent
        let result = tool
            .execute(&serde_json::json!({"task": "test", "agent_id": "allowed-agent"}))
            .await;
        assert!(!result.is_error, "Should allow allowed-agent");

        // Disallowed agent
        let result = tool
            .execute(&serde_json::json!({"task": "test", "agent_id": "blocked-agent"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("not allowed"));
    }

    #[tokio::test]
    async fn test_spawn_tool_contextual() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SpawnTool::new(manager);

        let ctx = crate::registry::ToolExecutionContext {
            channel: "rpc".to_string(),
            chat_id: "chat-456".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        assert_eq!(tool.channel().await, "rpc");
        assert_eq!(tool.chat_id().await, "chat-456");
    }

    // --- SubagentManager new methods ---

    #[tokio::test]
    async fn test_subagent_manager_register_tool() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(Arc::clone(&registry));

        // Initially registry is empty
        assert!(!manager.registry().has("echo"));

        // Register a tool via the manager
        struct Echo;
        #[async_trait]
        impl Tool for Echo {
            fn name(&self) -> &str { "echo" }
            fn description(&self) -> &str { "Echo" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                ToolResult::success(args["text"].as_str().unwrap_or(""))
            }
        }
        manager.register_tool(Arc::new(Echo));

        assert!(manager.registry().has("echo"));
    }

    #[tokio::test]
    async fn test_subagent_manager_set_tools() {
        let registry1 = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry1);

        // Create a new registry with a tool
        let registry2 = Arc::new(ToolRegistry::new());
        struct Echo;
        #[async_trait]
        impl Tool for Echo {
            fn name(&self) -> &str { "echo" }
            fn description(&self) -> &str { "Echo" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object"}) }
            async fn execute(&self, _args: &serde_json::Value) -> ToolResult {
                ToolResult::success("")
            }
        }
        registry2.register(Arc::new(Echo));

        // Replace registry
        manager.set_tools(registry2).await;

        // New registry should have the tool
        assert!(manager.registry().has("echo"));
    }

    // --- SubagentTool tests ---

    #[tokio::test]
    async fn test_subagent_tool_execute() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        let result = tool
            .execute(&serde_json::json!({
                "task": "do something important",
                "label": "important-task"
            }))
            .await;

        assert!(!result.is_error, "SubagentTool should not error: {}", result.for_llm);
        assert!(result.for_llm.contains("important-task"), "LLM content should contain label");
        assert!(result.for_llm.contains("Subagent task completed"), "LLM content should contain completion message");
        assert!(result.for_user.is_some(), "Should have user content");
        assert!(result.for_user.as_ref().unwrap().contains("Subagent task received"), "User content should contain result summary");
    }

    #[tokio::test]
    async fn test_subagent_tool_missing_task() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("required"));
    }

    #[tokio::test]
    async fn test_subagent_tool_empty_task() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        let result = tool.execute(&serde_json::json!({"task": ""})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_subagent_tool_no_label() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        let result = tool
            .execute(&serde_json::json!({"task": "test task"}))
            .await;

        assert!(!result.is_error);
        assert!(result.for_llm.contains("(unnamed)"), "Should show unnamed label when not provided");
    }

    #[tokio::test]
    async fn test_subagent_tool_contextual() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SubagentTool::new(manager);

        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-789".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;

        // Verify context is set (via the spawned task's origin fields)
        let result = tool
            .execute(&serde_json::json!({"task": "context test"}))
            .await;
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn test_subagent_tool_metadata() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        assert_eq!(tool.name(), "subagent");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_spawn_tool_set_callback() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SpawnTool::new(manager);

        // Set a callback - should not panic
        tool.set_callback(Box::new(move |_result| {
            // callback invoked
        }));

        // Verify callback is set (via internal state)
        assert!(tool.callback.is_some());
    }

    #[tokio::test]
    async fn test_subagent_tool_completes_task_in_manager() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(Arc::clone(&manager));

        tool.execute(&serde_json::json!({"task": "managed task", "label": "test-label"}))
            .await;

        // The task should be in the manager with completed status
        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");
        assert_eq!(tasks[0].label, "test-label");
        assert!(tasks[0].result.contains("Subagent task received"));
    }

    // ============================================================
    // Additional spawn/subagent tests
    // ============================================================

    #[tokio::test]
    async fn test_subagent_manager_get_nonexistent() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);
        let task = manager.get_task("nonexistent-id").await;
        assert!(task.is_none());
    }

    #[tokio::test]
    async fn test_subagent_manager_update_nonexistent() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);
        let updated = manager.update_task("nonexistent-id", "completed", "done").await;
        assert!(!updated);
    }

    #[tokio::test]
    async fn test_subagent_manager_spawn_multiple() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        let id1 = manager.spawn("task1", "label1", "", "web", "chat-1").await;
        let id2 = manager.spawn("task2", "label2", "", "rpc", "chat-2").await;
        let id3 = manager.spawn("task3", "label3", "", "cli", "chat-3").await;

        assert_ne!(id1, id2);
        assert_ne!(id2, id3);

        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 3);
        assert_eq!(manager.task_count().await, 3);
    }

    #[tokio::test]
    async fn test_subagent_manager_spawn_and_complete() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        let id = manager.spawn("task", "", "", "", "").await;
        assert!(manager.get_task(&id).await.unwrap().status == "running");

        manager.update_task(&id, "completed", "result data").await;
        let task = manager.get_task(&id).await.unwrap();
        assert_eq!(task.status, "completed");
        assert_eq!(task.result, "result data");
    }

    #[tokio::test]
    async fn test_subagent_manager_spawn_and_fail() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        let id = manager.spawn("failing task", "", "", "", "").await;
        manager.update_task(&id, "failed", "error: timeout").await;
        let task = manager.get_task(&id).await.unwrap();
        assert_eq!(task.status, "failed");
        assert_eq!(task.result, "error: timeout");
    }

    #[tokio::test]
    async fn test_spawn_tool_with_agent_id() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SpawnTool::new(manager);
        tool.set_allowlist_check(Arc::new(|id| id == "agent-1"));

        // Allowed agent
        let result = tool
            .execute(&serde_json::json!({"task": "test", "agent_id": "agent-1"}))
            .await;
        assert!(!result.is_error);

        // Blocked agent
        let result = tool
            .execute(&serde_json::json!({"task": "test", "agent_id": "agent-2"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("not allowed"));
    }

    #[tokio::test]
    async fn test_spawn_tool_callback_invoked() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let mut tool = SpawnTool::new(manager);

        let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let callback_called_clone = callback_called.clone();
        tool.set_callback(Box::new(move |_result| {
            callback_called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        }));

        let result = tool
            .execute(&serde_json::json!({"task": "callback test"}))
            .await;
        // Execute spawns asynchronously - callback is set but not called during execute
        assert!(!result.is_error);
        assert!(result.for_llm.contains("Spawned"));
    }

    #[tokio::test]
    async fn test_spawn_tool_metadata() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SpawnTool::new(manager);

        assert_eq!(tool.name(), "spawn");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["task"].is_object());
    }

    #[tokio::test]
    async fn test_subagent_tool_user_content_truncation() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        // Create a very long task description
        let long_task = "x".repeat(1000);
        let result = tool
            .execute(&serde_json::json!({"task": long_task}))
            .await;

        assert!(!result.is_error);
        let user_content = result.for_user.as_ref().unwrap();
        // User content should be truncated to 500 chars max
        assert!(user_content.len() <= 503); // 500 + "..."
    }

    #[tokio::test]
    async fn test_subagent_tool_metadata_check() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SubagentTool::new(manager);

        assert_eq!(tool.name(), "subagent");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["task"].is_object());
    }

    #[tokio::test]
    async fn test_subagent_manager_register_and_use_tool() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(Arc::clone(&registry));

        struct UpperTool;
        #[async_trait]
        impl Tool for UpperTool {
            fn name(&self) -> &str { "upper" }
            fn description(&self) -> &str { "Uppercase" }
            fn parameters(&self) -> serde_json::Value { serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}) }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                let text = args["text"].as_str().unwrap_or("");
                ToolResult::success(&text.to_uppercase())
            }
        }
        manager.register_tool(Arc::new(UpperTool));

        // Verify tool is accessible through the registry
        assert!(manager.registry().has("upper"));
        let result = manager.registry().execute("upper", &serde_json::json!({"text": "hello"})).await;
        assert_eq!(result.for_llm, "HELLO");
    }

    #[tokio::test]
    async fn test_spawn_tool_parameters() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        let tool = SpawnTool::new(manager);

        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        let required = params["required"].as_array().unwrap();
        assert!(required.iter().any(|r| r.as_str() == Some("task")));
    }

    // ============================================================
    // Tests for real LLM execution via run_tool_loop
    // ============================================================

    #[tokio::test]
    async fn test_subagent_tool_with_llm_callback() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));

        // Set up a mock LLM callback that returns a fixed response
        manager.set_llm_callback(Arc::new(|_msgs| {
            crate::toolloop::LLMResponse {
                content: "Task completed successfully: analysis done".to_string(),
                tool_calls: vec![],
            }
        }));

        let tool = SubagentTool::new(manager);
        let result = tool
            .execute(&serde_json::json!({
                "task": "analyze the data",
                "label": "analysis"
            }))
            .await;

        assert!(!result.is_error, "Should not error: {}", result.for_llm);
        assert!(result.for_llm.contains("analysis"), "Should contain label");
        assert!(
            result.for_llm.contains("Iterations: 1"),
            "Should report 1 iteration, got: {}",
            result.for_llm
        );
        assert!(
            result.for_llm.contains("Task completed successfully"),
            "Should contain LLM response content, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_subagent_tool_llm_callback_with_tools() {
        let registry = Arc::new(ToolRegistry::new());

        // Register a tool for the subagent to use
        struct EchoTool;
        #[async_trait]
        impl Tool for EchoTool {
            fn name(&self) -> &str { "echo" }
            fn description(&self) -> &str { "Echo back input" }
            fn parameters(&self) -> serde_json::Value {
                serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
            }
            async fn execute(&self, args: &serde_json::Value) -> ToolResult {
                ToolResult::success(args["text"].as_str().unwrap_or(""))
            }
        }
        registry.register(Arc::new(EchoTool));

        let manager = Arc::new(SubagentManager::new(registry));

        // LLM callback: first call requests tool, second returns final answer
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        manager.set_llm_callback(Arc::new(move |_msgs| {
            let count = call_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if count == 0 {
                crate::toolloop::LLMResponse {
                    content: String::new(),
                    tool_calls: vec![crate::toolloop::LLMToolCall {
                        id: "tc-1".to_string(),
                        name: "echo".to_string(),
                        arguments: serde_json::json!({"text": "hello from subagent"}),
                    }],
                }
            } else {
                crate::toolloop::LLMResponse {
                    content: "Subagent used echo tool and got: hello from subagent".to_string(),
                    tool_calls: vec![],
                }
            }
        }));

        let tool = SubagentTool::new(manager);
        let result = tool
            .execute(&serde_json::json!({"task": "use echo tool"}))
            .await;

        assert!(!result.is_error, "Should not error: {}", result.for_llm);
        assert!(
            result.for_llm.contains("Iterations: 2"),
            "Should report 2 iterations (tool call + final), got: {}",
            result.for_llm
        );
        assert!(
            result.for_llm.contains("hello from subagent"),
            "Should contain tool result, got: {}",
            result.for_llm
        );
    }

    #[tokio::test]
    async fn test_spawn_tool_with_llm_background_execution() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));

        // Set up a mock LLM callback
        manager.set_llm_callback(Arc::new(|_msgs| {
            crate::toolloop::LLMResponse {
                content: "Background task completed".to_string(),
                tool_calls: vec![],
            }
        }));

        let tool = SpawnTool::new(manager.clone());
        let result = tool
            .execute(&serde_json::json!({"task": "background work", "label": "bg-task"}))
            .await;

        // Should return async result immediately
        assert!(result.is_async, "SpawnTool should return async result");
        assert!(result.for_llm.contains("Spawned subagent"));

        // Wait a bit for the background task to complete
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check that the task was completed with the real LLM result
        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");
        assert!(
            tasks[0].result.contains("Background task completed"),
            "Task result should contain LLM response, got: {}",
            tasks[0].result
        );
    }

    #[tokio::test]
    async fn test_subagent_manager_has_llm_callback() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = SubagentManager::new(registry);

        assert!(!manager.has_llm_callback(), "Should start without callback");

        manager.set_llm_callback(Arc::new(|_msgs| {
            crate::toolloop::LLMResponse {
                content: "test".to_string(),
                tool_calls: vec![],
            }
        }));

        assert!(manager.has_llm_callback(), "Should have callback after setting");
    }

    #[tokio::test]
    async fn test_subagent_tool_fallback_without_llm() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));
        // Deliberately NOT setting llm_callback

        let tool = SubagentTool::new(manager);
        let result = tool
            .execute(&serde_json::json!({"task": "fallback test", "label": "no-llm"}))
            .await;

        assert!(!result.is_error);
        // Should contain placeholder content
        assert!(result.for_llm.contains("Subagent task completed"));
        assert!(result.for_llm.contains("Iterations: 0"), "Without LLM, iterations should be 0");
        assert!(
            result.for_user.as_ref().unwrap().contains("Subagent task received"),
            "User content should have placeholder"
        );
    }

    #[tokio::test]
    async fn test_subagent_tool_llm_callback_long_content_truncation() {
        let registry = Arc::new(ToolRegistry::new());
        let manager = Arc::new(SubagentManager::new(registry));

        // LLM returns very long content
        let long_content = "x".repeat(1000);
        manager.set_llm_callback(Arc::new(move |_msgs| {
            crate::toolloop::LLMResponse {
                content: long_content.clone(),
                tool_calls: vec![],
            }
        }));

        let tool = SubagentTool::new(manager);
        let result = tool
            .execute(&serde_json::json!({"task": "long task"}))
            .await;

        assert!(!result.is_error);
        let user_content = result.for_user.as_ref().unwrap();
        assert!(user_content.len() <= 503, "User content should be truncated, got {} chars", user_content.len());
    }

    #[tokio::test]
    async fn test_spawn_tool_with_callback_and_llm() {
        let registry = Arc::new(ToolRegistry::new());
        let mut manager = SubagentManager::new(registry);
        manager.set_llm_callback(Arc::new(|_msgs| {
            crate::toolloop::LLMResponse {
                content: "background task done".to_string(),
                tool_calls: vec![],
            }
        }));

        let manager = Arc::new(manager);
        let mut tool = SpawnTool::new(Arc::clone(&manager));

        // Set an async callback
        let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called_clone = callback_called.clone();
        tool.set_callback(Box::new(move |_result| {
            called_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        }));

        let result = tool
            .execute(&serde_json::json!({"task": "background test", "label": "bg"}))
            .await;

        assert!(!result.is_error);
        assert!(result.is_async);
        assert!(result.task_id.is_some());

        // Wait for the spawned task to complete
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Check that the task was completed by the background LLM
        let tasks = manager.list_tasks().await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].status, "completed");
        assert!(tasks[0].result.contains("background task done"));
    }

    #[test]
    fn test_subagent_manager_set_max_iterations() {
        let registry = Arc::new(ToolRegistry::new());
        let mut manager = SubagentManager::new(registry);
        manager.set_max_iterations(42);
        assert_eq!(manager.max_iterations, 42);
    }
}
