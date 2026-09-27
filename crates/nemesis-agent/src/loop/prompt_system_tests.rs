// prompt-pack pro（M3）：`prompt_system` 双体系闸门测试。
//
// 覆盖面：
// ① Classic 默认——tool_defs 描述恒为注册表原文（classic 字节不变承诺的
//    AgentLoop 级证明，与 golden 特征测试互为表里）；
// ② Pro + Big 档——表命中工具拿分档描述（首句==lean 首句），表外工具
//    原文回落（forge/board/动态 MCP 依赖此语义）；
// ③ Pro + Mini 档——表命中工具拿 lean 文本；
// ④ 运行时切回 Classic——描述立即恢复原文。
//
// 自带迷你 Mock（兄弟模块私有类型不共享）。

use super::*;
use nemesis_types::capability::ModelTier;

/// 恒定回复的迷你 provider。
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
            content: "ok".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// 固定描述的迷你工具（description 可控，验证透传/改写两条路）。
struct DescTool(&'static str);

#[async_trait]
impl Tool for DescTool {
    async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
        Ok("ok".to_string())
    }
    fn description(&self) -> String {
        self.0.to_string()
    }
}

const REGISTRY_DESC: &str = "registry original description";

fn ps_loop() -> AgentLoop {
    let mut al = AgentLoop::new(
        Box::new(QuietProvider),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("You are a test assistant.".to_string()),
            max_turns: 5,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    );
    // exec：描述表内槽位；zzz_custom：表外（模拟 forge/board/动态注册）。
    al.register_tool("exec".to_string(), Box::new(DescTool(REGISTRY_DESC)));
    al.register_tool("zzz_custom".to_string(), Box::new(DescTool(REGISTRY_DESC)));
    al
}

fn desc_of<'a>(defs: &'a [crate::types::ToolDefinition], name: &str) -> &'a str {
    &defs
        .iter()
        .find(|d| d.function.name == name)
        .unwrap_or_else(|| panic!("工具 {name} 应在供给里"))
        .function
        .description
}

// ---------------------------------------------------------------------------
// ① Classic 默认
// ---------------------------------------------------------------------------

#[test]
fn classic_default_keeps_registry_descriptions() {
    let al = ps_loop();
    // 构造默认 Classic；未 set_config_path 不读任何文件 → 纯内存路径。
    let defs = al.build_tool_defs();
    assert_eq!(desc_of(&defs, "exec"), REGISTRY_DESC);
    assert_eq!(desc_of(&defs, "zzz_custom"), REGISTRY_DESC);
}

// ---------------------------------------------------------------------------
// ② Pro + Big：分档生效 + 表外回落
// ---------------------------------------------------------------------------

#[test]
fn pro_big_serves_table_description_and_falls_back_for_unknown() {
    let al = ps_loop();
    al.set_prompt_system(crate::prompt::PromptSystem::Pro);
    let defs = al.build_tool_defs();

    let expected = crate::prompt::tool_description("exec", "", ModelTier::Big);
    assert!(!expected.is_empty(), "exec 应命中描述表");
    assert_eq!(
        desc_of(&defs, "exec"),
        expected,
        "Pro+Big 档 exec 描述应与表内容一致"
    );
    assert_eq!(
        desc_of(&defs, "zzz_custom"),
        REGISTRY_DESC,
        "表外工具必须原文回落"
    );
}

// ---------------------------------------------------------------------------
// ③ Pro + Mini：lean 档
// ---------------------------------------------------------------------------

#[test]
fn pro_mini_serves_lean_description() {
    let al = ps_loop();
    al.set_prompt_system(crate::prompt::PromptSystem::Pro);
    al.set_tier(ModelTier::Mini);
    let defs = al.build_tool_defs();

    let lean = crate::prompt::tool_description("exec", "", ModelTier::Mini);
    let big = crate::prompt::tool_description("exec", "", ModelTier::Big);
    assert_eq!(desc_of(&defs, "exec"), lean, "Mini 档应取 lean 文本");
    assert_ne!(lean, big, "lean 与 full 应为不同文本");
}

// ---------------------------------------------------------------------------
// ④ 运行时切回 Classic
// ---------------------------------------------------------------------------

#[test]
fn switching_back_to_classic_restores_registry_descriptions() {
    let al = ps_loop();
    al.set_prompt_system(crate::prompt::PromptSystem::Pro);
    assert_ne!(desc_of(&al.build_tool_defs(), "exec"), REGISTRY_DESC);

    al.set_prompt_system(crate::prompt::PromptSystem::Classic);
    let defs = al.build_tool_defs();
    assert_eq!(desc_of(&defs, "exec"), REGISTRY_DESC);
    assert_eq!(desc_of(&defs, "zzz_custom"), REGISTRY_DESC);
}

// ---------------------------------------------------------------------------
// ⑤ DetachedOpts.role 消费链路（prompt-pack pro M4）
// ---------------------------------------------------------------------------

/// 捕获 system 消息的迷你 provider（验证 role 模板进入请求的首条 system）。
struct SystemCaptureProvider(std::sync::Arc<std::sync::Mutex<Option<String>>>);

#[async_trait]
impl LlmProvider for SystemCaptureProvider {
    async fn chat(
        &self,
        _model: &str,
        messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        if let Some(sys) = messages.iter().find(|m| m.role == "system") {
            *self.0.lock().unwrap() = Some(sys.content.clone());
        }
        Ok(LlmResponse {
            content: "done".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[tokio::test]
async fn detached_role_overlays_persona_in_system_prompt() {
    let state: std::sync::Arc<std::sync::Mutex<Option<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let al = AgentLoop::new(
        Box::new(SystemCaptureProvider(state.clone())),
        AgentConfig {
            model: "test-model".to_string(),
            system_prompt: Some("你是个测试助手。".to_string()),
            max_turns: 3,
            tools: vec![],
            models: std::collections::HashMap::new(),
        },
    );

    // Some(role)：主人格在前 + 角色段叠加。
    al.run_detached(
        "调查这个目录",
        DetachedOpts {
            role: Some(crate::prompt::SubagentRole::Explorer),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let sys = state.lock().unwrap().clone().expect("system 消息缺失");
    assert!(sys.starts_with("你是个测试助手。"), "主人格应在最前");
    assert!(sys.contains("# 角色：侦察员"), "角色段应叠加在主人格之后");
}

// ---------------------------------------------------------------------------
// ⑥ 快照注入节闸门（M4：输入数据声明 + 外部来源降权）
// ---------------------------------------------------------------------------

fn build_digest(al: &AgentLoop) -> String {
    let instance = al.get_or_create_instance("sess:snap");
    instance.add_user_message("你好");
    let (msgs, _) = al.build_messages_with_memory_annotated(&instance, None, None);
    msgs.iter()
        .find(|m| m.content.contains("<system-reminder>"))
        .map(|m| m.content.clone())
        .expect("合并快照消息缺失")
}

#[test]
fn snapshot_sections_pro_vs_classic_gate() {
    let al = ps_loop();
    // Pro：数据声明节在场。
    al.set_prompt_system(crate::prompt::PromptSystem::Pro);
    let digest = build_digest(&al);
    assert!(
        digest.contains("# 输入数据声明"),
        "Pro 体系应渲染输入数据声明节"
    );

    // Classic：两节都不渲染（字节不变承诺的快照侧证明）。
    al.set_prompt_system(crate::prompt::PromptSystem::Classic);
    let digest = build_digest(&al);
    assert!(
        !digest.contains("# 输入数据声明") && !digest.contains("# 来源声明"),
        "Classic 体系不得渲染新节"
    );
}

#[test]
fn external_channel_section_gates_on_channel() {
    let al = ps_loop();
    al.set_prompt_system(crate::prompt::PromptSystem::Pro);

    // web（None 传入等价不声明）：无来源节。
    let digest = build_digest(&al);
    assert!(!digest.contains("# 来源声明"), "web/未知通道不渲染来源节");

    // 外部通道（telegram）：来源节在场且带通道名。
    let instance = al.get_or_create_instance("sess:ext");
    instance.add_user_message("你好");
    let (msgs, _) = al.build_messages_with_memory_annotated(&instance, None, Some("telegram"));
    let digest = msgs
        .iter()
        .find(|m| m.content.contains("<system-reminder>"))
        .map(|m| m.content.clone())
        .expect("合并快照消息缺失");
    assert!(
        digest.contains("# 来源声明") && digest.contains("channel: telegram"),
        "外部通道应渲染来源降权节"
    );

    // 内部通道（subagent/cli/system）：不渲染（自家代理不是第三方输入）。
    let instance = al.get_or_create_instance("sess:internal");
    instance.add_user_message("你好");
    let (msgs, _) = al.build_messages_with_memory_annotated(&instance, None, Some("subagent"));
    let digest = msgs
        .iter()
        .find(|m| m.content.contains("<system-reminder>"))
        .map(|m| m.content.clone())
        .expect("合并快照消息缺失");
    assert!(!digest.contains("# 来源声明"), "内部通道不渲染来源节");
}
