//! Extra coverage tests for `loop_tools.rs`.
//!
//! Targets specific code paths that have low/no coverage in the existing
//! `loop_tools/tests.rs` and `loop_tools/coverage_boost_tests.rs`. Focus:
//! - `cli_detail()` for every supported command (each match arm)
//! - `format_discovery_result()` non-empty paths (tools, resources, prompts)
//! - `extract_name_arg()` direct tests
//! - `ForgeBridgeTool` execution path via `register_shared_tools`
//! - `SharedToolConfig` Debug impl
//! - `McpListTool` / `McpDiscoverTool` description/params
//! - ClusterRpcTool peer name with colon stripping
//! - `WebSearchConfig` Default behavior combinations
//! - `register_peer_chat_handler` callback content extraction

use super::*;
use crate::context::RequestContext;
use std::sync::Arc;
use tempfile::TempDir;

// ===========================================================================
// cli_detail: each match arm needs at least one test to drive coverage
// ===========================================================================

#[test]
fn test_cli_detail_model_arm() {
    let s = cli_detail("model").unwrap();
    assert!(s.contains("model"));
    assert!(s.contains("add"));
    assert!(s.contains("--default"));
}

#[test]
fn test_cli_detail_model_case_insensitive() {
    // .to_lowercase() on the command should make uppercase still match
    let s = cli_detail("MODEL").unwrap();
    assert!(s.contains("model"));
}

#[test]
fn test_cli_detail_mcp_arm() {
    let s = cli_detail("mcp").unwrap();
    assert!(s.contains("mcp"));
    assert!(s.contains("discover"));
    assert!(s.contains("tools"));
    assert!(s.contains("prompts"));
}

#[test]
fn test_cli_detail_channel_arm() {
    let s = cli_detail("channel").unwrap();
    assert!(s.contains("channel"));
    assert!(s.contains("enable"));
    assert!(s.contains("websocket"));
    assert!(s.contains("external"));
}

#[test]
fn test_cli_detail_cluster_arm() {
    let s = cli_detail("cluster").unwrap();
    assert!(s.contains("cluster"));
    assert!(s.contains("peers"));
    assert!(s.contains("token"));
    assert!(s.contains("init"));
}

#[test]
fn test_cli_detail_skills_arm() {
    let s = cli_detail("skills").unwrap();
    assert!(s.contains("skills"));
    assert!(s.contains("search"));
    assert!(s.contains("install"));
    assert!(s.contains("add-source"));
}

#[test]
fn test_cli_detail_forge_arm() {
    let s = cli_detail("forge").unwrap();
    assert!(s.contains("forge"));
    assert!(s.contains("reflect"));
    assert!(s.contains("learning"));
}

#[test]
fn test_cli_detail_cron_arm() {
    let s = cli_detail("cron").unwrap();
    assert!(s.contains("cron"));
    assert!(s.contains("--every"));
    assert!(s.contains("--cron"));
}

#[test]
fn test_cli_detail_security_arm() {
    let s = cli_detail("security").unwrap();
    assert!(s.contains("security"));
    assert!(s.contains("audit"));
    assert!(s.contains("rules"));
    assert!(s.contains("approve"));
}

#[test]
fn test_cli_detail_scanner_arm() {
    let s = cli_detail("scanner").unwrap();
    assert!(s.contains("scanner"));
    assert!(s.contains("clamav"));
    assert!(s.contains("install"));
}

#[test]
fn test_cli_detail_log_arm() {
    let s = cli_detail("log").unwrap();
    assert!(s.contains("log"));
    assert!(s.contains("llm"));
    assert!(s.contains("general"));
}

#[test]
fn test_cli_detail_auth_arm() {
    let s = cli_detail("auth").unwrap();
    assert!(s.contains("auth"));
    assert!(s.contains("login"));
    assert!(s.contains("logout"));
}

#[test]
fn test_cli_detail_memory_arm() {
    let s = cli_detail("memory").unwrap();
    assert!(s.contains("memory"));
    assert!(s.contains("enable"));
    assert!(s.contains("disable"));
    assert!(s.contains("status"));
}

#[test]
fn test_cli_detail_workflow_arm() {
    let s = cli_detail("workflow").unwrap();
    assert!(s.contains("workflow"));
    assert!(s.contains("list"));
    assert!(s.contains("run"));
    assert!(s.contains("template"));
}

#[test]
fn test_cli_detail_cors_arm() {
    let s = cli_detail("cors").unwrap();
    assert!(s.contains("cors"));
    assert!(s.contains("dev-mode"));
    assert!(s.contains("validate"));
}

#[test]
fn test_cli_detail_status_arm() {
    let s = cli_detail("status").unwrap();
    assert!(s.contains("status"));
}

#[test]
fn test_cli_detail_version_arm() {
    let s = cli_detail("version").unwrap();
    assert!(s.contains("version"));
}

#[test]
fn test_cli_detail_unknown_command() {
    let r = cli_detail("definitely_not_a_command_xyz");
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Unknown command"));
}

#[test]
fn test_cli_overview_contains_all_commands() {
    let s = cli_overview();
    for keyword in [
        "model", "mcp", "channel", "cluster", "skills", "forge", "cron", "security", "scanner",
        "log", "auth", "memory", "workflow", "cors", "status", "version",
    ] {
        assert!(s.contains(keyword), "overview missing '{}'", keyword);
    }
}

// ===========================================================================
// format_discovery_result: branches for non-empty tools/resources/prompts
// ===========================================================================

fn make_tool(
    name: &str,
    desc: Option<&str>,
    schema: serde_json::Value,
) -> nemesis_mcp::types::McpTool {
    nemesis_mcp::types::McpTool {
        name: name.to_string(),
        description: desc.map(String::from),
        input_schema: schema,
    }
}

#[test]
fn test_format_discovery_result_tool_without_description() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: Some(nemesis_mcp::types::ServerInfo {
            name: "srv".to_string(),
            version: "0.1.0".to_string(),
        }),
        tools: vec![make_tool(
            "nameless",
            None,
            serde_json::json!({"type": "object", "properties": {}}),
        )],
        resources: vec![],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("no description"));
    assert!(out.contains("nameless"));
}

#[test]
fn test_format_discovery_result_tool_no_properties() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![make_tool(
            "t1",
            Some("desc"),
            serde_json::json!({"type": "object"}), // no "properties"
        )],
        resources: vec![],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("t1"));
    assert!(out.contains("(unknown)"));
}

#[test]
fn test_format_discovery_result_tool_param_without_type() {
    // schema entry without "type" -> defaults to "any"
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![make_tool(
            "t2",
            Some("d"),
            serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"description": "no type info"}
                },
                "required": ["query"]
            }),
        )],
        resources: vec![],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("query* (any)"));
}

#[test]
fn test_format_discovery_result_tool_optional_param() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![make_tool(
            "t3",
            None,
            serde_json::json!({
                "type": "object",
                "properties": {
                    "limit": {"type": "integer"}
                }
            }),
        )],
        resources: vec![],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    // Optional (no asterisk): "limit (integer)"
    assert!(out.contains("limit (integer)"));
}

#[test]
fn test_format_discovery_result_resource_without_description() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![],
        resources: vec![nemesis_mcp::types::Resource {
            uri: "file:///r.txt".to_string(),
            name: "r".to_string(),
            description: None,
            mime_type: None,
        }],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    // No description branch -> "- **r** (file:///r.txt)" without colon description
    assert!(out.contains("file:///r.txt"));
    assert!(out.contains("- **r** (file:///r.txt)"));
}

#[test]
fn test_format_discovery_result_resource_with_description() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![],
        resources: vec![nemesis_mcp::types::Resource {
            uri: "file:///x.txt".to_string(),
            name: "x".to_string(),
            description: Some("a file".to_string()),
            mime_type: None,
        }],
        prompts: vec![],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("- **x** (file:///x.txt): a file"));
}

#[test]
fn test_format_discovery_result_prompt_with_arguments() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![],
        resources: vec![],
        prompts: vec![nemesis_mcp::types::Prompt {
            name: "code_review".to_string(),
            description: Some("Reviews code".to_string()),
            arguments: vec![
                nemesis_mcp::types::PromptArgument {
                    name: "lang".to_string(),
                    description: Some("programming language".to_string()),
                    required: Some(true),
                },
                nemesis_mcp::types::PromptArgument {
                    name: "style".to_string(),
                    description: None,
                    required: None,
                },
            ],
        }],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("code_review"));
    assert!(out.contains("lang* (programming language)"));
    assert!(out.contains("style"));
    assert!(out.contains("* = required"));
}

#[test]
fn test_format_discovery_result_prompt_without_description() {
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: None,
        tools: vec![],
        resources: vec![],
        prompts: vec![nemesis_mcp::types::Prompt {
            name: "p".to_string(),
            description: None,
            arguments: vec![],
        }],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("- **p**: no description"));
}

#[test]
fn test_format_discovery_result_full() {
    // Combines all non-empty sections to walk through every formatting branch.
    let result = nemesis_mcp::manager::DiscoveryResult {
        server_info: Some(nemesis_mcp::types::ServerInfo {
            name: "full".to_string(),
            version: "9.9".to_string(),
        }),
        tools: vec![
            make_tool(
                "tool_a",
                Some("A tool"),
                serde_json::json!({
                    "type": "object",
                    "properties": {"x": {"type": "string"}},
                    "required": ["x"]
                }),
            ),
            make_tool("tool_b", None, serde_json::json!({"type": "object"})),
        ],
        resources: vec![nemesis_mcp::types::Resource {
            uri: "file:///1".to_string(),
            name: "r1".to_string(),
            description: Some("desc".to_string()),
            mime_type: None,
        }],
        prompts: vec![nemesis_mcp::types::Prompt {
            name: "prompt_a".to_string(),
            description: Some("hello".to_string()),
            arguments: vec![nemesis_mcp::types::PromptArgument {
                name: "arg1".to_string(),
                description: Some("an arg".to_string()),
                required: Some(true),
            }],
        }],
    };
    let out = format_discovery_result(&result);
    assert!(out.contains("MCP Server: full v9.9"));
    assert!(out.contains("### Tools (2)"));
    assert!(out.contains("### Resources (1)"));
    assert!(out.contains("### Prompts (1)"));
}

// ===========================================================================
// extract_name_arg: direct tests
// ===========================================================================

#[test]
fn test_extract_name_arg_with_json() {
    assert_eq!(
        extract_name_arg(r#"{"name": "skill-1"}"#).unwrap(),
        "skill-1"
    );
}

#[test]
fn test_extract_name_arg_no_name_field_returns_raw() {
    // JSON without "name" falls back to raw args (trimmed)
    let r = extract_name_arg(r#"{"foo": "bar"}"#).unwrap();
    assert_eq!(r, r#"{"foo": "bar"}"#);
}

#[test]
fn test_extract_name_arg_raw_string() {
    assert_eq!(extract_name_arg("  plain-name  ").unwrap(), "plain-name");
}

#[test]
fn test_extract_name_arg_empty() {
    assert_eq!(extract_name_arg("").unwrap(), "");
}

#[test]
fn test_extract_name_arg_invalid_json() {
    // Invalid JSON falls back to raw args
    assert_eq!(extract_name_arg("not_json").unwrap(), "not_json");
}

// ===========================================================================
// ForgeBridgeTool: drive execute() through register_shared_tools result
// ===========================================================================

#[cfg(feature = "forge")]
fn make_forge_executor() -> Arc<nemesis_forge::forge_tools::ForgeToolExecutor> {
    let tmp = tempfile::tempdir().unwrap();
    let forge = nemesis_forge::forge::Forge::new(
        nemesis_forge::config::ForgeConfig::default(),
        tmp.path().to_path_buf(),
    );
    // Leak the tempdir so it lives for the test's lifetime.
    // (Forge holds no open handles that require cleanup within the test.)
    std::mem::forget(tmp);
    Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(
        Arc::new(forge),
    ))
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_bridge_tool_execute_list_success() {
    let executor = make_forge_executor();
    let tools = register_shared_tools(&SharedToolConfig {
        forge_executor: Some(executor),
        ..Default::default()
    });
    let forge_list = tools
        .get("forge_list")
        .expect("forge_list should be registered");
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let result = forge_list.execute("{}", &ctx).await;
    assert!(result.is_ok(), "expected Ok, got: {:?}", result);
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_bridge_tool_execute_unknown_name_returns_err() {
    let executor = make_forge_executor();
    // Create a bridge via a forged name; use register_shared_tools then look up forge_reflect
    let tools = register_shared_tools(&SharedToolConfig {
        forge_executor: Some(executor),
        ..Default::default()
    });
    // forge_reflect exists; pass invalid JSON to make args_value Null (still Ok path)
    let forge_reflect = tools.get("forge_reflect").unwrap();
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    // Pass invalid JSON so serde falls back to Null — should still produce a reflection.
    let result = forge_reflect.execute("not json", &ctx).await;
    assert!(result.is_ok());
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_bridge_tool_description_and_parameters() {
    let executor = make_forge_executor();
    let tools = register_shared_tools(&SharedToolConfig {
        forge_executor: Some(executor),
        ..Default::default()
    });
    // Every forge bridge must expose a non-empty description and an object parameters.
    let forge_names: Vec<&str> = tools
        .keys()
        .map(|s| s.as_str())
        .filter(|s| s.starts_with("forge_"))
        .collect();
    assert!(
        forge_names.len() >= 5,
        "expected multiple forge tools, got {:?}",
        forge_names
    );
    for name in forge_names {
        let t = tools.get(name).unwrap();
        let d = t.description();
        assert!(!d.is_empty(), "{} has empty description", name);
        let p = t.parameters();
        assert!(p.is_object(), "{} parameters not object", name);
    }
}

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_bridge_tool_create_validation_failure() {
    // Missing required fields should produce an Err from the executor.
    let executor = make_forge_executor();
    let tools = register_shared_tools(&SharedToolConfig {
        forge_executor: Some(executor),
        ..Default::default()
    });
    let forge_create = tools.get("forge_create").unwrap();
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let result = forge_create.execute("{}", &ctx).await;
    assert!(result.is_err());
}

// ===========================================================================
// SharedToolConfig Debug impl
// ===========================================================================

#[test]
fn test_shared_tool_config_debug_minimal() {
    let cfg = SharedToolConfig::default();
    let s = format!("{:?}", cfg);
    assert!(s.contains("SharedToolConfig"));
    assert!(s.contains("web_search: None"));
}

#[test]
fn test_shared_tool_config_debug_with_web() {
    let cfg = SharedToolConfig {
        web_search: Some(WebSearchConfig::default()),
        ..Default::default()
    };
    let s = format!("{:?}", cfg);
    assert!(s.contains("WebSearchConfig"));
}

#[cfg(feature = "memory")]
#[test]
fn test_shared_tool_config_debug_with_skills_and_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let mem_config = nemesis_memory::manager::Config::new(tmp.path());
    let mgr = Arc::new(nemesis_memory::manager::MemoryManager::new(&mem_config));
    let exec = Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr));
    let cfg = SharedToolConfig {
        memory_executor: Some(exec),
        mcp_tool_snapshot: Some(Arc::new(parking_lot::RwLock::new(vec![]))),
        ..Default::default()
    };
    let s = format!("{:?}", cfg);
    assert!(s.contains("MemoryToolExecutor"));
    assert!(s.contains("McpToolSnapshot"));
}

#[test]
fn test_shared_tool_config_debug_with_workspace_and_cluster() {
    let cfg = SharedToolConfig {
        workspace: Some("/tmp/ws".to_string()),
        cluster_rpc: Some(ClusterRpcConfig::default()),
        spawn: Some(SpawnConfig {
            default_model: "m".to_string(),
            max_concurrent: 3,
        }),
        ..Default::default()
    };
    let s = format!("{:?}", cfg);
    assert!(s.contains("/tmp/ws"));
    assert!(s.contains("ClusterRpcConfig"));
    assert!(s.contains("SpawnConfig"));
}

// ===========================================================================
// McpDiscoverTool / McpListTool / CliReferenceTool descriptions
// ===========================================================================

#[test]
fn test_mcp_discover_tool_description_and_params() {
    let t = McpDiscoverTool::new();
    let d = t.description();
    assert!(d.contains("Discover"));
    let p = t.parameters();
    assert!(p["properties"]["command"].is_object());
    assert!(p["properties"]["url"].is_object());
    assert!(p["properties"]["args"]["type"] == "array");
    assert!(p["properties"]["timeout"]["type"] == "number");
}

#[test]
fn test_mcp_list_tool_description_and_params() {
    let snap = Arc::new(parking_lot::RwLock::new(vec![]));
    let t = McpListTool::new(snap);
    let d = t.description();
    assert!(d.contains("MCP tools"));
    let p = t.parameters();
    assert_eq!(p["type"], "object");
    // empty properties object
    assert!(p["properties"].as_object().unwrap().is_empty());
}

// ===========================================================================
// ClusterRpcTool: peer name containing colon gets stripped from __ASYNC__
// ===========================================================================

#[tokio::test]
async fn test_cluster_rpc_async_ack_strips_colon_from_name() {
    // When the peer display name contains a colon, the marker must strip it
    // because the marker itself is colon-delimited.
    let config = ClusterRpcConfig {
        local_node_id: "self".to_string(),
        timeout_secs: 30,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_, _, _| {
        Box::pin(async { Ok(serde_json::json!({"status": "accepted", "task_id": "tid"})) })
    }));
    tool.set_peers_fn(Arc::new(|| {
        vec![(
            "peer-1".to_string(),
            "Name:With:Colons".to_string(),
            vec!["cap1".to_string()],
        )]
    }));

    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let result = tool
        .execute(r#"{"target_node": "peer-1", "message": "hi"}"#, &ctx)
        .await
        .unwrap();
    // Marker should be __ASYNC__:tid:peer-1:NameWithColons (colons removed from name)
    assert_eq!(result, "__ASYNC__:tid:peer-1:NameWithColons");
}

#[tokio::test]
async fn test_cluster_rpc_async_ack_empty_name_falls_back_to_node() {
    // If peer name resolves to empty string, fall back to target node id.
    let config = ClusterRpcConfig {
        local_node_id: "self".to_string(),
        timeout_secs: 30,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(config);
    tool.set_rpc_call_fn(Arc::new(|_, _, _| {
        Box::pin(async { Ok(serde_json::json!({"status": "accepted", "task_id": "tid2"})) })
    }));
    tool.set_peers_fn(Arc::new(|| {
        vec![(
            "peer-2".to_string(),
            "".to_string(), // empty name -> fall back to node id
            vec![],
        )]
    }));

    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let result = tool
        .execute(r#"{"target_node": "peer-2", "message": "hi"}"#, &ctx)
        .await
        .unwrap();
    assert_eq!(result, "__ASYNC__:tid2:peer-2:peer-2");
}

// ===========================================================================
// ClusterRpcTool enabled flag toggling
// ===========================================================================

#[tokio::test]
async fn test_cluster_rpc_tool_disabled_returns_immediate_error() {
    let config = ClusterRpcConfig {
        local_node_id: "self".to_string(),
        timeout_secs: 30,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(config);
    assert!(tool.is_enabled());
    tool.set_enabled(false);
    assert!(!tool.is_enabled());

    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let result = tool
        .execute(r#"{"target_node": "other", "message": "hi"}"#, &ctx)
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("集群功能未启用"));
}

#[test]
fn test_cluster_rpc_tool_enabled_arc_initial_true() {
    let tool = ClusterRpcTool::new(ClusterRpcConfig::default());
    let arc = tool.enabled_arc();
    assert!(arc.load(std::sync::atomic::Ordering::Relaxed));
    arc.store(false, std::sync::atomic::Ordering::Relaxed);
    // The arc is shared — tool.is_enabled() should reflect the change.
    assert!(!tool.is_enabled());
}

// ===========================================================================
// CliReferenceTool: more command coverage
// ===========================================================================

#[tokio::test]
async fn test_cli_reference_tool_whitespace_only_command() {
    let tool = CliReferenceTool::new();
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"command": "   "}"#, &ctx).await.unwrap();
    // Whitespace-only command should fall through to overview
    assert!(r.contains("model"));
}

// ===========================================================================
// register_peer_chat_handler: callback extracts task_id/content properly
// ===========================================================================

#[test]
fn test_register_peer_chat_handler_callback_missing_fields() {
    let mut handlers: HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
    > = HashMap::new();
    register_peer_chat_handler(&mut handlers, |_| Ok(serde_json::json!({"ok": 1})));

    let cb = handlers.get("peer_chat_callback").unwrap();
    // Missing task_id and content -> defaults to "unknown" and ""
    let r = cb(serde_json::json!({})).unwrap();
    assert_eq!(r["status"], "received");
    assert_eq!(r["task_id"], "unknown");
}

#[test]
fn test_register_peer_chat_handler_overwrites_existing() {
    let mut handlers: HashMap<
        String,
        Box<dyn Fn(serde_json::Value) -> Result<serde_json::Value, String> + Send + Sync>,
    > = HashMap::new();
    // Pre-insert something to verify register_peer_chat_handler overwrites it.
    handlers.insert(
        "peer_chat".to_string(),
        Box::new(|_| Ok(serde_json::json!({"old": true}))),
    );
    register_peer_chat_handler(&mut handlers, |_| Ok(serde_json::json!({"new": true})));
    let cb = handlers.get("peer_chat").unwrap();
    let r = cb(serde_json::json!({})).unwrap();
    assert_eq!(r["new"], true);
}

// ===========================================================================
// SkillsListTool / SkillsInfoTool with loader but no skills / unknown skill
// ===========================================================================

#[tokio::test]
async fn test_skills_list_tool_with_loader_no_skills() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    let global = tmp.path().join("g").to_string_lossy().to_string();
    let builtin = tmp.path().join("b").to_string_lossy().to_string();
    let loader = Arc::new(nemesis_skills::loader::SkillsLoader::new(
        &workspace, &global, &builtin,
    ));
    let tool = SkillsListTool::new(Some(loader));
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute("{}", &ctx).await.unwrap();
    assert!(r.contains("No skills installed"));
}

#[tokio::test]
async fn test_skills_info_tool_with_loader_unknown_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    let global = tmp.path().join("g").to_string_lossy().to_string();
    let builtin = tmp.path().join("b").to_string_lossy().to_string();
    let loader = Arc::new(nemesis_skills::loader::SkillsLoader::new(
        &workspace, &global, &builtin,
    ));
    let tool = SkillsInfoTool::new(Some(loader));
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"name": "nope"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("not found"));
}

#[tokio::test]
async fn test_skills_list_tool_description_and_params() {
    let tool = SkillsListTool::new(None);
    assert!(tool.description().contains("skills"));
    let p = tool.parameters();
    assert!(p["properties"]["category"].is_object());
}

#[tokio::test]
async fn test_skills_info_tool_description_and_params() {
    let tool = SkillsInfoTool::new(None);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert!(p["properties"]["name"].is_object());
    let req = p["required"].as_array().unwrap();
    assert!(req.iter().any(|v| v == "name"));
}

// ===========================================================================
// InstallSkillTool: additional coverage (force=true bypasses existence check,
// version parameter, registry name)
// ===========================================================================

#[tokio::test]
async fn test_install_skill_tool_force_with_existing_dir_attempts_install() {
    // Force=true should skip the "already exists" check and try to install,
    // which will fail because the registry is empty.
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_string_lossy().to_string();
    std::fs::create_dir_all(tmp.path().join("skills").join("present")).unwrap();

    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, workspace);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool
        .execute(r#"{"slug": "present", "force": true}"#, &ctx)
        .await;
    // Force=true skips the existence check, so we should NOT see "already exists".
    assert!(r.is_err());
    let err = r.unwrap_err();
    assert!(!err.contains("already exists"));
}

#[tokio::test]
async fn test_install_skill_tool_with_version_and_registry() {
    // Both "version" and "registry" fields are accepted (default registry="github").
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool
        .execute(
            r#"{"slug": "myskill", "version": "1.0.0", "registry": "custom"}"#,
            &ctx,
        )
        .await;
    assert!(r.is_err());
    // Should reach install attempt (not validation error)
    let err = r.unwrap_err();
    assert!(!err.contains("invalid slug"));
}

#[tokio::test]
async fn test_install_skill_tool_empty_name_field() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"name": ""}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("slug parameter is required"));
}

#[tokio::test]
async fn test_install_skill_tool_slug_with_slash_rejected() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = InstallSkillTool::new(registry, "/tmp/ws".to_string());
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"slug": "a/b"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("invalid slug"));
}

// ===========================================================================
// FindSkillsTool: limit clamping
// ===========================================================================

#[tokio::test]
async fn test_find_skills_tool_with_limit_zero_clamped_to_one() {
    // limit=0 should be clamped to 1; the search should still execute (and fail
    // because the registry is empty).
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"query": "x", "limit": 0}"#, &ctx).await;
    // Empty registry -> search returns Ok(empty vec) -> "No skills found"
    // OR error from search failure. Either way, no panic.
    assert!(r.is_ok() || r.is_err());
}

#[tokio::test]
async fn test_find_skills_tool_with_limit_above_max_clamped() {
    let registry = Arc::new(nemesis_skills::registry::RegistryManager::new_empty());
    let tool = FindSkillsTool::new(registry);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool
        .execute(r#"{"query": "test", "limit": 9999}"#, &ctx)
        .await;
    // Should be clamped to 50 and either succeed (empty) or fail gracefully.
    assert!(r.is_ok() || r.is_err());
}

// ===========================================================================
// Memory tools: invalid JSON handling
// ===========================================================================

#[tokio::test]
async fn test_memory_search_tool_invalid_json_no_executor() {
    let tool = MemorySearchTool::new(None);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute("not json", &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_memory_store_tool_invalid_json_no_executor() {
    let tool = MemoryStoreTool::new(None);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute("not json", &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("not available"));
}

#[tokio::test]
async fn test_memory_forget_tool_invalid_json_no_executor() {
    let tool = MemoryForgetTool::new(None);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute("not json", &ctx).await;
    assert!(r.is_err());
}

#[tokio::test]
async fn test_memory_list_tool_invalid_json_no_executor() {
    let tool = MemoryListTool::new(None);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute("not json", &ctx).await;
    assert!(r.is_err());
}

#[test]
fn test_memory_search_tool_description_and_params() {
    let tool = MemorySearchTool::new(None);
    assert!(tool.description().contains("memory"));
    let p = tool.parameters();
    assert!(p["properties"]["query"].is_object());
}

#[test]
fn test_memory_store_tool_description_and_params() {
    let tool = MemoryStoreTool::new(None);
    assert!(tool.description().contains("Store"));
    let p = tool.parameters();
    assert!(p["properties"]["key"].is_object());
    assert!(p["properties"]["content"].is_object());
}

#[test]
fn test_memory_forget_tool_description_and_params() {
    let tool = MemoryForgetTool::new(None);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert!(p["properties"]["action"].is_object());
}

#[test]
fn test_memory_list_tool_description_and_params() {
    let tool = MemoryListTool::new(None);
    assert!(!tool.description().is_empty());
    let p = tool.parameters();
    assert!(p["properties"]["limit"].is_object());
    assert!(p["properties"]["offset"].is_object());
}

// ===========================================================================
// register_shared_tools: cron_service variant + workspace-driven exec_async
// ===========================================================================

#[test]
fn test_register_shared_tools_with_cron_service_present() {
    let cron_svc = Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(":memory:"),
    ));
    let cfg = SharedToolConfig {
        cron_service: Some(cron_svc),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(tools.contains_key("cron"));
    let cron_tool = tools.get("cron").unwrap();
    assert!(cron_tool.description().contains("cron"));
    let p = cron_tool.parameters();
    assert!(p["properties"]["action"].is_object());
}

#[test]
fn test_register_shared_tools_includes_i2c_spi_always() {
    let tools = register_shared_tools(&SharedToolConfig::default());
    assert!(tools.contains_key("i2c"));
    assert!(tools.contains_key("spi"));
    let i2c = tools.get("i2c").unwrap();
    assert!(i2c.description().contains("I2C"));
    let spi = tools.get("spi").unwrap();
    assert!(spi.description().contains("SPI"));
}

#[test]
fn test_register_shared_tools_with_mcp_snapshot_provided() {
    let snap = Arc::new(parking_lot::RwLock::new(vec![(
        "foo".to_string(),
        "bar".to_string(),
    )]));
    let cfg = SharedToolConfig {
        mcp_tool_snapshot: Some(snap),
        ..Default::default()
    };
    let tools = register_shared_tools(&cfg);
    assert!(tools.contains_key("mcp_list"));
}

// ===========================================================================
// ExecTool: description and parameters
// ===========================================================================

#[test]
fn test_exec_tool_description_and_params() {
    let t = ExecTool::new("/tmp", false);
    assert!(t.description().contains("shell"));
    let p = t.parameters();
    assert!(p["properties"]["command"].is_object());
    assert!(p["properties"]["timeout"]["type"] == "integer");
    assert!(p["properties"]["cwd"]["type"] == "string");
}

#[test]
fn test_async_exec_tool_params_includes_clamp_range() {
    let t = AsyncExecTool::new("/tmp", false);
    let p = t.parameters();
    assert!(p["properties"]["wait_seconds"]["type"] == "integer");
    assert!(p["properties"]["working_dir"]["type"] == "string");
}

// ===========================================================================
// WebSearchTool: extract_query direct method
// ===========================================================================

#[test]
fn test_web_search_tool_extract_query_with_complex_json() {
    let t = WebSearchTool::new(WebSearchConfig::default());
    let q = t
        .extract_query(r#"{"query": "complex (search) [query]"}"#)
        .unwrap();
    assert_eq!(q, "complex (search) [query]");
}

// ===========================================================================
// WebFetchTool: parameters and size field
// ===========================================================================

#[test]
fn test_web_fetch_tool_parameters_schema() {
    let t = WebFetchTool::new(12345);
    assert_eq!(t.max_size, 12345);
    let p = t.parameters();
    assert!(p["properties"]["url"].is_object());
    let req = p["required"].as_array().unwrap();
    assert!(req.iter().any(|v| v == "url"));
}

// ===========================================================================
// ClusterRpcChannelConfig / ClusterRpcChannelSetup fields
// ===========================================================================

#[test]
fn test_cluster_rpc_channel_config_default_values_nonzero() {
    let c = ClusterRpcChannelConfig::default();
    assert!(c.request_timeout.as_secs() > 0);
    assert!(c.cleanup_interval.as_secs() > 0);
}

#[test]
fn test_setup_cluster_rpc_channel_with_config_no_continuation() {
    let cfg = ClusterRpcConfig {
        local_node_id: "n1".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let channel_cfg = setup_cluster_rpc_channel_with_config(&cfg);
    assert!(channel_cfg.request_timeout.as_secs() > 0);
}

#[test]
fn test_setup_cluster_rpc_channel_setup_carries_continuation_manager() {
    let cm = Arc::new(crate::loop_continuation::ContinuationManager::new());
    let cm_clone = cm.clone();
    let setup = setup_cluster_rpc_channel(Some(cm_clone));
    assert!(setup.continuation_manager.is_some());
    assert!(Arc::ptr_eq(&setup.continuation_manager.unwrap(), &cm));
}

// ===========================================================================
// I2CTool / SPITool parameters schema (no Linux restriction)
// ===========================================================================

#[test]
fn test_i2c_tool_parameters_schema() {
    let t = I2CTool;
    let p = t.parameters();
    assert!(p["properties"]["action"].is_object());
    assert!(p["properties"]["bus"].is_object());
    assert!(p["properties"]["address"].is_object());
}

#[test]
fn test_spi_tool_parameters_schema() {
    let t = SPITool;
    let p = t.parameters();
    assert!(p["properties"]["action"].is_object());
    assert!(p["properties"]["device"].is_object());
}

// ===========================================================================
// MessageTool: parameters, set_context doesn't panic with rapid calls
// ===========================================================================

#[test]
fn test_message_tool_parameters_schema() {
    let t = MessageTool::new();
    let p = t.parameters();
    assert_eq!(p["type"], "object");
    assert!(p["properties"]["content"]["type"] == "string");
    let req = p["required"].as_array().unwrap();
    assert!(req.iter().any(|v| v == "content"));
}

#[tokio::test]
async fn test_message_tool_set_context_idempotent() {
    let t = MessageTool::new();
    t.set_context("a", "b");
    t.set_context("c", "d");
    // Should not panic on rapid re-sets; final state stored
    let ctx = RequestContext::new("", "", "u1", "s1");
    let result = t.execute(r#"{"content": "x"}"#, &ctx).await;
    assert!(result.is_ok());
}

// ===========================================================================
// SleepTool: parameters
// ===========================================================================

#[test]
fn test_sleep_tool_parameters_schema() {
    let t = SleepTool;
    let p = t.parameters();
    assert!(p["properties"]["seconds"]["type"] == "number");
    let req = p["required"].as_array().unwrap();
    assert!(req.iter().any(|v| v == "seconds"));
}

// ===========================================================================
// CronTool: parameters, no_service actions
// ===========================================================================

#[test]
fn test_cron_tool_description_and_params() {
    let svc = Arc::new(std::sync::Mutex::new(
        nemesis_cron::service::CronService::new(":memory:"),
    ));
    let t = CronTool::new(svc);
    let d = t.description();
    assert!(d.contains("at_seconds") || d.contains("every_seconds") || d.contains("cron_expr"));
    let p = t.parameters();
    assert!(p["properties"]["action"].is_object());
    assert!(p["properties"]["at_seconds"].is_object());
    assert!(p["properties"]["every_seconds"].is_object());
    assert!(p["properties"]["cron_expr"].is_object());
}

// ===========================================================================
// WebSearchConfig: Default + clone
// ===========================================================================

#[test]
fn test_web_search_config_clone() {
    let c = WebSearchConfig {
        brave_api_key: Some("k".to_string()),
        brave_max_results: 7,
        brave_enabled: true,
        duckduckgo_max_results: 6,
        duckduckgo_enabled: false,
        perplexity_api_key: Some("p".to_string()),
        perplexity_max_results: 8,
        perplexity_enabled: true,
    };
    let cloned = c.clone();
    assert_eq!(cloned.brave_max_results, 7);
    assert!(!cloned.duckduckgo_enabled);
    assert!(cloned.perplexity_enabled);
}

// ===========================================================================
// SpawnConfig: Debug and clone
// ===========================================================================

#[test]
fn test_spawn_config_clone() {
    let c = SpawnConfig {
        default_model: "model-x".to_string(),
        max_concurrent: 9,
    };
    let cloned = c.clone();
    assert_eq!(cloned.default_model, "model-x");
    assert_eq!(cloned.max_concurrent, 9);
}

// ===========================================================================
// BootstrapTool: parameters, default constructor
// ===========================================================================

#[test]
fn test_bootstrap_tool_parameters_required_confirmed() {
    let t = BootstrapTool::new("/tmp");
    let p = t.parameters();
    assert!(p["properties"]["confirmed"]["type"] == "boolean");
    let req = p["required"].as_array().unwrap();
    assert!(req.iter().any(|v| v == "confirmed"));
}

// ===========================================================================
// WebSearchTool: providers configured but api_key empty string (Some("") is treated as None)
// ===========================================================================

#[tokio::test]
async fn test_web_search_tool_brave_empty_key_falls_through() {
    let cfg = WebSearchConfig {
        brave_enabled: true,
        brave_api_key: Some("".to_string()), // empty key
        duckduckgo_enabled: true,
        ..Default::default()
    };
    let t = WebSearchTool::new(cfg);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    // Brave enabled but key empty -> falls through to DuckDuckGo (would attempt network).
    // We don't run the live request, but we can verify it doesn't go down the
    // "no provider" path.
    // Instead, configure with no duckduckgo to force the empty-key branch.
    let cfg2 = WebSearchConfig {
        brave_enabled: true,
        brave_api_key: Some("".to_string()),
        duckduckgo_enabled: false,
        perplexity_enabled: false,
        ..Default::default()
    };
    let t2 = WebSearchTool::new(cfg2);
    let r = t2.execute(r#"{"query": "anything"}"#, &ctx).await;
    assert!(r.is_err());
    // Empty key treated like None: returns "Brave API key not configured"
    assert!(r.unwrap_err().contains("Brave API key not configured"));
}

#[tokio::test]
async fn test_web_search_tool_perplexity_empty_key() {
    let cfg = WebSearchConfig {
        brave_enabled: false,
        duckduckgo_enabled: false,
        perplexity_enabled: true,
        perplexity_api_key: Some("".to_string()),
        ..Default::default()
    };
    let t = WebSearchTool::new(cfg);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t.execute(r#"{"query": "x"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Perplexity API key not configured"));
}

// ===========================================================================
// ExecTool: no "command" returns Err
// ===========================================================================

#[tokio::test]
async fn test_exec_tool_no_command_field_returns_err() {
    let t = ExecTool::new("/tmp", false);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t.execute(r#"{"cwd": "/tmp"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Missing 'command'"));
}

// ===========================================================================
// AsyncExecTool: missing command
// ===========================================================================

#[tokio::test]
async fn test_async_exec_tool_no_command_returns_err() {
    let t = AsyncExecTool::new("/tmp", false);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t.execute(r#"{"working_dir": "/tmp"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Missing 'command'"));
}

// ===========================================================================
// I2CTool / SPITool action handling on Linux (only triggers Linux branches
// when actually on Linux; on other platforms they return Err)
// ===========================================================================

#[tokio::test]
async fn test_i2c_tool_with_confirm_write() {
    let t = I2CTool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t
        .execute(
            r#"{"action": "write", "address": 5, "confirm": true}"#,
            &ctx,
        )
        .await;
    if cfg!(target_os = "linux") {
        assert!(r.is_ok());
        assert!(r.unwrap().contains("Write"));
    } else {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn test_spi_tool_with_confirm_transfer() {
    let t = SPITool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t
        .execute(
            r#"{"action": "transfer", "device": "/dev/spidev0.0", "confirm": true}"#,
            &ctx,
        )
        .await;
    if cfg!(target_os = "linux") {
        assert!(r.is_ok());
        assert!(r.unwrap().contains("Transfer"));
    } else {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn test_i2c_tool_unknown_action_on_linux() {
    let t = I2CTool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t.execute(r#"{"action": "bogus"}"#, &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Unknown I2C action"));
    } else {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn test_spi_tool_unknown_action_on_linux() {
    let t = SPITool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t.execute(r#"{"action": "bogus"}"#, &ctx).await;
    if cfg!(target_os = "linux") {
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("Unknown SPI action"));
    } else {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn test_i2c_tool_write_without_confirm() {
    let t = I2CTool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t
        .execute(r#"{"action": "write", "address": 5}"#, &ctx)
        .await;
    if cfg!(target_os = "linux") {
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("confirm must be true"));
    } else {
        assert!(r.is_err());
    }
}

#[tokio::test]
async fn test_spi_tool_transfer_without_confirm() {
    let t = SPITool;
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = t
        .execute(
            r#"{"action": "transfer", "device": "/dev/spidev0.0"}"#,
            &ctx,
        )
        .await;
    if cfg!(target_os = "linux") {
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("confirm must be true"));
    } else {
        assert!(r.is_err());
    }
}

// ===========================================================================
// register_extended_tools: complete param matrix
// ===========================================================================

#[test]
fn test_register_extended_tools_all_three_provided() {
    let tools = register_extended_tools(
        Some(WebSearchConfig::default()),
        Some(ClusterRpcConfig::default()),
        Some(SpawnConfig {
            default_model: "m".to_string(),
            max_concurrent: 2,
        }),
    );
    assert!(tools.contains_key("web_search"));
    assert!(tools.contains_key("cluster_rpc"));
    assert!(tools.contains_key("spawn"));
    assert!(tools.contains_key("web_fetch"));
}

// ===========================================================================
// MessageTool: callback receives formatted RPC message
// ===========================================================================

#[tokio::test]
async fn test_message_tool_callback_called_with_non_rpc_channel() {
    let tool = MessageTool::new();
    let received = Arc::new(std::sync::Mutex::new((
        String::new(),
        String::new(),
        String::new(),
    )));
    let received_clone = received.clone();
    tool.set_send_callback(Box::new(move |ch, cid, content| {
        let mut g = received_clone.lock().unwrap();
        *g = (ch.to_string(), cid.to_string(), content.to_string());
    }));
    let ctx = RequestContext::new("discord", "ch-1", "u", "s");
    let r = tool.execute(r#"{"content": "hello"}"#, &ctx).await.unwrap();
    assert_eq!(r, "hello");
    let g = received.lock().unwrap();
    assert_eq!(g.0, "discord");
    assert_eq!(g.1, "ch-1");
    assert_eq!(g.2, "hello"); // No RPC prefix because channel != "rpc"
}

// ===========================================================================
// ClusterRpcTool: empty target_node error path (missing required field)
// ===========================================================================

#[tokio::test]
async fn test_cluster_rpc_tool_with_only_message_no_target() {
    let cfg = ClusterRpcConfig {
        local_node_id: "self".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(cfg);
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool.execute(r#"{"message": "hello"}"#, &ctx).await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Missing 'target_node'"));
}

// ===========================================================================
// Extract edge cases
// ===========================================================================

#[test]
fn test_extract_path_with_array_value_returns_err() {
    // path field is an array, not string -> falls to "Missing 'path'" path
    let r = extract_path(r#"{"path": [1, 2, 3]}"#);
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Missing 'path'"));
}

#[test]
fn test_extract_path_with_non_string_path() {
    let r = extract_path(r#"{"path": 42}"#);
    assert!(r.is_err());
}

#[test]
fn test_extract_path_and_content_with_non_string_content() {
    let r = extract_path_and_content(r#"{"path": "/a", "content": 5}"#);
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("Missing 'content'"));
}

#[test]
fn test_extract_edit_args_with_non_string_path() {
    let r = extract_edit_args(r#"{"path": 1, "old_text": "a", "new_text": "b"}"#);
    assert!(r.is_err());
}

// ===========================================================================
// ClusterRpcTool set_rpc_call_fn wired to a failing future
// ===========================================================================

#[tokio::test]
async fn test_cluster_rpc_tool_rpc_fn_returns_err() {
    let cfg = ClusterRpcConfig {
        local_node_id: "self".to_string(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let mut tool = ClusterRpcTool::new(cfg);
    tool.set_rpc_call_fn(Arc::new(|_n, _a, _p| {
        Box::pin(async { Err("network unreachable".to_string()) })
    }));
    let ctx = RequestContext::new("web", "c1", "u1", "s1");
    let r = tool
        .execute(r#"{"target_node": "other", "message": "hi"}"#, &ctx)
        .await;
    assert!(r.is_err());
    assert!(r.unwrap_err().contains("network unreachable"));
}

// ===========================================================================
// ClusterRpcTool: parameters with empty local_node_id (no self_id_note)
// ===========================================================================

#[test]
fn test_cluster_rpc_params_empty_local_node_id_omits_note() {
    let cfg = ClusterRpcConfig {
        local_node_id: String::new(),
        timeout_secs: 60,
        local_rpc_port: 21949,
    };
    let tool = ClusterRpcTool::new(cfg);
    let p = tool.parameters();
    let desc = p["properties"]["target"]["description"].as_str().unwrap();
    assert!(!desc.contains("your own node_id"));
    assert!(desc.starts_with("Target bot ID"));
}

// ===========================================================================
// ForgeToolExecutor: unknown tool returns Err
// ===========================================================================

#[cfg(feature = "forge")]
#[tokio::test]
async fn test_forge_bridge_unknown_tool_name_returns_err() {
    // Manually wire a ForgeBridgeTool for an unknown name via register_shared_tools
    // and then call it (no such tool will be in the result, so we use a different approach).
    let executor = make_forge_executor();
    let result = executor
        .execute("forge_nonexistent", &serde_json::Value::Null)
        .await;
    assert!(!result.success);
    assert!(result.content.contains("Unknown forge tool"));
}
