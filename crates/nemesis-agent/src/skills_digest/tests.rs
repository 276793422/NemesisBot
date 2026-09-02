//! Tests for skills-catalog digest injection (H3 / P2.2).

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
    // I2 semantics: the injection is NOT persisted in history, so every
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
        },
    ]);

    // First build: digest injected (catalog never seen for this session).
    // H5 merge: the message is wrapped in <system-reminder>.
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
    // byte-identically (I2 stable re-emission — injection is not persisted,
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
        },
    ]);
    let m = agent_loop.build_messages(&instance);
    assert!(
        !m.iter().any(|m| m.content.contains("# Available Skills")),
        "no skills → no digest message"
    );
}
