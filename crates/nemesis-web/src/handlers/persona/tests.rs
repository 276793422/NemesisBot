use super::*;

#[test]
fn test_ensure_initialized_self_heals_when_active_json_missing() {
    // Regression: a persona-shop download creates personas/<id>/ (hence the
    // personas/ dir) without writing _active.json. ensure_initialized must
    // STILL run and create _active.json — otherwise cmd_current fails with
    // "failed to read _active.json". The gate must be _active.json, not the
    // mere existence of the personas/ dir.
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().to_str().unwrap();

    // Simulate the post-download half-initialized state.
    let downloaded = tmp.path().join("personas").join("some-downloaded");
    std::fs::create_dir_all(&downloaded).unwrap();
    assert!(!tmp.path().join("personas").join("_active.json").exists());

    let handler = PersonaHandler::new();
    handler.ensure_initialized(workspace).unwrap();

    // Init must have run despite personas/ already existing.
    assert!(
        tmp.path().join("personas").join("_active.json").exists(),
        "_active.json should be created even when personas/ already exists"
    );
    // The downloaded persona dir must be preserved.
    assert!(
        downloaded.exists(),
        "existing persona dir must not be removed"
    );
}

#[test]
fn test_is_agent_file() {
    assert!(is_agent_file("engineering/engineering-code-reviewer.md"));
    assert!(is_agent_file("game-development/unity/unity-architect.md"));
    assert!(is_agent_file("marketing/marketing-douyin-strategist.md"));
    assert!(!is_agent_file("README.md"));
    assert!(!is_agent_file("scripts/convert.sh"));
    assert!(!is_agent_file("integrations/claude-code/README.md"));
    assert!(!is_agent_file("strategy/playbooks/phase-0-discovery.md"));
    assert!(!is_agent_file("engineering/README.md"));
    assert!(!is_agent_file("CONTRIBUTING.md"));
    assert!(!is_agent_file("examples/something.py"));
}

#[test]
fn test_parse_agent_from_path() {
    let entry = parse_agent_from_path("engineering/engineering-code-reviewer.md");
    assert_eq!(entry.id, "engineering-code-reviewer");
    assert_eq!(entry.name, "Engineering Code Reviewer");
    assert_eq!(entry.category, "开发");
    assert_eq!(entry.path, "engineering/engineering-code-reviewer.md");

    let entry2 = parse_agent_from_path("game-development/unity/unity-architect.md");
    assert_eq!(entry2.id, "unity-architect");
    assert_eq!(entry2.name, "Unity Architect");
    assert_eq!(entry2.category, "游戏开发");

    let entry3 = parse_agent_from_path("marketing/marketing-douyin-strategist.md");
    assert_eq!(entry3.category, "营销");
}

#[test]
fn test_map_category() {
    assert_eq!(map_category("engineering"), "开发");
    assert_eq!(map_category("marketing"), "营销");
    assert_eq!(map_category("security"), "安全");
    assert_eq!(map_category("design"), "创意");
    assert_eq!(map_category("game-development"), "游戏开发");
    assert_eq!(map_category("unknown"), "通用");
}

#[test]
fn test_parse_frontmatter() {
    let content = "---\nname: Code Reviewer\ndescription: Expert code reviewer\nemoji: 👁️\ncolor: purple\nvibe: Reviews code like a mentor.\n---\n\n# Content";
    let fm = parse_frontmatter(content).unwrap();
    assert_eq!(fm.name, "Code Reviewer");
    assert_eq!(fm.emoji, "👁️");
    assert_eq!(fm.description, "Expert code reviewer");
    assert_eq!(fm.vibe, "Reviews code like a mentor.");
}

#[test]
fn test_parse_frontmatter_missing() {
    assert!(parse_frontmatter("No frontmatter here").is_none());
}

#[test]
fn test_strip_emoji() {
    assert_eq!(
        strip_emoji("🧠 Your Identity & Memory"),
        "your identity & memory"
    );
    assert_eq!(strip_emoji("Critical Rules"), "critical rules");
    assert_eq!(strip_emoji("🎯 Your Core Mission"), "your core mission");
}

#[test]
fn test_parse_sections() {
    let content = "---\nname: Test\n---\n\n# Test Agent\n\nYou are a test agent.\n\n## 🧠 Your Identity\n- Role: Expert\n\n## 🔧 Critical Rules\n1. Be specific\n\n## 🎯 Core Mission\nDo the thing\n";
    let parsed = parse_sections(content);
    assert!(
        parsed.preamble.contains("You are a test agent"),
        "preamble should capture role description"
    );
    assert_eq!(parsed.sections.len(), 3);
    assert!(parsed.sections[0].title.contains("Identity"));
    assert!(parsed.sections[1].title.contains("Critical Rules"));
    assert!(parsed.sections[2].title.contains("Core Mission"));
    assert!(parsed.sections[0].content.contains("Role: Expert"));
}

#[test]
fn test_classify_section() {
    assert!(matches!(
        classify_section("🧠 Your Identity & Memory"),
        SectionTarget::Identity
    ));
    assert!(matches!(
        classify_section("🔧 Critical Rules"),
        SectionTarget::Soul
    ));
    assert!(matches!(
        classify_section("Communication Style"),
        SectionTarget::Soul
    ));
    assert!(matches!(
        classify_section("🎯 Core Mission"),
        SectionTarget::Agent
    ));
    assert!(matches!(
        classify_section("Workflow Process"),
        SectionTarget::Agent
    ));
    assert!(matches!(
        classify_section("Review Checklist"),
        SectionTarget::Soul
    ));
}

#[test]
fn test_convert_agent_md() {
    let content = "---\nname: Code Reviewer\ndescription: Expert reviewer\nemoji: 👁️\nvibe: Reviews like a mentor.\n---\n\n# Code Reviewer Agent\n\n## 🧠 Your Identity & Memory\n- Role: Code review specialist\n\n## 🔧 Critical Rules\n1. Be specific\n2. Explain why\n\n## 🎯 Core Mission\nProvide code reviews that improve quality.\n\n## 💬 Communication Style\nStart with a summary.\n";
    let files = convert_agent_md(content);

    // Identity should contain persona info and identity section
    assert!(files.identity.contains("Code Reviewer"));
    assert!(files.identity.contains("👁️"));
    assert!(files.identity.contains("Role: Code review specialist"));

    // Soul should contain rules and communication
    assert!(files.soul.contains("Critical Rules"));
    assert!(files.soul.contains("Communication Style"));
    assert!(files.soul.contains("Be specific"));

    // Agent extra should contain mission
    assert!(files.agent_extra.contains("Core Mission"));
    assert!(files.agent_extra.contains("improve quality"));

    // Tools extra should be empty (no tools section)
    assert!(files.tools_extra.is_empty());
}

#[test]
fn test_convert_preserves_system_agent_md() {
    let content = "---\nname: Test Agent\nemoji: 🤖\n---\n\n# Test\n\n## Core Mission\nDo stuff.\n";
    let files = convert_agent_md(content);
    // agent_extra is meant to be appended to DEFAULT_AGENT_MD
    assert!(files.agent_extra.contains("Test Agent"));
    assert!(files.agent_extra.contains("Do stuff."));
}

#[test]
fn test_parse_sections_no_frontmatter() {
    let content = "# Agent\n\n## Section One\nContent one\n\n## Section Two\nContent two\n";
    let parsed = parse_sections(content);
    assert_eq!(parsed.sections.len(), 2);
    assert_eq!(parsed.sections[0].title, "Section One");
    assert_eq!(parsed.sections[1].title, "Section Two");
}

#[test]
fn test_convert_real_agent_file() {
    // Test with a real agency-agent file if available locally
    let local_path = "C:/AI/NemesisBot/agency-agents/engineering/engineering-code-reviewer.md";
    if !std::path::Path::new(local_path).exists() {
        eprintln!("Skipping real agent file test (file not found)");
        return;
    }
    let content = std::fs::read_to_string(local_path).expect("read agent file");

    // Parse frontmatter
    let fm = parse_frontmatter(&content).expect("should have frontmatter");
    assert_eq!(fm.name, "Code Reviewer");
    assert_eq!(fm.emoji, "👁️");
    assert!(!fm.description.is_empty());

    // Parse sections
    let parsed = parse_sections(&content);
    assert!(
        parsed.sections.len() >= 4,
        "expected at least 4 sections, got {}",
        parsed.sections.len()
    );

    // Verify section titles contain expected keywords
    let titles: Vec<String> = parsed
        .sections
        .iter()
        .map(|s| strip_emoji(&s.title))
        .collect();
    assert!(
        titles.iter().any(|t| t.contains("identity")),
        "missing identity section"
    );
    assert!(
        titles
            .iter()
            .any(|t| t.contains("critical rule") || t.contains("communication")),
        "missing rules section"
    );

    // Convert and verify
    let files = convert_agent_md(&content);
    assert!(files.identity.contains("Code Reviewer"));
    assert!(files.soul.contains("Code Reviewer"));
    assert!(!files.agent_extra.is_empty());
}

#[test]
fn test_convert_real_agent_files_batch() {
    // Test a few different agent files to ensure robustness
    let test_cases = [
        (
            "C:/AI/NemesisBot/agency-agents/engineering/engineering-code-reviewer.md",
            "Code Reviewer",
        ),
        (
            "C:/AI/NemesisBot/agency-agents/security/security-architect.md",
            "Security Architect",
        ),
        (
            "C:/AI/NemesisBot/agency-agents/marketing/marketing-douyin-strategist.md",
            "Douyin Strategist",
        ),
    ];

    for (path, expected_name_part) in &test_cases {
        if !std::path::Path::new(path).exists() {
            continue;
        }
        let content = std::fs::read_to_string(path).unwrap_or_default();
        if content.is_empty() {
            continue;
        }
        let fm = parse_frontmatter(&content);
        assert!(fm.is_some(), "frontmatter missing in {}", path);
        let files = convert_agent_md(&content);
        assert!(!files.identity.is_empty(), "identity empty for {}", path);
        assert!(!files.soul.is_empty(), "soul empty for {}", path);
        assert!(
            !files.agent_extra.is_empty(),
            "agent_extra empty for {}",
            path
        );
        // AGENT.md must preserve system defaults
        let full_agent = format!("{}\n{}", DEFAULT_AGENT_MD, files.agent_extra);
        assert!(
            full_agent.contains("首次运行"),
            "AGENT.md missing system defaults"
        );
    }
}

#[test]
fn test_local_management() {
    use tempfile::TempDir;

    let tmp = TempDir::new().unwrap();
    let ws = tmp.path().to_string_lossy().to_string();

    // Create initial persona files
    std::fs::write(
        std::path::Path::new(&ws).join("IDENTITY.md"),
        "# Test Identity",
    )
    .unwrap();
    std::fs::write(std::path::Path::new(&ws).join("SOUL.md"), "# Test Soul").unwrap();
    std::fs::write(std::path::Path::new(&ws).join("AGENT.md"), "# Test Agent").unwrap();
    std::fs::write(std::path::Path::new(&ws).join("TOOLS.md"), "# Test Tools").unwrap();

    let handler = PersonaHandler::new();

    // Test auto-migration
    handler.ensure_initialized(&ws).unwrap();

    // Test current
    let result = handler.cmd_current(&ws).unwrap().unwrap();
    assert_eq!(result["active_dir"], "default");
    assert_eq!(result["name"], "default");

    // Test list
    let result = handler.cmd_list(&ws).unwrap().unwrap();
    let personas = result["personas"].as_array().unwrap();
    assert_eq!(personas.len(), 1);
    assert_eq!(personas[0]["dir"], "default");
    assert!(personas[0]["is_default"].as_bool().unwrap());

    // Test file.get
    let result = handler
        .cmd_file_get(&ws, "default", "IDENTITY.md")
        .unwrap()
        .unwrap();
    assert_eq!(result["content"], "# Test Identity");

    // Test file.save
    handler
        .cmd_file_save(&ws, "default", "IDENTITY.md", "# Updated Identity")
        .unwrap();
    let result = handler
        .cmd_file_get(&ws, "default", "IDENTITY.md")
        .unwrap()
        .unwrap();
    assert_eq!(result["content"], "# Updated Identity");

    // Test remove default fails
    assert!(handler.cmd_remove(&ws, "default").is_err());
}

// ---- pure conversion-engine helpers ----

#[test]
fn extract_identity_info_parses_name_and_emoji() {
    let md = "**姓名：** Alice\nsome text\n**表情符号：** 🎯\n";
    let (name, emoji) = extract_identity_info(md);
    assert_eq!(name, "Alice");
    assert_eq!(emoji, "🎯");
}

#[test]
fn extract_identity_info_defaults_when_missing() {
    let (name, emoji) = extract_identity_info("no identity info here");
    assert_eq!(name, "default");
    assert_eq!(emoji, "🤖");
}

#[test]
fn parse_frontmatter_full() {
    let md = "---\nname: \"Bot\"\nemoji: 🤖\ndescription: desc\ncolor: red\ntools: web\nvibe: chill\n---\nbody";
    let fm = parse_frontmatter(md).expect("frontmatter");
    assert_eq!(fm.name, "Bot");
    assert_eq!(fm.emoji, "🤖");
    assert_eq!(fm.description, "desc");
    assert_eq!(fm.color, "red");
    assert_eq!(fm.tools, "web");
    assert_eq!(fm.vibe, "chill");
}

#[test]
fn parse_frontmatter_none_without_marker() {
    assert!(parse_frontmatter("no frontmatter").is_none());
}

#[test]
fn parse_frontmatter_none_without_name() {
    // name is required — empty name → None.
    assert!(parse_frontmatter("---\nemoji: 🤖\n---\n").is_none());
}

#[test]
fn strip_emoji_removes_emoji_and_lowercases() {
    assert_eq!(strip_emoji("🎯 Target"), "target");
    assert_eq!(strip_emoji("Plain"), "plain");
}

#[test]
fn is_emoji_char_classifies_correctly() {
    assert!(is_emoji_char('🎯'));
    assert!(!is_emoji_char('a'));
    assert!(!is_emoji_char(' '));
}

#[test]
fn classify_section_routes_by_keyword() {
    assert!(matches!(
        classify_section("🪪 Identity"),
        SectionTarget::Identity
    ));
    assert!(matches!(
        classify_section("Memory"),
        SectionTarget::Identity
    ));
    assert!(matches!(
        classify_section("⚠️ Critical Rules"),
        SectionTarget::Soul
    ));
    assert!(matches!(
        classify_section("Communication Style"),
        SectionTarget::Soul
    ));
    assert!(matches!(classify_section("🔧 Tools"), SectionTarget::Tools));
    assert!(matches!(
        classify_section("Integrations"),
        SectionTarget::Tools
    ));
    // Everything else → Agent.
    assert!(matches!(
        classify_section("Core Mission"),
        SectionTarget::Agent
    ));
}

#[test]
fn build_persona_json_with_frontmatter() {
    let fm = Frontmatter {
        name: "Bot".into(),
        emoji: "🤖".into(),
        description: "d".into(),
        vibe: "v".into(),
        color: "c".into(),
        tools: "t".into(),
        raw_yaml: "name: Bot".into(),
    };
    let obj = build_persona_json(Some(&fm));
    assert_eq!(obj["name"], "Bot");
    assert_eq!(obj["color"], "c");
    assert_eq!(obj["tools"], "t");
    assert_eq!(obj["vibe"], "v");
    assert_eq!(obj["frontmatter"], "name: Bot");
}

#[test]
fn build_persona_json_without_frontmatter_uses_defaults() {
    let obj = build_persona_json(None);
    assert_eq!(obj["name"], "Unknown");
    assert_eq!(obj["emoji"], "🤖");
    assert_eq!(obj["description"], "");
    // No optional keys when frontmatter absent.
    assert!(obj.get("color").is_none());
}

#[test]
fn parse_sections_splits_preamble_and_h2() {
    let md = "---\nname: X\n---\n\nI am the preamble.\n\n## Section A\n\ncontent a\n\n## Section B\n\ncontent b\n";
    let parsed = parse_sections(md);
    assert!(parsed.preamble.contains("I am the preamble"));
    assert_eq!(parsed.sections.len(), 2);
    assert_eq!(parsed.sections[0].title, "Section A");
    assert_eq!(parsed.sections[1].title, "Section B");
}

#[test]
fn convert_agent_md_routes_sections_to_files() {
    let md = "---\nname: TestBot\nemoji: 🤖\ndescription: a test bot\nvibe: friendly\ncolor: blue\ntools: web\n---\n\nI am a helpful assistant.\n\n## 🪪 Identity\n\nMy name is TestBot.\n\n## ⚠️ Critical Rules\n\nBe safe.\n\n## 🔧 Tools\n\nUse web search.\n";
    let files = convert_agent_md(md);
    // Identity file gets the Identity section + 基本信息 block.
    assert!(files.identity.contains("TestBot"));
    assert!(files.identity.contains("基本信息"));
    assert!(files.identity.contains("My name is TestBot."));
    // Soul file gets Critical Rules.
    assert!(files.soul.contains("Critical Rules"));
    assert!(files.soul.contains("Be safe."));
    // Tools extra gets the Tools section.
    assert!(files.tools_extra.contains("Use web search."));
    // Preamble is folded into identity.
    assert!(files.identity.contains("I am a helpful assistant."));
}

#[test]
fn convert_agent_md_empty_tools_when_no_tools_section() {
    let md = "---\nname: Minimal\n---\n\n## Identity\n\njust identity\n";
    let files = convert_agent_md(md);
    assert!(files.tools_extra.is_empty());
}
