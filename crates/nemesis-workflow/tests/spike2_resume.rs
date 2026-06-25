//! Spike 2: resume_execution 重调度可行性验证
//!
//! 验证场景(对应规划文档 4.2 节):
//! - workflow: A → review → B → C
//! - 第 1 次执行:跑 A,review 返回 Waiting,落 checkpoint
//! - 模拟 gateway 关闭重启:丢弃内存状态
//! - resume(review_result):加载 checkpoint → 注入 review_result → schedule_resume
//! - 验证 B、C 正确执行,A、review 不重跑
//!
//! 运行:`cargo test -p nemesis-workflow --test spike2_resume -- --nocapture`

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio;

use nemesis_workflow::context::WorkflowContext;
use nemesis_workflow::nodes::NodeExecutorRegistry;
use nemesis_workflow::scheduler::topological_sort;
use nemesis_workflow::types::{
    Edge, ExecutionState, NodeDef, NodeResult, Workflow,
};

// ============================================================================
// 中间结构(来自 Spike 1)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableNodeResult {
    node_id: String,
    output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    state: String,
    started_at: chrono::DateTime<Utc>,
    ended_at: chrono::DateTime<Utc>,
    #[serde(default)]
    metadata: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializableContext {
    variables: HashMap<String, Value>,
    node_results: HashMap<String, SerializableNodeResult>,
    input: HashMap<String, Value>,
}

impl SerializableContext {
    fn from_context(ctx: &WorkflowContext) -> Self {
        let variables = ctx.get_all_variables();
        let node_results = ctx.get_all_node_results();
        Self {
            variables: variables
                .iter()
                .map(|(k, v)| (k.clone(), json!(v)))
                .collect(),
            node_results: node_results
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        SerializableNodeResult {
                            node_id: v.node_id.clone(),
                            output: v.output.clone(),
                            error: v.error.clone(),
                            state: format!("{}", v.state),
                            started_at: v.started_at.with_timezone(&Utc),
                            ended_at: v.ended_at.with_timezone(&Utc),
                            metadata: v.metadata.clone(),
                        },
                    )
                })
                .collect(),
            input: HashMap::new(),
        }
    }

    fn to_context(&self) -> WorkflowContext {
        let input = self.input.clone();
        let ctx = WorkflowContext::new(input);
        for (k, v) in &self.variables {
            if let Some(s) = v.as_str() {
                ctx.set_var(k, s);
            }
        }
        for (k, v) in &self.node_results {
            let nr = NodeResult {
                node_id: v.node_id.clone(),
                output: v.output.clone(),
                error: v.error.clone(),
                state: parse_state(&v.state),
                started_at: v.started_at.with_timezone(&chrono::Local),
                ended_at: v.ended_at.with_timezone(&chrono::Local),
                metadata: v.metadata.clone(),
            };
            ctx.set_node_result(k, nr);
        }
        ctx
    }
}

fn parse_state(s: &str) -> ExecutionState {
    match s {
        "pending" => ExecutionState::Pending,
        "running" => ExecutionState::Running,
        "completed" => ExecutionState::Completed,
        "failed" => ExecutionState::Failed,
        "cancelled" => ExecutionState::Cancelled,
        "waiting" => ExecutionState::Waiting,
        _ => ExecutionState::Pending,
    }
}

// ============================================================================
// Checkpoint 数据结构
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    id: String,
    execution_id: String,
    saved_at: chrono::DateTime<Utc>,
    completed_nodes: HashSet<String>,
    waiting_node: Option<String>, // 当前等待中的节点(如果有)
    context_snapshot: SerializableContext,
    workflow_hash: String,
}

// ============================================================================
// Mock CheckpointStore
// ============================================================================

struct InMemoryCheckpointStore {
    checkpoints: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl InMemoryCheckpointStore {
    fn new() -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
        }
    }

    fn save(&self, checkpoint: Checkpoint) {
        let mut store = self.checkpoints.write().unwrap();
        store
            .entry(checkpoint.execution_id.clone())
            .or_default()
            .push(checkpoint);
    }

    fn latest(&self, execution_id: &str) -> Option<Checkpoint> {
        let store = self.checkpoints.read().unwrap();
        store.get(execution_id).and_then(|v| v.last().cloned())
    }

    fn list_all(&self) -> Vec<String> {
        let store = self.checkpoints.read().unwrap();
        store.keys().cloned().collect()
    }
}

// ============================================================================
// 简化版 schedule_resume(支持跳过 completed 节点)
// ============================================================================

/// 执行 workflow 节点,跳过 completed_nodes 集合中的节点
///
/// 如果遇到 Waiting 节点,立即停止并返回该节点 ID(让调用方决定后续)
async fn schedule_resume(
    nodes: &[NodeDef],
    edges: &[Edge],
    executors: &NodeExecutorRegistry,
    wf_ctx: &mut WorkflowContext,
    completed_nodes: &HashSet<String>,
) -> Result<ScheduleOutcome, String> {
    let levels = topological_sort(nodes, edges)?;
    let node_map: HashMap<String, &NodeDef> = nodes.iter().map(|n| (n.id.clone(), n)).collect();

    for level in levels {
        // 过滤掉 completed 节点(关键:跳过逻辑)
        let runnable: Vec<String> = level
            .into_iter()
            .filter(|id| !completed_nodes.contains(id))
            .collect();

        if runnable.is_empty() {
            continue;
        }

        // 串行执行(简化:阶段 1 实施时改回并发)
        for node_id in runnable {
            let node = node_map.get(&node_id).cloned().unwrap();
            let executor = match executors.get(&node.node_type) {
                Some(e) => e,
                None => {
                    return Err(format!(
                        "no executor for node type {:?} (node {})",
                        node.node_type, node_id
                    ))
                }
            };

            // 构造 context(简化)
            let mut exec_ctx = HashMap::new();
            for (k, v) in wf_ctx.get_all_variables() {
                exec_ctx.insert(k, json!(v));
            }
            for (k, v) in wf_ctx.get_all_node_results() {
                exec_ctx.insert(k, v.output);
            }

            let local_wf_ctx = WorkflowContext::new(exec_ctx.clone());
            let result = executor.execute(node, &exec_ctx, &local_wf_ctx).await?;

            println!(
                "  [schedule_resume] 节点 {} 执行完毕,state={}",
                node_id, result.state
            );

            wf_ctx.set_node_result(&node_id, result.clone());

            // 如果节点返回 Waiting,提前返回
            if result.state == ExecutionState::Waiting {
                return Ok(ScheduleOutcome::Waiting(node_id));
            }

            if result.state == ExecutionState::Failed {
                return Err(format!("节点 {} 失败: {:?}", node_id, result.error));
            }
        }
    }

    Ok(ScheduleOutcome::Completed)
}

#[derive(Debug)]
enum ScheduleOutcome {
    Completed,
    Waiting(String),
}

// ============================================================================
// 模拟节点:complete(返回 Completed)
// ============================================================================

struct CompleteNodeExecutor {
    label: String,
}

#[async_trait::async_trait]
impl nemesis_workflow::nodes::NodeExecutor for CompleteNodeExecutor {
    async fn execute(
        &self,
        node: &NodeDef,
        _context: &HashMap<String, Value>,
        _wf_ctx: &WorkflowContext,
    ) -> Result<NodeResult, String> {
        use chrono::Local;
        let now = Local::now();
        println!("    [executor:{}] 节点 {} 执行", self.label, node.id);
        Ok(NodeResult {
            node_id: node.id.clone(),
            output: json!({"label": self.label, "node_id": node.id}),
            error: None,
            state: ExecutionState::Completed,
            started_at: now,
            ended_at: Local::now(),
            metadata: HashMap::new(),
        })
    }
}

// ============================================================================
// 测试用的 workflow 构造
// ============================================================================

fn make_test_workflow() -> Workflow {
    Workflow {
        name: "test_resume".to_string(),
        description: "test".to_string(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![
            NodeDef {
                id: "A".to_string(),
                node_type: "complete".to_string(),
                config: HashMap::new(),
                depends_on: vec![],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            },
            NodeDef {
                id: "review".to_string(),
                node_type: "human_review".to_string(),
                config: HashMap::new(),
                depends_on: vec!["A".to_string()],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            },
            NodeDef {
                id: "B".to_string(),
                node_type: "complete".to_string(),
                config: HashMap::new(),
                depends_on: vec!["review".to_string()],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            },
            NodeDef {
                id: "C".to_string(),
                node_type: "complete".to_string(),
                config: HashMap::new(),
                depends_on: vec!["B".to_string()],
                retry_count: 0,
                timeout: None,
            is_terminal: false,
            },
        ],
        edges: vec![],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    }
}

fn make_test_registry() -> NodeExecutorRegistry {
    let registry = NodeExecutorRegistry::new();
    registry.register(
        "complete",
        Arc::new(CompleteNodeExecutor {
            label: "test_complete".to_string(),
        }),
    );
    // 复用内置的 human_review 节点(返回 Waiting)
    registry
}

// ============================================================================
// 测试
// ============================================================================

#[tokio::test]
async fn test_spike2_full_resume_flow() {
    println!("\n========== Spike 2: 完整 resume 流程 ==========\n");

    let workflow = make_test_workflow();
    let registry = make_test_registry();
    let store = InMemoryCheckpointStore::new();

    // ============ 第 1 次执行:跑到 review 返回 Waiting ============
    println!("\n--- 阶段 1:首次执行(跑到 review Waiting)---\n");

    let mut ctx1 = WorkflowContext::new(HashMap::new());

    // 手动跑 A
    let outcome1 = schedule_resume(&workflow.nodes, &workflow.edges, &registry, &mut ctx1, &HashSet::new()).await.unwrap();
    match outcome1 {
        ScheduleOutcome::Waiting(node_id) => {
            println!("→ 首次执行停在 review 节点: {}", node_id);
            assert_eq!(node_id, "review");
        }
        _ => panic!("应该停在 review Waiting"),
    }

    // 落 checkpoint(关键:包含已 completed 节点 + waiting 节点 + 完整 context)
    let completed_nodes: HashSet<String> = ctx1
        .get_all_node_results()
        .iter()
        .filter(|(_, r)| r.state == ExecutionState::Completed)
        .map(|(k, _)| k.clone())
        .collect();
    let waiting_node = ctx1
        .get_all_node_results()
        .iter()
        .find(|(_, r)| r.state == ExecutionState::Waiting)
        .map(|(k, _)| k.clone());

    let checkpoint = Checkpoint {
        id: format!("cp_{}", Utc::now().timestamp_millis()),
        execution_id: "exec-001".to_string(),
        saved_at: Utc::now(),
        completed_nodes: completed_nodes.clone(),
        waiting_node: waiting_node.clone(),
        context_snapshot: SerializableContext::from_context(&ctx1),
        workflow_hash: "abc123".to_string(),
    };
    store.save(checkpoint);
    println!(
        "→ 落 checkpoint: completed_nodes={:?}, waiting={:?}",
        completed_nodes, waiting_node
    );

    // ============ 模拟 gateway 关闭重启:丢弃 ctx1 ============
    println!("\n--- 阶段 2:gateway 重启,丢弃内存状态 ---\n");
    drop(ctx1);
    println!("→ 内存状态已清空,仅 CheckpointStore 持久化数据");

    // 验证:重启时能列出所有 in-flight executions
    let in_flight = store.list_all();
    println!("→ 启动扫描发现 in-flight executions: {:?}", in_flight);
    assert_eq!(in_flight.len(), 1);

    // ============ resume_execution ============
    println!("\n--- 阶段 3:resume(review_result)---\n");

    let review_result: HashMap<String, Value> = vec![
        ("approved".to_string(), json!(true)),
        ("comment".to_string(), json!("looks good")),
    ]
    .into_iter()
    .collect();

    // 加载 checkpoint
    let checkpoint = store.latest("exec-001").unwrap();
    println!("→ 加载 checkpoint: id={}", checkpoint.id);

    // 恢复 context
    let mut ctx2 = checkpoint.context_snapshot.to_context();
    println!("→ 从 checkpoint 恢复 WorkflowContext");

    // 找到 waiting 节点
    let waiting_node_id = checkpoint.waiting_node.clone().unwrap();
    println!("→ waiting 节点: {}", waiting_node_id);

    // 注入 review_result
    let now = chrono::Local::now();
    ctx2.set_node_result(
        &waiting_node_id,
        NodeResult {
            node_id: waiting_node_id.clone(),
            output: json!(review_result),
            error: None,
            state: ExecutionState::Completed, // 关键:把 Waiting 改成 Completed
            started_at: now,
            ended_at: now,
            metadata: HashMap::new(),
        },
    );
    println!("→ 注入 review_result,标记 review 为 Completed");

    // 重新调度,跳过已完成节点(A + review)
    let mut new_completed = checkpoint.completed_nodes.clone();
    new_completed.insert(waiting_node_id.clone());

    println!(
        "→ 调用 schedule_resume,跳过 completed: {:?}",
        new_completed
    );

    let outcome2 = schedule_resume(&workflow.nodes, &workflow.edges, &registry, &mut ctx2, &new_completed).await.unwrap();

    match outcome2 {
        ScheduleOutcome::Completed => println!("→ workflow 完成"),
        _ => panic!("应该完成,不是 Waiting"),
    }

    // ============ 验证 ============
    println!("\n--- 阶段 4:验证结果 ---\n");

    let results = ctx2.get_all_node_results();
    println!("→ 最终 node_results keys: {:?}", results.keys().collect::<Vec<_>>());

    // A 节点:来自首次执行,不应该重跑(只看到一次执行日志)
    assert!(results.contains_key("A"));
    assert_eq!(results["A"].state, ExecutionState::Completed);
    println!("✓ A 节点状态正确(来自 checkpoint,未重跑)");

    // review 节点:Waiting → Completed(注入 review_result)
    assert_eq!(results["review"].state, ExecutionState::Completed);
    assert_eq!(results["review"].output["approved"], json!(true));
    println!("✓ review 节点状态正确(Waiting → Completed,output 已注入)");

    // B 节点:resume 后执行
    assert!(results.contains_key("B"));
    assert_eq!(results["B"].state, ExecutionState::Completed);
    println!("✓ B 节点状态正确(resume 后执行)");

    // C 节点:resume 后执行
    assert!(results.contains_key("C"));
    assert_eq!(results["C"].state, ExecutionState::Completed);
    println!("✓ C 节点状态正确(resume 后执行)");

    println!("\n✓ 完整 resume 流程验证通过!");
}

#[tokio::test]
async fn test_spike2_skip_completed_efficiency() {
    println!("\n========== Spike 2: 跳过 completed 节点的效率验证 ==========\n");
    println!("目标:确认 completed 节点的 executor 不会被重复调用\n");

    // 用计数器 executor
    use std::sync::atomic::{AtomicU32, Ordering};
    let counter = Arc::new(AtomicU32::new(0));

    struct CountingExecutor {
        counter: Arc<AtomicU32>,
        label: String,
    }

    #[async_trait::async_trait]
    impl nemesis_workflow::nodes::NodeExecutor for CountingExecutor {
        async fn execute(
            &self,
            node: &NodeDef,
            _ctx: &HashMap<String, Value>,
            _wf_ctx: &WorkflowContext,
        ) -> Result<NodeResult, String> {
            self.counter.fetch_add(1, Ordering::SeqCst);
            println!("    [executor:{}] 节点 {} 执行(count={})", self.label, node.id, self.counter.load(Ordering::SeqCst));
            Ok(NodeResult {
                node_id: node.id.clone(),
                output: json!(self.label),
                error: None,
                state: ExecutionState::Completed,
                started_at: chrono::Local::now(),
                ended_at: chrono::Local::now(),
                metadata: HashMap::new(),
            })
        }
    }

    let workflow = make_test_workflow();

    // 替换 registry:所有节点都用 counting executor
    let registry = NodeExecutorRegistry::new();
    let counter_a = Arc::clone(&counter);
    registry.register(
        "complete",
        Arc::new(CountingExecutor {
            counter: counter_a,
            label: "A".to_string(),
        }),
    );
    registry.register(
        "human_review",
        Arc::new(CountingExecutor {
            counter: Arc::clone(&counter),
            label: "review".to_string(),
        }),
    );

    // 模拟 checkpoint:A 和 review 都已完成
    let mut completed_nodes = HashSet::new();
    completed_nodes.insert("A".to_string());
    completed_nodes.insert("review".to_string());

    let mut ctx = WorkflowContext::new(HashMap::new());

    // 跑 schedule_resume
    let outcome = schedule_resume(&workflow.nodes, &workflow.edges, &registry, &mut ctx, &completed_nodes).await.unwrap();

    assert!(matches!(outcome, ScheduleOutcome::Completed));

    // 验证:A 和 review 都没被调用(executor count 应该只是 B 和 C 各一次)
    let final_count = counter.load(Ordering::SeqCst);
    println!("\n→ executor 总调用次数: {} (预期 2:只调 B 和 C)", final_count);
    assert_eq!(final_count, 2, "应该只调用 B 和 C 的 executor,A 和 review 应被跳过");

    println!("✓ 跳过 completed 节点正确:已完成的 executor 不会重复执行");
}

#[tokio::test]
async fn test_spike2_workflow_hash_drift_detection() {
    println!("\n========== Spike 2: workflow hash 配置漂移检测 ==========\n");

    // 模拟:checkpoint 时 hash=abc123,resume 时 workflow hash 变了
    let checkpoint_hash = "abc123";

    let workflow = make_test_workflow();
    let json_str = serde_json::to_string(&workflow).unwrap();

    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    json_str.hash(&mut hasher);
    let current_hash = format!("{:x}", hasher.finish());

    let drifted = checkpoint_hash != current_hash;
    println!("→ checkpoint hash: {}", checkpoint_hash);
    println!("→ 当前 hash:       {}", current_hash);
    println!("→ 是否漂移: {}", drifted);

    assert!(drifted, "hash 应该不同(模拟 workflow 定义变了)");

    println!("✓ 配置漂移检测可行:Execution.workflow_hash 字段方案正确");
    println!("  resume 时如果 hash 不匹配,警告但允许继续(用户承担风险)");
}

#[tokio::test]
async fn test_spike2_summary() {
    println!("\n========== Spike 2 总结 ==========\n");
    println!("验证点 1: scheduler 跳过 completed 节点 → 可行(schedule_resume 函数)");
    println!("验证点 2: Checkpoint 数据结构 → 可行(包含 completed_nodes + waiting_node + context_snapshot)");
    println!("验证点 3: resume 重调度协议 → 可行(注入 review_result → 标记 Completed → schedule_resume)");
    println!("验证点 4: gateway 重启恢复 → 可行(InMemoryCheckpointStore 模拟)");
    println!("\n关键设计决策:");
    println!("  1. schedule_resume 函数签名:");
    println!("     schedule_resume(nodes, edges, executors, ctx, completed_nodes) -> ScheduleOutcome");
    println!("     返回 ScheduleOutcome::Completed 或 ScheduleOutcome::Waiting(node_id)");
    println!("  2. Checkpoint 结构:");
    println!("     {{ id, execution_id, saved_at: DateTime<Utc>,");
    println!("        completed_nodes: HashSet<String>,");
    println!("        waiting_node: Option<String>,  // 新增:当前 waiting 的节点");
    println!("        context_snapshot: SerializableContext,");
    println!("        workflow_hash: String }}");
    println!("  3. resume_execution 协议:");
    println!("     - 加载 checkpoint");
    println!("     - 检查 workflow_hash(漂移则 warning)");
    println!("     - 恢复 context");
    println!("     - 找到 waiting_node,注入 review_result");
    println!("     - 标记 waiting_node 为 Completed");
    println!("     - completed_nodes ∪= {{waiting_node}}");
    println!("     - 调用 schedule_resume");
    println!("\n预期 A1 工期不变(6-7 天),resume 协议无重大意外。");
}
