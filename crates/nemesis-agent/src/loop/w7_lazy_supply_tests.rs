//! （2026-10-25 加固）：
//! MCP 工具惰性供给测试——供给闸（lazy on →
//! tool_defs 无 mcp_* 长尾，有 meta 两件套 mcp_call + mcp_list；tier 白
//! 名单放行 meta； 补 mcp_list 豁免——查询面被长尾隐藏
//! 一并裁掉则两步流程无工具可用）、dispatch 解包（真实名重走瀑布、
//! args 透传、四诚实拒绝臂）、mcp_list dispatch 拦截可见性面
//! （BUG-20261118-095：hidden/tier 过滤 + 自闸 + registry 真值渲染）、
//! lazy off 行为零变化。
//!
//! 独立测试文件（仓库纪律）。

use super::AgentLoop;
use crate::context::RequestContext;
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
        system_prompt_split: None,
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
        // 装配——测试自插，等价供给闸面对的注册面。mcp_list 同为恒注册
        // （补：首版 fixture 漏插导致长尾隐藏把两步流程的
        // 查询面一起裁掉的缺陷在供给面测试里不可）。
        t.insert(
            "mcp_call".to_string(),
            Arc::new(crate::loop_tools::McpCallTool),
        );
        t.insert(
            "mcp_list".to_string(),
            Arc::new(crate::loop_tools::McpListTool),
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
    assert!(
        names.contains(&"mcp_list".to_string()),
        "lazy off：mcp_list 照常供给（行为零变化面）: {names:?}"
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

// mcp_list 是两步流程的 schema 查询面（mcp_call 的描述与
// 错误臂都指路它），lazy on 的长尾隐藏必须豁免这个 meta 伴侣工具——否则
// 模型「查名字/查 schema」无工具可用，元流程自断（模型照指路调
// mcp_list → tool not found → 循环空转）。
#[test]
fn lazy_on_supply_keeps_mcp_list_companion() {
    let al = w7_loop();
    with_lazy_config(&al, true);
    let names = def_names(&al);
    assert!(
        names.contains(&"mcp_list".to_string()),
        "lazy on：mcp_list（两步流程查询面）必须在供给面: {names:?}"
    );
}

#[test]
fn lazy_on_mini_supply_keeps_mcp_list_companion() {
    let al = w7_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Mini;
    with_lazy_config(&al, true);
    let names = def_names(&al);
    assert!(
        names.contains(&"mcp_list".to_string()),
        "mini 档 lazy on：mcp_list 白名单放行（mcp_call 的指路对象）: {names:?}"
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
// mcp_list schema 查询面（BUG-20261118-095 后形态：dispatch 拦截 +
// registry 真值可见性过滤渲染；execute 直调 = 诚实报错安全网）
// ---------------------------------------------------------------------------

/// 写临时 config.json（agents.mcp_lazy_supply + agents.hidden_tools）并挂
/// 到 loop（with_lazy_config 的 hidden 扩展形态；l10 with_config 同款
/// 分段拼接——raw string 内嵌 `}}` 会被 format! 吃成一个 `}`，整段一次
/// format! 会少闭合外层对象，JSON 坏文件落缺省全绿假象）。
fn with_lazy_and_hidden_config(al: &AgentLoop, lazy: bool, hidden: &[&str]) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut body = String::from(r#"{"agents": {"mcp_lazy_supply": "#);
    body.push_str(if lazy { "true" } else { "false" });
    if !hidden.is_empty() {
        let list = hidden
            .iter()
            .map(|h| format!("\"{h}\""))
            .collect::<Vec<_>>()
            .join(",");
        body.push_str(&format!(r#", "hidden_tools": [{list}]"#));
    }
    body.push_str("}}");
    std::fs::write(&path, body).unwrap();
    // tempdir 在函数尾析构会删目录——泄漏给进程退出回收（测试环境可接受，
    // 换取 config_path 持有期内文件稳定在盘；w7 同款）。
    std::mem::forget(dir);
    al.set_config_path(path);
}

async fn mcp_list_call(al: &AgentLoop, args: &str) -> String {
    al.handle_tool_call(
        &ToolCallInfo {
            id: "cl".to_string(),
            name: "mcp_list".to_string(),
            arguments: args.to_string(),
        },
        &ctx(),
    )
    .await
}

#[tokio::test]
async fn mcp_list_dispatch_hidden_tool_filtered_and_detail_refused() {
    let al = w7_loop();
    with_lazy_and_hidden_config(&al, false, &["mcp_fake_echo"]);
    // 清单：hidden 工具消失（名字/描述都不出现）；mcp_list 自身可见；
    // lazy off 下 mcp_call 不列（自身 lazy 闸——调了必拒，列了是误导）。
    let out = mcp_list_call(&al, "{}").await;
    assert!(!out.contains("mcp_fake_echo"), "hidden 工具不得列名: {out}");
    assert!(out.contains("mcp_list"), "mcp_list 自身在清单: {out}");
    // 条目级断言（**name**: 形态）——mcp_list 自身 description 文本里
    // 含 "via mcp_call" 字样，裸子串断言会误伤。
    assert!(
        !out.contains("**mcp_call**"),
        "lazy off 下 mcp_call 不列条目: {out}"
    );
    // 按名查 hidden 工具：拒绝（镜像 hidden 闸文案、schema 不可列）。
    let out = mcp_list_call(&al, r#"{"name": "mcp_fake_echo"}"#).await;
    assert!(out.contains("hidden by configuration"), "{out}");
    assert!(out.contains("cannot be listed"), "{out}");
    // 未知名：诚实提示（历史文案不变）。
    let out = mcp_list_call(&al, r#"{"name": "mcp_ghost"}"#).await;
    assert!(out.contains("No MCP tool named `mcp_ghost`"), "{out}");
}

#[tokio::test]
async fn mcp_list_dispatch_lazy_on_discovers_longtail_with_schema() {
    let al = w7_loop();
    with_lazy_and_hidden_config(&al, true, &[]);
    // lazy on（big 档）：长尾经 mcp_call 仍可达——列名正是两步流程的
    // 发现面；meta 两件套在列。
    let out = mcp_list_call(&al, "{}").await;
    assert!(out.contains("mcp_fake_echo"), "长尾列名（发现面）: {out}");
    assert!(out.contains("mcp_call"), "{out}");
    assert!(out.contains("mcp_list"), "{out}");
    // 按名：完整 schema + mcp_call 用法提示（历史格式）。桩工具未覆写
    // read_only_hint/destructive_hint——只断言键在 schema 里有投影。
    let out = mcp_list_call(&al, r#"{"name": "mcp_fake_echo"}"#).await;
    assert!(out.contains("## MCP Tool `mcp_fake_echo`"), "{out}");
    assert!(out.contains("read_only_hint**"), "{out}");
    assert!(out.contains("destructive_hint**"), "{out}");
    assert!(out.contains("required"), "{out}");
    assert!(out.contains("Call it with `mcp_call`"), "{out}");
}

#[tokio::test]
async fn mcp_list_dispatch_tier_face_mini() {
    let al = w7_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Mini;
    with_lazy_and_hidden_config(&al, true, &[]);
    // mini + lazy on：meta 两件套豁免白名单在列；长尾白名单外不可见
    // （内层 mcp_call 必拒——列了是误导循环）。
    let out = mcp_list_call(&al, "{}").await;
    assert!(out.contains("mcp_call"), "{out}");
    assert!(out.contains("mcp_list"), "{out}");
    assert!(!out.contains("mcp_fake_echo"), "tier 外长尾不列: {out}");
    let out = mcp_list_call(&al, r#"{"name": "mcp_fake_echo"}"#).await;
    assert!(
        out.contains("not visible at the active model's tier"),
        "{out}"
    );
}

#[tokio::test]
async fn mcp_list_self_hidden_refused() {
    let al = w7_loop();
    with_lazy_and_hidden_config(&al, true, &["mcp_list"]);
    // 自闸：mcp_list 自身被 hidden 时拒绝（拦截臂先于通用 hidden 闸——
    // 无自闸则「供给面隐藏 + dispatch 面放行」双闸矛盾对 meta 名成立）。
    let out = mcp_list_call(&al, "{}").await;
    assert!(out.contains("hidden by configuration"), "{out}");
}

#[tokio::test]
async fn mcp_call_self_hidden_refused() {
    let al = w7_loop();
    with_lazy_and_hidden_config(&al, true, &["mcp_call"]);
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c9".to_string(),
                name: "mcp_call".to_string(),
                arguments: r#"{"tool": "mcp_fake_echo", "args": {}}"#.to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("hidden by configuration"), "{res}");
}

#[tokio::test]
async fn mcp_list_direct_execute_is_honest_dead_end() {
    let t = McpListTool;
    let err = t.execute("{}", &ctx()).await;
    assert!(err.is_err(), "直调必须报错（dispatch 拦截是唯一执行路径）");
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
