//! Tests for skills-catalog digest injection (/.2).

use super::*;

fn entry(name: &str, desc: &str) -> SkillCatalogEntry {
    SkillCatalogEntry {
        name: name.to_string(),
        description: desc.to_string(),
    }
}

#[test]
fn test_render_digest_sorted_and_truncated() {
    let skills = vec![
        entry("zeta", "last skill"),
        entry("alpha", "first skill"),
        entry("mid", ""),
    ];
    let d = render_skills_digest(&skills);
    let lines: Vec<&str> = d.lines().collect();
    assert_eq!(lines.len(), 3);
    // Sorted by name.
    assert!(lines[0].starts_with("alpha"));
    assert!(lines[1].starts_with("mid"));
    assert!(lines[2].starts_with("zeta"));
    // Empty description renders as bare name.
    assert_eq!(lines[1], "mid");
    // Truncation at 500 chars.
    let long = entry("long", &"x".repeat(800));
    let d2 = render_skills_digest(&[long]);
    assert!(d2.chars().count() < "long: ".len() + 800);
    assert!(d2.chars().count() >= "long: ".len() + 500);
}

#[test]
fn test_digest_message_replacement_semantics() {
    let m = digest_message("alpha: a\nbeta: b");
    assert!(m.starts_with("# Available Skills"));
    assert!(m.contains("取代之前"));
    assert!(m.contains("skills_list"));
    assert!(m.contains("skill"));
}

#[test]
fn test_digest_state_inject_only_on_change() {
    // semantics: the injection is NOT persisted in history, so every
    // build must re-emit the digest message — byte-identically when
    // unchanged (stable re-emission preserves the provider prefix), and
    // with fresh content when changed.
    let st = DigestState::new();
    let rendered = "alpha: a";
    // First call injects.
    assert_eq!(
        st.should_inject("sess1", rendered).as_deref(),
        Some("alpha: a")
    );
    // Same content again → SAME rendering re-emitted (stable).
    assert_eq!(
        st.should_inject("sess1", rendered).as_deref(),
        Some("alpha: a")
    );
    // Changed content → the NEW content is returned (replacement).
    assert_eq!(
        st.should_inject("sess1", "alpha: a2").as_deref(),
        Some("alpha: a2")
    );
    // Per-session isolation: another session gets its own state.
    assert_eq!(
        st.should_inject("sess2", "alpha: a2").as_deref(),
        Some("alpha: a2")
    );
}

#[test]
fn test_digest_hash_stable_and_sensitive() {
    let h1 = digest_hash("abc");
    assert_eq!(h1, digest_hash("abc"));
    assert_ne!(h1, digest_hash("abd"));
    assert_eq!(h1.len(), 64); // sha256 hex
}

// ---------------------------------------------------------------------------
// Integration through build_messages (goal-required tests)
// ---------------------------------------------------------------------------

// Minimal no-op provider for build_messages construction (chat is never
// called by these tests).
struct NoopProvider;
#[async_trait::async_trait]
impl crate::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<crate::r#loop::LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Err("noop".to_string())
    }
}

#[test]
fn test_skills_digest_injected_when_changed() {
    use crate::r#loop::AgentLoop;
    use crate::types::AgentConfig;

    // Workspace with one skill.
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let skill_dir = ws.join("skills").join("demo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: demo\ndescription: A demo skill\n---\nbody",
    )
    .unwrap();

    let loader = nemesis_skills::loader::SkillsLoader::new(
        &ws.to_string_lossy(),
        "/nonexistent-global",
        "/nonexistent-builtin",
    );
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), AgentConfig::default());
    agent_loop.set_skills_loader(std::sync::Arc::new(loader));

    let instance = crate::instance::AgentInstance::new(AgentConfig {
        system_prompt: Some("SYS".to_string()),
        system_prompt_split: None,
        ..Default::default()
    });
    instance.set_history(vec![
        crate::types::ConversationTurn {
            role: "system".to_string(),
            content: "SYS".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "hello there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ]);

    // First build: digest injected (catalog never seen for this session).
    // merge: the message is wrapped in <system-reminder>.
    let m1 = agent_loop.build_messages(&instance);
    assert!(
        m1.iter().any(|m| {
            m.content.contains("<system-reminder>")
                && m.content.contains("# Available Skills")
                && m.content.contains("demo")
        }),
        "first build injects the catalog (system-reminder wrapped)"
    );
    // Second build, same catalog: the digest message RE-APPEARS
    // byte-identically (stable re-emission — injection is not persisted,
    // so each build re-sends it; identical bytes keep the prefix).
    let m2 = agent_loop.build_messages(&instance);
    assert_eq!(
        m2.len(),
        m1.len(),
        "unchanged catalog re-emits the same message (same count)"
    );
    let strip_time = |c: &str| -> String {
        c.lines()
            .filter(|l| !l.contains("Current Time") && !l.trim_start().starts_with("20"))
            .collect::<Vec<_>>()
            .join(
                "
",
            )
    };
    let snap1: Vec<String> = m1
        .iter()
        .filter(|m| m.content.contains("# Available Skills"))
        .map(|m| strip_time(&m.content))
        .collect();
    let snap2: Vec<String> = m2
        .iter()
        .filter(|m| m.content.contains("# Available Skills"))
        .map(|m| strip_time(&m.content))
        .collect();
    assert_eq!(snap1, snap2, "identical re-emission (time-insensitive)");
}

#[test]
fn test_skills_digest_empty_no_injection() {
    use crate::r#loop::AgentLoop;
    use crate::types::AgentConfig;

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join("skills")).unwrap(); // empty skills dir
    let loader = nemesis_skills::loader::SkillsLoader::new(
        &ws.to_string_lossy(),
        "/nonexistent-global",
        "/nonexistent-builtin",
    );
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), AgentConfig::default());
    agent_loop.set_skills_loader(std::sync::Arc::new(loader));

    let instance = crate::instance::AgentInstance::new(AgentConfig {
        system_prompt: Some("SYS".to_string()),
        system_prompt_split: None,
        ..Default::default()
    });
    instance.set_history(vec![
        crate::types::ConversationTurn {
            role: "system".to_string(),
            content: "SYS".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "hi".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ]);
    let m = agent_loop.build_messages(&instance);
    assert!(
        !m.iter().any(|m| m.content.contains("# Available Skills")),
        "no skills → no digest message"
    );
}

// wave5：DigestState::default 转发 new（80-82）。单元结构体直呼
// Default::default() 会触发 clippy::default_constructed_unit_structs，
// 经泛型间接调用既保 lint 干净又真正执行 trait 转发。
#[test]
fn digest_state_default_delegates_to_new() {
    fn via_default<H: Default>() -> H {
        H::default()
    }
    let state: DigestState = via_default();
    assert_eq!(
        state.should_inject("s1", "rendered"),
        Some("rendered".to_string())
    );
}

// ---------------------------------------------------------------------------
// P2-1（2026-11-19）：技能集变更提醒（change_reminder）
// ---------------------------------------------------------------------------

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn test_change_reminder_baseline_and_unchanged_are_none() {
    let st = DigestState::new();
    // 首见 = 基线：不提醒（目录节首次出现已是足量信号）。
    assert_eq!(st.change_reminder("s1", &names(&["a", "b"])), None);
    // 集相同 = None（零注入字节——cache 纪律核心不变量）；顺序无关
    // （内部 sort+dedup）。
    assert_eq!(st.change_reminder("s1", &names(&["b", "a"])), None);
    // 重复条目 dedup 后同集。
    assert_eq!(st.change_reminder("s1", &names(&["a", "a", "b"])), None);
}

#[test]
fn test_change_reminder_add() {
    let st = DigestState::new();
    st.change_reminder("s1", &names(&["a"]));
    let msg = st
        .change_reminder("s1", &names(&["a", "weather"]))
        .expect("装技能 → 下一轮有提醒");
    assert!(msg.starts_with("# Skills Changed"), "独立节形态：{msg}");
    assert!(msg.contains("+weather"), "{msg}");
    assert!(!msg.contains('-'), "纯新增无移除段：{msg}");
    // 下一轮快照已更新 → 回到 None（merged 字节回到稳定形态）。
    assert_eq!(st.change_reminder("s1", &names(&["a", "weather"])), None);
}

#[test]
fn test_change_reminder_remove_and_uninstall_all() {
    let st = DigestState::new();
    st.change_reminder("s1", &names(&["a", "b"]));
    let msg = st
        .change_reminder("s1", &names(&["a"]))
        .expect("卸技能 → 提醒");
    assert!(msg.contains("-b"), "{msg}");
    assert!(!msg.contains('+'), "纯移除无新增段：{msg}");
    // 卸空：removed 覆盖整个旧集（此时目录节本身消失，提醒行是唯一信号）。
    let msg2 = st.change_reminder("s1", &[]).expect("卸空 → 提醒");
    assert!(msg2.contains("-a"), "{msg2}");
    // 空集基线后装回 → 0→N 提醒。
    let msg3 = st
        .change_reminder("s1", &names(&["back"]))
        .expect("0→N 装 → 提醒");
    assert!(msg3.contains("+back"), "{msg3}");
}

#[test]
fn test_change_reminder_add_and_remove_both() {
    let st = DigestState::new();
    st.change_reminder("s1", &names(&["old"]));
    let msg = st
        .change_reminder("s1", &names(&["new"]))
        .expect("换技能 → 提醒");
    assert!(msg.contains("+new"), "{msg}");
    assert!(msg.contains("-old"), "{msg}");
    // 段序：新增在前移除在后。
    assert!(
        msg.find("+new").unwrap() < msg.find("-old").unwrap(),
        "{msg}"
    );
}

#[test]
fn test_change_reminder_per_session_independent() {
    let st = DigestState::new();
    st.change_reminder("A", &names(&["a"]));
    st.change_reminder("B", &names(&["a"]));
    // A 变化只影响 A；B 同集仍 None。
    assert!(st.change_reminder("A", &names(&["a", "x"])).is_some());
    assert_eq!(st.change_reminder("B", &names(&["a"])), None);
    // session_key=None 的兼容路径落 "" 键自成基线，与具名会话互不串扰。
    assert_eq!(st.change_reminder("", &names(&["a"])), None);
}

#[test]
fn test_change_reminder_clear_all_rebaselines() {
    let st = DigestState::new();
    st.change_reminder("s1", &names(&["a"]));
    assert!(st.change_reminder("s1", &names(&["a", "b"])).is_some());
    // 清空 = 基线重锚：下一调用视为首见（touch-driven 稀有事件的既定
    // 代价，见 DigestState 类型文档）。
    st.clear_all();
    assert_eq!(st.change_reminder("s1", &names(&["a", "b"])), None);
    // 重锚后变化照常提醒（功能恢复）。
    assert!(st.change_reminder("s1", &names(&["a"])).is_some());
}

#[test]
fn test_render_change_reminder_pure_function() {
    let add = render_change_reminder(&["w1", "w2"], &[]);
    assert!(add.contains("+w1、+w2"), "{add}");
    let rem = render_change_reminder(&[], &["gone"]);
    assert!(rem.contains("-gone"), "{rem}");
    let both = render_change_reminder(&["n"], &["o"]);
    assert!(both.contains("skills_list"), "{both}");
}

// 集成：build_messages 链路上装/卸技能 → 下一轮 merged snapshot 带
// 「# Skills Changed」节；再下一轮回到无提醒稳定形态（loader 每 build
// 重扫盘，纯文件系统操作即可驱动）。会话无 session_key（兼容路径）走
// "" 基线键。
#[test]
fn test_skills_change_reminder_in_build_messages() {
    use crate::r#loop::AgentLoop;
    use crate::types::AgentConfig;

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().join("ws");
    let mk_skill = |name: &str| {
        let d = ws.join("skills").join(name);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Demo {name}\n---\nbody"),
        )
        .unwrap();
    };
    mk_skill("demo");
    let loader = nemesis_skills::loader::SkillsLoader::new(
        &ws.to_string_lossy(),
        "/nonexistent-global",
        "/nonexistent-builtin",
    );
    let agent_loop = AgentLoop::new(Box::new(NoopProvider), AgentConfig::default());
    agent_loop.set_skills_loader(std::sync::Arc::new(loader));

    let instance = crate::instance::AgentInstance::new(AgentConfig {
        system_prompt: Some("SYS".to_string()),
        system_prompt_split: None,
        ..Default::default()
    });
    instance.set_history(vec![
        crate::types::ConversationTurn {
            role: "system".to_string(),
            content: "SYS".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
        crate::types::ConversationTurn {
            role: "user".to_string(),
            content: "hello there".to_string(),
            tool_calls: vec![],
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
            image_refs: Vec::new(),
        },
    ]);

    // ① 首建：基线——有目录节，无提醒。
    let m1 = agent_loop.build_messages(&instance);
    assert!(m1.iter().any(|m| m.content.contains("# Available Skills")));
    assert!(
        !m1.iter().any(|m| m.content.contains("# Skills Changed")),
        "首建落基线不提醒"
    );

    // ② 装技能（纯文件操作，下一 build 重扫可见）→ 提醒节。
    mk_skill("extra");
    let m2 = agent_loop.build_messages(&instance);
    assert!(
        m2.iter()
            .any(|m| m.content.contains("# Skills Changed") && m.content.contains("+extra")),
        "装技能后下一轮有提醒"
    );

    // ③ 再下一轮：快照已更新 → 提醒消失（merged 字节回到稳定形态）。
    let m3 = agent_loop.build_messages(&instance);
    assert!(
        !m3.iter().any(|m| m.content.contains("# Skills Changed")),
        "提醒只在变化轮出现"
    );

    // ④ 卸技能 → -extra 提醒。
    std::fs::remove_dir_all(ws.join("skills").join("extra")).unwrap();
    let m4 = agent_loop.build_messages(&instance);
    assert!(
        m4.iter()
            .any(|m| m.content.contains("# Skills Changed") && m.content.contains("-extra")),
        "卸技能后下一轮有提醒"
    );
}
