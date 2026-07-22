//! Subagent tool - spawns sub-agent tasks for delegated execution.
//!
//! Port of Go's module/tools/subagent.go.
//!
//! When an `llm_callback` is configured on the manager (via `set_llm_callback`),
//! `SubagentTool::execute()` runs a real LLM + tool-call iteration loop
//! (`run_tool_loop`). Without a callback it falls back to a placeholder result
//! so that unit tests remain fast and deterministic.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use nemesis_types::utils;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::registry::{AsyncCallback, ContextualTool, Tool};
use crate::toolloop::{LLMCallback, ToolLoopConfig, run_tool_loop};
use crate::types::ToolResult;

/// System prompt injected into every subagent conversation.
const SUBAGENT_SYSTEM_PROMPT: &str = "You are a subagent. Complete the given task independently and provide a clear, concise result.";

// ---------------------------------------------------------------------------
// SubagentTask
// ---------------------------------------------------------------------------

/// A subagent task record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub id: String,
    pub task: String,
    pub label: String,
    pub agent_id: String,
    pub origin_channel: String,
    pub origin_chat_id: String,
    pub status: String,
    pub result: String,
    pub created: u64,
}

// ---------------------------------------------------------------------------
// SubagentManager
// ---------------------------------------------------------------------------

/// Manages subagent tasks, providing spawn, get, and list operations.
pub struct SubagentManager {
    tasks: RwLock<HashMap<String, SubagentTask>>,
    next_id: AtomicU32,
    max_iterations: u32,
    /// Optional LLM callback for real execution.
    llm_callback: RwLock<Option<LLMCallback>>,
}

impl SubagentManager {
    /// Create a new SubagentManager.
    pub fn new() -> Self {
        Self {
            tasks: RwLock::new(HashMap::new()),
            next_id: AtomicU32::new(1),
            max_iterations: 10,
            llm_callback: RwLock::new(None),
        }
    }

    /// Set the maximum tool-loop iterations for subagent execution.
    pub fn set_max_iterations(&mut self, max: u32) {
        self.max_iterations = max;
    }

    /// Set the LLM callback used by subagent tasks.
    ///
    /// When set, `SubagentTool` will call `run_tool_loop` with this callback.
    /// When `None`, tasks fall back to a placeholder result.
    pub fn set_llm_callback(&self, callback: LLMCallback) {
        tracing::info!("[Tools/Subagent] LLM callback configured");
        let mut guard = self.llm_callback.write().unwrap();
        *guard = Some(callback);
    }

    /// Returns `true` if an LLM callback is configured.
    pub fn has_llm_callback(&self) -> bool {
        self.llm_callback.read().unwrap().is_some()
    }

    /// Run the tool loop for a subagent task.
    ///
    /// Returns the `ToolLoopResult` if a callback is configured, or `None`.
    pub async fn run_task_llm(&self, task: &str) -> Option<crate::toolloop::ToolLoopResult> {
        let callback = self.llm_callback.read().unwrap().clone()?;

        let config = ToolLoopConfig {
            tools: Arc::new(crate::registry::ToolRegistry::new()),
            max_iterations: self.max_iterations as usize,
            timeout_secs: 300,
        };

        let messages = vec![
            serde_json::json!({"role": "system", "content": SUBAGENT_SYSTEM_PROMPT}),
            serde_json::json!({"role": "user", "content": task}),
        ];

        Some(run_tool_loop(config, &callback, messages).await)
    }

    /// Spawn a new subagent task asynchronously.
    ///
    /// Returns the task ID. The task runs in a background tokio task.
    /// If a callback is provided, it is called when the task completes.
    /// When an LLM callback is configured, the background task runs the real
    /// LLM + tool-call iteration loop; otherwise it completes with a placeholder.
    pub fn spawn(
        &self,
        task: String,
        label: String,
        agent_id: String,
        origin_channel: String,
        origin_chat_id: String,
        callback: Option<AsyncCallback>,
    ) -> String {
        let id_val = self.next_id.fetch_add(1, Ordering::SeqCst);
        let id = format!("subagent-{}", id_val);

        tracing::info!(
            task_id = %id,
            label = %label,
            has_llm = self.llm_callback.read().unwrap().is_some(),
            "[Tools/Subagent] Spawning subagent task"
        );

        let record = SubagentTask {
            id: id.clone(),
            task: task.clone(),
            label: label.clone(),
            agent_id,
            origin_channel,
            origin_chat_id,
            status: "running".to_string(),
            result: String::new(),
            created: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        };

        self.tasks.write().unwrap().insert(id.clone(), record);

        // Capture what we need for the background task
        let llm_cb = self.llm_callback.read().unwrap().clone();
        let max_iter = self.max_iterations;

        // Only spawn if we're inside a tokio runtime
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            let task_id_for_spawn = id.clone();
            let task_for_spawn = task;

            handle.spawn(async move {
                // If we have an LLM callback, run the real tool loop
                if let Some(cb) = llm_cb {
                    let config = ToolLoopConfig {
                        tools: Arc::new(crate::registry::ToolRegistry::new()),
                        max_iterations: max_iter as usize,
                        timeout_secs: 300,
                    };
                    let messages = vec![
                        serde_json::json!({"role": "system", "content": SUBAGENT_SYSTEM_PROMPT}),
                        serde_json::json!({"role": "user", "content": task_for_spawn}),
                    ];

                    let loop_result = run_tool_loop(config, &cb, messages).await;

                    // We can't update the task here since we don't have the manager's
                    // tasks map. The caller (SpawnTool in spawn.rs) handles the
                    // task update. We just invoke the callback with the result.
                    if let Some(async_cb) = callback {
                        let result_summary = format!(
                            "Subagent '{}' completed (iterations: {}): {}",
                            task_id_for_spawn, loop_result.iterations, loop_result.content
                        );
                        async_cb(ToolResult::success(&result_summary));
                    }
                } else {
                    // No LLM callback - just invoke callback with placeholder
                    if let Some(async_cb) = callback {
                        let placeholder = format!(
                            "Subagent task '{}' completed (placeholder)",
                            task_id_for_spawn
                        );
                        async_cb(ToolResult::success(&placeholder));
                    }
                }
            });
        }

        id
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<SubagentTask> {
        self.tasks.read().unwrap().get(task_id).cloned()
    }

    /// List all tasks.
    pub fn list_tasks(&self) -> Vec<SubagentTask> {
        self.tasks.read().unwrap().values().cloned().collect()
    }

    /// Update a task's status and result.
    pub fn update_task(&self, task_id: &str, status: &str, result: &str) {
        if let Some(task) = self.tasks.write().unwrap().get_mut(task_id) {
            task.status = status.to_string();
            task.result = result.to_string();
        }
    }

    /// Remove a task.
    pub fn remove_task(&self, task_id: &str) -> bool {
        self.tasks.write().unwrap().remove(task_id).is_some()
    }
}

impl Default for SubagentManager {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// SubagentTool
// ---------------------------------------------------------------------------

/// SubagentTool executes a subagent task synchronously and returns the result.
///
/// Unlike SpawnTool which runs tasks asynchronously, SubagentTool waits for
/// completion and returns the result directly in the ToolResult.
pub struct SubagentTool {
    manager: Arc<SubagentManager>,
    origin_channel: String,
    origin_chat_id: String,
}

impl SubagentTool {
    /// Create a new SubagentTool with the given manager.
    pub fn new(manager: Arc<SubagentManager>) -> Self {
        Self {
            manager,
            origin_channel: "cli".to_string(),
            origin_chat_id: "direct".to_string(),
        }
    }
}

impl ContextualTool for SubagentTool {
    fn set_context(&mut self, ctx: &crate::registry::ToolExecutionContext) {
        self.origin_channel = ctx.channel.clone();
        self.origin_chat_id = ctx.chat_id.clone();
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Execute a subagent task synchronously and return the result. Use this for delegating \
         specific tasks to an independent agent instance. Returns execution summary to user \
         and full details to LLM."
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
            Some(t) if !t.is_empty() => t.to_string(),
            _ => return ToolResult::error("parameter 'task' is required"),
        };

        let label = args["label"].as_str().unwrap_or("").to_string();

        // Spawn the task through the manager
        let task_id = self.manager.spawn(
            task.clone(),
            label.clone(),
            String::new(),
            self.origin_channel.clone(),
            self.origin_chat_id.clone(),
            None,
        );

        let label_str = if label.is_empty() {
            "(unnamed)".to_string()
        } else {
            label
        };

        // Try to run the real LLM tool loop if a callback is configured.
        let (result_summary, iterations) = match self.manager.run_task_llm(&task).await {
            Some(loop_result) => {
                let content = loop_result.content.clone();
                self.manager.update_task(&task_id, "completed", &content);
                (content, loop_result.iterations)
            }
            None => {
                // No LLM callback configured - fall back to placeholder.
                let placeholder = format!("Subagent task received: {}", task);
                self.manager
                    .update_task(&task_id, "completed", &placeholder);
                (placeholder, 0)
            }
        };

        // Build the LLM content with full execution details
        let llm_content = format!(
            "Subagent task completed:\nLabel: {}\nIterations: {}\nResult: {}",
            label_str, iterations, result_summary
        );

        // Truncate user content if too long
        let user_content = utils::truncate(&result_summary, 500);

        ToolResult::user_result(&llm_content, &user_content)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
