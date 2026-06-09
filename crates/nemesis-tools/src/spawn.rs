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

use nemesis_types::utils;
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
            created_ms: chrono::Local::now().timestamp_millis(),
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
        let user_content = utils::truncate(&result_summary, 500);

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
mod tests;
