//! S10b (quality-hardening goal 冲刺, web 批次 2): drive the
//! `WorkflowObserver::on_event` delivery path end-to-end (previously only the
//! pure `build_completed_reply_with_workflow` was tested):
//!
//! - Completed with a persisted WorkflowChat-triggered execution → reply from
//!   the terminal node + `chat_log` append under `wf_chat:<name>` + broadcast
//!   failure warn (session has no send queue)
//! - Failed / Cancelled → fixed reply texts with the execution error
//! - unknown execution id → the eviction warn arm
//! - non-WorkflowChat trigger → the early-return arm
//! - `build_completed_reply` with a missing workflow def → JSON-dump fallback
//!
//! The engine is `WorkflowEngine::with_persistence(tempdir)`; the execution is
//! seeded by writing a persistence JSONL directly (the format is one
//! `Execution` JSON per line), which `get_execution` loads from disk.
//! chat_log writes go through the GLOBAL path manager — nanos-unique
//! workflow names + `delete_chat_log` cleanup (fork_route_tests house pattern).

use super::*;
use chrono::Local;
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::persistence::WorkflowPersistence;
use nemesis_workflow::types::{
    Execution, ExecutionState, NodeDef, NodeResult, TriggerSource, Workflow,
};
use std::collections::HashMap;

fn unique_wf(prefix: &str) -> String {
    format!(
        "{}s10b{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn terminal_workflow(name: &str) -> Workflow {
    Workflow {
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![NodeDef {
            id: "out".to_string(),
            node_type: "transform".to_string(),
            config: HashMap::new(),
            depends_on: Vec::new(),
            retry_count: 0,
            timeout: None,
            is_terminal: true,
        }],
        edges: Vec::new(),
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn wf_chat_trigger(workflow_name: &str, session_id: &str) -> TriggerSource {
    TriggerSource::WorkflowChat {
        chat_id: format!("web:{}", session_id),
        session_id: session_id.to_string(),
        workflow_name: workflow_name.to_string(),
        index: "abcd1234".to_string(),
        session_key: format!("wf_chat:{}", workflow_name),
    }
}

fn seeded_execution(
    id: &str,
    workflow_name: &str,
    trigger: Option<TriggerSource>,
    reply: Option<&str>,
    error: Option<&str>,
) -> Execution {
    let now = Local::now();
    let mut nr = HashMap::new();
    if let Some(r) = reply {
        nr.insert(
            "out".to_string(),
            NodeResult {
                node_id: "out".to_string(),
                output: serde_json::Value::String(r.to_string()),
                error: None,
                state: ExecutionState::Completed,
                started_at: now,
                ended_at: now,
                metadata: HashMap::new(),
            },
        );
    }
    Execution {
        id: id.to_string(),
        workflow_name: workflow_name.to_string(),
        state: ExecutionState::Completed,
        input: HashMap::new(),
        node_results: nr,
        started_at: now,
        ended_at: Some(now),
        error: error.map(|e| e.to_string()),
        variables: HashMap::new(),
        trigger_source: trigger,
        chat_id: None,
        session_key: None,
        owner: None,
        tags: HashMap::new(),
        workflow_hash: None,
    }
}

fn persist(dir: &std::path::Path, exec: &Execution) {
    WorkflowPersistence::new(dir.join("s10b_seed.jsonl"))
        .save_execution(exec)
        .expect("seed persistence");
}

fn make_observer(engine: WorkflowEngine) -> WorkflowChatReplyObserver {
    WorkflowChatReplyObserver::new(
        std::sync::Arc::new(crate::session::SessionManager::with_default_timeout()),
        std::sync::Arc::new(engine),
    )
}

#[tokio::test]
async fn completed_event_delivers_terminal_reply_to_chat_log() {
    let dir = tempfile::tempdir().unwrap();
    let wf_name = unique_wf("reply");
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    engine
        .register_workflow(terminal_workflow(&wf_name))
        .expect("register");
    let exec = seeded_execution(
        "exec-s10b-completed",
        &wf_name,
        Some(wf_chat_trigger(&wf_name, "sess-no-queue")),
        Some("这是最终回复"),
        None,
    );
    persist(dir.path(), &exec);
    let obs = make_observer(engine);

    obs.on_event(WorkflowEvent::Completed {
        execution_id: "exec-s10b-completed".to_string(),
        workflow_name: wf_name.clone(),
        timestamp: Local::now(),
    })
    .await;

    // The reply was persisted under the wf_chat session key…
    let key = format!("wf_chat:{}", wf_name);
    let (rows, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(rows[0]["role"], "assistant");
    assert_eq!(rows[0]["content"], "这是最终回复");
    // …and the broadcast to the queue-less session failed into a warn (not a
    // panic) — reaching here proves the full delivery path ran.
    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn failed_and_cancelled_events_deliver_fixed_reply_texts() {
    let dir = tempfile::tempdir().unwrap();
    let wf_fail = unique_wf("fail");
    let wf_cancel = unique_wf("cancel");
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    engine
        .register_workflow(terminal_workflow(&wf_fail))
        .expect("register fail wf");
    engine
        .register_workflow(terminal_workflow(&wf_cancel))
        .expect("register cancel wf");

    let exec_fail = seeded_execution(
        "exec-s10b-fail",
        &wf_fail,
        Some(wf_chat_trigger(&wf_fail, "sess-fail")),
        None,
        Some("引擎超时"),
    );
    let exec_cancel = seeded_execution(
        "exec-s10b-cancel",
        &wf_cancel,
        Some(wf_chat_trigger(&wf_cancel, "sess-cancel")),
        None,
        None,
    );
    persist(dir.path(), &exec_fail);
    persist(dir.path(), &exec_cancel);
    let obs = make_observer(engine);

    obs.on_event(WorkflowEvent::Failed {
        execution_id: "exec-s10b-fail".to_string(),
        workflow_name: wf_fail.clone(),
        error: "ignored-from-event".to_string(),
        timestamp: Local::now(),
    })
    .await;
    let key = format!("wf_chat:{}", wf_fail);
    let (rows, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    // The reply text comes from the EXECUTION's error field.
    assert_eq!(rows[0]["content"], "[工作流失败] 引擎超时");
    nemesis_agent::chat_log::delete_chat_log(&key);

    obs.on_event(WorkflowEvent::Cancelled {
        execution_id: "exec-s10b-cancel".to_string(),
        workflow_name: wf_cancel.clone(),
        timestamp: Local::now(),
    })
    .await;
    let key = format!("wf_chat:{}", wf_cancel);
    let (rows, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 1);
    assert_eq!(rows[0]["content"], "[工作流已取消]");
    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn on_event_ignores_unknown_execution_and_non_chat_triggers() {
    // Unknown id → the "execution not found" warn arm; must not panic.
    let dir = tempfile::tempdir().unwrap();
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    let obs = make_observer(engine);
    obs.on_event(WorkflowEvent::Completed {
        execution_id: "no-such-exec".to_string(),
        workflow_name: "wf".to_string(),
        timestamp: Local::now(),
    })
    .await;

    // Persisted execution WITHOUT a WorkflowChat trigger → early return; no
    // chat_log write happens for it.
    let wf_name = unique_wf("other");
    let engine = WorkflowEngine::with_persistence(dir.path().to_path_buf());
    let exec = seeded_execution("exec-s10b-cli", &wf_name, Some(TriggerSource::Cli), None, None);
    persist(dir.path(), &exec);
    let obs = make_observer(engine);
    obs.on_event(WorkflowEvent::Completed {
        execution_id: "exec-s10b-cli".to_string(),
        workflow_name: wf_name.clone(),
        timestamp: Local::now(),
    })
    .await;
    let key = format!("wf_chat:{}", wf_name);
    let (_, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 10, None);
    assert_eq!(total, 0, "non-workflow_chat executions must be ignored");
}

#[tokio::test]
async fn build_completed_reply_without_workflow_def_dumps_node_results() {
    // Engine with NO registered workflow → get_workflow misses → the
    // JSON-dump fallback arm.
    let engine = WorkflowEngine::new();
    let exec = seeded_execution("exec-x", "vanished", None, Some("原始输出"), None);
    let reply = build_completed_reply(&engine, &exec).await;
    assert!(reply.contains("原始输出"), "dumped node_results, got: {}", reply);
    assert!(reply.starts_with('{'), "pretty JSON dump: {}", reply);
}
