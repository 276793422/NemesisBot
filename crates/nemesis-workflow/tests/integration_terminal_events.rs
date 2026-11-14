//! BUG-20261114-035 终态事件契约回归锁。
//!
//! run_async 首跑尾段历来发终态 [`WorkflowEvent`]，但 resume 收口
//! （resume_execution_node 尾段 / settle_resume_outcome）与
//! cancel_execution 的 Waiting 取消臂此前不发任何事件——唯一生产观察者
//! WorkflowChatReplyObserver 只在 Completed/Failed/Cancelled 事件里释放
//! workflow_chat 的 per-workflow 互斥守卫，事件缺席 = 守卫永久滞留 =
//! 该工作流后续所有聊天请求静默卡死在 acquire()。
//!
//! 本文件钉住四条契约：
//! 1. resume 到 Completed 必须发恰好一次 Completed 事件；
//! 2. resume 中途再次 Waiting 不发事件（非终态，守卫必须留到下一次）；
//! 3. 取消 Waiting 执行必须发恰好一次 Cancelled 事件（旧实现 0 次）；
//! 4. 取消 Running 执行恰好一次 Cancelled 事件（run_async 尾段发，
//!    cancel_execution 的 token-缺席判别器不得重复发）；
//! 5. sub_workflow 子执行失败 → 父子两个执行各自发一次 Failed 事件
//!    （子走 resume 尾段、父走 settle_resume_outcome 早期收口臂）。
//!
//! 运行：`cargo test -p nemesis-workflow --test integration_terminal_events`

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::events::{WorkflowEvent, WorkflowObserver};
use nemesis_workflow::types::{ExecutionState, NodeDef, TriggerSource, Workflow};

// ---------------------------------------------------------------------------
// Scaffolding
// ---------------------------------------------------------------------------

struct StubProvider;

#[async_trait]
impl LLMProvider for StubProvider {
    async fn chat(
        &self,
        _messages: &[Message],
        _tools: &[ToolDefinition],
        _model: &str,
        _options: &ChatOptions,
    ) -> Result<LLMResponse, FailoverError> {
        Ok(LLMResponse {
            content: "stub".to_string(),
            tool_calls: Vec::new(),
            finish_reason: "stop".to_string(),
            usage: None,
            reasoning_content: None,
            extra: HashMap::new(),
            raw_request_body: None,
            raw_response_body: None,
        })
    }
    fn default_model(&self) -> &str {
        "stub-model"
    }
    fn name(&self) -> &str {
        "stub"
    }
}

#[derive(Default)]
struct RecordingObserver {
    events: Mutex<Vec<WorkflowEvent>>,
}

impl RecordingObserver {
    fn snapshot(&self) -> Vec<WorkflowEvent> {
        self.events.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkflowObserver for RecordingObserver {
    fn name(&self) -> &str {
        "terminal_events_test_observer"
    }
    async fn on_event(&self, event: WorkflowEvent) {
        self.events.lock().unwrap().push(event);
    }
}

/// 终态事件（Completed/Failed/Cancelled）快照；Started 等非终态事件
/// 不计入——本文件的契约只关终态。
fn terminal_events(events: &[WorkflowEvent]) -> Vec<WorkflowEvent> {
    events
        .iter()
        .filter(|e| {
            matches!(
                e,
                WorkflowEvent::Completed { .. }
                    | WorkflowEvent::Failed { .. }
                    | WorkflowEvent::Cancelled { .. }
            )
        })
        .cloned()
        .collect()
}

/// 事件投递是 fire-and-forget spawn（每个观察者独立 task）——轮询等待
/// 直到满足谓词或超时。避免固定 sleep 在慢机上的假红。
async fn wait_until(
    obs: &RecordingObserver,
    timeout: Duration,
    pred: impl Fn(&[WorkflowEvent]) -> bool,
) -> Vec<WorkflowEvent> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        let snap = obs.snapshot();
        if pred(&snap) {
            return snap;
        }
        if tokio::time::Instant::now() >= deadline {
            return snap;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn node(id: &str, node_type: &str, depends_on: &[&str]) -> NodeDef {
    NodeDef {
        id: id.to_string(),
        node_type: node_type.to_string(),
        config: HashMap::new(),
        depends_on: depends_on.iter().map(|s| s.to_string()).collect(),
        retry_count: 0,
        timeout: None,
        is_terminal: false,
    }
}

fn node_with_config(
    id: &str,
    node_type: &str,
    depends_on: &[&str],
    config: HashMap<String, serde_json::Value>,
) -> NodeDef {
    NodeDef {
        config,
        ..node(id, node_type, depends_on)
    }
}

fn workflow_with_nodes(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    let edges: Vec<nemesis_workflow::types::Edge> = nodes
        .iter()
        .flat_map(|n| {
            n.depends_on
                .iter()
                .map(move |dep| nemesis_workflow::types::Edge {
                    from_node: dep.clone(),
                    to_node: n.id.clone(),
                    condition: None,
                })
        })
        .collect();
    Workflow {
        name: name.to_string(),
        description: String::new(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes,
        edges,
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn integrated_engine() -> Arc<WorkflowEngine> {
    WorkflowEngine::new_integrated(
        Arc::new(StubProvider) as Arc<dyn LLMProvider>,
        Arc::new(nemesis_tools::registry::ToolRegistry::new()),
        None,
    )
}

fn assert_event_ids(events: &[WorkflowEvent], want: &[(&str, &str)]) {
    // (state_kind, execution_id) 多重集合必须精确相等。
    let got: Vec<(String, String)> = events
        .iter()
        .map(|e| {
            let kind = match e {
                WorkflowEvent::Completed { .. } => "completed",
                WorkflowEvent::Failed { .. } => "failed",
                WorkflowEvent::Cancelled { .. } => "cancelled",
                _ => "other",
            };
            (kind.to_string(), e.execution_id().to_string())
        })
        .collect();
    let mut want_sorted: Vec<(String, String)> = want
        .iter()
        .map(|(k, id)| (k.to_string(), id.to_string()))
        .collect();
    want_sorted.sort();
    let mut got_sorted = got.clone();
    got_sorted.sort();
    assert_eq!(
        got_sorted, want_sorted,
        "终态事件集合不匹配（可能漏发或重复发）：got={got:?}"
    );
}

// ---------------------------------------------------------------------------
// 契约 1+2：resume 收口——Completed 发事件、中途 Waiting 不发
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_emits_completed_exactly_once_and_waiting_stays_silent() {
    let engine = integrated_engine();
    let obs = Arc::new(RecordingObserver::default());
    engine.event_manager().register(obs.clone()).await;

    // n1 review → n2 transform → n3 review：两次 resume 之间必须经历
    // 一次「再次 Waiting」（契约 2：此刻不得发事件）。
    engine
        .register_workflow(workflow_with_nodes(
            "resume_events_wf",
            vec![
                node("n1", "human_review", &[]),
                node("n2", "transform", &["n1"]),
                node("n3", "human_review", &["n2"]),
            ],
        ))
        .unwrap();

    let execution = engine
        .run("resume_events_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Waiting);
    // 首跑到 Waiting：只有 Started，无终态事件。
    assert!(
        terminal_events(&obs.snapshot()).is_empty(),
        "Waiting 是非终态，不得发终态事件（守卫必须留到下一次 resume）"
    );

    // 第一次 resume → 再次 Waiting（n3）。仍不得发事件。
    let resumed = engine
        .resume_execution(
            &execution.id,
            HashMap::from([("approved".to_string(), serde_json::json!(true))]),
        )
        .await
        .unwrap();
    assert_eq!(resumed.state, ExecutionState::Waiting);
    // 事件投递是异步 spawn——留一拍窗口再断言（若实现误发，此处大概率
    // 已可见；窗口不够时下面的 wait_until 也会暴露多余事件）。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        terminal_events(&obs.snapshot()).is_empty(),
        "resume 中途再次 Waiting 不得发终态事件"
    );

    // 第二次 resume → Completed。恰好一次 Completed 事件。
    let done = engine
        .resume_execution(
            &execution.id,
            HashMap::from([("approved".to_string(), serde_json::json!(true))]),
        )
        .await
        .unwrap();
    assert_eq!(done.state, ExecutionState::Completed);

    let final_snap = wait_until(&obs, Duration::from_secs(5), |events| {
        !terminal_events(events).is_empty()
    })
    .await;
    assert_event_ids(
        &terminal_events(&final_snap),
        &[("completed", done.id.as_str())],
    );
}

// ---------------------------------------------------------------------------
// 契约 3：取消 Waiting 执行发恰好一次 Cancelled（旧实现 0 次——chat
// 守卫泄漏的根因路径）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_waiting_execution_emits_cancelled_exactly_once() {
    let engine = integrated_engine();
    let obs = Arc::new(RecordingObserver::default());
    engine.event_manager().register(obs.clone()).await;

    engine
        .register_workflow(workflow_with_nodes(
            "cancel_waiting_wf",
            vec![node("n1", "human_review", &[])],
        ))
        .unwrap();

    let execution = engine
        .run(
            "cancel_waiting_wf",
            HashMap::new(),
            Some(TriggerSource::Cli),
        )
        .await
        .unwrap();
    assert_eq!(execution.state, ExecutionState::Waiting);

    let cancelled = engine.cancel_execution(&execution.id).await.unwrap();
    assert_eq!(cancelled.state, ExecutionState::Cancelled);

    let final_snap = wait_until(&obs, Duration::from_secs(5), |events| {
        !terminal_events(events).is_empty()
    })
    .await;
    assert_event_ids(
        &terminal_events(&final_snap),
        &[("cancelled", execution.id.as_str())],
    );
}

// ---------------------------------------------------------------------------
// 契约 4：取消 Running 执行恰好一次（run_async 尾段发；token 在场时
// cancel_execution 不得重复发）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn cancel_running_execution_emits_cancelled_exactly_once() {
    let engine = integrated_engine();
    let obs = Arc::new(RecordingObserver::default());
    engine.event_manager().register(obs.clone()).await;

    engine
        .register_workflow(workflow_with_nodes(
            "cancel_running_wf",
            vec![node_with_config(
                "n1",
                "delay",
                &[],
                // delay 节点配置键是 seconds（f64，秒）——不是 duration。
                HashMap::from([("seconds".to_string(), serde_json::json!(30))]),
            )],
        ))
        .unwrap();

    let exec_id = engine
        .clone()
        .start_async(
            "cancel_running_wf",
            HashMap::new(),
            Some(TriggerSource::Cli),
        )
        .await
        .unwrap();

    // 让 run_async 起飞并装好 cancel token（create_execution 与 run_async
    // 之间的窗口竞态在本测试无害：任一臂都是恰好一次）。run_async 的
    // token 在首个 await 点前插入——100ms 足够常规调度，慢机上即使错过
    // （token 尚未插入），断言依旧成立（cancel_execution 补发 + run_async
    // 早退，仍是恰好一次）。
    tokio::time::sleep(Duration::from_millis(100)).await;

    let cancelled = engine.cancel_execution(&exec_id).await.unwrap();
    assert_eq!(cancelled.state, ExecutionState::Cancelled);

    let final_snap = wait_until(&obs, Duration::from_secs(5), |events| {
        !terminal_events(events).is_empty()
    })
    .await;
    // 静默窗口后再核对一次总数（防慢机上第二个事件迟到）。
    tokio::time::sleep(Duration::from_millis(200)).await;
    let settled = obs.snapshot();
    assert_event_ids(
        &terminal_events(&settled),
        &[("cancelled", exec_id.as_str())],
    );
    let _ = final_snap;
}

// ---------------------------------------------------------------------------
// 契约 5：sub_workflow 子执行失败 → 父子各发一次 Failed（子走 resume
// 尾段；父走 settle_resume_outcome 早期收口臂）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn resume_subworkflow_child_failure_emits_failed_for_both() {
    let engine = integrated_engine();
    let obs = Arc::new(RecordingObserver::default());
    engine.event_manager().register(obs.clone()).await;

    // 子工作流：review → tool（裸 ToolNodeExecutor 诚实失败）。
    engine
        .register_workflow(workflow_with_nodes(
            "child_fail_wf",
            vec![node("n1", "human_review", &[]), node("n2", "tool", &["n1"])],
        ))
        .unwrap();
    // 父工作流：单个 sub_workflow 节点。
    engine
        .register_workflow(workflow_with_nodes(
            "parent_fail_wf",
            vec![node_with_config(
                "s1",
                "sub_workflow",
                &[],
                HashMap::from([("workflow".to_string(), serde_json::json!("child_fail_wf"))]),
            )],
        ))
        .unwrap();

    let parent = engine
        .run("parent_fail_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(parent.state, ExecutionState::Waiting);
    assert!(
        terminal_events(&obs.snapshot()).is_empty(),
        "父子都停在 Waiting（子 human_review），不得有终态事件"
    );

    let resumed = engine
        .resume_execution(
            &parent.id,
            HashMap::from([("approved".to_string(), serde_json::json!(true))]),
        )
        .await
        .unwrap();
    assert_eq!(resumed.state, ExecutionState::Failed);

    // 等到两个 Failed（父 + 子）。子执行 id 从父节点结果的
    // metadata.execution_id 反查。
    let child_id = resumed
        .node_results
        .get("s1")
        .and_then(|r| r.metadata.get("execution_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .expect("sub_workflow 等待结果必须带子执行 id");
    let final_snap = wait_until(&obs, Duration::from_secs(5), |events| {
        terminal_events(events).len() >= 2
    })
    .await;
    assert_event_ids(
        &terminal_events(&final_snap),
        &[
            ("failed", resumed.id.as_str()),
            ("failed", child_id.as_str()),
        ],
    );
}
