//! G3 运行账本接线（workflow_run）：run_async 开账/终态收口 + Waiting
//! 保持开账、resume 凭确定性 id 收口。全局槽由本文件唯一测试安装（进程
//! 内其他并行测试的 run 也会写进该账，但断言按 execution id 圈定，互不
//! 干扰）；漏斗核心语义的详测在 nemesis-taskstore/tests/store_tests.rs。

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nemesis_providers::failover::FailoverError;
use nemesis_providers::router::LLMProvider;
use nemesis_providers::types::{ChatOptions, LLMResponse, Message, ToolDefinition};
use nemesis_workflow::engine::WorkflowEngine;
use nemesis_workflow::types::{Edge, ExecutionState, NodeDef, TriggerSource, Workflow};

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
        "stub"
    }
    fn name(&self) -> &str {
        "stub"
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

fn wf(name: &str, nodes: Vec<NodeDef>) -> Workflow {
    let edges: Vec<Edge> = nodes
        .iter()
        .flat_map(|n| {
            n.depends_on.iter().map(move |dep| Edge {
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

#[tokio::test]
async fn ledger_records_workflow_lifecycle_end_to_end() {
    // 进程级唯一 install 点：本测试独占临时库（先到先得，重复安装诚实拒绝）。
    let ledger_dir = tempfile::tempdir().unwrap();
    let db = nemesis_taskstore::TaskDb::open(&ledger_dir.path().join("ledger.db"))
        .expect("open ledger db");
    let _ = nemesis_taskstore::ledger::install(db);

    let engine = {
        let provider = Arc::new(StubProvider) as Arc<dyn LLMProvider>;
        let tools = Arc::new(nemesis_tools::registry::ToolRegistry::new());
        WorkflowEngine::new_integrated(provider, tools, None)
    };

    // 成功臂：run 终态 = succeeded，trigger 序列化进 payload。
    engine
        .register_workflow(wf("ok_wf", vec![node("n1", "delay", &[])]))
        .unwrap();
    let exec = engine
        .run("ok_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Completed);
    {
        let db = nemesis_taskstore::ledger::installed().unwrap();
        let run_id = nemesis_taskstore::ledger::workflow_run_id(&exec.id);
        let run = db.get_run(&run_id).expect("成功 run 在账");
        assert_eq!(run.status, "succeeded");
        assert_eq!(run.kind, "workflow_run");
        assert_eq!(run.title, "ok_wf");
        assert_eq!(run.payload["trigger"]["kind"], "cli");
        assert!(run.completed_at.is_some());
        let attempts = db.attempts_of(&run_id).expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(attempts[0].status, "succeeded");
    }

    // 失败臂：未知节点类型 → schedule Err → failed + 错误文本进 attempt。
    engine
        .register_workflow(wf(
            "bad_wf",
            vec![node("bad", "nonexistent_node_type", &[])],
        ))
        .unwrap();
    let exec = engine
        .run("bad_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Failed);
    {
        let db = nemesis_taskstore::ledger::installed().unwrap();
        let run = db
            .get_run(&nemesis_taskstore::ledger::workflow_run_id(&exec.id))
            .expect("失败 run 在账");
        assert_eq!(run.status, "failed");
        let attempts = db.attempts_of(&run.id).expect("attempts");
        assert_eq!(attempts[0].status, "failed");
        assert!(attempts[0].error.is_some(), "错误文本进 attempt 账");
    }

    // Waiting 臂：human_review 挂起 → run 保持开账（挂起的执行没有结束，
    // 账面如实镜像）→ resume 凭同一确定性 id 收口 succeeded。
    let mut hr_config = HashMap::new();
    hr_config.insert("message".to_string(), serde_json::json!("review me"));
    let mut hr_node = node("gate", "human_review", &[]);
    hr_node.config = hr_config;
    engine
        .register_workflow(wf("hr_wf", vec![hr_node, node("n2", "delay", &["gate"])]))
        .unwrap();
    let exec = engine
        .run("hr_wf", HashMap::new(), Some(TriggerSource::Cli))
        .await
        .unwrap();
    assert_eq!(exec.state, ExecutionState::Waiting);
    {
        let db = nemesis_taskstore::ledger::installed().unwrap();
        let run = db
            .get_run(&nemesis_taskstore::ledger::workflow_run_id(&exec.id))
            .expect("挂起 run 在账");
        assert_eq!(run.status, "running", "Waiting 保持开账");
        assert!(run.completed_at.is_none());
    }
    let mut review = HashMap::new();
    review.insert("approved".to_string(), serde_json::json!(true));
    let resumed = engine.resume_execution(&exec.id, review).await.unwrap();
    assert_eq!(resumed.state, ExecutionState::Completed);
    {
        let db = nemesis_taskstore::ledger::installed().unwrap();
        let run = db
            .get_run(&nemesis_taskstore::ledger::workflow_run_id(&exec.id))
            .expect("resume 收口后仍在账");
        assert_eq!(run.status, "succeeded", "resume 凭确定性 id 收口同一行");
        assert!(run.completed_at.is_some());
    }
}
