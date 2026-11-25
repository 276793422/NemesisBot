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

    // `expected_name_part` is documentation-only (which repo file maps to
    // which persona name); underscored to keep the test build warning-free.
    for (path, _expected_name_part) in &test_cases {
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

// pure conversion-engine helpers ----

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
fn extract_identity_info_strips_trailing_bold_markers() {
    // 模板形态「- **姓名：Alice**」（值整体带强调标记）
    // 的尾部 `**` 必须剥掉——trim 链只剥前导星号，残留曾写进 PERSONA.json
    // 展示名（Dashboard 人格名带星号）。全角/半角冒号两臂都要干净。
    let (name, _) = extract_identity_info("- **姓名：Alice**\n");
    assert_eq!(name, "Alice", "全角冒号 + 尾部强调标记");
    let (name, _) = extract_identity_info("- **姓名: Bob**\n");
    assert_eq!(name, "Bob", "半角冒号 + 尾部强调标记");
    let (name, _) = extract_identity_info("- **姓名：** Carol\n");
    assert_eq!(name, "Carol", "既有模板形态不回归");
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

// ============================================================
// 覆盖率：shop.* 快乐路径离线覆盖。
// GITHUB_API 是硬编码 const（persona.rs:25，无注入点），但三级缓存
// （TREE_CACHE / FM_CACHE / CONTENT_CACHE）就是天然注入点——预填缓存后
// fetch_tree 走 命中臂、fetch_agent_content 走 命中臂，全程无网络。
// 本文件是 persona 的子模块，可直接访问这些私有 static。
// ============================================================

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 测试 RequestContext（sibling 测试模块复用：recommend_tests 等）。
pub(crate) fn shop_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 预填 树缓存 + frontmatter 缓存（browse/search 只依赖这两级）。
fn seed_tree_and_fm() {
    *TREE_CACHE.lock().unwrap() = Some(vec![
        ("engineering/rust-dev.md".to_string(), 100),
        ("security/pentest-pro.md".to_string(), 200),
    ]);
    let mut fm = FM_CACHE.lock().unwrap();
    // 复合键（2026-11-15）：`<source>/<id>`，browse/search 按 agency 复合键取。
    fm.insert(
        "agency/rust-dev".to_string(),
        Frontmatter {
            name: "Rusty".to_string(),
            emoji: "🦀".to_string(),
            description: "Rust expert".to_string(),
            vibe: String::new(),
            color: String::new(),
            tools: String::new(),
            raw_yaml: String::new(),
        },
    );
}

const AGENT_MD_SAMPLE: &str = "---\nname: Code Reviewer\ndescription: Expert reviewer\nemoji: 👁️\nvibe: Reviews like a mentor.\n---\n\n# Code Reviewer Agent\n\n## 🧠 Your Identity & Memory\n- Role: Code review specialist\n\n## 🔧 Critical Rules\n1. Be specific\n";

/// shop.* 测试串行锁：TREE_CACHE/FM_CACHE/CONTENT_CACHE 是进程级全局，
/// shop.refresh（persona_extra/persona_more 里）和本文件的种子+断言互踩
/// （首闸门 3 失败的根因：并发 invalidate 清掉了别家种子 → fetch_tree 落到
/// 真网络取回 270 条真数据）。凡触碰这三级缓存的测试都必须先拿这把锁。
pub(crate) static SHOP_TEST_LOCK: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

#[tokio::test]
async fn shop_browse_offline_via_seeded_caches() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    seed_tree_and_fm();
    // 预装一个 persona → installed=true 臂。注意 installed 探测路径是
    // personas/<id>（id=文件名 stem，不含目录），不是仓库的分类子路径；
    // 安装态口径（2026-11-15）：PERSONA.json 在场——只雇佣专家落盘的
    // raw.md-only 目录不算已安装。
    let pdir = dir.path().join("personas/pentest-pro");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("PERSONA.json"), r#"{"name": "pentest-pro"}"#).unwrap();

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd("shop.browse", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let items = out["items"].as_array().unwrap();
    // 2 条目录源 + builtin 伪源合并的内建 bug-analyst（内嵌模板随编译期
    // 进列表，division「内置」）。
    assert_eq!(items.len(), 3);
    let rusty = items.iter().find(|i| i["id"] == "rust-dev").unwrap();
    // FM_CACHE 覆盖 parse_agent_from_path 的默认名/emoji。
    assert_eq!(rusty["name"], "Rusty");
    assert_eq!(rusty["emoji"], "🦀");
    assert_eq!(rusty["division"], "开发");
    assert_eq!(rusty["installed"], false);
    assert_eq!(rusty["source"], "agency");
    let pentest = items.iter().find(|i| i["id"] == "pentest-pro").unwrap();
    assert_eq!(pentest["installed"], true);
    let builtin = items.iter().find(|i| i["id"] == "bug-analyst").unwrap();
    assert_eq!(builtin["source"], "builtin");
    assert_eq!(builtin["division"], "内置");
    assert_eq!(builtin["installed"], false);

    // division 过滤臂：只要安全类（builtin 条目 division=内置，不混入）。
    let out = h
        .handle_cmd(
            "shop.browse",
            Some(serde_json::json!({"division": "安全"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "pentest-pro");
    invalidate_cache();
}

#[tokio::test]
async fn shop_search_offline_via_seeded_caches() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    seed_tree_and_fm();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);

    // 命中：frontmatter 名 "Rusty" 进 haystack。
    let out = h
        .handle_cmd(
            "shop.search",
            Some(serde_json::json!({"query": "rusty"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["items"].as_array().unwrap().len(), 1);

    // 不命中。
    let out = h
        .handle_cmd(
            "shop.search",
            Some(serde_json::json!({"query": "zzzqqq"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["items"].as_array().unwrap().len(), 0);

    // 空 query = browse 全量（2 目录源 + 1 builtin）。
    let out = h
        .handle_cmd("shop.search", Some(serde_json::json!({"query": ""})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["items"].as_array().unwrap().len(), 3);

    // builtin 条目可被搜到（haystack 含「内置」与 id）。
    let out = h
        .handle_cmd(
            "shop.search",
            Some(serde_json::json!({"query": "bug-analyst"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let items = out["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "bug-analyst");
    assert_eq!(items[0]["source"], "builtin");
    invalidate_cache();
}

#[tokio::test]
async fn shop_preview_offline_via_content_cache() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    // 内容缓存命中 → fetch_agent_content 不发网络请求（复合键）。
    CONTENT_CACHE.lock().unwrap().insert(
        "agency/code-reviewer".to_string(),
        AGENT_MD_SAMPLE.to_string(),
    );

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.preview",
            Some(serde_json::json!({"id": "code-reviewer"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["id"], "code-reviewer");
    assert_eq!(out["name"], "Code Reviewer", "frontmatter 名透传");
    assert_eq!(out["emoji"], "👁️");
    assert_eq!(out["installed"], false);
    assert_eq!(out["raw"], AGENT_MD_SAMPLE);
    let converted = &out["converted"];
    assert!(
        converted["IDENTITY.md"]
            .as_str()
            .unwrap()
            .contains("Code Reviewer")
    );
    assert!(
        converted["SOUL.md"]
            .as_str()
            .unwrap()
            .contains("Critical Rules")
    );
    assert!(!converted["AGENT.md"].as_str().unwrap().is_empty());
    assert!(
        converted["PERSONA.json"]
            .as_str()
            .unwrap()
            .contains("\"name\"")
    );
    invalidate_cache();
}

#[tokio::test]
async fn shop_download_offline_via_content_cache() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    CONTENT_CACHE.lock().unwrap().insert(
        "agency/code-reviewer".to_string(),
        AGENT_MD_SAMPLE.to_string(),
    );

    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "code-reviewer"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["downloaded"], true);
    assert_eq!(out["name"], "Code Reviewer");
    let pdir = dir.path().join("personas/code-reviewer");
    for f in [
        "PERSONA.json",
        "IDENTITY.md",
        "SOUL.md",
        "AGENT.md",
        "TOOLS.md",
        "HEARTBEAT.md",
    ] {
        assert!(pdir.join(f).exists(), "download 必须落盘 {f}");
    }
    // ensure_initialized 副作用：default persona + _active.json 就位。
    assert!(dir.path().join("personas/_active.json").exists());

    // 重复下载 → already installed。
    let err = h
        .handle_cmd(
            "shop.download",
            Some(serde_json::json!({"id": "code-reviewer"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("already installed"), "{err}");
    invalidate_cache();
}

// ============================================================
// 覆盖率：超市数据源降级臂（2026-11-12 附加资源 C-ABI 插件）。
// 分类纯函数 / 消息构造纯函数 / ShopSource 序列化 / tree_path_for 匹配规则 /
// refresh 强制臂的缓存语义 / 快照树缺席形态（测试进程无桥注入）。
// ============================================================

#[test]
fn classify_github_error_category_table() {
    // 顺序敏感的特定类别：rate limit（403+专属文案）先于泛 4xx。
    assert_eq!(
        classify_github_error(
            "GitHub tree API returned 403 Forbidden: {\"message\":\"API rate limit exceeded\"}"
        ),
        "rate_limited"
    );
    assert_eq!(
        classify_github_error("GitHub tree API returned 403 Forbidden: x"),
        "rate_limited"
    );
    // upstream_gone：404（仓库/分支被删主形态）/ 409。
    assert_eq!(
        classify_github_error("GitHub tree API returned 404 Not Found: {}"),
        "upstream_gone"
    );
    assert_eq!(
        classify_github_error("GitHub tree API returned 409 Conflict: {}"),
        "upstream_gone"
    );
    // network：reqwest source 链展开后的各形态。
    assert_eq!(
        classify_github_error(
            "GitHub tree API error: error sending request for url (https://api.github.com/...): client error (Connect): dns error"
        ),
        "network"
    );
    assert_eq!(
        classify_github_error(
            "GitHub contents API error: error sending request for url (...): operation timed out"
        ),
        "network"
    );
    assert_eq!(
        classify_github_error("connect error (os error 10061)"),
        "network"
    );
    assert_eq!(classify_github_error("connection reset by peer"), "network");
    assert_eq!(
        classify_github_error("stream closed: broken pipe"),
        "network"
    );
    // http_5xx / http_4xx / other。
    assert_eq!(
        classify_github_error("GitHub tree API returned 502 Bad Gateway: {}"),
        "http_5xx"
    );
    assert_eq!(
        classify_github_error("GitHub tree API returned 503 Service Unavailable: {}"),
        "http_5xx"
    );
    assert_eq!(
        classify_github_error("GitHub tree API returned 418 Teapot: {}"),
        "http_4xx"
    );
    assert_eq!(
        classify_github_error("failed to parse tree response: eof"),
        "other"
    );
}

#[test]
fn refresh_failure_message_both_arms() {
    let with = refresh_failure_message("network", Some("8ae6164 @ 2026-11-12"));
    assert!(with.contains("无法刷新"), "{with}");
    assert!(with.contains("上游不可达（network）"), "{with}");
    assert!(with.contains("内置快照（8ae6164 @ 2026-11-12）"), "{with}");
    let without = refresh_failure_message("upstream_gone", None);
    assert!(without.contains("无法刷新"), "{without}");
    assert!(without.contains("无内置快照插件"), "{without}");
    assert!(!without.contains("当前展示"), "{without}");
}

#[test]
fn shop_source_serialization_shapes() {
    // live：as_str=live、info=null。
    let live = ShopSource::Live;
    assert_eq!(live.as_str(), "live");
    assert_eq!(live.info_json(), serde_json::Value::Null);
    // snapshot：info 四字段齐全。
    let snap = ShopSource::Snapshot(SnapshotInfo {
        commit: "8ae6164602c95ca78828b148b9052460ee426885".to_string(),
        fetched_at: "2026-11-12T12:22:00Z".to_string(),
        file_count: 330,
        label: "8ae6164 @ 2026-11-12".to_string(),
    });
    assert_eq!(snap.as_str(), "snapshot");
    let info = snap.info_json();
    assert_eq!(info["commit"], "8ae6164602c95ca78828b148b9052460ee426885");
    assert_eq!(info["fetched_at"], "2026-11-12T12:22:00Z");
    assert_eq!(info["file_count"], 330);
    assert_eq!(info["label"], "8ae6164 @ 2026-11-12");
}

#[test]
fn tree_path_for_matching_rules() {
    let tree = vec![
        ("engineering/rust-dev.md".to_string(), 100),
        ("game-dev/unity/unity-architect.md".to_string(), 200),
    ];
    // stem 命中（id 不带 .md）
    assert_eq!(
        tree_path_for(&tree, "rust-dev"),
        Some("engineering/rust-dev.md")
    );
    // 深路径 stem 命中
    assert_eq!(
        tree_path_for(&tree, "unity-architect"),
        Some("game-dev/unity/unity-architect.md")
    );
    // 完整文件名命中（id 带 .md）
    assert_eq!(
        tree_path_for(&tree, "rust-dev.md"),
        Some("engineering/rust-dev.md")
    );
    // 无命中
    assert_eq!(tree_path_for(&tree, "no-such"), None);
    // stem 不前缀误命中：另一文件名的前缀子串不算
    let tree2 = vec![("x/rust-dev-pro.md".to_string(), 1)];
    assert_eq!(tree_path_for(&tree2, "rust-dev"), None);
}

#[tokio::test]
async fn shop_refresh_forced_success_leaves_tree_cache_empty() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    *TREE_CACHE.lock().unwrap() = Some(vec![("x/y.md".to_string(), 1)]);
    TEST_REFRESH_MODE.store(1, std::sync::atomic::Ordering::Relaxed);
    invalidate_cache();
    let ok = refresh_fetch_tree().await;
    assert!(ok.is_ok(), "强制成功臂必须 Ok");
    // 强制臂/快照臂都不写 TREE_CACHE（缓存只服务在线臂）→ refresh 后树缓存空。
    assert!(TREE_CACHE.lock().unwrap().is_none());
    TEST_REFRESH_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
    invalidate_cache();
}

#[tokio::test]
async fn shop_refresh_forced_failure_classifies() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    TEST_REFRESH_MODE.store(2, std::sync::atomic::Ordering::Relaxed);
    let err = refresh_fetch_tree().await.unwrap_err();
    assert!(err.contains("forced for test"), "{err}");
    assert_eq!(
        classify_github_error(&err),
        "network",
        "强制文案必须落 network 类"
    );
    TEST_REFRESH_MODE.store(0, std::sync::atomic::Ordering::Relaxed);
}

#[test]
fn snapshot_tree_absent_in_test_process() {
    // nemesis-web 测试进程从不装载 plugin_resources（桥空）→ None =
    // 纯在线形态。若未来桥被注入（不应发生），该断言会翻红提示重新审视。
    assert!(
        snapshot_tree().is_none(),
        "测试进程不应有 resources 桥（无 loader 装配）"
    );
}

#[tokio::test]
async fn shop_browse_seeded_cache_reports_live_source() {
    let _shop_guard = SHOP_TEST_LOCK.lock().await;
    let dir = tempfile::tempdir().unwrap();
    seed_tree_and_fm();
    let h = PersonaHandler::new();
    let ctx = shop_ctx(&dir);
    let out = h
        .handle_cmd("shop.browse", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    // 树缓存命中 = 在线臂（缓存只由在线臂写入）→ source=live、info=null。
    assert_eq!(out["source"].as_str(), Some("live"));
    assert_eq!(out["snapshot_info"], serde_json::Value::Null);
    invalidate_cache();
}

// BUG-20261116-016：目录符号链接环——旧实现 `src_path.is_dir()` 跟随
// 链接（metadata 语义），链接指回祖先目录时无限递归拷贝，以
// 「loop\loop\...」假路径重复整子树直到 OS 路径长度上限才收敛（与
// dossier.rs copy_dir_sorted 已修先例同族；仓内其余 7 份目录递归实现
// 均有防护，persona 这份是漏接位点）。链接创建免提权降级链：
// symlink_dir（开发者模式/管理员）→ junction（mklink /J，任何用户）。
#[cfg(windows)]
fn persona_make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    use std::os::windows::fs::symlink_dir;
    if symlink_dir(target, link).is_ok() {
        return;
    }
    let out = std::process::Command::new("cmd")
        .args(["/c", "mklink", "/J"])
        .arg(link)
        .arg(target)
        .output()
        .expect("spawn cmd mklink");
    assert!(
        out.status.success(),
        "创建目录链接失败: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(not(windows))]
fn persona_make_dir_link(target: &std::path::Path, link: &std::path::Path) {
    std::os::unix::fs::symlink(target, link).unwrap();
}

#[test]
fn copy_dir_recursive_skips_symlink_cycles() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("persona_src");
    let sub = src.join("assets").join("deep");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("a.md"), "data").unwrap();

    // 链接环：persona_src/loop → persona_src 自身
    persona_make_dir_link(&src, &src.join("loop"));

    // 旧实现：跟随环病态重复递归（红证 = dst/loop 在位——整子树被
    // 重复拷贝进假路径）。
    let dst = dir.path().join("persona_dst");
    copy_dir_recursive(&src, &dst).unwrap();
    assert!(
        dst.join("assets").join("deep").join("a.md").is_file(),
        "真实子树照常拷贝"
    );
    assert!(!dst.join("loop").exists(), "目录链接必须跳过");
}

// ---- BUG-20261126：人格归档的项目会话过滤 ----
//
// 裁定：项目数据全局通用（换人格继续处理同一项目 = 多人协同），对话数据
// 人格私有。旧实现整目录归档 sessions/ 与 logs/session_logs/ 把项目绑定
// 会话一并收进 personas/<人格>/——侧栏项目组消失。
// 判定真相源 = 根 sidecar 的 project_id 非空；stem 提取一律后缀 strip
//（BUG-20261125-079 家族纪律：含点会话键不得用 with_extension 两步构造）。

/// 在根上构造一个会话的完整文件族（logs 侧 3 件 + store 侧 2 件）。
fn bug26_write_session_family(ws: &std::path::Path, safe_stem: &str, project_id: Option<&str>) {
    let logs = ws.join("logs").join("session_logs");
    std::fs::create_dir_all(&logs).unwrap();
    let meta = match project_id {
        Some(pid) => format!(r#"{{"title":"t","project_id":"{}"}}"#, pid),
        None => r#"{"title":"t"}"#.to_string(),
    };
    std::fs::write(logs.join(format!("{}.meta.json", safe_stem)), meta).unwrap();
    std::fs::write(logs.join(format!("{}.jsonl", safe_stem)), "line1\n").unwrap();
    std::fs::write(logs.join(format!("{}.pending", safe_stem)), "p").unwrap();
    let store = ws.join("sessions");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join(format!("{}.json", safe_stem)), "{}").unwrap();
    std::fs::write(store.join(format!("todo_{}.json", safe_stem)), "[]").unwrap();
}

/// 断言会话文件族完整在根（5 件核心文件）。
fn bug26_assert_family_at_root(ws: &std::path::Path, safe_stem: &str) {
    let logs = ws.join("logs").join("session_logs");
    let store = ws.join("sessions");
    for (name, desc) in [
        (format!("{}.meta.json", safe_stem), "meta"),
        (format!("{}.jsonl", safe_stem), "jsonl"),
        (format!("{}.pending", safe_stem), "pending"),
    ] {
        assert!(
            logs.join(&name).is_file(),
            "{} 应留在根 logs/session_logs：{}",
            desc,
            name
        );
    }
    for (name, desc) in [
        (format!("{}.json", safe_stem), "store"),
        (format!("todo_{}.json", safe_stem), "todo"),
    ] {
        assert!(
            store.join(&name).is_file(),
            "{} 应留在根 sessions：{}",
            desc,
            name
        );
    }
}

#[test]
fn bug26_stem_extractors_handle_dotted_session_keys() {
    // 079 家族纪律回归锁：含点会话键必须按后缀 strip，不得被当扩展名拆开。
    assert_eq!(session_stem_of_log_file_name("a.b.jsonl"), Some("a.b"));
    assert_eq!(session_stem_of_log_file_name("a.b.meta.json"), Some("a.b"));
    assert_eq!(session_stem_of_log_file_name("x.pending"), Some("x"));
    assert_eq!(session_stem_of_log_file_name("x.iqueue"), Some("x"));
    assert_eq!(session_stem_of_log_file_name("x.notices"), Some("x"));
    // .taking 瞬态崩溃残留三变体（pending=with_extension 换扩展；iqueue/
    // notices=追加后缀）。顺序敏感回归锁：双段变体必须先于裸 .taking
    // 判定，否则 strip 出错误 stem。
    assert_eq!(session_stem_of_log_file_name("x.taking"), Some("x"));
    assert_eq!(session_stem_of_log_file_name("x.iqueue.taking"), Some("x"));
    assert_eq!(session_stem_of_log_file_name("x.notices.taking"), Some("x"));
    assert_eq!(
        session_stem_of_log_file_name("a.b.iqueue.taking"),
        Some("a.b")
    );
    assert_eq!(session_stem_of_store_file_name("a.b.json"), Some("a.b"));
    assert_eq!(
        session_stem_of_store_file_name("todo_a.b.json"),
        Some("a.b")
    );
    assert_eq!(session_stem_of_meta_file_name("a.b.meta.json"), Some("a.b"));
    // 未知形态不归类（按人格私有的旧整文件语义处理）。
    assert_eq!(session_stem_of_log_file_name("whatever.txt"), None);
    assert_eq!(session_stem_of_store_file_name("notes.md"), None);
}

#[test]
fn bug26_ensure_persona_store_initial_archive_leaves_project_sessions_at_root() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    std::fs::write(ws.join("IDENTITY.md"), "id").unwrap();
    bug26_write_session_family(ws, "chat-dialog", None);
    bug26_write_session_family(ws, "proj-live", Some("p-abc"));

    ensure_persona_store(&ws.to_string_lossy()).unwrap();

    // 对话会话照旧归档进 default（人格私有语义不变）。
    assert!(
        ws.join("personas/default/logs/session_logs/chat-dialog.jsonl")
            .is_file(),
        "对话会话应归档进人格目录"
    );
    assert!(
        ws.join("personas/default/sessions/chat-dialog.json")
            .is_file()
    );
    // 项目绑定会话留根，且不进人格目录（首次初始化同样不收编项目数据）。
    bug26_assert_family_at_root(ws, "proj-live");
    assert!(
        !ws.join("personas/default/logs/session_logs/proj-live.jsonl")
            .exists()
    );
    assert!(!ws.join("personas/default/sessions/proj-live.json").exists());
}

#[test]
fn bug26_activate_switch_preserves_project_sessions_and_parks_dialogue() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ws_str = ws.to_string_lossy().to_string();
    std::fs::write(ws.join("IDENTITY.md"), "default-id").unwrap();
    bug26_write_session_family(ws, "dialog-a", None);
    bug26_write_session_family(ws, "proj-live", Some("p-1"));
    ensure_persona_store(&ws_str).unwrap();

    // 建第二个人格并切换（事故场景：用户换人格继续处理同一项目）。
    let researcher = ws.join("personas/researcher");
    std::fs::create_dir_all(&researcher).unwrap();
    std::fs::write(researcher.join("IDENTITY.md"), "researcher-id").unwrap();

    activate_persona_workspace(&ws_str, "researcher").unwrap();

    // 项目会话纹丝不动在根（BUG 主诉：切换人格不得影响项目信息/会话）。
    bug26_assert_family_at_root(ws, "proj-live");
    // 对话会话归档进 default（人格私有）并从根消失。
    assert!(
        ws.join("personas/default/logs/session_logs/dialog-a.jsonl")
            .is_file()
    );
    assert!(ws.join("personas/default/sessions/dialog-a.json").is_file());
    assert!(!ws.join("logs/session_logs/dialog-a.jsonl").exists());
    // 活体人格确实切了过去。
    assert_eq!(
        std::fs::read_to_string(ws.join("IDENTITY.md")).unwrap(),
        "researcher-id"
    );
}

#[test]
fn bug26_activate_roundtrip_project_session_never_moves() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let ws_str = ws.to_string_lossy().to_string();
    std::fs::write(ws.join("IDENTITY.md"), "default-id").unwrap();
    bug26_write_session_family(ws, "dialog-a", None);
    bug26_write_session_family(ws, "proj-live", Some("p-1"));
    ensure_persona_store(&ws_str).unwrap();
    let researcher = ws.join("personas/researcher");
    std::fs::create_dir_all(&researcher).unwrap();
    std::fs::write(researcher.join("IDENTITY.md"), "researcher-id").unwrap();

    // A→B→A→B 往返：项目会话全程钉在根；对话会话随人格往复。
    activate_persona_workspace(&ws_str, "researcher").unwrap();
    bug26_assert_family_at_root(ws, "proj-live");
    assert!(!ws.join("logs/session_logs/dialog-a.jsonl").exists());

    activate_persona_workspace(&ws_str, "default").unwrap();
    bug26_assert_family_at_root(ws, "proj-live");
    assert!(
        ws.join("logs/session_logs/dialog-a.jsonl").is_file(),
        "切回 default 对话会话应恢复"
    );

    activate_persona_workspace(&ws_str, "researcher").unwrap();
    bug26_assert_family_at_root(ws, "proj-live");
    // 项目文件从未进任何人格目录。
    assert!(
        !ws.join("personas/default/logs/session_logs/proj-live.jsonl")
            .exists()
    );
    assert!(
        !ws.join("personas/researcher/logs/session_logs/proj-live.jsonl")
            .exists()
    );
    assert!(!ws.join("personas/default/sessions/proj-live.json").exists());
    assert!(
        !ws.join("personas/researcher/sessions/proj-live.json")
            .exists()
    );
}

#[test]
fn bug26_repair_moves_back_legacy_layout_complete_family() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // legacy 布局：store 已初始化（_active.json 在），项目会话整体被旧版
    // 收进了 personas/default（事发 home 的真实形态）。
    let pdir = ws.join("personas/default");
    let plogs = pdir.join("logs").join("session_logs");
    let pstore = pdir.join("sessions");
    std::fs::create_dir_all(&plogs).unwrap();
    std::fs::create_dir_all(&pstore).unwrap();
    std::fs::write(ws.join("personas/_active.json"), r#"{"name":"default"}"#).unwrap();
    std::fs::write(
        plogs.join("legacy-proj.meta.json"),
        r#"{"project_id":"p-x"}"#,
    )
    .unwrap();
    std::fs::write(plogs.join("legacy-proj.jsonl"), "archived\n").unwrap();
    std::fs::write(plogs.join("legacy-proj.pending"), "p").unwrap();
    std::fs::write(plogs.join("legacy-proj.iqueue"), "q").unwrap();
    std::fs::write(plogs.join("legacy-proj.notices"), "n").unwrap();
    // .taking 瞬态崩溃残留（take rename 后删除前终止）同样随族回根——
    // drain_orphan_pending 每 10s 兜底扫描只看根，不回根 = 排队消息滞留。
    std::fs::write(plogs.join("legacy-proj.taking"), "r").unwrap();
    std::fs::write(pstore.join("legacy-proj.json"), "{}").unwrap();
    std::fs::write(pstore.join("todo_legacy-proj.json"), "[]").unwrap();

    let moved = repair_project_sessions(&ws.to_string_lossy()).unwrap();

    assert_eq!(moved, 8, "logs 侧 6 件 + store 侧 2 件应全部搬回");
    bug26_assert_family_at_root(ws, "legacy-proj");
    assert!(ws.join("logs/session_logs/legacy-proj.iqueue").is_file());
    assert!(ws.join("logs/session_logs/legacy-proj.notices").is_file());
    // 源已清（搬回不是复制）。
    assert!(!plogs.join("legacy-proj.jsonl").exists());
    assert!(!pstore.join("legacy-proj.json").exists());
}

#[test]
fn bug26_repair_is_idempotent_and_root_wins_collisions() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let plogs = ws.join("personas/default/logs").join("session_logs");
    std::fs::create_dir_all(&plogs).unwrap();
    std::fs::write(ws.join("personas/_active.json"), r#"{"name":"default"}"#).unwrap();
    std::fs::write(plogs.join("proj.meta.json"), r#"{"project_id":"p-x"}"#).unwrap();
    std::fs::write(plogs.join("proj.jsonl"), "persona-copy\n").unwrap();
    // 根上已有同名会话（根是活体权威）：撞车保留根。
    std::fs::create_dir_all(ws.join("logs").join("session_logs")).unwrap();
    std::fs::write(ws.join("logs/session_logs/proj.jsonl"), "root-live\n").unwrap();

    let moved = repair_project_sessions(&ws.to_string_lossy()).unwrap();
    assert_eq!(moved, 1, "只有 meta 搬回（jsonl 根已在跳过）");
    assert_eq!(
        std::fs::read_to_string(ws.join("logs/session_logs/proj.jsonl")).unwrap(),
        "root-live\n",
        "撞车必须保留根上活体"
    );
    assert!(plogs.join("proj.jsonl").is_file(), "人格侧副本不动");

    // 幂等：再跑一遍零搬运。
    let moved2 = repair_project_sessions(&ws.to_string_lossy()).unwrap();
    assert_eq!(moved2, 0, "回迁必须幂等");
}

#[test]
fn bug26_repair_leaves_dialogue_sessions_in_persona() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let plogs = ws.join("personas/default/logs").join("session_logs");
    std::fs::create_dir_all(&plogs).unwrap();
    std::fs::write(ws.join("personas/_active.json"), r#"{"name":"default"}"#).unwrap();
    // 对话会话（无 project_id）：留在人格目录，人格私有语义不变。
    std::fs::write(plogs.join("dialog.meta.json"), r#"{"title":"t"}"#).unwrap();
    std::fs::write(plogs.join("dialog.jsonl"), "d\n").unwrap();
    // 损坏 meta：诚实按对话处理（不猜）。
    std::fs::write(plogs.join("broken.meta.json"), "not-json").unwrap();
    std::fs::write(plogs.join("broken.jsonl"), "b\n").unwrap();

    let moved = repair_project_sessions(&ws.to_string_lossy()).unwrap();
    assert_eq!(moved, 0);
    assert!(plogs.join("dialog.jsonl").is_file());
    assert!(plogs.join("broken.jsonl").is_file());
    assert!(!ws.join("logs/session_logs/dialog.jsonl").exists());
}

// ---- BUG-20261126-093：case 变体调用 vs 大小写不敏感文件系统（NTFS）----
//
// persona 目录解析在 Windows 上大小写不敏感（`personas/coder` 与
// `personas/Coder` 是同一目录），而 active 标记比对是精确 `==`——case
// 变体调用会绕过「激活中人格先恢复 default」「编辑同步活体」两条语义。
// 与 BUG-20261125-069（default 保留名闸 ASCII 变体）同族相邻位点，修复
// 口径同款：ASCII 大小写变体视为同一人格。目录解析仍大小写不敏感（
// Windows 文件系统语义），比对口径与物理目录对齐。

/// Windows-only：目录查找大小写不敏感才能命中 case 变体目录。
#[cfg(windows)]
fn persona_case_variant_setup(handler: &PersonaHandler, ws: &str) {
    handler.ensure_initialized(ws).unwrap();
    // 安装 Coder 人格（手工物化最小目录）并激活。
    let coder_dir = resolve_path(ws, "personas/Coder").unwrap();
    std::fs::create_dir_all(&coder_dir).unwrap();
    std::fs::write(coder_dir.join("IDENTITY.md"), "# Coder Identity").unwrap();
    handler.cmd_activate(ws, "Coder").unwrap();
    // 激活前置确认：marker 与活体均在位。
    assert_eq!(handler.read_active(ws).unwrap(), "Coder");
    assert_eq!(
        std::fs::read_to_string(resolve_path(ws, "IDENTITY.md").unwrap()).unwrap(),
        "# Coder Identity"
    );
}

#[cfg(windows)]
#[test]
fn case_variant_remove_of_active_persona_restores_default() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap();
    let handler = PersonaHandler::new();
    persona_case_variant_setup(&handler, ws);

    // case 变体 remove：目录解析命中同一物理目录，active 比对必须同样
    // 命中（否则跳过 default 恢复——marker 悬空指向已删人格）。
    handler.cmd_remove(ws, "coder").unwrap();

    assert_eq!(
        handler.read_active(ws).unwrap(),
        "default",
        "case 变体 remove 激活中人格必须先恢复 default（marker 不悬空）"
    );
    assert!(
        !resolve_path(ws, "personas/coder").unwrap().exists(),
        "目标人格目录必须已删除"
    );
}

#[cfg(windows)]
#[test]
fn case_variant_file_save_syncs_active_live_files() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap();
    let handler = PersonaHandler::new();
    persona_case_variant_setup(&handler, ws);

    // case 变体 file.save：存档副本（personas/CODER → Coder）与活体
    // （workspace 根）是同一人格——比对必须命中，活体同步不能漏。
    handler
        .cmd_file_save(ws, "CODER", "IDENTITY.md", "# Patched")
        .unwrap();

    assert_eq!(
        std::fs::read_to_string(resolve_path(ws, "IDENTITY.md").unwrap()).unwrap(),
        "# Patched",
        "case 变体保存激活中人格必须同步 workspace 根活体文件"
    );
}

/// Linux 对照（保守分歧声明）：目录查找大小写敏感，case 变体 remove
/// 诚实报 not found——与 BUG-20261125-069 注记的平台分歧口径一致。
#[cfg(not(windows))]
#[test]
fn case_variant_remove_missing_dir_is_honest_not_found() {
    let tmp = tempfile::tempdir().unwrap();
    let ws = tmp.path().to_str().unwrap();
    let handler = PersonaHandler::new();
    handler.ensure_initialized(ws).unwrap();
    let coder_dir = resolve_path(ws, "personas/Coder").unwrap();
    std::fs::create_dir_all(&coder_dir).unwrap();
    std::fs::write(coder_dir.join("IDENTITY.md"), "# Coder Identity").unwrap();
    handler.cmd_activate(ws, "Coder").unwrap();

    let err = handler.cmd_remove(ws, "coder").unwrap_err();
    assert!(err.contains("not found"), "实际错误：{err}");
    // 原目录不受影响。
    assert!(resolve_path(ws, "personas/Coder").unwrap().exists());
}
