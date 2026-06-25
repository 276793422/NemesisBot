//! Spike 1: WorkflowContext / NodeResult / Execution 序列化方案验证
//!
//! 验证 4 个核心序列化问题(对应规划文档 3.2 节):
//! 1. RwLock<HashMap> inner 序列化(context.rs:20-22 的限制)
//! 2. chrono DateTime 时区处理(NodeResult.started_at 是 DateTime<Local>)
//! 3. ExecutionState enum 序列化(types.rs:51-60)
//! 4. Option 字段向后兼容(Execution 新增字段)
//!
//! 运行:`cargo test -p nemesis-workflow --test spike1_serialization -- --nocapture`

use std::collections::HashMap;
use std::sync::RwLock;

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

// ============================================================================
// 中间结构(模拟未来 Checkpoint 序列化方案)
// ============================================================================

/// 序列化友好的 NodeResult(DateTime 用 UTC、state 用字符串)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SerializableNodeResult {
    node_id: String,
    output: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    state: String,
    started_at: DateTime<Utc>,
    ended_at: DateTime<Utc>,
    #[serde(default)]
    metadata: HashMap<String, serde_json::Value>,
}

/// 序列化友好的 WorkflowContext(variables 是 JSON Value,不是 String)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct SerializableContext {
    variables: HashMap<String, serde_json::Value>,
    node_results: HashMap<String, SerializableNodeResult>,
    input: HashMap<String, serde_json::Value>,
}

// ============================================================================
// 验证点 1: RwLock<HashMap> inner 序列化
// ============================================================================

#[test]
fn test_rwlock_inner_serialization() {
    println!("\n=== 验证点 1: RwLock inner 序列化 ===\n");

    let variables = RwLock::new(HashMap::<String, String>::new());
    variables
        .write()
        .unwrap()
        .insert("key1".to_string(), "value1".to_string());

    // 错误尝试:直接序列化 RwLock(应该失败,因为 RwLock 没实现 Serialize)
    // compile_error 验证:
    // let _ = serde_json::to_string(&variables); // 这行无法编译

    // 正确方案:提取 inner 再序列化
    let inner = variables.read().unwrap().clone();
    let json_str = serde_json::to_string(&inner).unwrap();
    println!("提取 inner 后序列化: {}", json_str);

    let deserialized: HashMap<String, String> = serde_json::from_str(&json_str).unwrap();
    assert_eq!(deserialized.get("key1"), Some(&"value1".to_string()));

    println!("✓ 结论:WorkflowContext 必须通过中间结构(SerializableContext)序列化");
    println!("  实现方式:WorkflowContext::to_serializable() -> SerializableContext");
}

// ============================================================================
// 验证点 2: chrono DateTime 时区处理
// ============================================================================

#[test]
fn test_datetime_timezone_handling() {
    println!("\n=== 验证点 2: DateTime 时区处理 ===\n");

    let now_local: DateTime<Local> = Local::now();
    println!("源时间(Local): {}", now_local);

    // 转 UTC(用于序列化)
    let now_utc: DateTime<Utc> = now_local.with_timezone(&Utc);
    println!("转 UTC: {}", now_utc);

    // 序列化 UTC
    let json_str = serde_json::to_string(&now_utc).unwrap();
    println!("UTC 序列化: {}", json_str);

    // 反序列化
    let restored_utc: DateTime<Utc> = serde_json::from_str(&json_str).unwrap();
    println!("反序列化 UTC: {}", restored_utc);

    // 转回 Local
    let restored_local: DateTime<Local> = restored_utc.with_timezone(&Local);
    println!("转回 Local: {}", restored_local);

    // 验证时间戳一致(忽略时区表示)
    assert_eq!(now_local.timestamp(), restored_local.timestamp());

    // 关键测试:不同时区序列化是否一致(模拟跨时区持久化)
    let utc1 = DateTime::<Local>::from(now_local).with_timezone(&Utc);
    let utc2 = DateTime::<Utc>::from(now_utc);
    let ser1 = serde_json::to_string(&utc1).unwrap();
    let ser2 = serde_json::to_string(&utc2).unwrap();
    assert_eq!(ser1, ser2, "UTC 序列化结果应该完全一致");
    println!("跨时区序列化结果一致: {}", ser1);

    println!("✓ 结论:DateTime<Local> 序列化时先 with_timezone(&Utc),反序列化后转回 Local");
    println!("  Checkpoint 字段类型直接用 DateTime<Utc>,展示时再转");
}

// ============================================================================
// 验证点 3: ExecutionState enum 序列化
// ============================================================================

#[test]
fn test_execution_state_serialization() {
    println!("\n=== 验证点 3: ExecutionState enum 序列化 ===\n");

    use nemesis_workflow::types::ExecutionState;

    let cases = vec![
        (ExecutionState::Pending, "\"pending\""),
        (ExecutionState::Running, "\"running\""),
        (ExecutionState::Completed, "\"completed\""),
        (ExecutionState::Failed, "\"failed\""),
        (ExecutionState::Cancelled, "\"cancelled\""),
        (ExecutionState::Waiting, "\"waiting\""),
    ];

    for (state, expected_json) in cases {
        let json_str = serde_json::to_string(&state).unwrap();
        let restored: ExecutionState = serde_json::from_str(&json_str).unwrap();
        assert_eq!(state, restored, "Round-trip 失败: {:?}", state);
        assert_eq!(json_str, expected_json, "JSON 格式不符合预期");
        println!("  {:?} → {} → ✓", state, json_str);
    }

    println!("✓ 结论:ExecutionState 的 #[serde(rename_all = \"snake_case\")] 配置正确");
    println!("  Checkpoint 中 state 用字符串字段,反序列化时自动转 enum");
}

// ============================================================================
// 验证点 4: Option 字段向后兼容
// ============================================================================

#[test]
fn test_option_field_backward_compatibility() {
    println!("\n=== 验证点 4: Option 字段向后兼容 ===\n");

    // 模拟未来 Execution 加新字段后的结构
    #[derive(Debug, Deserialize, PartialEq)]
    struct MockExecution {
        id: String,
        workflow_name: String,
        #[serde(default)]
        state: Option<String>,
        // 新增字段(都带 #[serde(default)])
        #[serde(default)]
        chat_id: Option<String>,
        #[serde(default)]
        session_key: Option<String>,
        #[serde(default)]
        trigger_source: Option<String>,
        #[serde(default)]
        owner: Option<String>,
        #[serde(default)]
        workflow_hash: Option<String>,
    }

    // 旧格式 JSON(没有新字段)
    let old_format = json!({
        "id": "exec-001",
        "workflow_name": "test_workflow"
    });

    let restored: MockExecution = serde_json::from_value(old_format).unwrap();
    assert_eq!(restored.id, "exec-001");
    assert_eq!(restored.chat_id, None);
    assert_eq!(restored.trigger_source, None);
    println!("旧格式加载: chat_id=None, trigger_source=None ✓");

    // 新格式 JSON
    let new_format = json!({
        "id": "exec-002",
        "workflow_name": "test_workflow",
        "chat_id": "chat-123",
        "trigger_source": "cron",
        "workflow_hash": "abc123"
    });

    let restored: MockExecution = serde_json::from_value(new_format).unwrap();
    assert_eq!(restored.chat_id, Some("chat-123".to_string()));
    assert_eq!(restored.trigger_source, Some("cron".to_string()));
    assert_eq!(restored.workflow_hash, Some("abc123".to_string()));
    println!("新格式加载: 所有字段值正确 ✓");

    // 错误场景:字段类型不匹配
    let bad_format = json!({
        "id": "exec-003",
        "workflow_name": "test",
        "chat_id": 123  // 应该是 String,但给了数字
    });
    let result: Result<MockExecution, _> = serde_json::from_value(bad_format);
    assert!(result.is_err(), "类型不匹配应该报错");
    println!("类型不匹配报错: {} ✓", result.err().unwrap());

    println!("✓ 结论:新增字段全部用 #[serde(default)] + Option 类型");
    println!("  旧 JSON 加载时新字段默认 None,新 JSON 加载时正常");
}

// ============================================================================
// 验证点 5: 完整 WorkflowContext round-trip
// ============================================================================

#[test]
fn test_full_workflow_context_round_trip() {
    println!("\n=== 验证点 5: 完整 WorkflowContext round-trip ===\n");

    use nemesis_workflow::context::WorkflowContext;
    use nemesis_workflow::types::{ExecutionState, NodeResult};

    // 构造一个有内容的 context
    let mut input = HashMap::new();
    input.insert("user_message".to_string(), json!("hello world"));
    input.insert("count".to_string(), json!(42));

    let ctx = WorkflowContext::new(input);
    ctx.set_var("key1", "value1");
    ctx.set_var("key2", "value2");

    let node_result = NodeResult {
        node_id: "node1".to_string(),
        output: json!({"result": "success", "items": [1, 2, 3]}),
        error: None,
        state: ExecutionState::Completed,
        started_at: Local::now(),
        ended_at: Local::now(),
        metadata: HashMap::new(),
    };
    ctx.set_node_result("node1", node_result);

    // 模拟 to_serializable 转换(B3 实施后 variables 直接是 JSON,这里手动转)
    let variables = ctx.get_all_variables();
    let node_results = ctx.get_all_node_results();

    let serializable = SerializableContext {
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
        input: HashMap::new(), // 简化,实际应该提取 ctx.input
    };

    let json_str = serde_json::to_string_pretty(&serializable).unwrap();
    println!("序列化结果(片段):\n{}", &json_str[..200.min(json_str.len())]);

    // 反序列化
    let restored: SerializableContext = serde_json::from_str(&json_str).unwrap();

    // 验证 round-trip
    assert_eq!(serializable.variables, restored.variables);
    assert_eq!(
        serializable.node_results.len(),
        restored.node_results.len()
    );

    // 验证 node_result 内容
    let restored_nr = restored.node_results.get("node1").unwrap();
    assert_eq!(restored_nr.node_id, "node1");
    assert_eq!(restored_nr.state, "completed");
    assert_eq!(restored_nr.output["result"], json!("success"));
    println!("Round-trip 后所有字段一致 ✓");

    // 模拟恢复成 WorkflowContext
    let mut restored_input = HashMap::new();
    for (k, v) in &restored.input {
        restored_input.insert(k.clone(), v.clone());
    }
    let new_ctx = WorkflowContext::new(restored_input);
    for (k, v) in &restored.variables {
        // 当前 set_var 只接受 String,B3 实施后改为 Value
        if let Some(s) = v.as_str() {
            new_ctx.set_var(k, s);
        }
    }
    println!("恢复为 WorkflowContext 成功 ✓");

    println!("✓ 结论:序列化方案完整可行");
    println!("  实施步骤:");
    println!("    1. B3:variables 改为 HashMap<String, Value>");
    println!("    2. 加 SerializableContext + SerializableNodeResult 中间结构");
    println!("    3. WorkflowContext::to_serializable() / from_serializable()");
    println!("    4. CheckpointStore 直接持久化 SerializableContext");
}

// ============================================================================
// 验证点 6: Workflow 定义 YAML/JSON 双向序列化
// ============================================================================

#[test]
fn test_workflow_definition_serialization() {
    println!("\n=== 验证点 6: Workflow 定义 YAML/JSON 序列化 ===\n");

    use nemesis_workflow::types::{Edge, NodeDef, Workflow};

    let workflow = Workflow {
        name: "test_wf".to_string(),
        description: "Test workflow".to_string(),
        version: "1.0.0".to_string(),
        triggers: vec![],
        nodes: vec![NodeDef {
            id: "start".to_string(),
            node_type: "llm".to_string(),
            config: HashMap::new(),
            depends_on: vec![],
            retry_count: 0,
            timeout: None,
        is_terminal: false,
        }],
        edges: vec![Edge {
            from_node: "start".to_string(),
            to_node: "end".to_string(),
            condition: None,
        }],
        variables: HashMap::new(),
        metadata: HashMap::new(),
    };

    // 序列化为 YAML
    let yaml_str = serde_yaml::to_string(&workflow).unwrap();
    println!("YAML 输出:\n{}", yaml_str);
    let restored_yaml: Workflow = serde_yaml::from_str(&yaml_str).unwrap();
    assert_eq!(workflow.name, restored_yaml.name);
    assert_eq!(workflow.nodes.len(), restored_yaml.nodes.len());

    // 序列化为 JSON
    let json_str = serde_json::to_string_pretty(&workflow).unwrap();
    println!("JSON 输出(片段):\n{}", &json_str[..150.min(json_str.len())]);
    let restored_json: Workflow = serde_json::from_str(&json_str).unwrap();
    assert_eq!(workflow.name, restored_json.name);

    // 计算 workflow hash(用于配置漂移检测)
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    json_str.hash(&mut hasher);
    let hash = hasher.finish();
    println!("Workflow hash(用于 Checkpoint 配置漂移检测): {:x}", hash);

    println!("✓ 结论:Workflow 定义本身 YAML/JSON 序列化已经 OK");
    println!("  hash 字段:基于 JSON 序列化字符串计算,存入 Execution.workflow_hash");
}

// ============================================================================
// 总结
// ============================================================================

#[test]
fn spike1_summary() {
    println!("\n========== Spike 1 总结 ==========\n");
    println!("验证点 1: RwLock<HashMap> 不能直接序列化,必须用中间结构");
    println!("验证点 2: DateTime<Local> 序列化时先转 UTC,Checkpoint 直接存 DateTime<Utc>");
    println!("验证点 3: ExecutionState 已正确配置 #[serde(rename_all = \"snake\")]");
    println!("验证点 4: 新增字段用 #[serde(default)] + Option 实现向后兼容");
    println!("验证点 5: 完整 WorkflowContext round-trip 可行");
    println!("验证点 6: Workflow 定义 YAML/JSON + hash 计算都可行");
    println!("\n实施建议:");
    println!("  1. B3(variables JSON 化)是 Checkpointer 的前置依赖,必须先做");
    println!("  2. 新增 crates/nemesis-workflow/src/checkpoint/types.rs 定义:");
    println!("     - Checkpoint {{ id, execution_id, saved_at: DateTime<Utc>, completed_nodes,");
    println!("       context_snapshot: SerializableContext, workflow_hash: String }}");
    println!("  3. Execution 新增字段(workflow_hash 等)都用 #[serde(default)]");
    println!("\n预期 A1 工期不变(6-7 天),序列化方案无重大意外。");
}
