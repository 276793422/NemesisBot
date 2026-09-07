//! H1 (2026-09-05): `todowrite` 工具测试。
//!
//! 覆盖计划验收三点：① 调用后 json 落盘正确；② 旧文件覆盖语义（全量
//! 替换）；③ TodoUpdated 广播可见（web pump 的上游）。另加：args 校验、
//! 安全管线放行（FileWrite 分类 passthrough）、tier 供给。

use super::*;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// 每测试唯一的临时 workspace（nanos 后缀避免并行碰撞）。
fn unique_workspace(tag: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("nb_h1_todo_{tag}_{nanos}"));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 3 项清单样例（覆盖三种状态）。
fn sample_todos_json() -> String {
    serde_json::json!({
        "todos": [
            {"content": "parse config", "status": "completed"},
            {"content": "implement parser", "status": "in_progress"},
            {"content": "write tests", "status": "pending"}
        ]
    })
    .to_string()
}

fn ctx(session_key: &str) -> RequestContext {
    RequestContext::new("web", "web:t1", "user", session_key)
}

fn tool_with(ws: &std::path::Path) -> TodoWriteTool {
    TodoWriteTool::new(ws.to_path_buf(), None)
}

fn todo_path(ws: &std::path::Path, session_key: &str) -> std::path::PathBuf {
    let safe = session_key.replace(':', "_");
    nemesis_path::resolve_sessions_dir_in_workspace(ws).join(format!("todo_{safe}.json"))
}

#[tokio::test]
async fn writes_json_to_sessions_dir() {
    let ws = unique_workspace("write");
    let tool = tool_with(&ws);
    let result = tool
        .execute(&sample_todos_json(), &ctx("agent:web:session:s1"))
        .await
        .expect("execute must succeed");
    assert!(
        result.contains("3 items"),
        "receipt mentions count: {result}"
    );

    let path = todo_path(&ws, "agent:web:session:s1");
    let body = std::fs::read_to_string(&path).expect("todo file written");
    let todos: Vec<nemesis_types::agent::TodoItem> =
        serde_json::from_str(&body).expect("valid todo json");
    assert_eq!(todos.len(), 3);
    assert_eq!(todos[0].content, "parse config");
    assert_eq!(
        todos[1].status,
        nemesis_types::agent::TodoStatus::InProgress
    );
    assert_eq!(todos[2].status, nemesis_types::agent::TodoStatus::Pending);
    // 原子写不留 tmp 残留。
    let tmp = path.with_extension("json.tmp");
    assert!(!tmp.exists(), "tmp file must be renamed away");
}

#[tokio::test]
async fn full_replacement_semantics() {
    let ws = unique_workspace("replace");
    let tool = tool_with(&ws);
    let key = "agent:web:session:rep";
    tool.execute(&sample_todos_json(), &ctx(key)).await.unwrap();

    // 第二次调用全量替换：旧三项消失，只剩新一项。
    let replacement = serde_json::json!({
        "todos": [{"content": "only task left", "status": "completed"}]
    })
    .to_string();
    tool.execute(&replacement, &ctx(key)).await.unwrap();

    let body = std::fs::read_to_string(todo_path(&ws, key)).unwrap();
    let todos: Vec<nemesis_types::agent::TodoItem> = serde_json::from_str(&body).unwrap();
    assert_eq!(todos.len(), 1, "second call replaces the whole list");
    assert_eq!(todos[0].content, "only task left");
}

#[tokio::test]
async fn invalid_status_rejected() {
    let ws = unique_workspace("badstatus");
    let tool = tool_with(&ws);
    let args = serde_json::json!({
        "todos": [{"content": "x", "status": "done"}]
    })
    .to_string();
    let err = tool
        .execute(&args, &ctx("agent:web:session:bad"))
        .await
        .expect_err("non-enum status must be rejected");
    assert!(err.contains("invalid todowrite args"), "{err}");
    // 拒绝时不落盘。
    assert!(!todo_path(&ws, "agent:web:session:bad").exists());
}

#[tokio::test]
async fn missing_todos_field_rejected() {
    let ws = unique_workspace("missing");
    let tool = tool_with(&ws);
    let err = tool
        .execute("{}", &ctx("agent:web:session:miss"))
        .await
        .expect_err("missing todos field must be rejected");
    assert!(err.contains("invalid todowrite args"), "{err}");
}

#[tokio::test]
async fn empty_list_clears_file() {
    let ws = unique_workspace("clear");
    let tool = tool_with(&ws);
    let key = "agent:web:session:clr";
    tool.execute(&sample_todos_json(), &ctx(key)).await.unwrap();
    tool.execute(r#"{"todos": []}"#, &ctx(key)).await.unwrap();
    let body = std::fs::read_to_string(todo_path(&ws, key)).unwrap();
    let todos: Vec<nemesis_types::agent::TodoItem> = serde_json::from_str(&body).unwrap();
    assert!(todos.is_empty(), "empty list = cleared, not skipped");
}

#[tokio::test]
async fn emits_todo_updated_event() {
    let ws = unique_workspace("event");
    let (tx, mut rx) = tokio::sync::broadcast::channel(4);
    let tool = TodoWriteTool::new(ws.to_path_buf(), Some(tx));
    let key = "agent:web:session:evt";
    tool.execute(&sample_todos_json(), &ctx(key)).await.unwrap();

    let ev = rx.recv().await.expect("event received");
    match ev {
        nemesis_types::agent::AgentEvent::TodoUpdated {
            session_key,
            chat_id,
            todos,
        } => {
            assert_eq!(session_key, key);
            assert_eq!(chat_id, "web:t1");
            assert_eq!(todos.len(), 3);
        }
        other => panic!("expected TodoUpdated, got {}", other.kind()),
    }
}

#[tokio::test]
async fn no_event_tx_still_persists() {
    let ws = unique_workspace("notx");
    let tool = tool_with(&ws); // event_tx = None：只落盘不广播
    let result = tool
        .execute(&sample_todos_json(), &ctx("agent:cli:session:nt"))
        .await;
    assert!(result.is_ok());
    assert!(todo_path(&ws, "agent:cli:session:nt").exists());
}

#[tokio::test]
async fn session_key_colons_sanitized_in_filename() {
    let ws = unique_workspace("sanitize");
    let tool = tool_with(&ws);
    tool.execute(&sample_todos_json(), &ctx("agent:web:session:a:b:c"))
        .await
        .unwrap();
    let path = todo_path(&ws, "agent:web:session:a:b:c");
    assert!(path.exists(), "colons replaced, single flat file: {path:?}");
    let name = path.file_name().unwrap().to_string_lossy();
    assert_eq!(
        name, "todo_agent_web_session_a_b_c.json",
        "sanitized name keeps todo_ prefix + .json suffix"
    );
}

/// 安全管线放行：todowrite 归 FileWrite（空 target 不匹配 ABAC 规则 →
/// default action）。默认配置下必须放行，否则工具在安全开启时不可用。
#[cfg(feature = "security")]
// multi_thread：管线 L7 scanner 层内部有 block_on（security crate 测试同款）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn security_pipeline_allows_todowrite() {
    let plugin = nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig {
            enabled: true,
            default_action: "allow".to_string(),
            ..Default::default()
        },
    );
    let invocation = nemesis_security::types::ToolInvocation {
        tool_name: "todowrite".to_string(),
        args: serde_json::from_str(&sample_todos_json()).unwrap(),
        user: "test".to_string(),
        source: "web".to_string(),
        metadata: Default::default(),
    };
    let (allowed, deny) = plugin.execute(&invocation);
    assert!(
        allowed,
        "default policy must allow todowrite: {:?}",
        deny.map(|d| d.summary)
    );
}

/// tier 供给：Normal 显式包含 todowrite；Mini 不含；Big/Auto 空表 = 全量。
#[test]
fn tier_supply_rules() {
    use nemesis_types::capability::ModelTier;
    use nemesis_types::capability::tier_allowed_tools;
    assert!(tier_allowed_tools(ModelTier::Normal).contains(&"todowrite"));
    assert!(!tier_allowed_tools(ModelTier::Mini).contains(&"todowrite"));
    assert!(tier_allowed_tools(ModelTier::Big).is_empty());
    assert!(tier_allowed_tools(ModelTier::Auto).is_empty());
}

/// 注册接线：SharedToolConfig.todo Some → todowrite 出现在注册表；
/// None（基线形态）→ 不注册。
#[test]
fn registration_gated_on_todo_config() {
    let mut config = SharedToolConfig::default();
    config.todo = Some(TodoToolConfig {
        workspace: unique_workspace("reg"),
        event_tx: None,
    });
    let tools = register_shared_tools(&config);
    assert!(tools.contains_key("todowrite"));

    let baseline = SharedToolConfig::default();
    let tools = register_shared_tools(&baseline);
    assert!(!tools.contains_key("todowrite"));
}

/// Arc<dyn Tool> 形态注册后 schema 可用（与 loop_tools 其它工具一致走
/// ToolDefinition 投影，这里直接验 trait object 下 description/parameters）。
#[test]
fn tool_metadata_via_trait_object() {
    let tool: Arc<dyn Tool> = Arc::new(TodoWriteTool::new(unique_workspace("meta"), None));
    let params = tool.parameters();
    assert_eq!(params["required"][0], "todos");
    let statuses = params["properties"]["todos"]["items"]["properties"]["status"]["enum"]
        .as_array()
        .unwrap();
    assert_eq!(statuses.len(), 3);
    assert!(tool.description().contains("FULL REPLACEMENT"));
}
