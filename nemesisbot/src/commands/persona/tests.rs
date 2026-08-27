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

// ===========================================================================
// wave_b（覆盖率补测 B 波）：本地命令剩余分支 + 解析辅助剩余臂。
//
// 不在此测（EXEMPT）：cmd_search / cmd_install 的联网路径与
// fetch_and_convert / search_personas 整体函数体 —— GitHub API 真网络，
// 结构性禁离线单测；Install 的唯一可离线触点是「已安装早退」（下方测）。
// ===========================================================================

mod wave_b {
    use super::*;

    fn wb_seed(workspace: &std::path::Path, name: &str, identity: &str) {
        let dir = workspace.join("personas").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), identity).unwrap();
        std::fs::write(dir.join("SOUL.md"), format!("soul of {name}")).unwrap();
    }

    fn wb_active(workspace: &std::path::Path, name: &str) {
        let pdir = workspace.join("personas");
        std::fs::create_dir_all(&pdir).unwrap();
        std::fs::write(
            pdir.join("_active.json"),
            serde_json::json!({ "name": name }).to_string(),
        )
        .unwrap();
    }

    /// cmd_current：PERSONA.json 的 description 缺失（as_str None 臂，78 行区）
    /// 与空串（Some("") 但 !is_empty 为假 → 不打印描述行）两种形态。
    #[test]
    fn wave_b_current_description_absent_and_empty_arms() {
        // (a) 无 description 键 → if let Some(desc) 的 None 落穿。
        let d1 = tempfile::tempdir().unwrap();
        wb_seed(d1.path(), "bare", "i-bare");
        std::fs::write(
            d1.path().join("personas/bare/PERSONA.json"),
            r#"{"name":"Bare","emoji":"🙂"}"#,
        )
        .unwrap();
        wb_active(d1.path(), "bare");
        cmd_current(d1.path()).expect("PERSONA.json 缺 description 也应 Ok");

        // (b) description 为空串 → 内层 !desc.is_empty() 为假。
        let d2 = tempfile::tempdir().unwrap();
        wb_seed(d2.path(), "quiet", "i-quiet");
        std::fs::write(
            d2.path().join("personas/quiet/PERSONA.json"),
            r#"{"name":"Quiet","emoji":"🤫","description":""}"#,
        )
        .unwrap();
        wb_active(d2.path(), "quiet");
        cmd_current(d2.path()).expect("空 description 也应 Ok");
    }

    /// cmd_list：读 _active.json 确定 active 标记（96-98）、循环中跳过
    /// 非目录杂散文件（107-108）、以及含 default 的多元素排序（132-140 三臂：
    /// Less=default 在左 / Greater=default 在右 / 双非 default 按名比较）。
    #[test]
    fn wave_b_list_reads_active_marker_sorts_and_skips_plain_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "default", "identity-default");
        wb_seed(ws, "zulu", "identity-z");
        wb_seed(ws, "alpha", "identity-a");
        wb_seed(ws, "mike", "identity-m");
        // 杂散文件（非目录）→ 循环 continue。
        std::fs::create_dir_all(ws.join("personas")).unwrap();
        std::fs::write(ws.join("personas/README.txt"), "not a persona").unwrap();
        wb_active(ws, "default");
        cmd_list(ws).expect("带 marker + 杂散文件 + 多人格的 list 应 Ok");
    }

    /// cmd_install 已安装早退：在 GitHub fetch 之前 bail（273-282），
    /// 经 run() 分发触达对应 arm（40）。这是 Install 唯一可离线测的路径。
    #[tokio::test]
    async fn wave_b_run_install_already_installed_bails_before_network() {
        let dir = tempfile::tempdir().unwrap();
        let ws_str = dir.path().to_string_lossy().to_string();
        wb_seed(dir.path(), "dupe", "already-here");
        let err = run(
            PersonaAction::Install { id: "dupe".to_string() },
            "home",
            &ws_str,
        )
        .await
        .expect_err("已安装人格必须 Err 且不发生任何网络请求");
        assert!(
            err.to_string().contains("已经安装"),
            "err: {err:#}"
        );
    }

    /// cmd_remove：删除活动人格但 personas/default 不存在 → 不执行恢复块，
    /// _active.json 悬空指向已删除的人格（产品现状，见报告可疑点）。
    /// 这覆盖「匹配活动名但 default 目录缺席」的落穿区域（209-221 家族）。
    #[test]
    fn wave_b_remove_active_persona_without_default_dir_leaves_marker_dangling() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "only", "only-identity");
        cmd_activate(ws, "only").unwrap();
        cmd_remove(ws, "only").expect("删除活动人格（无 default 可恢复）应 Ok");
        assert!(!ws.join("personas/only").exists(), "目录已删");
        let marker: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(ws.join("personas/_active.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            marker["name"], "only",
            "无 default 时 marker 原样悬空指向已删人格"
        );
    }

    /// cmd_remove：完全无 _active.json → 跳过整个活动恢复检测块（204 存在性假臂），
    /// 直接删目录。
    #[test]
    fn wave_b_remove_persona_without_any_active_marker() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        wb_seed(ws, "loner", "loner-identity");
        cmd_remove(ws, "loner").expect("无 marker 时 remove 应 Ok");
        assert!(!ws.join("personas/loner").exists());
        assert!(!ws.join("personas/_active.json").exists(), "不创建 marker");
    }

    /// strip_emoji_simple：U+1F000 段（低补充平面 emoji 区间臂，592 行模式段）。
    #[test]
    fn wave_b_strip_emoji_low_supplementary_plane_range() {
        // U+1F000（麻将牌）落在 '\u{1F000}'..='\u{1FFFF}' 过滤臂，
        // 与既有测试常用的 U+1F300.. 区分臂不同。
        assert_eq!(strip_emoji_simple("\u{1F000}kept"), "kept");
        assert_eq!(strip_emoji_simple("\u{1F0CF}"), "");
    }

    /// parse_sections_simple：frontmatter 以 --- 开头但无闭合 ---
    /// → body 回退整段原文（635-637 else 臂）；所有非 H2 行进 preamble，
    /// sections 为空。
    #[test]
    fn wave_b_parse_sections_unclosed_frontmatter_keeps_whole_body() {
        let md = "---\nname: never-closed\nnot-a-section tail";
        let (preamble, sections) = parse_sections_simple(md);
        assert!(sections.is_empty(), "无闭合 frontmatter 不产生 section");
        assert_eq!(preamble, md, "原文整体留在 preamble");
    }
}

// ===========================================================================
// r10 wave（覆盖率 95% goal 第七波）：Search / Install 的网络前置段。
//
// GitHub API 端点在 search_personas / fetch_and_convert 内部硬编码，无注入
// seam；用 reqwest 系统代理语义把出网钉死在本机死端口（HTTPS_PROXY=
// http://127.0.0.1:9）→ send().await? 的 Err 一路 `?` 上抛。前置段（参数
// 归一、workspace 快照、client 构造、GET 发起）由此全部可达；渲染段结构性
// 属真网集成域（与文件头既有豁免口径一致）。罕见的「机器有系统级代理穿透」
// 环境下这两个命令可能转 Ok——本块按主环境（无系统代理、env 生效）断言。
// cmd_search / cmd_install 内部 block_in_place 必须 multi_thread runtime；
// env 是进程全局 → 持 crate::GLOBAL_STATE_LOCK，prev 值按 Option 恢复。
// ===========================================================================

mod r10_lead_in {
    use super::*;
    use std::ffi::OsString;

    struct NetDead {
        prev: Option<OsString>,
    }

    impl NetDead {
        fn engage() -> Self {
            let prev = std::env::var_os("HTTPS_PROXY");
            unsafe { std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9") };
            Self { prev }
        }
    }

    impl Drop for NetDead {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("HTTPS_PROXY", v) },
                None => unsafe { std::env::remove_var("HTTPS_PROXY") },
            }
        }
    }

    /// cmd_search 前置段：query 归一 → 提示打印 → workspace 快照克隆 →
    /// block_in_place 桥内 GET；send Err 经 `?` 一路上抛成 Err。
    #[tokio::test(flavor = "multi_thread")]
    async fn r10_persona_search_lead_in_errors_at_dead_upstream() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();

        let res = cmd_search(ws.path(), Some("r10-query"));
        assert!(res.is_err(), "上游不可达 → 前置段之后必须以 Err 收场");
    }

    /// cmd_install 前置段：persona 目录不存在性检查通过 → 下载提示 →
    /// fetch_and_convert 第一腿（tree GET）即败；双层 Result 的两个 `?`
    /// 无论哪层接力，最终都 Err 且绝不创建 personas/<id>（安装目录只在
    /// 内容写盘阶段创建——离线 Err 与在线"未找到"两条世界同守此断言）。
    #[tokio::test(flavor = "multi_thread")]
    async fn r10_persona_install_uninstalled_id_bails_after_network_leg_one() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();
        let id = "r10-absent-persona";

        let res = cmd_install(ws.path(), id);
        assert!(res.is_err(), "第一腿即断 → install 必须 Err");
        assert!(
            !ws.path().join("personas").join(id).exists(),
            "前置段失败不得留下安装目录"
        );
    }

    /// run() 的 Search / Install 两条分发臂（此前只钉过本地五臂）：即便套上
    /// run 的 async 包装层，前置段的 Err 语义同样成立。
    #[tokio::test(flavor = "multi_thread")]
    async fn r10_persona_run_dispatch_reaches_search_and_install_arms() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = NetDead::engage();
        let ws = tempfile::tempdir().unwrap();
        let ws_str = ws.path().to_string_lossy().to_string();

        assert!(
            run(
                PersonaAction::Search {
                    query: Some("q".to_string()),
                },
                "home-unused",
                &ws_str,
            )
            .await
            .is_err(),
            "Search 分发臂前置段应 Err"
        );

        let id = "r10-absent-persona-run";
        assert!(
            run(
                PersonaAction::Install {
                    id: id.to_string(),
                },
                "home-unused",
                &ws_str,
            )
            .await
            .is_err(),
            "Install 分发臂前置段应 Err"
        );
        assert!(!ws.path().join("personas").join(id).exists());
    }
}
