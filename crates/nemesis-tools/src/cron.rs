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
mod tests {
    use super::*;

    fn make_tool() -> (Arc<Mutex<CronService>>, CronTool) {
        let service = Arc::new(Mutex::new(CronService::new()));
        let tool = CronTool::new(Arc::clone(&service));
        (service, tool)
    }

    #[test]
    fn test_cron_service_add_job() {
        let mut service = CronService::new();
        let job = service.add_job(
            "test job",
            CronSchedule::Every { every_ms: 60000 },
            "test message",
            true,
            "web",
            "chat-1",
        );
        assert_eq!(job.id, "cron-1");
        assert!(job.enabled);
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn test_cron_service_remove_job() {
        let mut service = CronService::new();
        service.add_job("job1", CronSchedule::At { at_ms: 0 }, "m", true, "", "");
        service.add_job("job2", CronSchedule::At { at_ms: 0 }, "m", true, "", "");
        assert!(service.remove_job("cron-1"));
        assert!(!service.remove_job("nonexistent"));
        assert_eq!(service.len(), 1);
    }

    #[test]
    fn test_cron_service_enable_disable() {
        let mut service = CronService::new();
        service.add_job("job1", CronSchedule::At { at_ms: 0 }, "m", true, "", "");
        let found = service.enable_job("cron-1", false);
        assert!(found);
        assert!(!service.get_job("cron-1").unwrap().enabled);
    }

    #[tokio::test]
    async fn test_cron_tool_add_no_context() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "test",
                "at_seconds": 60
            }))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("no session context"));
    }

    #[tokio::test]
    async fn test_cron_tool_add_success() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "remind me",
                "at_seconds": 60
            }))
            .await;
        // Give a small delay for the mutex to be released from set_context
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
        assert!(result.for_llm.contains("Cron job added"));
    }

    #[tokio::test]
    async fn test_cron_tool_list_empty() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({"action": "list"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("No scheduled jobs"));
    }

    #[tokio::test]
    async fn test_cron_tool_remove_not_found() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({
                "action": "remove",
                "job_id": "nonexistent"
            }))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_cron_tool_unknown_action() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({"action": "invalid"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("unknown action"));
    }

    #[tokio::test]
    async fn test_cron_tool_add_recurring() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "check server",
                "every_seconds": 3600
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
    }

    #[tokio::test]
    async fn test_cron_tool_add_with_command() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "disk check",
                "command": "df -h",
                "at_seconds": 60
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
    }

    // ============================================================
    // Additional Cron lifecycle tests
    // ============================================================

    #[test]
    fn test_cron_service_default() {
        let service = CronService::default();
        assert!(service.is_empty());
        assert_eq!(service.len(), 0);
    }

    #[test]
    fn test_cron_service_incremental_ids() {
        let mut service = CronService::new();
        let j1_id = service.add_job("j1", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c1").id.clone();
        let j2_id = service.add_job("j2", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c2").id.clone();
        let j3_id = service.add_job("j3", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c3").id.clone();
        assert_eq!(j1_id, "cron-1");
        assert_eq!(j2_id, "cron-2");
        assert_eq!(j3_id, "cron-3");
    }

    #[test]
    fn test_cron_service_get_job() {
        let mut service = CronService::new();
        service.add_job("findable", CronSchedule::Every { every_ms: 1000 }, "msg", true, "ch", "cid");
        let found = service.get_job("cron-1");
        assert!(found.is_some());
        assert_eq!(found.unwrap().name, "findable");

        let missing = service.get_job("cron-999");
        assert!(missing.is_none());
    }

    #[test]
    fn test_cron_service_update_job() {
        let mut service = CronService::new();
        service.add_job("original", CronSchedule::At { at_ms: 0 }, "msg", true, "ch", "cid");

        let mut updated = service.get_job("cron-1").unwrap().clone();
        updated.name = "updated".to_string();
        updated.enabled = false;
        service.update_job(updated);

        let job = service.get_job("cron-1").unwrap();
        assert_eq!(job.name, "updated");
        assert!(!job.enabled);
    }

    #[test]
    fn test_cron_service_update_nonexistent() {
        let mut service = CronService::new();
        let job = CronJob {
            id: "cron-999".to_string(),
            name: "ghost".to_string(),
            schedule: CronSchedule::At { at_ms: 0 },
            message: "msg".to_string(),
            deliver: true,
            channel: "ch".to_string(),
            chat_id: "cid".to_string(),
            enabled: true,
            command: None,
        };
        // Should not panic
        service.update_job(job);
        assert_eq!(service.len(), 0);
    }

    #[test]
    fn test_cron_service_remove_all() {
        let mut service = CronService::new();
        service.add_job("j1", CronSchedule::At { at_ms: 0 }, "m", true, "", "");
        service.add_job("j2", CronSchedule::At { at_ms: 0 }, "m", true, "", "");
        service.add_job("j3", CronSchedule::At { at_ms: 0 }, "m", true, "", "");

        assert!(service.remove_job("cron-1"));
        assert!(service.remove_job("cron-2"));
        assert!(service.remove_job("cron-3"));
        assert!(service.is_empty());
    }

    #[test]
    fn test_cron_service_enable_nonexistent() {
        let mut service = CronService::new();
        let found = service.enable_job("cron-999", true);
        assert!(!found);
    }

    #[test]
    fn test_cron_schedule_serialization() {
        let at = CronSchedule::At { at_ms: 12345 };
        let json = serde_json::to_string(&at).unwrap();
        assert!(json.contains("at_ms"));
        assert!(json.contains("12345"));

        let every = CronSchedule::Every { every_ms: 60000 };
        let json = serde_json::to_string(&every).unwrap();
        assert!(json.contains("every_ms"));
        assert!(json.contains("60000"));

        let cron = CronSchedule::Cron { expr: "0 * * * *".to_string() };
        let json = serde_json::to_string(&cron).unwrap();
        assert!(json.contains("0 * * * *"));
    }

    #[test]
    fn test_cron_schedule_deserialization() {
        let json = r#"{"kind":"at","at_ms":99999}"#;
        let sched: CronSchedule = serde_json::from_str(json).unwrap();
        match sched {
            CronSchedule::At { at_ms } => assert_eq!(at_ms, 99999),
            _ => panic!("Expected At variant"),
        }
    }

    #[test]
    fn test_cron_job_serialization_roundtrip() {
        let job = CronJob {
            id: "cron-42".to_string(),
            name: "test job".to_string(),
            schedule: CronSchedule::Every { every_ms: 30000 },
            message: "check status".to_string(),
            deliver: false,
            channel: "rpc".to_string(),
            chat_id: "chat-abc".to_string(),
            enabled: true,
            command: Some("echo hello".to_string()),
        };
        let json = serde_json::to_string(&job).unwrap();
        let restored: CronJob = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.id, job.id);
        assert_eq!(restored.name, job.name);
        assert_eq!(restored.command, job.command);
        assert!(restored.enabled);
    }

    #[tokio::test]
    async fn test_cron_tool_add_cron_expression() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "cli".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "hourly check",
                "cron_expr": "0 * * * *"
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error, "Expected success, got: {}", result.for_llm);
    }

    #[tokio::test]
    async fn test_cron_tool_add_no_schedule() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "no schedule"
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("at_seconds"));
    }

    #[tokio::test]
    async fn test_cron_tool_add_no_message() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "at_seconds": 60
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("message"));
    }

    #[tokio::test]
    async fn test_cron_tool_add_remove_lifecycle() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        // Add a job
        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "temporary job",
                "at_seconds": 120
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error);

        // Extract job ID from result
        let for_llm = result.for_llm.clone();
        let job_id_start = for_llm.find("id: ").unwrap() + 4;
        let job_id_end = for_llm[job_id_start..].find(')').unwrap() + job_id_start;
        let job_id = &for_llm[job_id_start..job_id_end];

        // List should contain the job
        let result = tool
            .execute(&serde_json::json!({"action": "list"}))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("temporary job"));

        // Remove the job
        let result = tool
            .execute(&serde_json::json!({
                "action": "remove",
                "job_id": job_id
            }))
            .await;
        assert!(!result.is_error);

        // List should be empty
        let result = tool
            .execute(&serde_json::json!({"action": "list"}))
            .await;
        assert!(result.for_llm.contains("No scheduled jobs"));
    }

    #[tokio::test]
    async fn test_cron_tool_enable_disable_lifecycle() {
        let mut tool = make_tool().1;
        let ctx = crate::registry::ToolExecutionContext {
            channel: "web".to_string(),
            chat_id: "chat-1".to_string(),
            ..Default::default()
        };
        ContextualTool::set_context(&mut tool, &ctx);

        // Add a job
        let result = tool
            .execute(&serde_json::json!({
                "action": "add",
                "message": "toggleable job",
                "every_seconds": 300
            }))
            .await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(!result.is_error);

        let for_llm = result.for_llm.clone();
        let job_id_start = for_llm.find("id: ").unwrap() + 4;
        let job_id_end = for_llm[job_id_start..].find(')').unwrap() + job_id_start;
        let job_id = &for_llm[job_id_start..job_id_end];

        // Disable
        let result = tool
            .execute(&serde_json::json!({
                "action": "disable",
                "job_id": job_id
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("disabled"));

        // Enable
        let result = tool
            .execute(&serde_json::json!({
                "action": "enable",
                "job_id": job_id
            }))
            .await;
        assert!(!result.is_error);
        assert!(result.for_llm.contains("enabled"));
    }

    #[tokio::test]
    async fn test_cron_tool_missing_action() {
        let (_, tool) = make_tool();
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("action is required"));
    }

    #[tokio::test]
    async fn test_cron_tool_remove_missing_job_id() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({"action": "remove"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("job_id"));
    }

    #[tokio::test]
    async fn test_cron_tool_enable_missing_job_id() {
        let (_, tool) = make_tool();
        let result = tool
            .execute(&serde_json::json!({"action": "enable"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("job_id"));
    }

    #[tokio::test]
    async fn test_cron_tool_execute_job_deliver_true() {
        let (service, mut tool) = make_tool();
        let output_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let output_called_clone = output_called.clone();

        struct MockOutput {
            called: Arc<std::sync::atomic::AtomicBool>,
        }
        impl CronJobOutput for MockOutput {
            fn publish_outbound(&self, _channel: &str, _chat_id: &str, _content: &str) {
                self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        tool.set_output(Arc::new(MockOutput { called: output_called_clone }));

        // Add a job with deliver=true
        {
            let mut svc = service.lock().await;
            svc.add_job("deliver test", CronSchedule::Every { every_ms: 1000 }, "msg", true, "web", "chat-1");
        }

        let job = service.lock().await.get_job("cron-1").unwrap().clone();
        let result = tool.execute_job(&job).await;
        assert_eq!(result, "ok");
        assert!(output_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_cron_tool_execute_job_deliver_false_no_executor() {
        let (service, mut tool) = make_tool();
        let output_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let output_called_clone = output_called.clone();

        struct MockOutput {
            called: Arc<std::sync::atomic::AtomicBool>,
        }
        impl CronJobOutput for MockOutput {
            fn publish_outbound(&self, _channel: &str, _chat_id: &str, _content: &str) {
                self.called.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }

        tool.set_output(Arc::new(MockOutput { called: output_called_clone }));

        // Add a job with deliver=false (no executor configured)
        {
            let mut svc = service.lock().await;
            svc.add_job("no executor", CronSchedule::Every { every_ms: 1000 }, "msg", false, "web", "chat-1");
        }

        let job = service.lock().await.get_job("cron-1").unwrap().clone();
        let result = tool.execute_job(&job).await;
        assert_eq!(result, "ok");
        // Should fall back to direct delivery
        assert!(output_called.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    async fn test_cron_tool_execute_job_no_shell_for_command() {
        let (service, tool) = make_tool();

        // Add a job with a command but no shell tool configured
        {
            let mut svc = service.lock().await;
            let job = svc.add_job("cmd test", CronSchedule::Every { every_ms: 1000 }, "msg", false, "web", "chat-1");
            let updated = CronJob {
                command: Some("echo hello".to_string()),
                ..job.clone()
            };
            svc.update_job(updated);
        }

        let job = service.lock().await.get_job("cron-1").unwrap().clone();
        let result = tool.execute_job(&job).await;
        assert!(result.contains("error: no shell tool"));
    }

    #[tokio::test]
    async fn test_cron_tool_execute_job_empty_channel_defaults_to_cli() {
        let (service, mut tool) = make_tool();
        let captured = Arc::new(tokio::sync::Mutex::new(("".to_string(), "".to_string())));
        let captured_clone = captured.clone();

        struct MockOutput {
            captured: Arc<tokio::sync::Mutex<(String, String)>>,
        }
        impl CronJobOutput for MockOutput {
            fn publish_outbound(&self, channel: &str, chat_id: &str, _content: &str) {
                // Use try_lock to avoid blocking
                if let Ok(mut g) = self.captured.try_lock() {
                    *g = (channel.to_string(), chat_id.to_string());
                }
            }
        }

        tool.set_output(Arc::new(MockOutput { captured: captured_clone }));

        {
            let mut svc = service.lock().await;
            svc.add_job("default channel", CronSchedule::Every { every_ms: 1000 }, "msg", true, "", "");
        }

        let job = service.lock().await.get_job("cron-1").unwrap().clone();
        let result = tool.execute_job(&job).await;
        assert_eq!(result, "ok");

        let (ch, cid) = captured.lock().await.clone();
        assert_eq!(ch, "cli");
        assert_eq!(cid, "direct");
    }

    #[test]
    fn test_cron_tool_metadata() {
        let service = Arc::new(Mutex::new(CronService::new()));
        let tool = CronTool::new(service);
        assert_eq!(tool.name(), "cron");
        assert!(!tool.description().is_empty());
        let params = tool.parameters();
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["action"].is_object());
    }
}
