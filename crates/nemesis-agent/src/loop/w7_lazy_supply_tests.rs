//! W7（2026-10-25 加固）：
//! MCP 工具惰性供给测试——供给闸（lazy on →
//! tool_defs 无 mcp_* 有 mcp_call；tier 白名单放行 meta 工具）、dispatch
//! 解包（真实名重走瀑布、args 透传、四诚实拒绝臂）、mcp_list schema 查询
//! 面、lazy off 行为零变化。
//!
//! 独立测试文件（仓库纪律）。

use super::AgentLoop;
use crate::context::RequestContext;
use crate::r#loop::McpToolSnapshotEntry;
use crate::r#loop::Tool;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use crate::loop_tools::McpListTool;
use crate::types::ToolCallInfo;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tempfile::tempdir;

struct QuietProvider;

#[async_trait]
impl LlmProvider for QuietProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
            content: "w7-ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 会话内回显的 mcp_* 桩工具（验证 dispatch 解包后真实名执行 + args 透传）。
struct McpEchoTool;

#[async_trait]
impl Tool for McpEchoTool {
    fn description(&self) -> String {
        "mcp echo stub".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"]
        })
    }
    async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok(format!("ECHO:{args}"))
    }
}

fn cov_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        system_prompt: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
        tool_fence_profession: None,
    }
}

fn w7_loop() -> AgentLoop {
    let al = AgentLoop::new(Box::new(QuietProvider), cov_config());
    {
        let mut t = al.tools.write();
        t.insert("mcp_fake_echo".to_string(), Arc::new(McpEchoTool));
        // 生产注册在 loop_tools 装配函数（恒注册）；裸 AgentLoop::new 不跑
        // 装配——测试自插，等价供给闸面对的注册面。
        t.insert(
            "mcp_call".to_string(),
            Arc::new(crate::loop_tools::McpCallTool),
        );
    }
    al
}

/// 写临时 config.json（可带 agents.mcp_lazy_supply）并挂到 loop。
fn with_lazy_config(al: &AgentLoop, lazy: bool) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    std::fs::write(
        &path,
        format!(r#"{{"agents": {{"mcp_lazy_supply": {lazy}}}}}"#),
    )
    .unwrap();
    // tempdir 在函数尾析构会删目录——泄漏给进程退出回收（测试环境可接受，
    // 换取 config_path 持有期内文件稳定在盘）。
    std::mem::forget(dir);
    al.set_config_path(path);
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "w7chat", "w7user", "agent:main:session:w7")
}

fn def_names(al: &AgentLoop) -> Vec<String> {
    al.build_tool_defs()
        .into_iter()
        .map(|d| d.function.name)
        .collect()
}

// ---------------------------------------------------------------------------
// 供给闸
// ---------------------------------------------------------------------------

#[test]
fn lazy_off_supply_unchanged_and_no_meta_tool() {
    let al = w7_loop();
    with_lazy_config(&al, false);
    let names = def_names(&al);
    assert!(
        names.contains(&"mcp_fake_echo".to_string()),
        "lazy off：mcp_* 照常供给: {names:?}"
    );
    assert!(
        !names.contains(&"mcp_call".to_string()),
        "lazy off：meta 工具不出现在供给面: {names:?}"
    );
}

#[test]
fn lazy_on_supply_hides_longtail_and_exposes_meta_tool() {
    let al = w7_loop();
    with_lazy_config(&al, true);
    let names = def_names(&al);
    assert!(
        !names.contains(&"mcp_fake_echo".to_string()),
        "lazy on：mcp_* 长尾不上行: {names:?}"
    );
    assert!(
        names.contains(&"mcp_call".to_string()),
        "lazy on：meta 工具在供给面: {names:?}"
    );
}

#[test]
fn lazy_on_meta_tool_bypasses_tier_whitelist() {
    let al = w7_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Mini;
    with_lazy_config(&al, true);
    let names = def_names(&al);
    assert!(
        !names.contains(&"mcp_fake_echo".to_string()),
        "mini 档 mcp_* 长尾仍不供给: {names:?}"
    );
    assert!(
        names.contains(&"mcp_call".to_string()),
        "mini 档 lazy on：mcp_call 白名单放行: {names:?}"
    );
}

// ---------------------------------------------------------------------------
// dispatch 解包
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dispatch_unwraps_meta_call_to_real_name_with_args() {
    let al = w7_loop();
    with_lazy_config(&al, true);
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c1".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "mcp_fake_echo", "args": {"msg": "hi"}}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.starts_with("ECHO:"), "解包后真实工具执行: {res}");
    assert!(
        res.contains(r#""msg":"hi""#) || res.contains(r#""msg": "hi""#),
        "args 逐字透传: {res}"
    );
}

#[tokio::test]
async fn dispatch_meta_call_honest_refusals() {
    let al = w7_loop();

    // ① lazy off：双闸 dispatch 端拒绝（供给面本就没有，防陈旧 cache）。
    with_lazy_config(&al, false);
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c0".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "mcp_fake_echo"}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("mcp_lazy_supply"), "{res}");

    with_lazy_config(&al, true);
    // ② 缺 tool 字段。
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c1".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("缺少必填字段 `tool`"), "{res}");
    // ③ 非 mcp_ 前缀（meta 工具不是通用调度旁路）。
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c2".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "exec"}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("mcp_*"), "{res}");
    // ④ 递归自指（inner == mcp_call）。
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c3".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "mcp_call"}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("mcp_*"), "{res}");
    // ⑤ 内层不在已发现集合。
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c4".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "mcp_ghost"}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("不在已发现集合"), "{res}");
}

// ---------------------------------------------------------------------------
// mcp_list schema 查询面
// ---------------------------------------------------------------------------

fn snapshot() -> Arc<parking_lot::RwLock<Vec<McpToolSnapshotEntry>>> {
    Arc::new(parking_lot::RwLock::new(vec![McpToolSnapshotEntry {
        name: "mcp_fake_echo".to_string(),
        description: "echo stub for tests".to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"msg": {"type": "string"}},
            "required": ["msg"]
        }),
        read_only_hint: Some(true),
        destructive_hint: None,
    }]))
}

#[tokio::test]
async fn mcp_list_by_name_prints_full_schema() {
    let t = McpListTool::new(snapshot());
    // 按名：完整 schema + mcp_call 用法提示。
    let out = t
        .execute(r#"{"name": "mcp_fake_echo"}"#, &ctx())
        .await
        .unwrap();
    assert!(out.contains("## MCP Tool `mcp_fake_echo`"), "{out}");
    assert!(out.contains("read_only_hint**: true"), "{out}");
    assert!(out.contains("destructive_hint**: unknown"), "{out}");
    assert!(out.contains("required"), "{out}");
    assert!(out.contains("mcp_call"), "{out}");
    // 未知名：诚实提示。
    let out = t.execute(r#"{"name": "mcp_ghost"}"#, &ctx()).await.unwrap();
    assert!(out.contains("No MCP tool named `mcp_ghost`"), "{out}");
    // 无名：全量列表（历史格式）。
    let out = t.execute("{}", &ctx()).await.unwrap();
    assert!(out.contains("## Registered MCP Tools (1)"), "{out}");
}

// ---------------------------------------------------------------------------
// McpCallTool 直调安全网
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mcp_call_direct_execute_is_honest_dead_end() {
    let t = crate::loop_tools::McpCallTool;
    let err = t.execute(r#"{"tool": "mcp_fake_echo"}"#, &ctx()).await;
    assert!(err.is_err(), "直调必须报错（dispatch 拦截是唯一执行路径）");
}
