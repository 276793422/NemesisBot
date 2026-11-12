//! 提示词内容可视化（2026-11-13）：`AgentLoop::session_prompt_segment`
//! 七段全文分解测试。
//!
//! 契约：与 `session_context_status` 共享同一底层取数/渲染（单一真相源）
//! ——本套件把「同源」锁在数字上：各段 parts tokens 逐段对齐 meter 的
//! breakdown（估算口径）。system 段有意口径分离（meter 值 = 全文 − 冻结
//! SKILL/MEMORY，split=None 时两者同值——此处只锁 split=None 形态）。
//! history 段 content = model_facing 原文（tool 轮为投影后的模型视角，
//! 与计量字节一致），tokens 加总恒 == meter history 段值（投影估算
//! 逐轮可加：estimate_tokens_for_turns_projected 是 map+sum）。

use super::*;
use crate::{ChatOptions, ToolDefinition};

/// 最小 config（system_prompt 与 instance.rs 注入路径联动，被断言计数）。
fn local_test_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
        tool_fence_profession: None,
    }
}

/// 空 provider——段分解是纯读路径，不发 LLM 请求，占位即可。
struct NoopProvider;
#[async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<ChatOptions>,
        _tools: Vec<ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Err("noop".to_string())
    }
}

fn seg_loop() -> (AgentLoop, std::sync::Arc<crate::session::SessionStore>) {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut al = AgentLoop::new(Box::new(NoopProvider), local_test_config());
    al.set_session_store(store.clone());
    (al, store)
}

/// split 在场变体（冻结 SKILL/MEMORY 段经 AgentConfig 注入）。
fn seg_loop_split(
    skills: &str,
    memory: &str,
) -> (AgentLoop, std::sync::Arc<crate::session::SessionStore>) {
    let store = std::sync::Arc::new(crate::session::SessionStore::new_in_memory());
    let mut cfg = local_test_config();
    cfg.system_prompt_split = Some(crate::types::SystemPromptSplit {
        skills: skills.to_string(),
        memory: memory.to_string(),
    });
    let mut al = AgentLoop::new(Box::new(NoopProvider), cfg);
    al.set_session_store(store.clone());
    (al, store)
}

fn msg(role: &str, content: &str) -> crate::session::StoredMessage {
    crate::session::StoredMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }
}

/// 段 key 白名单完整性：七段全部可查（空会话也诚实回空 parts 不报错）。
#[test]
fn all_seven_segments_queryable_on_bare_session() {
    let (al, _store) = seg_loop();
    let key = "agent:main:session:whitelist";
    for s in [
        "system", "skill", "memory", "tools", "mcp", "summary", "history",
    ] {
        let v = al.session_prompt_segment(key, s).unwrap_or_else(|e| {
            panic!("segment {s} must be queryable: {e}");
        });
        assert_eq!(v["segment"], s);
        assert!(v["parts"].is_array());
    }
}

/// 非法段 key loud 报错（白名单外拒绝，不静默回落）。
#[test]
fn unknown_segment_key_loud_error() {
    let (al, _store) = seg_loop();
    let err = al
        .session_prompt_segment("agent:main:session:x", "bogus")
        .unwrap_err();
    assert!(err.contains("unknown segment"), "实际: {err}");
    assert!(err.contains("system"), "报错带合法值清单: {err}");
}

/// system 段 = system turn 全文；split=None 时与 meter 的 system 段值同值。
#[test]
fn system_segment_returns_full_text_aligned_with_breakdown() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:sys";
    store.get_or_create(key);
    store.add_message(key, "user", "hello");
    let v = al.session_prompt_segment(key, "system").unwrap();
    assert_eq!(v["title"], "系统提示词");
    let parts = v["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0]["content"], "You are a test assistant.");
    let toks = parts[0]["tokens"].as_u64().unwrap();
    assert_eq!(v["total_tokens"], toks, "单条 part，total == part tokens");
    let st = al.session_context_status(key);
    assert_eq!(
        toks,
        st["breakdown"]["system"].as_u64().unwrap(),
        "split=None：全文估算 == meter system 段值"
    );
}

/// split 在场：skill/memory 各回冻结段一条（loader 未装配 → digest 缺席），
/// tokens 与 meter 两段值逐段对齐。
#[test]
fn skill_memory_split_frozen_parts_align_with_breakdown() {
    let (al, store) = seg_loop_split("## Loaded Skills\n- alpha", "## Memory Context\n- beta");
    let key = "agent:main:session:split";
    store.get_or_create(key);
    let vs = al.session_prompt_segment(key, "skill").unwrap();
    let sp = vs["parts"].as_array().unwrap();
    assert_eq!(sp.len(), 1, "loader 缺席 → 仅冻结段");
    assert_eq!(sp[0]["content"], "## Loaded Skills\n- alpha");
    let st = al.session_context_status(key);
    assert_eq!(
        vs["total_tokens"],
        st["breakdown"]["skill"].as_u64().unwrap(),
        "冻结段估算 == meter skill 段值（digest 缺席）"
    );
    let vm = al.session_prompt_segment(key, "memory").unwrap();
    let mp = vm["parts"].as_array().unwrap();
    assert_eq!(mp.len(), 1, "workspace 未设 → 画像缺席，仅冻结段");
    assert_eq!(mp[0]["content"], "## Memory Context\n- beta");
    assert_eq!(
        vm["total_tokens"],
        st["breakdown"]["memory"].as_u64().unwrap()
    );
}

/// tools/mcp 段：空注册也回传一条（"[]"）——tokens 与 meter 的空注册
/// 基线对齐，不造「meter 有值查看器为空」。
#[test]
fn tools_mcp_empty_registry_baseline_aligned() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:emptydefs";
    store.get_or_create(key);
    let vt = al.session_prompt_segment(key, "tools").unwrap();
    let tp = vt["parts"].as_array().unwrap();
    assert_eq!(tp.len(), 1, "空表也回传（诚实空态）");
    assert_eq!(tp[0]["content"], "[]");
    let vm = al.session_prompt_segment(key, "mcp").unwrap();
    let st = al.session_context_status(key);
    assert_eq!(
        vt["total_tokens"],
        st["breakdown"]["tools"].as_u64().unwrap(),
        "紧凑序列化估算与 meter 同公式"
    );
    assert_eq!(vm["total_tokens"], st["breakdown"]["mcp"].as_u64().unwrap());
}

/// summary 段：缺席 → 空 parts + total 0（== meter 0）；在场 → 摘要全文。
#[test]
fn summary_segment_absent_then_present() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:sum";
    store.get_or_create(key);
    let v0 = al.session_prompt_segment(key, "summary").unwrap();
    assert_eq!(v0["parts"].as_array().unwrap().len(), 0);
    assert_eq!(v0["total_tokens"], 0);
    store.set_summary(key, "Earlier: user and agent discussed INI parsing.");
    let v1 = al.session_prompt_segment(key, "summary").unwrap();
    let p = v1["parts"].as_array().unwrap();
    assert_eq!(p.len(), 1);
    assert_eq!(
        p[0]["content"],
        "Earlier: user and agent discussed INI parsing."
    );
    let st = al.session_context_status(key);
    assert_eq!(
        v1["total_tokens"],
        st["breakdown"]["summary"].as_u64().unwrap()
    );
}

/// history 段：剔除首部 system turn（meter 同款剔除），每轮一条、
/// 全历史序号编址，tokens 加总 == meter history 段值。
#[test]
fn history_skips_system_turn_and_aligns() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:hist";
    store.get_or_create(key);
    store.add_message(key, "user", "hello world");
    store.add_message(key, "assistant", "hi there");
    let v = al.session_prompt_segment(key, "history").unwrap();
    let parts = v["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 2, "system turn 被剔除");
    assert_eq!(parts[0]["label"], "user #2", "全历史序号（system 是 #1）");
    assert_eq!(parts[1]["label"], "assistant #3");
    assert_eq!(parts[0]["content"], "hello world");
    let sum: u64 = parts.iter().filter_map(|p| p["tokens"].as_u64()).sum();
    let st = al.session_context_status(key);
    assert_eq!(sum, st["breakdown"]["history"].as_u64().unwrap());
}

/// history 段富轮次：tool 轮 content = model_facing 投影原文（与计量
/// 字节一致）；assistant 轮 tool_calls 走旁注字段；图片引用只计数注记。
#[test]
fn history_tool_turn_projection_and_aux_fields() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:rich";
    store.get_or_create(key);
    let mut user = msg("user", "run the tool");
    user.image_refs = vec!["/ws/uploads/a.png".to_string()];
    let mut asst = msg("assistant", "");
    asst.tool_calls = vec![crate::session::StoredToolCall {
        id: "c1".to_string(),
        name: "exec".to_string(),
        arguments: "{\"cmd\":\"ls\"}".to_string(),
    }];
    let mut tool = msg("tool", "RAW_TOOL_OUTPUT huge body that was pruned");
    tool.tool_call_id = Some("c1".to_string());
    tool.tool_name = Some("exec".to_string());
    tool.tool_result_projection = Some("[pruned: oversized output]".to_string());
    store.set_history(
        key,
        vec![msg("system", "You are a test assistant."), user, asst, tool],
    );
    let v = al.session_prompt_segment(key, "history").unwrap();
    let parts = v["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(
        parts[0]["image_refs_count"], 1,
        "图片引用只计数注记，不混入被估算文本"
    );
    let calls = parts[1]["tool_calls"].as_array().unwrap();
    assert_eq!(calls[0]["name"], "exec");
    assert_eq!(calls[0]["arguments"], "{\"cmd\":\"ls\"}");
    assert_eq!(
        parts[2]["content"], "[pruned: oversized output]",
        "tool 轮 content = 投影后的模型视角（recorded override 优先）"
    );
    assert_eq!(parts[2]["label"], "tool(exec) #4");
    let sum: u64 = parts.iter().filter_map(|p| p["tokens"].as_u64()).sum();
    let st = al.session_context_status(key);
    assert_eq!(sum, st["breakdown"]["history"].as_u64().unwrap());
}

/// history 段 covers 边界：覆盖点之后的逐字尾部（与 meter 切片一致），
/// 序号延续全历史编址。
#[test]
fn history_covers_boundary_slices_tail_and_aligns() {
    let (al, store) = seg_loop();
    let key = "agent:main:session:covers";
    store.get_or_create(key);
    for i in 0..10 {
        let role = if i % 2 == 0 { "user" } else { "assistant" };
        store.add_message(key, role, &format!("turn {i}: {}", "detail ".repeat(20)));
    }
    // covers=6 是全历史索引（[sys, s0..s9] 的 index 6 = s5，i=5 → assistant）。
    store.set_summary(key, "Earlier: turns 0-4.");
    store.set_summary_covers_up_to(key, Some(6));
    let v = al.session_prompt_segment(key, "history").unwrap();
    let parts = v["parts"].as_array().unwrap();
    assert_eq!(parts.len(), 5, "history[6..] = s5..s9");
    assert_eq!(parts[0]["label"], "assistant #7");
    assert_eq!(parts[4]["label"], "assistant #11");
    let sum: u64 = parts.iter().filter_map(|p| p["tokens"].as_u64()).sum();
    let st = al.session_context_status(key);
    assert_eq!(sum, st["breakdown"]["history"].as_u64().unwrap());
}
