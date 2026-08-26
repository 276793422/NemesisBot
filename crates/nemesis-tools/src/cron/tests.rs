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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
    assert!(result.for_llm.contains("Cron job added"));
}

#[tokio::test]
async fn test_cron_tool_list_empty() {
    let (_, tool) = make_tool();
    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
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
    let j1_id = service
        .add_job("j1", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c1")
        .id
        .clone();
    let j2_id = service
        .add_job("j2", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c2")
        .id
        .clone();
    let j3_id = service
        .add_job("j3", CronSchedule::At { at_ms: 0 }, "m", true, "web", "c3")
        .id
        .clone();
    assert_eq!(j1_id, "cron-1");
    assert_eq!(j2_id, "cron-2");
    assert_eq!(j3_id, "cron-3");
}

#[test]
fn test_cron_service_get_job() {
    let mut service = CronService::new();
    service.add_job(
        "findable",
        CronSchedule::Every { every_ms: 1000 },
        "msg",
        true,
        "ch",
        "cid",
    );
    let found = service.get_job("cron-1");
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "findable");

    let missing = service.get_job("cron-999");
    assert!(missing.is_none());
}

#[test]
fn test_cron_service_update_job() {
    let mut service = CronService::new();
    service.add_job(
        "original",
        CronSchedule::At { at_ms: 0 },
        "msg",
        true,
        "ch",
        "cid",
    );

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

    let cron = CronSchedule::Cron {
        expr: "0 * * * *".to_string(),
    };
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
    assert!(
        !result.is_error,
        "Expected success, got: {}",
        result.for_llm
    );
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
    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
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
    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
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
    let result = tool.execute(&serde_json::json!({"action": "remove"})).await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("job_id"));
}

#[tokio::test]
async fn test_cron_tool_enable_missing_job_id() {
    let (_, tool) = make_tool();
    let result = tool.execute(&serde_json::json!({"action": "enable"})).await;
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

    tool.set_output(Arc::new(MockOutput {
        called: output_called_clone,
    }));

    // Add a job with deliver=true
    {
        let mut svc = service.lock().await;
        svc.add_job(
            "deliver test",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            true,
            "web",
            "chat-1",
        );
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

    tool.set_output(Arc::new(MockOutput {
        called: output_called_clone,
    }));

    // Add a job with deliver=false (no executor configured)
    {
        let mut svc = service.lock().await;
        svc.add_job(
            "no executor",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            false,
            "web",
            "chat-1",
        );
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
        let job = svc.add_job(
            "cmd test",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            false,
            "web",
            "chat-1",
        );
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

    tool.set_output(Arc::new(MockOutput {
        captured: captured_clone,
    }));

    {
        let mut svc = service.lock().await;
        svc.add_job(
            "default channel",
            CronSchedule::Every { every_ms: 1000 },
            "msg",
            true,
            "",
            "",
        );
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

// ============================================================
// W4a coverage gap closure (set_executor/set_shell_tool wiring,
// command-execution path, executor path, list formats, enable
// not-found)
// ============================================================

/// Output sink capturing every published (channel, chat_id, content) tuple.
struct W4aCapturingOutput {
    published: Arc<std::sync::Mutex<Vec<(String, String, String)>>>,
}
impl CronJobOutput for W4aCapturingOutput {
    fn publish_outbound(&self, channel: &str, chat_id: &str, content: &str) {
        self.published
            .lock()
            .unwrap()
            .push((channel.to_string(), chat_id.to_string(), content.to_string()));
    }
}

fn w4a_capturing_output() -> (Arc<W4aCapturingOutput>, Arc<std::sync::Mutex<Vec<(String, String, String)>>>) {
    let published = Arc::new(std::sync::Mutex::new(Vec::new()));
    (Arc::new(W4aCapturingOutput { published: published.clone() }), published)
}

/// JobExecutor double: records the call args, returns a canned result.
struct W4aMockExecutor {
    reply: Result<String, String>,
    calls: Arc<std::sync::Mutex<Vec<String>>>,
}
impl JobExecutor for W4aMockExecutor {
    fn process_direct_with_channel(
        &self,
        content: &str,
        session_key: &str,
        channel: &str,
        chat_id: &str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, String>> + Send + '_>,
    > {
        let calls = self.calls.clone();
        let reply = self.reply.clone();
        let content = content.to_string();
        let session_key = session_key.to_string();
        let channel = channel.to_string();
        let chat_id = chat_id.to_string();
        Box::pin(async move {
            calls
                .lock()
                .unwrap()
                .push(format!("{content}|{session_key}|{channel}|{chat_id}"));
            reply
        })
    }
}

/// Add a job carrying a shell command via the tool's add action.
/// (The add action requires session context; channel/chat_id args are ignored.)
async fn w4a_add_command_job(service: &Arc<Mutex<CronService>>, tool: &mut CronTool, command: &str) -> String {
    let ctx = crate::registry::ToolExecutionContext {
        channel: "web".to_string(),
        chat_id: "chat-w4a".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(tool, &ctx);
    tool.execute(&serde_json::json!({
        "action": "add",
        "message": "w4a cmd job",
        "command": command,
        "at_seconds": 3600,
    }))
    .await;
    let svc = service.lock().await;
    svc.jobs.last().unwrap().id.clone()
}

#[tokio::test]
async fn w4a_execute_job_with_command_runs_shell_and_publishes() {
    let (service, mut tool) = make_tool();
    let (_sink, published) = w4a_capturing_output();
    tool.set_output(Arc::new(W4aCapturingOutput {
        published: published.clone(),
    }));
    // set_shell_tool wiring (line 202-204) + full command lane
    tool.set_shell_tool(Arc::new(ShellTool::new(".", false)));

    let job_id = w4a_add_command_job(&service, &mut tool, "echo w4a_cron_ok").await;
    let job = service.lock().await.get_job(&job_id).unwrap().clone();
    assert_eq!(job.command.as_deref(), Some("echo w4a_cron_ok"));

    let result = tool.execute_job(&job).await;
    assert_eq!(result, "ok");
    let published = published.lock().unwrap();
    assert_eq!(published.len(), 1, "command output must be published exactly once");
    let (channel, chat_id, content) = &published[0];
    assert_eq!(channel, "web");
    assert_eq!(chat_id, "chat-w4a");
    assert!(
        content.contains("Scheduled command 'echo w4a_cron_ok' executed"),
        "unexpected published content: {content}"
    );
    assert!(content.contains("w4a_cron_ok"), "shell stdout must reach the output: {content}");
}

#[tokio::test]
async fn w4a_execute_job_failing_command_publishes_error_output() {
    let (service, mut tool) = make_tool();
    let (_sink, published) = w4a_capturing_output();
    tool.set_output(Arc::new(W4aCapturingOutput {
        published: published.clone(),
    }));
    tool.set_shell_tool(Arc::new(ShellTool::new(".", false)));

    let job_id = w4a_add_command_job(&service, &mut tool, "exit 7").await;
    let job = service.lock().await.get_job(&job_id).unwrap().clone();

    let result = tool.execute_job(&job).await;
    assert_eq!(result, "ok", "execute_job itself reports ok; failure is in the published output");
    let published = published.lock().unwrap();
    assert!(
        published[0].2.contains("Error executing scheduled command"),
        "unexpected published content: {}",
        published[0].2
    );
    assert!(published[0].2.contains("Exit code"), "shell error detail expected: {}", published[0].2);
}

#[tokio::test]
async fn w4a_execute_job_deliver_false_routes_through_executor() {
    let (service, mut tool) = make_tool();
    // set_executor wiring (line 192-194) + executor Ok arm
    let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
    tool.set_executor(Arc::new(W4aMockExecutor {
        reply: Ok("agent reply".to_string()),
        calls: calls.clone(),
    }));
    // deliver=false job without command -> executor lane
    {
        let mut svc = service.lock().await;
        svc.add_job(
            "agent task",
            CronSchedule::Every { every_ms: 60_000 },
            "please summarize",
            false,
            "web",
            "chat-exec",
        );
    }
    let job = service.lock().await.get_job("cron-1").unwrap().clone();

    let result = tool.execute_job(&job).await;
    assert_eq!(result, "ok");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 1, "executor must be invoked exactly once");
    assert_eq!(calls[0], "please summarize|cron-cron-1|web|chat-exec");
}

#[tokio::test]
async fn w4a_execute_job_executor_error_is_surfaced() {
    let (service, mut tool) = make_tool();
    tool.set_executor(Arc::new(W4aMockExecutor {
        reply: Err("agent kaput".to_string()),
        calls: Arc::new(std::sync::Mutex::new(Vec::new())),
    }));
    {
        let mut svc = service.lock().await;
        svc.add_job(
            "agent task",
            CronSchedule::At { at_ms: 0 },
            "do work",
            false,
            "",
            "",
        );
    }
    let job = service.lock().await.get_job("cron-1").unwrap().clone();

    let result = tool.execute_job(&job).await;
    assert_eq!(result, "Error: agent kaput");
}

#[tokio::test]
async fn w4a_list_jobs_formats_every_and_cron_schedules() {
    let (service, mut tool) = make_tool();
    let ctx = crate::registry::ToolExecutionContext {
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        ..Default::default()
    };
    ContextualTool::set_context(&mut tool, &ctx);
    {
        let mut svc = service.lock().await;
        svc.add_job(
            "every job",
            CronSchedule::Every { every_ms: 60_000 },
            "m",
            true,
            "web",
            "c1",
        );
    }
    tool.execute(&serde_json::json!({
        "action": "add",
        "message": "expr job",
        "cron_expr": "*/5 * * * *",
    }))
    .await;

    let result = tool.execute(&serde_json::json!({"action": "list"})).await;
    assert!(!result.is_error);
    assert!(result.for_llm.contains("every 60s"), "Every arm not formatted: {}", result.for_llm);
    assert!(result.for_llm.contains("*/5 * * * *"), "Cron arm not formatted: {}", result.for_llm);
}

#[tokio::test]
async fn w4a_enable_job_not_found_returns_error() {
    let (_, tool) = make_tool();
    let result = tool
        .execute(&serde_json::json!({"action": "enable", "job_id": "cron-999"}))
        .await;
    assert!(result.is_error);
    assert!(result.for_llm.contains("Job cron-999 not found"), "got: {}", result.for_llm);

    let result = tool
        .execute(&serde_json::json!({"action": "disable", "job_id": "cron-999"}))
        .await;
    assert!(result.is_error);
}

// ===========================================================================
// S2 coverage (2026-08-26): enable_job non-matching iteration edge +
// execute_job command=Some("") fall-through to deliver
// ===========================================================================

/// Enabling the second job forces the first loop iteration to fall through
/// the id comparison.
#[test]
fn s2_enable_job_skips_non_matching_iterations() {
    let mut service = CronService::new();
    service.add_job("job1", CronSchedule::At { at_ms: 0 }, "m1", true, "", "");
    service.add_job("job2", CronSchedule::At { at_ms: 0 }, "m2", true, "", "");

    assert!(service.enable_job("cron-2", false));
    let jobs = service.list_jobs();
    assert!(jobs[0].enabled, "job1 must be untouched");
    assert!(!jobs[1].enabled, "job2 must be disabled");
}

/// A job whose command is Some("") must skip the command branch entirely and
/// fall through to the deliver branch.
#[tokio::test]
async fn s2_execute_job_empty_command_falls_through_to_deliver() {
    let (service, mut tool) = make_tool();
    let called = Arc::new(std::sync::atomic::AtomicBool::new(false));
    struct S2Out {
        called: Arc<std::sync::atomic::AtomicBool>,
    }
    impl CronJobOutput for S2Out {
        fn publish_outbound(&self, _channel: &str, _chat_id: &str, content: &str) {
            assert_eq!(content, "s2 message");
            self.called
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
    }
    tool.set_output(Arc::new(S2Out { called: called.clone() }));

    {
        let mut svc = service.lock().await;
        let mut job = svc
            .add_job(
                "s2 empty cmd",
                CronSchedule::Every { every_ms: 1000 },
                "s2 message",
                true,
                "web",
                "chat-1",
            )
            .clone();
        job.command = Some(String::new());
        svc.update_job(job);
    }

    let job = service.lock().await.get_job("cron-1").unwrap().clone();
    let result = tool.execute_job(&job).await;
    assert_eq!(result, "ok");
    assert!(
        called.load(std::sync::atomic::Ordering::SeqCst),
        "deliver branch must publish the message"
    );
}
