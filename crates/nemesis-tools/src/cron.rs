//! Cron tool - schedule reminders, tasks, and system commands.

use crate::registry::{ContextualTool, Tool};
use crate::shell::ShellTool;
use crate::types::ToolResult;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Job executor trait (decoupled from agent loop)
// ---------------------------------------------------------------------------

/// Trait for executing cron jobs through the agent.
///
/// Mirrors Go's `JobExecutor` interface. Implementations route the job's
/// message through the agent loop and return the response text.
pub trait JobExecutor: Send + Sync {
    /// Process a direct message with the given channel context.
    /// Returns the agent's response text.
    fn process_direct_with_channel(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>>;
}

/// Trait for publishing outbound messages from cron job execution.
///
/// Mirrors the message bus `PublishOutbound` functionality needed by
/// `execute_job` without depending on the full bus module.
pub trait CronJobOutput: Send + Sync {
    /// Publish an outbound message to the specified channel/chat.
    fn publish_outbound(&self, channel: &str, chat_id: &str, content: &str);
}

/// Schedule type for cron jobs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum CronSchedule {
    /// One-time trigger at a specific timestamp (milliseconds since epoch).
    #[serde(rename = "at")]
    At { at_ms: i64 },
    /// Recurring interval in milliseconds.
    #[serde(rename = "every")]
    Every { every_ms: i64 },
    /// Cron expression for complex schedules.
    #[serde(rename = "cron")]
    Cron { expr: String },
}

/// Cron job definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: String,
    pub name: String,
    pub schedule: CronSchedule,
    pub message: String,
    pub deliver: bool,
    pub channel: String,
    pub chat_id: String,
    pub enabled: bool,
    pub command: Option<String>,
}

/// Cron service - manages scheduled jobs.
pub struct CronService {
    jobs: Vec<CronJob>,
    next_id: u32,
}

impl CronService {
    /// Create a new empty cron service.
    pub fn new() -> Self {
        Self {
            jobs: Vec::new(),
            next_id: 1,
        }
    }

    /// Add a new job.
    pub fn add_job(
        &mut self,
        name: &str,
        schedule: CronSchedule,
        message: &str,
        deliver: bool,
        channel: &str,
        chat_id: &str,
    ) -> &CronJob {
        let id = format!("cron-{}", self.next_id);
        self.next_id += 1;

        let job = CronJob {
            id,
            name: name.to_string(),
            schedule,
            message: message.to_string(),
            deliver,
            channel: channel.to_string(),
            chat_id: chat_id.to_string(),
            enabled: true,
            command: None,
        };

        self.jobs.push(job);
        self.jobs.last().unwrap()
    }

    /// Remove a job by ID.
    pub fn remove_job(&mut self, job_id: &str) -> bool {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != job_id);
        self.jobs.len() < before
    }

    /// Enable or disable a job. Returns true if found.
    pub fn enable_job(&mut self, job_id: &str, enabled: bool) -> bool {
        for job in &mut self.jobs {
            if job.id == job_id {
                job.enabled = enabled;
                return true;
            }
        }
        false
    }

    /// Update a job.
    pub fn update_job(&mut self, updated: CronJob) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == updated.id) {
            *job = updated;
        }
    }

    /// List all jobs.
    pub fn list_jobs(&self) -> &[CronJob] {
        &self.jobs
    }

    /// Get a job by ID.
    pub fn get_job(&self, job_id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == job_id)
    }

    /// Number of jobs.
    pub fn len(&self) -> usize {
        self.jobs.len()
    }

    /// Check if service has no jobs.
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

impl Default for CronService {
    fn default() -> Self {
        Self::new()
    }
}

/// Cron tool - provides scheduling capabilities.
pub struct CronTool {
    cron_service: Arc<Mutex<CronService>>,
    channel: Arc<Mutex<String>>,
    chat_id: Arc<Mutex<String>>,
    /// Optional job executor for processing deliver=false jobs through the agent.
    executor: Option<Arc<dyn JobExecutor>>,
    /// Optional output sink for publishing outbound messages (deliver=true or command results).
    output: Option<Arc<dyn CronJobOutput>>,
    /// Shell tool for executing scheduled commands.
    shell_tool: Option<Arc<ShellTool>>,
}

impl CronTool {
    /// Create a new cron tool with the given cron service.
    pub fn new(cron_service: Arc<Mutex<CronService>>) -> Self {
        Self {
            cron_service,
            channel: Arc::new(Mutex::new(String::new())),
            chat_id: Arc::new(Mutex::new(String::new())),
            executor: None,
            output: None,
            shell_tool: None,
        }
    }

    /// Set the job executor for processing complex tasks through the agent.
    pub fn set_executor(&mut self, executor: Arc<dyn JobExecutor>) {
        self.executor = Some(executor);
    }

    /// Set the output sink for publishing outbound messages.
    pub fn set_output(&mut self, output: Arc<dyn CronJobOutput>) {
        self.output = Some(output);
    }

    /// Set the shell tool for executing scheduled commands.
    pub fn set_shell_tool(&mut self, shell_tool: Arc<ShellTool>) {
        self.shell_tool = Some(shell_tool);
    }

    /// Execute a cron job. Called by the CronService when a job triggers.
    ///
    /// Mirrors Go's `CronTool.ExecuteJob()`. Handles three cases:
    /// 1. If the job has a command, execute it via shell tool and publish the result.
    /// 2. If deliver=true, publish the message directly to the channel.
    /// 3. If deliver=false, process the message through the agent via JobExecutor.
    ///
    /// Returns "ok" on success, or an error description on failure.
    pub async fn execute_job(&self, job: &CronJob) -> String {
        let channel = if job.channel.is_empty() {
            "cli"
        } else {
            &job.channel
        };
        let chat_id = if job.chat_id.is_empty() {
            "direct"
        } else {
            &job.chat_id
        };

        // Execute command if present
        if let Some(ref command) = job.command {
            if !command.is_empty() {
                if let Some(ref shell) = self.shell_tool {
                    let args = serde_json::json!({
                        "command": command
                    });
                    let result = shell.execute(&args).await;
                    let output = if result.is_error {
                        format!("Error executing scheduled command: {}", result.for_llm)
                    } else {
                        format!("Scheduled command '{}' executed:\n{}", command, result.for_llm)
                    };

                    if let Some(ref out) = self.output {
                        out.publish_outbound(channel, chat_id, &output);
                    }
                    return "ok".to_string();
                } else {
                    return "error: no shell tool configured".to_string();
                }
            }
        }

        // If deliver=true, send message directly without agent processing
        if job.deliver {
            if let Some(ref out) = self.output {
                out.publish_outbound(channel, chat_id, &job.message);
            }
            return "ok".to_string();
        }

        // For deliver=false, process through agent (for complex tasks)
        if let Some(ref executor) = self.executor {
            let session_key = format!("cron-{}", job.id);
            match executor
                .process_direct_with_channel(&job.message, &session_key, channel, chat_id)
                .await
            {
                Ok(_response) => {
                    // Response is automatically sent via the output by AgentLoop
                    "ok".to_string()
                }
                Err(e) => format!("Error: {}", e),
            }
        } else {
            // No executor configured, fall back to direct delivery
            if let Some(ref out) = self.output {
                out.publish_outbound(channel, chat_id, &job.message);
            }
            "ok".to_string()
        }
    }
}

#[async_trait]
impl Tool for CronTool {
    fn name(&self) -> &str {
        "cron"
    }

    fn description(&self) -> &str {
        "Schedule reminders, tasks, or system commands. Use 'at_seconds' for one-time reminders, 'every_seconds' for recurring tasks, or 'cron_expr' for complex schedules."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["add", "list", "remove", "enable", "disable"],
                    "description": "Action to perform"
                },
                "message": {"type": "string", "description": "Reminder/task message"},
                "command": {"type": "string", "description": "Optional shell command to execute"},
                "at_seconds": {"type": "integer", "description": "One-time: seconds from now"},
                "every_seconds": {"type": "integer", "description": "Recurring interval in seconds"},
                "cron_expr": {"type": "string", "description": "Cron expression"},
                "job_id": {"type": "string", "description": "Job ID (for remove/enable/disable)"},
                "deliver": {"type": "boolean", "description": "Send message directly (default: true)"}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let action = match args["action"].as_str() {
            Some(a) => a,
            None => return ToolResult::error("action is required"),
        };

        match action {
            "add" => self.add_job(args).await,
            "list" => self.list_jobs().await,
            "remove" => self.remove_job(args).await,
            "enable" => self.enable_job(args, true).await,
            "disable" => self.enable_job(args, false).await,
            _ => ToolResult::error(&format!("unknown action: {}", action)),
        }
    }
}

impl CronTool {
    async fn add_job(&self, args: &serde_json::Value) -> ToolResult {
        let channel = self.channel.lock().await.clone();
        let chat_id = self.chat_id.lock().await.clone();

        if channel.is_empty() || chat_id.is_empty() {
            return ToolResult::error(
                "no session context (channel/chat_id not set). Use this tool in an active conversation.",
            );
        }

        let message = match args["message"].as_str() {
            Some(m) if !m.is_empty() => m,
            _ => return ToolResult::error("message is required for add"),
        };

        // Determine schedule
        let schedule = if let Some(at_secs) = args["at_seconds"].as_u64() {
            let at_ms = chrono::Utc::now().timestamp_millis() + (at_secs as i64) * 1000;
            CronSchedule::At { at_ms }
        } else if let Some(every_secs) = args["every_seconds"].as_u64() {
            let every_ms = (every_secs as i64) * 1000;
            CronSchedule::Every { every_ms }
        } else if let Some(expr) = args["cron_expr"].as_str() {
            CronSchedule::Cron {
                expr: expr.to_string(),
            }
        } else {
            return ToolResult::error(
                "one of at_seconds, every_seconds, or cron_expr is required",
            );
        };

        let mut deliver = args["deliver"].as_bool().unwrap_or(true);
        let command = args["command"].as_str().unwrap_or("").to_string();
        if !command.is_empty() {
            deliver = false;
        }

        // Truncate name to 30 chars
        let name: String = message.chars().take(30).collect();

        let mut service = self.cron_service.lock().await;
        let job = service.add_job(&name, schedule, message, deliver, &channel, &chat_id);

        let job_id = job.id.clone();
        if !command.is_empty() {
            let updated_job = CronJob {
                command: Some(command),
                ..job.clone()
            };
            service.update_job(updated_job);
        }

        ToolResult::silent(&format!("Cron job added: {} (id: {})", name, job_id))
    }

    async fn list_jobs(&self) -> ToolResult {
        let service = self.cron_service.lock().await;
        let jobs = service.list_jobs();

        if jobs.is_empty() {
            return ToolResult::silent("No scheduled jobs");
        }

        let mut result = "Scheduled jobs:\n".to_string();
        for j in jobs {
            let schedule_info = match &j.schedule {
                CronSchedule::Every { every_ms } => format!("every {}s", every_ms / 1000),
                CronSchedule::Cron { expr } => expr.clone(),
                CronSchedule::At { .. } => "one-time".to_string(),
            };
            result.push_str(&format!("- {} (id: {}, {})\n", j.name, j.id, schedule_info));
        }

        ToolResult::silent(&result)
    }

    async fn remove_job(&self, args: &serde_json::Value) -> ToolResult {
        let job_id = match args["job_id"].as_str() {
            Some(id) if !id.is_empty() => id,
            _ => return ToolResult::error("job_id is required for remove"),
        };

        let mut service = self.cron_service.lock().await;
        if service.remove_job(job_id) {
            ToolResult::silent(&format!("Cron job removed: {}", job_id))
        } else {
            ToolResult::error(&format!("Job {} not found", job_id))
        }
    }

    async fn enable_job(&self, args: &serde_json::Value, enable: bool) -> ToolResult {
        let job_id = match args["job_id"].as_str() {
            Some(id) if !id.is_empty() => id,
            _ => return ToolResult::error("job_id is required for enable/disable"),
        };

        let mut service = self.cron_service.lock().await;
        if service.enable_job(job_id, enable) {
            let job = service.get_job(job_id).unwrap();
            let status = if enable { "enabled" } else { "disabled" };
            ToolResult::silent(&format!("Cron job '{}' {}", job.name, status))
        } else {
            ToolResult::error(&format!("Job {} not found", job_id))
        }
    }
}

impl ContextualTool for CronTool {
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
