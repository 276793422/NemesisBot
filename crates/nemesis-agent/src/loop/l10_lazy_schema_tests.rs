//! 建设（2026-11-02）：
//! 工具惰性 schema 供给测试——档位裁决矩阵（mini 强制惰性 / normal 缺省
//! 惰性 / normal config off / big·Auto 恒全量）、折叠保真（name+description
//! 全保 + 占位 + 取回工具豁免）、解锁账与注记渲染（空键跳过/去重/append
//! 前缀稳定/registry 缺名跳过）、dispatch 拦截臂（lazy off / 缺 name /
//! 未注册 / hidden / tier 不可见 / 成功记账）、校验回灌指引、messages
//! 注入集成。
//!
//! 独立测试文件（仓库纪律）。形态同 w7_lazy_supply_tests（裸 AgentLoop +
//! 手插工具 + tempdir config + std::mem:forget）。

use super::AgentLoop;
use crate::context::RequestContext;
use crate::instance::AgentInstance;
use crate::r#loop::Tool;
use crate::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use crate::loop_tools::ToolsSchemaGetTool;
use crate::types::{ConversationTurn, ToolCallInfo};
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
            content: "l10-ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 白名单内桩工具（read_file 在 mini/normal 两档白名单内——三档全供给）。
struct StubTool {
    desc: &'static str,
}

#[async_trait]
impl Tool for StubTool {
    fn description(&self) -> String {
        self.desc.to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {"path": {"type": "string", "description": "target path"}},
            "required": ["path"]
        })
    }
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("stub-ok".to_string())
    }
}

/// normal 白名单外桩（tier 可见性镜像的负样本——normal 档供给面看不）。
struct MiniInvisibleTool;

#[async_trait]
impl Tool for MiniInvisibleTool {
    fn description(&self) -> String {
        "mini-invisible stub (multiedit-shaped)".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("mini-invisible-ok".to_string())
    }
}

/// 声明 min_tier=big 的桩（WASM min-tier 秩比较镜像的正/负样本）。
struct BigOnlyTool;

#[async_trait]
impl Tool for BigOnlyTool {
    fn description(&self) -> String {
        "big-only stub".to_string()
    }
    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    fn min_tier(&self) -> Option<&str> {
        Some("big")
    }
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("big-only-ok".to_string())
    }
}

fn l10_config() -> crate::types::AgentConfig {
    crate::types::AgentConfig {
        model: "test-model".to_string(),
        // system 节在位时 build_messages 才渲染 sections（ws9 同款前置条件
        // ——digest 注入点保护 system 前缀，无 system 节 = 无注入）。
        system_prompt: Some("You are a test assistant.".to_string()),
        system_prompt_split: None,
        max_turns: 5,
        tools: vec![],
        models: HashMap::new(),
        tool_fence_profession: None,
    }
}

fn l10_loop() -> AgentLoop {
    let al = AgentLoop::new(Box::new(QuietProvider), l10_config());
    {
        let mut t = al.tools.write();
        t.insert(
            "read_file".to_string(),
            Arc::new(StubTool {
                desc: "read a file from disk",
            }),
        );
        // normal 白名单内、mini 白名单外（折叠供给正样本 + mini 取回负样本）。
        t.insert("multiedit".to_string(), Arc::new(MiniInvisibleTool));
        // min_tier=big 声明桩（秩比较镜像）。
        t.insert("big_only".to_string(), Arc::new(BigOnlyTool));
        // 生产装配恒注册；裸 AgentLoop::new 不跑装配——测试自插（w7 同款）。
        t.insert("tools_schema_get".to_string(), Arc::new(ToolsSchemaGetTool));
    }
    al
}

/// 写临时 config.json（agents.lazy_tool_schema / agents.hidden_tools）并挂
/// 到 loop。`None` = 空 agents（缺键=缺省路径）。
fn with_config(al: &AgentLoop, lazy: Option<bool>, hidden: &[&str]) {
    let dir = tempdir().unwrap();
    let path = dir.path().join("config.json");
    let mut body = String::from("{\"agents\": {");
    if let Some(v) = lazy {
        body.push_str(&format!(r#""lazy_tool_schema": {v}"#));
    }
    if !hidden.is_empty() {
        if !body.ends_with('{') {
            body.push(',');
        }
        let list = hidden
            .iter()
            .map(|h| format!("\"{h}\""))
            .collect::<Vec<_>>()
            .join(",");
        body.push_str(&format!(r#""hidden_tools": [{list}]"#));
    }
    body.push_str("}}");
    std::fs::write(&path, body).unwrap();
    // tempdir 在函数尾析构会删目录——泄漏给进程退出回收（测试环境可接受，
    // 换取 config_path 持有期内文件稳定在盘；w7 同款）。
    std::mem::forget(dir);
    al.set_config_path(path);
}

fn ctx() -> RequestContext {
    RequestContext::new("web", "l10chat", "l10user", "agent:main:session:l10")
}

fn def_by_name(al: &AgentLoop, name: &str) -> Option<crate::types::ToolDefinition> {
    al.build_tool_defs()
        .into_iter()
        .find(|d| d.function.name == name)
}

// ---------------------------------------------------------------------------
// 档位裁决矩阵（current_lazy_tool_schema 统一定义）
// ---------------------------------------------------------------------------

// mini 强制惰性：即使 config 显式关（用户意志不覆盖强制面——字节节省对
// 9-35B 档是硬收益，且 mini 拿到全量 schema 历史上正是参数事故重灾区）。
#[test]
fn tier_matrix_mini_forced_lazy() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Mini;
    with_config(&al, Some(false), &[]);
    assert!(
        al.current_lazy_tool_schema(),
        "mini 档 config off 不影响强制惰性"
    );
    let names = al
        .build_tool_defs()
        .into_iter()
        .map(|d| d.function.name)
        .collect::<Vec<_>>();
    assert!(
        names.contains(&"tools_schema_get".to_string()),
        "mini 惰性下取回工具必供（白名单必含——惰性断供防线）: {names:?}"
    );
    let rf = def_by_name(&al, "read_file").expect("read_file 在 mini 白名单内");
    assert_eq!(
        rf.function.parameters,
        serde_json::json!({"type": "object"}),
        "mini 档参数折叠为占位"
    );
}

#[test]
fn tier_matrix_normal_default_lazy() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    // 缺 config_path：缺省 true。
    assert!(al.current_lazy_tool_schema(), "normal 无 config = 缺省惰性");
    // 有 config 但缺键：缺省 true。
    with_config(&al, None, &[]);
    assert!(al.current_lazy_tool_schema(), "normal 缺键 = 缺省惰性");
}

#[test]
fn tier_matrix_normal_config_off_full() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(false), &[]);
    assert!(!al.current_lazy_tool_schema(), "normal config off = 全量");
    let names = al
        .build_tool_defs()
        .into_iter()
        .map(|d| d.function.name)
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"tools_schema_get".to_string()),
        "lazy off：取回工具不供给（无消费场景）: {names:?}"
    );
    let rf = def_by_name(&al, "read_file").expect("read_file 供给");
    assert!(
        rf.function.parameters.get("properties").is_some(),
        "lazy off：参数 schema 全量上行（行为零变化面）"
    );
}

// big/Auto 恒全量：config 显式开也不折叠（旗舰不缺上下文预算，惰性徒增
// 取回往返——档位裁决不吃用户意志，与 mini 的强制方向对称）。
#[test]
fn tier_matrix_big_always_full_even_config_on() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Big;
    with_config(&al, Some(true), &[]);
    assert!(!al.current_lazy_tool_schema(), "big config on 仍恒全量");
    let names = al
        .build_tool_defs()
        .into_iter()
        .map(|d| d.function.name)
        .collect::<Vec<_>>();
    assert!(
        !names.contains(&"tools_schema_get".to_string()),
        "big 恒全量：取回工具不供给: {names:?}"
    );
    let rf = def_by_name(&al, "read_file").expect("read_file 供给");
    assert!(
        rf.function.parameters.get("required").is_some(),
        "big：参数 schema 全量"
    );
}

#[test]
fn tier_matrix_auto_always_full() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Auto;
    with_config(&al, Some(true), &[]);
    assert!(!al.current_lazy_tool_schema(), "Auto 恒全量");
}

// ---------------------------------------------------------------------------
// 折叠保真（name+description 全保 + 占位 + 取回工具豁免）
// ---------------------------------------------------------------------------

#[test]
fn lazy_on_folds_params_keeps_name_and_desc() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    let rf = def_by_name(&al, "read_file").expect("read_file 供给");
    assert_eq!(rf.function.name, "read_file");
    assert_eq!(rf.function.description, "read a file from disk");
    assert_eq!(
        rf.function.parameters,
        serde_json::json!({"type": "object"}),
        "lazy on：参数折叠为最小占位（name+description 全保）"
    );
}

#[test]
fn lazy_on_fetch_tool_keeps_full_schema() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    let def = def_by_name(&al, "tools_schema_get").expect("取回工具在供给面");
    assert!(
        def.function.parameters.get("required").is_some(),
        "取回工具自身豁免折叠（模型必须先看到它怎么用）: {}",
        def.function.parameters
    );
}

// ---------------------------------------------------------------------------
// dispatch 拦截臂
// ---------------------------------------------------------------------------

async fn fetch(al: &AgentLoop, name: &str) -> String {
    al.handle_tool_call(
        &ToolCallInfo {
            id: "c1".to_string(),
            name: "tools_schema_get".to_string(),
            arguments: format!(r#"{{"name": "{name}"}}"#),
        },
        &ctx(),
    )
    .await
}

#[tokio::test]
async fn dispatch_lazy_off_refused() {
    let al = l10_loop();
    // normal + config off：双闸 dispatch 端拒绝（防陈旧 cache）。
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(false), &[]);
    let res = fetch(&al, "read_file").await;
    assert!(res.contains("惰性 schema 供给"), "{res}");
    // big + config on（恒全量）：同样拒绝。
    *al.tier.write() = nemesis_types::capability::ModelTier::Big;
    with_config(&al, Some(true), &[]);
    let res = fetch(&al, "read_file").await;
    assert!(res.contains("惰性 schema 供给"), "{res}");
}

#[tokio::test]
async fn dispatch_missing_name_refused() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    let res = al
        .handle_tool_call(
            &ToolCallInfo {
                id: "c0".to_string(),
                name: "tools_schema_get".to_string(),
                arguments: "{}".to_string(),
            },
            &ctx(),
        )
        .await;
    assert!(res.contains("缺少必填字段 `name`"), "{res}");
}

#[tokio::test]
async fn dispatch_unknown_name_honest_error() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    let res = fetch(&al, "ghost_tool").await;
    assert!(res.contains("未注册"), "{res}");
}

#[tokio::test]
async fn dispatch_hidden_name_refused() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &["read_file"]);
    let res = fetch(&al, "read_file").await;
    assert!(res.contains("hidden by configuration"), "{res}");
}

// tier 可见性镜像 build_tool_defs 同一判定：白名单外（mini 档的 multiedit）
// 与 min_tier 秩不足（normal 档的 big_only）都不可取；秩足（big 档的
// big_only）可取。
#[tokio::test]
async fn dispatch_tier_invisible_refused() {
    let al = l10_loop();
    with_config(&al, Some(true), &[]);
    // mini 档：multiedit 在白名单外。
    *al.tier.write() = nemesis_types::capability::ModelTier::Mini;
    let res = fetch(&al, "multiedit").await;
    assert!(
        res.contains("not visible at the active model's tier"),
        "{res}"
    );
    // normal 档：big_only 的 min_tier=big 秩不足。
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    let res = fetch(&al, "big_only").await;
    assert!(
        res.contains("not visible at the active model's tier"),
        "{res}"
    );
    // big 档：big_only 秩足可取（min_tier 分支正样本）。
    *al.tier.write() = nemesis_types::capability::ModelTier::Big;
    let res = fetch(&al, "big_only").await;
    // big 恒全量 → lazy off → 取回工具本身被供给闸拒绝（另一道闸先拦）——
    // 该形态在 big 档无消费场景，拒绝即正确行为。
    assert!(res.contains("惰性 schema 供给"), "{res}");
}

#[tokio::test]
async fn dispatch_success_renders_schema_and_records_unlock() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    let res = fetch(&al, "read_file").await;
    assert!(res.contains("# Schema: `read_file`"), "{res}");
    assert!(res.contains("read a file from disk"), "{res}");
    assert!(res.contains("required"), "返回完整 schema 非占位: {res}");
    // 解锁账落账（per-session）。
    {
        let ledger = al.tool_schema_unlocks.read();
        let entry = ledger.get("agent:main:session:l10").expect("解锁账落账");
        assert_eq!(entry, &vec!["read_file".to_string()]);
    }
    // 幂等：重复取不去重即重复渲染——账面只增不减且去重。
    let _ = fetch(&al, "read_file").await;
    let _ = fetch(&al, "read_file").await;
    {
        let ledger = al.tool_schema_unlocks.read();
        let entry = ledger.get("agent:main:session:l10").unwrap();
        assert_eq!(entry.len(), 1, "去重：重复取回不重复记账");
    }
    // 注记节随之渲染。
    let section = al.schema_unlock_section(Some("agent:main:session:l10"));
    assert!(section.contains("## `read_file`"), "{section}");
}

// ---------------------------------------------------------------------------
// 解锁账与注记渲染
// ---------------------------------------------------------------------------

#[test]
fn unlock_ledger_empty_session_key_skipped() {
    let al = l10_loop();
    al.record_schema_unlock("", "read_file");
    assert!(
        al.tool_schema_unlocks.read().is_empty(),
        "空 session_key 不挂账"
    );
}

#[test]
fn unlock_section_empty_cases() {
    let al = l10_loop();
    // 无解锁：空串。
    assert_eq!(al.schema_unlock_section(Some("agent:main:session:l10")), "");
    // session_key None：空串。
    assert_eq!(al.schema_unlock_section(None), "");
    // 解锁后 session 不匹配：空串（per-session 隔离）。
    al.record_schema_unlock("agent:main:session:other", "read_file");
    assert_eq!(
        al.schema_unlock_section(Some("agent:main:session:l10")),
        "",
        "会话隔离：别的会话解锁不注入"
    );
}

#[test]
fn unlock_section_renders_all_and_skips_missing_registry() {
    let al = l10_loop();
    al.record_schema_unlock("agent:main:session:l10", "read_file");
    al.record_schema_unlock("agent:main:session:l10", "ghost_tool");
    let section = al.schema_unlock_section(Some("agent:main:session:l10"));
    assert!(section.starts_with("# Unlocked Tool Schemas"), "{section}");
    assert!(section.contains("## `read_file`"), "{section}");
    assert!(section.contains("read a file from disk"), "{section}");
    assert!(section.contains("required"), "完整 schema 渲染: {section}");
    assert!(
        !section.contains("ghost_tool"),
        "registry 缺名跳过（账面与供给面诚实解耦）: {section}"
    );
}

// append-only 前缀稳定：新增解锁只把新条目插在尾部注记之前——先前渲染的
// 字节（header + 已有条目）原位不变（prompt cache 只失效尾部）。
#[test]
fn unlock_section_append_only_prefix_stable() {
    let al = l10_loop();
    al.record_schema_unlock("agent:main:session:l10", "read_file");
    let s1 = al.schema_unlock_section(Some("agent:main:session:l10"));
    al.record_schema_unlock("agent:main:session:l10", "multiedit");
    let s2 = al.schema_unlock_section(Some("agent:main:session:l10"));
    const TAIL_MARK: &str = "\n其余工具仍为惰性供给";
    let s1_core = s1.split(TAIL_MARK).next().unwrap();
    let s2_core = s2.split(TAIL_MARK).next().unwrap();
    assert!(
        s2_core.starts_with(s1_core) && s2_core.len() > s1_core.len(),
        "先前渲染字节原位不变，新条目只追加在尾注之前: s1_core={s1_core:?} s2_core={s2_core:?}"
    );
    assert!(s2.contains("## `multiedit`"), "新条目在位: {s2}");
}

// ---------------------------------------------------------------------------
// 校验回灌指引（validation_error_feedback 统一定义）
// ---------------------------------------------------------------------------

#[test]
fn validation_feedback_lazy_appends_guidance() {
    let msg = "missing required field `path`";
    let out = super::tool_defs::validation_error_feedback(msg, true, "read_file");
    assert!(out.starts_with("Tool error: "), "{out}");
    assert!(out.contains(msg), "{out}");
    assert!(
        out.contains("tools_schema_get"),
        "惰性回灌附取回指引: {out}"
    );
    assert!(out.contains("`read_file`"), "指引点名目标工具: {out}");
}

#[test]
fn validation_feedback_lazy_off_plain() {
    let out = super::tool_defs::validation_error_feedback("bad args", false, "read_file");
    assert_eq!(out, "Tool error: bad args", "lazy off 回灌零变化");
}

#[test]
fn validation_feedback_fetch_tool_exempt() {
    // 取回工具自身 schema 恒全量——它的校验失败无折叠原因，不加指引。
    let out = super::tool_defs::validation_error_feedback("bad args", true, "tools_schema_get");
    assert_eq!(out, "Tool error: bad args");
}

// ---------------------------------------------------------------------------
// 直调安全网 + messages 注入集成
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fetch_tool_direct_execute_is_honest_dead_end() {
    let t = ToolsSchemaGetTool;
    let err = t.execute(r#"{"name": "read_file"}"#, &ctx()).await;
    assert!(err.is_err(), "直调必须报错（dispatch 拦截是唯一执行路径）");
}

fn seeded_instance() -> AgentInstance {
    let inst = AgentInstance::new(l10_config());
    inst.set_history(vec![ConversationTurn {
        role: "user".to_string(),
        content: "当前轮问题".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: chrono::Local::now().to_rfc3339(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
        image_refs: Vec::new(),
    }]);
    inst
}

// 注入集成：解锁后显式 session_key 的主循环形态（build_round_messages 同参）
// 注入注记节；lazy off / 不传 session_key 不注入（字节兼容面）。
#[tokio::test]
async fn messages_integration_unlock_section_injected_and_gated() {
    let al = l10_loop();
    *al.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al, Some(true), &[]);
    al.record_schema_unlock("agent:main:session:l10", "read_file");
    let inst = seeded_instance();

    let (msgs, _) = al.build_messages_with_memory_annotated_for(
        &inst,
        None,
        Some("agent:main:session:l10"),
        None,
    );
    let joined = msgs.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        joined.contains("# Unlocked Tool Schemas"),
        "解锁后主循环形态注入注记节: {joined}"
    );
    assert!(
        joined.contains("## `read_file`"),
        "schema 正文在位: {joined}"
    );
    assert!(joined.contains("当前轮问题"), "原对话不受影响: {joined}");

    // lazy off（同 loop 换 config）：不注入。
    let al2 = l10_loop();
    *al2.tier.write() = nemesis_types::capability::ModelTier::Normal;
    with_config(&al2, Some(false), &[]);
    al2.record_schema_unlock("agent:main:session:l10", "read_file");
    let (msgs2, _) = al2.build_messages_with_memory_annotated_for(
        &inst,
        None,
        Some("agent:main:session:l10"),
        None,
    );
    let joined2 = msgs2.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        !joined2.contains("# Unlocked Tool Schemas"),
        "lazy off 不注入"
    );

    // 旧路径（不传 session_key）：不注入（Branch Context 同款先例）。
    let (msgs3, _) = al.build_messages_with_memory_annotated(&inst, None, None);
    let joined3 = msgs3.iter().map(|m| m.content.as_str()).collect::<String>();
    assert!(
        !joined3.contains("# Unlocked Tool Schemas"),
        "无 session_key 不注入"
    );
}
