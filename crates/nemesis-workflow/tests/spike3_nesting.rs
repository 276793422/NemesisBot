//! Spike 3: 嵌套调用栈数据模型验证
//!
//! 验证场景(对应规划文档 5.2 节):
//! - agent → workflow A → agent_node → workflow B → human_review
//! - 每个 workflow 独立 execution_id
//! - CheckpointStore 按 execution_id 隔离
//! - 递归深度限制(MAX_RECURSION=3)
//!
//! 注意:本 Spike 验证**数据模型支持嵌套**,不验证实际执行流程
//! (实际执行流程属于 F1-F2 实施范围)
//!
//! 运行:`cargo test -p nemesis-workflow --test spike3_nesting -- --nocapture`

use std::collections::{HashMap, HashSet};
use std::sync::RwLock;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::json;

// ============================================================================
// TriggerSource(决策 2 的核心抽象)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum TriggerSource {
    Cli,
    Cron,
    Webhook {
        #[serde(default)]
        payload: serde_json::Value,
    },
    AgentTool {
        tool_call_id: String,
        recursion_depth: u32, // 关键:递归深度
    },
    Chat {
        chat_id: String,
        session_key: String,
        sender_id: String,
    },
    Event {
        event_type: String,
        #[serde(default)]
        data: serde_json::Value,
    },
}

impl TriggerSource {
    /// 获取递归深度(CLI/Cron/Webhook/Chat/Event 都是 0)
    fn recursion_depth(&self) -> u32 {
        match self {
            TriggerSource::AgentTool {
                recursion_depth, ..
            } => *recursion_depth,
            _ => 0,
        }
    }

    /// 是否是 AgentTool 触发(影响递归检查)
    fn is_agent_tool(&self) -> bool {
        matches!(self, TriggerSource::AgentTool { .. })
    }
}

// ============================================================================
// WorkflowFrame:workflow 调用栈的一层
// ============================================================================

#[derive(Debug, Clone)]
struct WorkflowFrame {
    execution_id: String,
    workflow_name: String,
    trigger_source: TriggerSource,
    parent_execution_id: Option<String>, // 用于追踪嵌套关系
    started_at: chrono::DateTime<Utc>,
}

// ============================================================================
// WorkflowCallStack:嵌套调用的运行时栈
// ============================================================================

const MAX_RECURSION_DEPTH: u32 = 3;

struct WorkflowCallStack {
    frames: Vec<WorkflowFrame>,
    max_depth: u32,
}

impl WorkflowCallStack {
    fn new() -> Self {
        Self {
            frames: vec![],
            max_depth: MAX_RECURSION_DEPTH,
        }
    }

    /// 推入新的一帧
    /// 如果触发源是 AgentTool 且深度超限,返回 Err
    fn push(&mut self, frame: WorkflowFrame) -> Result<(), String> {
        if frame.trigger_source.is_agent_tool() {
            let depth = frame.trigger_source.recursion_depth();
            if depth >= self.max_depth {
                return Err(format!(
                    "递归深度超限:max_depth={}, 当前 depth={} (拒绝启动 workflow {})",
                    self.max_depth, depth, frame.workflow_name
                ));
            }
        }
        println!(
            "→ 推入栈帧: exec={} workflow={} trigger={:?} depth={}",
            frame.execution_id,
            frame.workflow_name,
            std::mem::discriminant(&frame.trigger_source),
            frame.trigger_source.recursion_depth()
        );
        self.frames.push(frame);
        Ok(())
    }

    fn pop(&mut self) -> Option<WorkflowFrame> {
        let frame = self.frames.pop();
        if let Some(ref f) = frame {
            println!("← 弹出栈帧: exec={}", f.execution_id);
        }
        frame
    }

    fn current_depth(&self) -> u32 {
        // 当前栈中 AgentTool 触发的最大深度
        self.frames
            .iter()
            .map(|f| f.trigger_source.recursion_depth())
            .max()
            .unwrap_or(0)
    }

    fn len(&self) -> usize {
        self.frames.len()
    }

    fn find(&self, execution_id: &str) -> Option<&WorkflowFrame> {
        self.frames.iter().find(|f| f.execution_id == execution_id)
    }
}

// ============================================================================
// Checkpoint(复用 Spike 1/2 结构)
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Checkpoint {
    id: String,
    execution_id: String,
    saved_at: chrono::DateTime<Utc>,
    completed_nodes: HashSet<String>,
    waiting_node: Option<String>,
    #[serde(default)]
    parent_execution_id: Option<String>, // 新增:嵌套关系
    workflow_hash: String,
}

// ============================================================================
// CheckpointStore(按 execution_id 强制隔离)
// ============================================================================

struct InMemoryCheckpointStore {
    /// 强制按 execution_id 索引,不允许跨 execution 查询
    checkpoints: RwLock<HashMap<String, Vec<Checkpoint>>>,
}

impl InMemoryCheckpointStore {
    fn new() -> Self {
        Self {
            checkpoints: RwLock::new(HashMap::new()),
        }
    }

    /// save 必须传 execution_id(从 checkpoint 取)
    fn save(&self, checkpoint: Checkpoint) {
        let mut store = self.checkpoints.write().unwrap();
        store
            .entry(checkpoint.execution_id.clone())
            .or_default()
            .push(checkpoint);
    }

    /// load 必须传 execution_id,不可能跨 execution 查
    fn list(&self, execution_id: &str) -> Vec<Checkpoint> {
        let store = self.checkpoints.read().unwrap();
        store.get(execution_id).cloned().unwrap_or_default()
    }

    fn latest(&self, execution_id: &str) -> Option<Checkpoint> {
        let store = self.checkpoints.read().unwrap();
        store.get(execution_id).and_then(|v| v.last().cloned())
    }

    /// 列所有 execution_id(用于 gateway 启动扫描)
    fn list_executions(&self) -> Vec<String> {
        let store = self.checkpoints.read().unwrap();
        store.keys().cloned().collect()
    }
}

// ============================================================================
// 测试 1: execution_id 隔离
// ============================================================================

#[test]
fn test_execution_isolation() {
    println!("\n========== Spike 3 验证点 1: execution_id 隔离 ==========\n");

    let mut stack = WorkflowCallStack::new();

    // 模拟:外层 agent 调用 workflow A(Cli 触发,depth=0)
    let exec_a = uuid::Uuid::new_v4().to_string();
    stack
        .push(WorkflowFrame {
            execution_id: exec_a.clone(),
            workflow_name: "wf_A".to_string(),
            trigger_source: TriggerSource::Cli,
            parent_execution_id: None,
            started_at: Utc::now(),
        })
        .unwrap();

    // workflow A 内的 agent_node 调用 workflow B(AgentTool 触发,depth=1)
    let exec_b = uuid::Uuid::new_v4().to_string();
    stack
        .push(WorkflowFrame {
            execution_id: exec_b.clone(),
            workflow_name: "wf_B".to_string(),
            trigger_source: TriggerSource::AgentTool {
                tool_call_id: "tc-001".to_string(),
                recursion_depth: 1,
            },
            parent_execution_id: Some(exec_a.clone()),
            started_at: Utc::now(),
        })
        .unwrap();

    // workflow B 内的 agent_node 调用 workflow C(depth=2)
    let exec_c = uuid::Uuid::new_v4().to_string();
    stack
        .push(WorkflowFrame {
            execution_id: exec_c.clone(),
            workflow_name: "wf_C".to_string(),
            trigger_source: TriggerSource::AgentTool {
                tool_call_id: "tc-002".to_string(),
                recursion_depth: 2,
            },
            parent_execution_id: Some(exec_b.clone()),
            started_at: Utc::now(),
        })
        .unwrap();

    println!("\n→ 当前栈深度: {} 帧", stack.len());
    assert_eq!(stack.len(), 3);
    assert_ne!(exec_a, exec_b);
    assert_ne!(exec_b, exec_c);
    assert_ne!(exec_a, exec_c);
    println!("✓ 三个 execution_id 完全独立,无冲突");

    // 验证 parent_execution_id 链路
    let frame_c = stack.find(&exec_c).unwrap();
    assert_eq!(frame_c.parent_execution_id, Some(exec_b.clone()));
    let frame_b = stack.find(&exec_b).unwrap();
    assert_eq!(frame_b.parent_execution_id, Some(exec_a.clone()));
    let frame_a = stack.find(&exec_a).unwrap();
    assert_eq!(frame_a.parent_execution_id, None);
    println!("✓ parent_execution_id 链路正确: C → B → A → None");

    println!("\n✓ Execution 链路隔离验证通过");
}

// ============================================================================
// 测试 2: CheckpointStore 按 execution_id 隔离
// ============================================================================

#[test]
fn test_checkpoint_store_isolation() {
    println!("\n========== Spike 3 验证点 2: CheckpointStore 按 execution_id 隔离 ==========\n");

    let store = InMemoryCheckpointStore::new();

    // 模拟外层 workflow A 落 checkpoint
    let cp_a1 = Checkpoint {
        id: "cp_a1".to_string(),
        execution_id: "exec-A".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["node_a1".to_string()]),
        waiting_node: Some("agent_node".to_string()),
        parent_execution_id: None,
        workflow_hash: "hash_a".to_string(),
    };
    store.save(cp_a1);
    println!("→ 落 checkpoint: exec=A, node=agent_node waiting");

    // 模拟内层 workflow B 落 checkpoint
    let cp_b1 = Checkpoint {
        id: "cp_b1".to_string(),
        execution_id: "exec-B".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["node_b1".to_string()]),
        waiting_node: Some("human_review".to_string()),
        parent_execution_id: Some("exec-A".to_string()),
        workflow_hash: "hash_b".to_string(),
    };
    store.save(cp_b1);
    println!("→ 落 checkpoint: exec=B, node=human_review waiting");

    // 再落 A 的一个 checkpoint(场景:agent_node 后续节点)
    let cp_a2 = Checkpoint {
        id: "cp_a2".to_string(),
        execution_id: "exec-A".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["node_a1".to_string(), "agent_node".to_string()]),
        waiting_node: None,
        parent_execution_id: None,
        workflow_hash: "hash_a".to_string(),
    };
    store.save(cp_a2);
    println!("→ 落 checkpoint: exec=A, agent_node completed");

    // 验证:list_executions 返回 2 个 execution
    let all_execs = store.list_executions();
    println!("\n→ CheckpointStore 中所有 execution: {:?}", all_execs);
    assert_eq!(all_execs.len(), 2);
    assert!(all_execs.contains(&"exec-A".to_string()));
    assert!(all_execs.contains(&"exec-B".to_string()));

    // 验证:A 的 checkpoints 不应包含 B 的数据
    let a_checkpoints = store.list("exec-A");
    let b_checkpoints = store.list("exec-B");
    println!(
        "\n→ exec-A 的 checkpoint 数: {},exec-B 的 checkpoint 数: {}",
        a_checkpoints.len(),
        b_checkpoints.len()
    );
    assert_eq!(a_checkpoints.len(), 2);
    assert_eq!(b_checkpoints.len(), 1);

    // 验证:每个 checkpoint 都属于正确的 execution
    for cp in &a_checkpoints {
        assert_eq!(cp.execution_id, "exec-A");
    }
    for cp in &b_checkpoints {
        assert_eq!(cp.execution_id, "exec-B");
    }

    // 验证:latest 永远只返回该 execution 的最新
    let latest_a = store.latest("exec-A").unwrap();
    let latest_b = store.latest("exec-B").unwrap();
    assert_eq!(latest_a.id, "cp_a2"); // A 的最新
    assert_eq!(latest_b.id, "cp_b1"); // B 的最新
    println!(
        "→ exec-A latest: {}, exec-B latest: {}",
        latest_a.id, latest_b.id
    );

    println!("\n✓ CheckpointStore 按 execution_id 强制隔离验证通过");
    println!("  设计要点:接口签名强制传 execution_id,无 'list_all_checkpoints' 方法");
}

// ============================================================================
// 测试 3: 递归深度限制
// ============================================================================

#[test]
fn test_recursion_depth_limit() {
    println!("\n========== Spike 3 验证点 3: 递归深度限制(MAX=3) ==========\n");

    let mut stack = WorkflowCallStack::new();
    println!("→ 配置: MAX_RECURSION_DEPTH = {}", MAX_RECURSION_DEPTH);

    // 第 1 层(CLI 触发,depth=0,不计入递归)
    stack
        .push(WorkflowFrame {
            execution_id: "exec-1".to_string(),
            workflow_name: "wf_outer".to_string(),
            trigger_source: TriggerSource::Cli,
            parent_execution_id: None,
            started_at: Utc::now(),
        })
        .unwrap();
    println!("\n→ 第 1 层(CLI)推入成功,栈深度={}", stack.len());

    // 第 2 层(AgentTool depth=1)
    let result = stack.push(WorkflowFrame {
        execution_id: "exec-2".to_string(),
        workflow_name: "wf_mid".to_string(),
        trigger_source: TriggerSource::AgentTool {
            tool_call_id: "tc-1".to_string(),
            recursion_depth: 1,
        },
        parent_execution_id: Some("exec-1".to_string()),
        started_at: Utc::now(),
    });
    assert!(result.is_ok());
    println!(
        "→ 第 2 层(AgentTool depth=1)推入成功,栈深度={}",
        stack.len()
    );

    // 第 3 层(AgentTool depth=2)
    let result = stack.push(WorkflowFrame {
        execution_id: "exec-3".to_string(),
        workflow_name: "wf_inner".to_string(),
        trigger_source: TriggerSource::AgentTool {
            tool_call_id: "tc-2".to_string(),
            recursion_depth: 2,
        },
        parent_execution_id: Some("exec-2".to_string()),
        started_at: Utc::now(),
    });
    assert!(result.is_ok());
    println!(
        "→ 第 3 层(AgentTool depth=2)推入成功,栈深度={}",
        stack.len()
    );

    // 第 4 层(AgentTool depth=3,应该被拒绝)
    let result = stack.push(WorkflowFrame {
        execution_id: "exec-4".to_string(),
        workflow_name: "wf_too_deep".to_string(),
        trigger_source: TriggerSource::AgentTool {
            tool_call_id: "tc-3".to_string(),
            recursion_depth: 3,
        },
        parent_execution_id: Some("exec-3".to_string()),
        started_at: Utc::now(),
    });
    assert!(result.is_err(), "第 4 层应该被拒绝");
    let err_msg = result.err().unwrap();
    println!("\n→ 第 4 层(AgentTool depth=3)被拒绝:");
    println!("  错误: {}", err_msg);

    assert_eq!(stack.len(), 3, "栈应该只有 3 帧");
    println!("\n✓ 递归深度限制验证通过");
    println!("  设计:AgentTool 触发时检查 recursion_depth,超限直接 Err");
    println!("  workflow_run 工具调用时,从父 trigger 取 depth + 1");
}

// ============================================================================
// 测试 4: 嵌套 Waiting 场景的数据模型
// ============================================================================

#[test]
fn test_nested_waiting_data_model() {
    println!("\n========== Spike 3 验证点 4: 嵌套 Waiting 场景数据模型 ==========\n");

    // 场景:外层 workflow A 在 agent_node 等待,内层 workflow B 在 human_review 等待
    // 阶段 1 是否支持自动级联 resume?

    let store = InMemoryCheckpointStore::new();

    // 外层 A 的 checkpoint(在 agent_node 等待内层 B)
    let cp_a = Checkpoint {
        id: "cp_a".to_string(),
        execution_id: "exec-A".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["start".to_string()]),
        waiting_node: Some("agent_node".to_string()),
        parent_execution_id: None,
        workflow_hash: "hash_a".to_string(),
    };
    store.save(cp_a);

    // 内层 B 的 checkpoint(在 human_review 等待)
    let cp_b = Checkpoint {
        id: "cp_b".to_string(),
        execution_id: "exec-B".to_string(),
        saved_at: Utc::now(),
        completed_nodes: HashSet::from(["start".to_string()]),
        waiting_node: Some("human_review".to_string()),
        parent_execution_id: Some("exec-A".to_string()), // ← 关键:嵌套关系
        workflow_hash: "hash_b".to_string(),
    };
    store.save(cp_b);

    println!("→ 嵌套 Waiting 场景已建模:");
    println!("  exec-A: waiting at agent_node");
    println!("  exec-B: waiting at human_review (parent=exec-A)");

    // 通过 parent_execution_id 链可以恢复嵌套关系
    let all_execs = store.list_executions();
    let waiting_execs: Vec<String> = all_execs
        .iter()
        .filter(|eid| store.latest(eid).unwrap().waiting_node.is_some())
        .cloned()
        .collect();
    println!("\n→ 当前 Waiting 的 execution: {:?}", waiting_execs);
    assert_eq!(waiting_execs.len(), 2);

    // 找出最内层的 waiting(exec-B,parent 是 exec-A,所以 exec-B 更内层)
    let innermost_waiting = waiting_execs
        .iter()
        .find(|eid| {
            // 没有 execution 把它作为 parent(它是最内层)
            !all_execs.iter().any(|other| {
                store.latest(other).unwrap().parent_execution_id.as_deref() == Some(eid.as_str())
            })
        })
        .unwrap();
    println!(
        "→ 最内层 Waiting execution: {} (用户应该先 resume 这个)",
        innermost_waiting
    );
    assert_eq!(innermost_waiting, "exec-B");

    // resume 内层后,外层是否自动续行?
    // 阶段 1 决策:**不自动级联**
    println!("\n→ 阶段 1 决策:");
    println!("  - resume(exec-B, review_result) 只恢复 exec-B");
    println!("  - exec-B 完成后,**不自动**级联恢复 exec-A");
    println!("  - 用户/agent 需要显式 resume(exec-A, agent_result)");
    println!("  - 文档明确,UI(阶段 3)可加'级联 resume'按钮");

    // 验证:parent_execution_id 字段足够支持未来"级联 resume"功能
    // 即使阶段 1 不实现,数据模型已支持
    let parent_of_b = store.latest("exec-B").unwrap().parent_execution_id;
    assert_eq!(parent_of_b, Some("exec-A".to_string()));
    println!("\n✓ parent_execution_id 字段足够支持未来级联 resume,数据模型正确");

    println!("\n✓ 嵌套 Waiting 场景数据模型验证通过");
}

// ============================================================================
// 测试 5: TriggerSource 序列化
// ============================================================================

#[test]
fn test_trigger_source_serialization() {
    println!("\n========== Spike 3 验证点 5: TriggerSource 序列化 ==========\n");

    let sources = vec![
        ("Cli", TriggerSource::Cli),
        ("Cron", TriggerSource::Cron),
        (
            "Webhook",
            TriggerSource::Webhook {
                payload: json!({"event": "push"}),
            },
        ),
        (
            "AgentTool",
            TriggerSource::AgentTool {
                tool_call_id: "tc-1".to_string(),
                recursion_depth: 2,
            },
        ),
        (
            "Chat",
            TriggerSource::Chat {
                chat_id: "c-1".to_string(),
                session_key: "s-1".to_string(),
                sender_id: "u-1".to_string(),
            },
        ),
        (
            "Event",
            TriggerSource::Event {
                event_type: "message_received".to_string(),
                data: json!({"text": "hi"}),
            },
        ),
    ];

    for (name, source) in sources {
        let json_str = serde_json::to_string(&source).unwrap();
        let restored: TriggerSource = serde_json::from_str(&json_str).unwrap();
        assert_eq!(source, restored);
        println!("  {:?}: {} ✓", name, json_str);
    }

    println!("\n✓ TriggerSource 序列化方案正确(内部 tagged enum)");
    println!("  Execution.trigger_source: Option<TriggerSource> 字段持久化可行");
}

// ============================================================================
// 测试 6: 总结
// ============================================================================

#[test]
fn test_spike3_summary() {
    println!("\n========== Spike 3 总结 ==========\n");
    println!("验证点 1: execution_id 隔离 → 可行(UUID + parent_execution_id 链)");
    println!("验证点 2: CheckpointStore 按 execution_id 隔离 → 可行(接口强制传 execution_id)");
    println!("验证点 3: 递归深度限制(MAX=3)→ 可行(AgentTool 触发时检查)");
    println!("验证点 4: 嵌套 Waiting 场景 → 数据模型支持,阶段 1 不自动级联");
    println!("验证点 5: TriggerSource 序列化 → 可行");
    println!("\n关键设计决策:");
    println!("  1. TriggerSource enum(决策 2 抽象):");
    println!("     - AgentTool 带 recursion_depth 字段");
    println!("     - Chat 带 chat_id / session_key(写入 Execution 字段)");
    println!("  2. Execution 字段(决策 3 + 嵌套):");
    println!("     - trigger_source: Option<TriggerSource>");
    println!("     - chat_id: Option<String>(从 Chat trigger 提取,便于查询)");
    println!("     - parent_execution_id: Option<String>(嵌套追踪,新发现)");
    println!("  3. CheckpointStore 接口:");
    println!("     - save(checkpoint) / latest(execution_id) / list(execution_id)");
    println!("     - **没有** list_all_checkpoints(防止跨 execution 误查)");
    println!("     - list_executions() 只返回 ID 列表(用于启动扫描)");
    println!("  4. 递归检查:");
    println!("     - workflow_run 工具调用时,从当前 trigger 取 depth + 1");
    println!("     - depth >= MAX_RECURSION(3)直接 Err,不启动 workflow");
    println!("  5. 嵌套 resume 决策:");
    println!("     - 阶段 1:resume(exec-B) 只恢复 exec-B,不级联");
    println!("     - 阶段 3:UI 加'级联 resume'按钮(可选)");
    println!("     - 数据模型已支持(parent_execution_id 链)");
    println!("\n预期工期不变:F1-F2(agent tool 集成)1 天,A1 不变。");
}
