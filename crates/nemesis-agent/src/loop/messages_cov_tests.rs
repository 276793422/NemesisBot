// loop/messages.rs 覆盖率补充测试（K3 shell 注入失败面 / I3 note_read
// 早退分支 / P3.1 prefetch_memory_context 门槛链 / workflow_edit 会话
// section 渲染三形态）。
//
// 形状同既有约定：恒定回复的迷你 provider（不真正跑 loop，只构造
// AgentLoop）、临时目录工作区。声明在 loop.rs 的 cfg(test) 块。

use super::*;

/// 恒定回复的迷你 provider（同 I3MockProvider 形状；本文件全部测试
/// 不真正调 LLM，只需要构造 AgentLoop）。
struct CovMockProvider;

#[async_trait]
impl LlmProvider for CovMockProvider {
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

fn cov_config() -> AgentConfig {
    AgentConfig {
        model: "test-model".to_string(),
        system_prompt: Some("You are a test assistant.".to_string()),
        max_turns: 5,
        tools: vec![],
        models: std::collections::HashMap::new(),
    }
}

fn cov_loop() -> AgentLoop {
    AgentLoop::new(Box::new(CovMockProvider), cov_config())
}

// ---------------------------------------------------------------------------
// K3：expand_shell_injections 失败面（经 pub(crate) 入口走私有
// exec_shell_injection 的各分支）
// ---------------------------------------------------------------------------

/// 注入命令成功（exit 0 + stdout）→ 输出原位替换。
#[tokio::test]
async fn shell_injection_success_replaces_with_stdout() {
    let al = cov_loop();
    let out = al
        .expand_shell_injections("前缀 !`echo hello-cov` 后缀".to_string())
        .await;
    assert!(out.contains("hello-cov"), "got: {out}");
    assert!(out.contains("前缀") && out.contains("后缀"));
    assert!(!out.contains("command failed"), "got: {out}");
}

/// 注入命令成功但零输出 → `(no output)` 占位（确定性内容）。
#[tokio::test]
async fn shell_injection_empty_output_gets_placeholder() {
    let al = cov_loop();
    let out = al.expand_shell_injections("!`exit 0`".to_string()).await;
    assert_eq!(out, "(no output)");
}

/// 非零退出且无 stderr → `[command failed: <cmd>: exit <c>]` 注记。
#[tokio::test]
async fn shell_injection_nonzero_exit_without_stderr() {
    let al = cov_loop();
    let out = al.expand_shell_injections("!`exit 7`".to_string()).await;
    assert!(out.contains("command failed"), "got: {out}");
    assert!(out.contains("exit 7"), "got: {out}");
}

/// 非零退出且 stderr 有内容 → 注记带 stderr 尾行（≤200 字符）。
/// （`1>&2` 重定向语义在 cmd 下稳定；sh 下 `&` 是后台符，不跨平台，故门控。）
#[cfg(windows)]
#[tokio::test]
async fn shell_injection_nonzero_exit_carries_stderr_tail() {
    let al = cov_loop();
    let out = al
        .expand_shell_injections("!`echo boom-cov 1>&2 & exit 3`".to_string())
        .await;
    assert!(out.contains("command failed"), "got: {out}");
    assert!(out.contains("exit 3"), "got: {out}");
    assert!(
        out.contains("boom-cov"),
        "stderr tail must ride along: {out}"
    );
}

/// cwd 指向不存在的目录 → spawn 失败（code=None）→ 注记带 spawn 原因。
#[cfg(windows)]
#[tokio::test]
async fn shell_injection_spawn_failure_notes_reason() {
    let al = cov_loop();
    let ghost = std::env::temp_dir().join(format!("cov-no-such-dir-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ghost);
    al.set_workspace_root(ghost.join("ws"));
    let out = al.expand_shell_injections("!`echo x`".to_string()).await;
    assert!(out.contains("command failed"), "got: {out}");
    assert!(out.contains("failed to spawn"), "got: {out}");
    let _ = std::fs::remove_dir_all(&ghost);
}

/// 无注入标记的文本 → 零开销原样直返（早退分支）。
#[tokio::test]
async fn shell_injection_absent_text_is_byte_identical() {
    let al = cov_loop();
    let text = "plain text with ! bang and `ticks` but no marker".to_string();
    let out = al.expand_shell_injections(text.clone()).await;
    assert_eq!(out, text);
}

// ---------------------------------------------------------------------------
// I3：note_read_for_instructions 早退分支（root 缺 / parent 缺 / 目录无
// instruction 文件保持未认领）
// ---------------------------------------------------------------------------

#[test]
fn note_read_without_workspace_root_is_noop() {
    let al = cov_loop(); // 未 set_workspace_root
    let inst = AgentInstance::new(cov_config());
    al.note_read_for_instructions(&inst, "src/notes.md");
    assert!(inst.drain_pending_instructions().is_empty());
}

/// 绝对路径只有根前缀（`C:\`）→ parent()=None → 早退。
#[test]
fn note_read_root_only_path_parent_none_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let al = cov_loop();
    al.set_workspace_root(dir.path().to_path_buf());
    let inst = AgentInstance::new(cov_config());
    al.note_read_for_instructions(&inst, std::path::Path::new("C:\\").to_str().unwrap());
    assert!(inst.drain_pending_instructions().is_empty());
}

/// 普通子目录（无 AGENTS.md/CLAUDE.md）→ 不认领（后续新增仍可被发现）。
#[test]
fn note_read_plain_subdir_without_instruction_files_stays_unclaimed() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("plain")).unwrap();
    std::fs::write(dir.path().join("plain/note.txt"), "not instructions").unwrap();

    let al = cov_loop();
    al.set_workspace_root(dir.path().to_path_buf());
    let inst = AgentInstance::new(cov_config());
    al.note_read_for_instructions(&inst, "plain/note.txt");
    assert!(inst.drain_pending_instructions().is_empty());

    // 未认领是持久状态：同一目录重复读也不产生注入。
    al.note_read_for_instructions(&inst, "plain/note.txt");
    assert!(inst.drain_pending_instructions().is_empty());
}

// ---------------------------------------------------------------------------
// P3.1：prefetch_memory_context 门槛链（memory feature 默认开）
// ---------------------------------------------------------------------------

/// auto=false（默认）→ None（feature 关闭语义，不查任何存储）。
#[test]
fn prefetch_memory_disabled_by_default_returns_none() {
    let al = cov_loop();
    let inst = AgentInstance::new(cov_config());
    inst.add_user_message("随便问一句");
    assert_eq!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(al.prefetch_memory_context(&inst)),
        None
    );
}

fn enable_auto_inject(al: &AgentLoop, top_k: usize) {
    *al.memory.memory_inject_cfg.write() = (true, top_k);
}

/// auto=true 但历史里没有 user 消息 → 取信号失败 → None。
#[test]
fn prefetch_memory_without_user_message_returns_none() {
    let al = cov_loop();
    enable_auto_inject(&al, 3);
    let inst = AgentInstance::new(cov_config());
    inst.add_assistant_message("只有助手轮", Vec::new(), None);
    assert_eq!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(al.prefetch_memory_context(&inst)),
        None
    );
}

/// 最新 user 消息为空白 → None。
#[test]
fn prefetch_memory_blank_user_message_returns_none() {
    let al = cov_loop();
    enable_auto_inject(&al, 3);
    let inst = AgentInstance::new(cov_config());
    inst.add_user_message("   ");
    assert_eq!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(al.prefetch_memory_context(&inst)),
        None
    );
}

/// auto=true、有信号，但 manager 未装配（默认）→ None。
#[test]
fn prefetch_memory_without_manager_returns_none() {
    let al = cov_loop();
    enable_auto_inject(&al, 3);
    let inst = AgentInstance::new(cov_config());
    inst.add_user_message("用户喜欢 Rust 吗");
    assert_eq!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(al.prefetch_memory_context(&inst)),
        None
    );
}

/// manager 装配（真实 MemoryManager，无向量库 → 关键词兜底恒空）→
/// 检索 Ok(空) → kept 空 → None（不误注入）。
#[cfg(feature = "memory")]
#[test]
fn prefetch_memory_with_manager_but_empty_hits_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = std::sync::Arc::new(nemesis_memory::manager::MemoryManager::new(
        &nemesis_memory::manager::Config::new(dir.path().join("mem")),
    ));
    let al = cov_loop();
    enable_auto_inject(&al, 3);
    *al.memory.memory_inject_manager.write() = Some(mgr);
    let inst = AgentInstance::new(cov_config());
    inst.add_user_message("用户喜欢 Rust 吗");
    assert_eq!(
        tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(al.prefetch_memory_context(&inst)),
        None
    );
}

// ---------------------------------------------------------------------------
// workflow_edit 会话 section（对话生成）：经 build_messages 渲染三形态
// ---------------------------------------------------------------------------

fn inst_with_user_msg() -> AgentInstance {
    let inst = AgentInstance::new(cov_config());
    inst.add_user_message("帮我整一个工作流");
    inst
}

/// 命名会话 + 引擎未装配 → 诚实注明（引擎 None 短路 `and_then` → 走
/// 「当前不存在」注记；「工作流引擎未装配」串仅在非 workflow 特性构建
/// 的 (Some, None) 臂可达——见交付报告豁免清单）。
#[cfg(feature = "workflow")]
#[test]
fn workflow_edit_named_session_without_engine_notes_honestly() {
    let al = cov_loop();
    *al.pending_workflow_edit.write() = Some(nemesis_types::channel::WorkflowEditTarget {
        workflow_name: Some("ghost-wf".to_string()),
    });
    let msgs = al.build_messages(&inst_with_user_msg());
    let joined: String = msgs
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("Workflow Editor Session"), "got: {joined}");
    assert!(joined.contains("ghost-wf"), "got: {joined}");
    assert!(
        joined.contains("当前不存在——可能是新名字，或已被删除"),
        "engine-absent must be honest: {joined}"
    );
}

/// `_new` 会话（workflow_name=None）→ 新建引导（能力表先行）。
#[cfg(feature = "workflow")]
#[test]
fn workflow_edit_new_session_renders_capability_guidance() {
    let al = cov_loop();
    *al.pending_workflow_edit.write() = Some(nemesis_types::channel::WorkflowEditTarget {
        workflow_name: None,
    });
    let msgs = al.build_messages(&inst_with_user_msg());
    let joined: String = msgs
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("新建工作流"), "got: {joined}");
    assert!(joined.contains("workflow_capabilities"), "got: {joined}");
}

/// 命名会话 + 引擎装配且工作流存在 → 渲染当前定义 YAML。
#[cfg(feature = "workflow")]
#[test]
fn workflow_edit_named_session_with_engine_renders_yaml() {
    let engine = std::sync::Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
    let wf = nemesis_workflow::types::Workflow {
        name: "cov-wf".to_string(),
        description: "覆盖率夹具".to_string(),
        version: "1.0.0".to_string(),
        triggers: Vec::new(),
        nodes: vec![nemesis_workflow::types::NodeDef {
            id: "n1".to_string(),
            node_type: "agent".to_string(),
            config: std::collections::HashMap::new(),
            depends_on: Vec::new(),
            retry_count: 0,
            timeout: None,
            is_terminal: true,
        }],
        edges: Vec::new(),
        variables: std::collections::HashMap::new(),
        metadata: std::collections::HashMap::new(),
    };
    engine
        .register_workflow(wf)
        .expect("register minimal workflow");

    let al = cov_loop();
    *al.workflow_engine.write() = Some(engine);
    *al.pending_workflow_edit.write() = Some(nemesis_types::channel::WorkflowEditTarget {
        workflow_name: Some("cov-wf".to_string()),
    });

    let msgs = al.build_messages(&inst_with_user_msg());
    let joined: String = msgs
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("```yaml"), "yaml block expected: {joined}");
    assert!(joined.contains("cov-wf"), "got: {joined}");
    assert!(
        !joined.contains("工作流引擎未装配"),
        "engine is wired: {joined}"
    );
}

/// 无 workflow_edit 目标（默认）→ 无该 section（字节稳定语义）。
#[test]
fn workflow_edit_absent_yields_no_section() {
    let al = cov_loop();
    let msgs = al.build_messages(&inst_with_user_msg());
    let joined: String = msgs
        .iter()
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!joined.contains("Workflow Editor Session"), "got: {joined}");
}
