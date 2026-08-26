//! persona 命令单测：本地纯函数（解析/清洗/分类）+ 文件型子命令
//! （current/list/activate/remove/restore —— 临时目录确定性）。
//!
//! cmd_search / cmd_install / fetch_and_convert / search_personas 走 GitHub
//! API（结构性不可离线单测，L2 集成真进程覆盖）。

use super::*;

// ---------------------------------------------------------------------------
// is_agent_file_simple —— 远程仓库 .md 文件筛选
// ---------------------------------------------------------------------------

#[test]
fn agent_file_accepts_normal_docs() {
    assert!(is_agent_file_simple("engineering/code-reviewer.md"));
    assert!(is_agent_file_simple("marketing/brand/writer.md"));
}

#[test]
fn agent_file_rejects_non_md_and_no_dir() {
    assert!(!is_agent_file_simple("docs/readme.txt"), "not .md");
    assert!(!is_agent_file_simple("readme.md"), "no '/' — root files rejected");
    assert!(!is_agent_file_simple(""));
}

#[test]
fn agent_file_rejects_meta_dirs() {
    for dir in ["scripts", "examples", "integrations", "strategy", ".github"] {
        assert!(
            !is_agent_file_simple(&format!("{dir}/agent.md")),
            "first dir {dir} must be rejected"
        );
    }
}

#[test]
fn agent_file_rejects_meta_filenames() {
    for name in [
        "README.md",
        "CONTRIBUTING.md",
        "LICENSE.md",
        "SECURITY.md",
        "CONTRIBUTING_zh-CN.md",
    ] {
        assert!(!is_agent_file_simple(&format!("engineering/{name}")), "{name}");
    }
    assert!(!is_agent_file_simple("engineering/QUICKSTART.md"));
    assert!(!is_agent_file_simple("engineering/EXECUTIVE_summary.md"));
    // 前缀匹配只看文件名段：目录叫 QUICKSTART-x 不受影响。
    assert!(is_agent_file_simple("QUICKSTART-x/agent.md"));
}

// ---------------------------------------------------------------------------
// map_category_simple —— 目录 → 中文分类
// ---------------------------------------------------------------------------

#[test]
fn category_maps_known_dirs() {
    assert_eq!(map_category_simple("engineering"), "开发");
    assert_eq!(map_category_simple("security"), "安全");
    assert_eq!(map_category_simple("game-development"), "游戏开发");
    assert_eq!(map_category_simple("project-management"), "项目管理");
    assert_eq!(map_category_simple("gis"), "GIS");
}

#[test]
fn category_unknown_dir_maps_to_generic() {
    assert_eq!(map_category_simple("whatever"), "通用");
    assert_eq!(map_category_simple(""), "通用");
}

// ---------------------------------------------------------------------------
// strip_emoji_simple —— emoji 剥离 + trim + 小写
// ---------------------------------------------------------------------------

#[test]
fn strip_emoji_removes_emoji_and_normalizes_case() {
    assert_eq!(strip_emoji_simple("🚀 Rocket Man"), "rocket man");
    assert_eq!(strip_emoji_simple("  Hello World  "), "hello world");
    assert_eq!(strip_emoji_simple("开发者"), "开发者");
}

#[test]
fn strip_emoji_handles_variation_selector_and_zwj() {
    // 变体选择符（FE0F）与零宽连接符（200D）都在过滤范围。
    assert_eq!(strip_emoji_simple("a\u{FE0F}b"), "ab");
    assert_eq!(strip_emoji_simple("a\u{200D}b"), "ab");
    // 全 emoji 输入剥完为空串。
    assert_eq!(strip_emoji_simple("🎯🔥"), "");
}

// ---------------------------------------------------------------------------
// parse_frontmatter_simple —— YAML frontmatter 四字段抽取
// ---------------------------------------------------------------------------

#[test]
fn frontmatter_parses_all_fields_and_strips_quotes() {
    let md = "---\nname: \"Code Reviewer\"\nemoji: \"🤖\"\ndescription: \"Reviews code\"\nvibe: strict\nother: ignored\n---\nbody";
    let (name, emoji, desc, vibe) =
        parse_frontmatter_simple(md).expect("valid frontmatter parses");
    assert_eq!(name, "Code Reviewer");
    assert_eq!(emoji, "🤖");
    assert_eq!(desc, "Reviews code");
    assert_eq!(vibe, "strict");
}

#[test]
fn frontmatter_without_marker_is_none() {
    assert!(parse_frontmatter_simple("# Just a title\n\nbody").is_none());
    assert!(parse_frontmatter_simple("").is_none());
}

#[test]
fn frontmatter_missing_name_is_none() {
    let md = "---\nemoji: 🤖\n---\nbody";
    assert!(parse_frontmatter_simple(md).is_none(), "name 是必需字段");
}

#[test]
fn frontmatter_name_only_yields_empty_optionals() {
    let md = "---\nname: Writer\n---\nbody";
    let (name, emoji, desc, vibe) =
        parse_frontmatter_simple(md).expect("name alone is enough");
    assert_eq!(name, "Writer");
    assert_eq!(emoji, "");
    assert_eq!(desc, "");
    assert_eq!(vibe, "");
}

#[test]
fn frontmatter_unclosed_marker_is_none() {
    assert!(parse_frontmatter_simple("---\nname: X\n").is_none(), "无闭合 ---");
}

// ---------------------------------------------------------------------------
// parse_sections_simple —— 正文 preamble + ## 段落切分
// ---------------------------------------------------------------------------

#[test]
fn sections_split_on_h2_and_keep_preamble() {
    let md = "# Title\n\nintro line\n\n## First\ncontent A1\ncontent A2\n\n## Second\ncontent B\n";
    let (preamble, sections) = parse_sections_simple(md);
    assert_eq!(preamble, "intro line");
    assert_eq!(sections.len(), 2);
    assert_eq!(sections[0], ("First".to_string(), "content A1\ncontent A2".to_string()));
    assert_eq!(sections[1], ("Second".to_string(), "content B".to_string()));
}

#[test]
fn sections_without_h2_yield_empty_vec() {
    let (preamble, sections) = parse_sections_simple("only text\nmore text");
    assert_eq!(preamble, "only text\nmore text");
    assert!(sections.is_empty());
}

#[test]
fn sections_strip_leading_frontmatter() {
    let md = "---\nname: X\n---\nintro\n\n## Section\nbody";
    let (preamble, sections) = parse_sections_simple(md);
    assert_eq!(preamble, "intro", "frontmatter 不能进 preamble");
    assert_eq!(sections.len(), 1);
    assert_eq!(sections[0].0, "Section");
}

#[test]
fn sections_h3_stays_inside_current_section() {
    let md = "## Outer\nbefore\n\n### Inner\ndeep\n";
    let (_preamble, sections) = parse_sections_simple(md);
    assert_eq!(sections.len(), 1, "### 不开新段");
    assert_eq!(sections[0].0, "Outer");
    assert!(sections[0].1.contains("### Inner"), "### 行留在段内容里");
}

#[test]
fn sections_after_first_h2_do_not_leak_into_preamble() {
    let md = "pre\n\n## A\nx\n\npost-section-line-in-a\n";
    let (preamble, sections) = parse_sections_simple(md);
    assert_eq!(preamble, "pre");
    assert_eq!(sections.len(), 1);
    assert!(sections[0].1.contains("post-section-line-in-a"));
}

// ---------------------------------------------------------------------------
// 文件型子命令 —— 临时 workspace 确定性
// ---------------------------------------------------------------------------

mod file_commands {
    use super::super::*;

    fn ws() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    fn seed_persona(workspace: &std::path::Path, name: &str, identity: &str) {
        let dir = workspace.join("personas").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), identity).unwrap();
        std::fs::write(dir.join("SOUL.md"), format!("soul of {name}")).unwrap();
        std::fs::write(dir.join("PERSONA.json"), format!(r#"{{"name":"{name}","emoji":"🤖","description":"d"}}"#)).unwrap();
    }

    #[test]
    fn current_without_active_marker_is_ok() {
        let dir = ws();
        cmd_current(dir.path()).expect("未初始化 → 干净 Ok（提示语），不 Err");
    }

    #[test]
    fn current_with_persona_json_is_ok() {
        let dir = ws();
        seed_persona(dir.path(), "p1", "identity-1");
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        std::fs::write(
            dir.path().join("personas/_active.json"),
            r#"{"name":"p1"}"#,
        )
        .unwrap();
        cmd_current(dir.path()).expect("active + PERSONA.json → Ok");
    }

    #[test]
    fn current_with_active_but_missing_persona_dir_is_ok() {
        let dir = ws();
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        std::fs::write(
            dir.path().join("personas/_active.json"),
            r#"{"name":"ghost"}"#,
        )
        .unwrap();
        cmd_current(dir.path()).expect("缺 PERSONA.json → 降级打印目录名，Ok");
    }

    #[test]
    fn list_without_personas_dir_is_ok() {
        let dir = ws();
        cmd_list(dir.path()).expect("无 personas 目录 → Ok");
    }

    #[test]
    fn list_with_empty_personas_dir_is_ok() {
        let dir = ws();
        std::fs::create_dir_all(dir.path().join("personas")).unwrap();
        cmd_list(dir.path()).expect("空目录 → Ok（打印 0 个）");
    }

    #[test]
    fn list_with_personas_is_ok() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "p2", "identity-2");
        cmd_list(dir.path()).expect("两个人格 → Ok");
    }

    #[test]
    fn activate_missing_persona_bails() {
        let dir = ws();
        let err = cmd_activate(dir.path(), "ghost").expect_err("不存在的人格必须 Err");
        assert!(err.to_string().contains("不存在"), "err: {err:#}");
    }

    #[test]
    fn activate_copies_files_and_writes_marker() {
        let dir = ws();
        seed_persona(dir.path(), "p1", "identity-1");
        cmd_activate(dir.path(), "p1").expect("activate ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "identity-1",
            "IDENTITY.md 复制到 workspace 根"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("SOUL.md")).unwrap(),
            "soul of p1"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "p1");
    }

    #[test]
    fn activate_partial_persona_only_copies_present_files() {
        // 只有 AGENT.md 的人格：IDENTITY 等缺文件跳过、不 Err。
        let dir = ws();
        let pdir = dir.path().join("personas/partial");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("AGENT.md"), "agent rules").unwrap();
        cmd_activate(dir.path(), "partial").expect("部分文件人格 → Ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("AGENT.md")).unwrap(),
            "agent rules"
        );
        assert!(!dir.path().join("IDENTITY.md").exists(), "缺的文件不复制");
    }

    #[test]
    fn remove_default_bails() {
        let dir = ws();
        let err = cmd_remove(dir.path(), "default").expect_err("默认人格不可删");
        assert!(err.to_string().contains("不能删除默认人格"), "err: {err:#}");
    }

    #[test]
    fn remove_missing_bails() {
        let dir = ws();
        let err = cmd_remove(dir.path(), "ghost").expect_err("不存在 → Err");
        assert!(err.to_string().contains("不存在"), "err: {err:#}");
    }

    #[test]
    fn remove_inactive_persona_deletes_only_dir() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "extra", "extra identity");
        cmd_activate(dir.path(), "default").unwrap();
        cmd_remove(dir.path(), "extra").expect("remove ok");
        assert!(!dir.path().join("personas/extra").exists(), "目录已删");
        // 非活动人格删除不动 workspace 根文件。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "default", "活动标记不变");
    }

    #[test]
    fn remove_active_persona_restores_default_first() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        seed_persona(dir.path(), "p1", "identity-1");
        cmd_activate(dir.path(), "p1").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "identity-1"
        );
        cmd_remove(dir.path(), "p1").expect("remove active ok");
        // workspace 根被恢复成 default 的文件 + 活动标记回 default。
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity",
            "删活动人格 → 先恢复默认人格文件"
        );
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(marker["name"], "default");
        assert!(!dir.path().join("personas/p1").exists());
    }

    #[test]
    fn restore_requires_default_persona() {
        let dir = ws();
        let err = cmd_restore(dir.path()).expect_err("无 default 目录 → activate(default) Err");
        assert!(err.to_string().contains("不存在"), "err: {err:#}");
    }

    #[test]
    fn restore_activates_default() {
        let dir = ws();
        seed_persona(dir.path(), "default", "default identity");
        cmd_restore(dir.path()).expect("restore ok");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("IDENTITY.md")).unwrap(),
            "default identity"
        );
    }
}

/// run() 本地分发（Search/Install 走 GitHub API —— 结构性，不在此测）。
#[tokio::test]
async fn run_dispatches_local_file_commands() {
    let dir = tempfile::tempdir().unwrap();
    let ws_str = dir.path().to_string_lossy().to_string();
    seed(&dir);

    fn seed(dir: &tempfile::TempDir) {
        let pdir = dir.path().join("personas/default");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(pdir.join("IDENTITY.md"), "default identity").unwrap();
    }

    run(PersonaAction::List, "home", &ws_str).await.expect("List ok");
    run(PersonaAction::Current, "home", &ws_str).await.expect("Current ok");
    run(PersonaAction::Restore, "home", &ws_str).await.expect("Restore ok");
    run(
        PersonaAction::Activate { name: "default".to_string() },
        "home",
        &ws_str,
    )
    .await
    .expect("Activate ok");
    run(
        PersonaAction::Remove { name: "default".to_string() },
        "home",
        &ws_str,
    )
    .await
    .expect_err("Remove default → Err（经 run 分发同样生效）");
}

// ===========================================================================
// S11c（quality-hardening goal 冲刺 S11）：map_category_simple 17 臂中只钉过
// 6 臂——补齐其余 11 臂 + 边界。
// ===========================================================================

#[test]
fn map_category_simple_remaining_arms() {
    assert_eq!(map_category_simple("marketing"), "营销");
    assert_eq!(map_category_simple("design"), "创意");
    assert_eq!(map_category_simple("academic"), "学术");
    assert_eq!(map_category_simple("product"), "产品");
    assert_eq!(map_category_simple("paid-media"), "付费媒体");
    assert_eq!(map_category_simple("sales"), "销售");
    assert_eq!(map_category_simple("finance"), "金融");
    assert_eq!(map_category_simple("spatial-computing"), "空间计算");
    assert_eq!(map_category_simple("specialized"), "专业");
    assert_eq!(map_category_simple("testing"), "测试");
    assert_eq!(map_category_simple("support"), "客服");
}
