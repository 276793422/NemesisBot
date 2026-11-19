//! onboard 提取模块的单测：Seed/Cli 双模式写盘语义钉死。

use super::{OnboardMode, onboard_default};
use std::fs;

#[test]
fn seed_mode_creates_missing_home() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("fresh-home");

    onboard_default(&home, false, OnboardMode::Seed).unwrap();

    // 关键件全被种子出来
    assert!(common_config(&home).exists(), "config.json must be seeded");
    assert!(home.join("workspace").join("IDENTITY.md").exists());
    assert!(home.join("workspace").join("SOUL.md").exists());
    assert!(home.join("workspace").join("USER.md").exists());
    assert!(!home.join("workspace").join("BOOTSTRAP.md").exists());
}

#[test]
fn seed_mode_never_clobbers_user_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("existing-home");
    onboard_default(&home, false, OnboardMode::Seed).unwrap();

    // 用户改过的文件：人格 + 主 config + 子系统配置
    let identity = home.join("workspace").join("IDENTITY.md");
    fs::write(&identity, "my custom persona").unwrap();
    let cfg_path = common_config(&home);
    let raw = fs::read_to_string(&cfg_path).unwrap();
    let mut cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("my/model");
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
    let mcp_path = home.join("workspace").join("config").join("mcp.json");
    fs::write(&mcp_path, "{\"user\":true}").unwrap();

    // 再次 seed（模拟「删 config.json 后 gateway 直启」等再入场景：
    // 只补缺失件，不动已有件）
    fs::remove_file(&cfg_path).unwrap();
    onboard_default(&home, false, OnboardMode::Seed).unwrap();

    assert_eq!(fs::read_to_string(&identity).unwrap(), "my custom persona");
    assert_eq!(fs::read_to_string(&mcp_path).unwrap(), "{\"user\":true}");
    // config.json 重新种子出来（缺失即补）
    assert!(cfg_path.exists());
}

#[test]
fn seed_mode_idempotent_double_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("idem-home");
    onboard_default(&home, false, OnboardMode::Seed).unwrap();
    let cfg_first = fs::read_to_string(common_config(&home)).unwrap();
    // peers.toml 路径与生产同源（cluster_dir = workspace/cluster，勿手拼）。
    let peers_path = crate::common::cluster_dir(&home).join("peers.toml");
    let peers_first = fs::read_to_string(&peers_path).unwrap();

    onboard_default(&home, false, OnboardMode::Seed).unwrap();

    // 种子模式二跑：config/peers 不重写（token/uuid 不漂移）
    assert_eq!(fs::read_to_string(common_config(&home)).unwrap(), cfg_first);
    assert_eq!(fs::read_to_string(&peers_path).unwrap(), peers_first);
}

#[test]
fn cli_mode_overwrites_persona() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("cli-home");
    onboard_default(&home, false, OnboardMode::Seed).unwrap();

    let identity = home.join("workspace").join("IDENTITY.md");
    fs::write(&identity, "my custom persona").unwrap();

    // Cli 模式：人格总是覆盖（default/ 权威源，re-onboard 修复损坏模板）
    onboard_default(&home, false, OnboardMode::Cli).unwrap();

    assert_ne!(fs::read_to_string(&identity).unwrap(), "my custom persona");
}

// ===========================================================================
// 回归锁：onboard 曾把 agents.defaults.restrict_to_workspace
// 强制改写 false（注释声称「security module enforces rules」，但安全管线不含
// 工作区包围——包围唯一执法点是 agent 侧 WorkspaceBoundary），fresh 装机
// 开箱即无文件系统写约束（`..` 相对路径越界写实机复现 ok:true）。修复后
// onboard 不再触碰该键，模板默认 true 直通落盘。
// ===========================================================================

mod bug353_restrict_default {
    use super::super::{OnboardMode, onboard_default};

    /// Seed 模式 fresh 装机：restrict_to_workspace 必须保持模板默认 true。
    #[test]
    fn bug353_seed_fresh_onboard_keeps_restrict_true() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("bug353-seed-home");

        onboard_default(&home, false, OnboardMode::Seed).unwrap();

        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(
            cfg["agents"]["defaults"]["restrict_to_workspace"],
            serde_json::json!(true),
            "fresh onboard 不得关闭工作区围栏: {}",
            cfg["agents"]["defaults"]
        );
    }

    /// Cli + local=true 臂：围栏同样保持 true，且 --local workspace 改写
    /// 照常生效（同块两语义不互斥）。
    #[test]
    fn bug353_cli_local_keeps_restrict_true_and_rewrites_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("bug353-cli-local-home");

        onboard_default(&home, true, OnboardMode::Cli).unwrap();

        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap())
                .unwrap();
        assert_eq!(
            cfg["agents"]["defaults"]["restrict_to_workspace"],
            serde_json::json!(true),
        );
        assert_eq!(
            cfg["agents"]["defaults"]["workspace"].as_str(),
            Some(".nemesisbot/workspace"),
            "--local workspace 改写不受影响"
        );
    }
}

fn common_config(home: &std::path::Path) -> std::path::PathBuf {
    crate::common::config_path(home)
}

// ===========================================================================
// wave5 batch-2：onboard_default 的 local=true 臂——
// agents.defaults.workspace 改写为相对路径 ".nemesisbot/workspace"
//（既有 onboard 测试全部 local=false，该分支从未执行）。
// ===========================================================================

mod w5b2 {
    use super::super::{OnboardMode, onboard_default};

    /// Cli 模式 + local=true：主 config 的 agents.defaults.workspace 必须
    /// 被改写为 ".nemesisbot/workspace"（local 布局语义）。
    #[test]
    fn w5_cli_mode_local_true_rewrites_default_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("w5-local-home");

        onboard_default(&home, true, OnboardMode::Cli).unwrap();

        let cfg_path = home.join("config.json");
        assert!(cfg_path.exists(), "Cli onboard 必须写主 config");
        let cfg: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(
            cfg["agents"]["defaults"]["workspace"].as_str(),
            Some(".nemesisbot/workspace"),
            "local=true 必须改写 defaults.workspace 为相对路径: {}",
            cfg
        );
    }
}

// ===========================================================================
// 持久化静默失败审计：人格文件安装（install_if_absent）此前是裸
// std::fs:write——(1) 漏掉 原子写横扫（半截 IDENTITY.md = 截断
// 人格，与配置文件同类伤害）；(2) 写失败静默 + 调用方无条件打印「installed」
// 假成功提示。修复后返回 bool 汇总计数，失败诚实报 Warning。
// ===========================================================================

mod r55_install_if_absent_contract {
    use super::super::{OnboardMode, install_if_absent};
    use std::fs;

    /// 父路径是文件 → create_dir_all 必败 → 返回 false（不再静默假成功）。
    #[test]
    fn r55_install_failure_parent_is_file_returns_false() {
        let tmp = tempfile::TempDir::new().unwrap();
        let blocker = tmp.path().join("blocker");
        fs::write(&blocker, "x").unwrap();
        let ok = install_if_absent(&blocker.join("IDENTITY.md"), "content", OnboardMode::Cli);
        assert!(!ok, "父路径为文件时安装必须报失败（返回 false）");
        assert!(
            !blocker.join("IDENTITY").exists(),
            "失败路径不得留下半截产物"
        );
    }

    /// 正常路径：原子写内容完整 + Cli 覆盖语义 + Seed 跳过视为成功（true）。
    #[test]
    fn r55_install_happy_path_atomic_and_intact() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("IDENTITY.md");
        assert!(
            install_if_absent(&path, "v1", OnboardMode::Cli),
            "正常路径必须返回 true"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "v1");
        assert!(install_if_absent(&path, "v2", OnboardMode::Cli));
        assert_eq!(fs::read_to_string(&path).unwrap(), "v2", "Cli 模式总是覆盖");
        assert!(
            install_if_absent(&path, "v3", OnboardMode::Seed),
            "Seed 存在即跳过 = 视为成功（返回 true）"
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "v2",
            "Seed 不覆盖用户文件"
        );
    }
}

// ===========================================================================
// 人格预装语义重构（2026-11-16）：身份三件 = default/ 权威源（老贾版），
// personas/ 两槽物化（default=贾维斯默认激活 / custom=空白可定制），
// bug-analyst 等内置人格不再预装落盘（内置页手动下载才落地）。
// ===========================================================================

mod persona_preinstall_2026_11_16 {
    use super::super::{OnboardMode, onboard_default};
    use std::fs;
    use std::path::{Path, PathBuf};

    /// 五件套清单（与 nemesis-web handlers/persona.rs 的 PERSONA_FILES ∪
    /// PERSONA_ARCHIVE_FILES 白名单口径一致——激活链清根恢复无补齐臂，
    /// 槽内缺哪件切走再切回根上就永久缺哪件）。
    const FIVE_PIECE: [&str; 5] = [
        "IDENTITY.md",
        "SOUL.md",
        "AGENT.md",
        "TOOLS.md",
        "HEARTBEAT.md",
    ];

    fn personas(home: &Path) -> PathBuf {
        home.join("workspace").join("personas")
    }

    /// seed 直启（空环境 config.json 缺失）：root 身份三件 = 老贾版 +
    /// personas/ 恰两槽五件套齐全 + _active=default + 无 bug-analyst。
    #[test]
    fn seed_fresh_home_materializes_two_slots_and_jarvis_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("persona-seed-home");

        onboard_default(&home, false, OnboardMode::Seed).unwrap();

        assert_root_identity_is_jarvis(&home);
        assert_two_slots_materialized(&home);
    }

    /// Cli 显式初始化：同断言（两条启动路径统一出老贾）。
    #[test]
    fn cli_fresh_home_materializes_two_slots_and_jarvis_identity() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("persona-cli-home");

        onboard_default(&home, false, OnboardMode::Cli).unwrap();

        assert_root_identity_is_jarvis(&home);
        assert_two_slots_materialized(&home);
    }

    /// re-onboard（Cli 最强语义：root 身份三件会覆盖）不得触碰 personas/
    /// 用户数据——槽文件、PERSONA.json、_active.json 全部保留用户手笔
    /// （恒 only-if-absent，偏离 Cli=overwrite 惯例是刻意决策）。
    #[test]
    fn re_onboard_never_clobbers_persona_slots() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("persona-re-onboard-home");
        onboard_default(&home, false, OnboardMode::Seed).unwrap();

        let custom_identity = personas(&home).join("custom").join("IDENTITY.md");
        fs::write(&custom_identity, "MY-CUSTOM-EDIT").unwrap();
        let default_persona_json = personas(&home).join("default").join("PERSONA.json");
        fs::write(&default_persona_json, r#"{"name": "用户改名"}"#).unwrap();
        let active = personas(&home).join("_active.json");
        fs::write(&active, r#"{"name": "custom"}"#).unwrap();

        onboard_default(&home, false, OnboardMode::Cli).unwrap();

        assert_eq!(
            fs::read_to_string(&custom_identity).unwrap(),
            "MY-CUSTOM-EDIT",
            "custom 槽用户编辑必须保留"
        );
        assert_eq!(
            fs::read_to_string(&default_persona_json).unwrap(),
            r#"{"name": "用户改名"}"#,
            "default 槽 PERSONA.json 用户编辑必须保留"
        );
        assert_eq!(
            fs::read_to_string(&active).unwrap(),
            r#"{"name": "custom"}"#,
            "_active.json 用户激活状态必须保留"
        );
        // root 身份三件仍走 Cli 覆盖语义（不因人格槽豁免而退化）
        assert_eq!(
            fs::read_to_string(home.join("workspace").join("IDENTITY.md")).unwrap(),
            crate::DEFAULT_IDENTITY,
            "root 身份三件保持 Cli=overwrite 现语义"
        );
    }

    /// custom 删除后 re-onboard 不复活（删除是用户明确意志，不造墓碑——
    /// 依赖 seed 仅 config.json 缺失时跑的装配闸 + 物化 only-if-absent）。
    #[test]
    fn re_onboard_does_not_revive_deleted_custom_slot() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("persona-delete-home");
        onboard_default(&home, false, OnboardMode::Seed).unwrap();

        fs::remove_dir_all(personas(&home).join("custom")).unwrap();
        onboard_default(&home, false, OnboardMode::Cli).unwrap();

        assert!(
            !personas(&home).join("custom").exists(),
            "custom 槽删除后 re-onboard 不得复活"
        );
        assert!(
            personas(&home).join("default").join("IDENTITY.md").exists(),
            "default 槽不受 custom 删除影响"
        );
    }

    /// 存量老用户 store（升级自预装 bug-analyst 的旧版：personas/ 只有
    /// 第三方槽、无 default/custom、无 _active.json）跑 Cli re-onboard：
    /// **不得写 _active.json**——web ensure_persona_store 的懒初始化归档以
    /// 标记在场为闸（persona.rs:1587），提前写入会把闸关死，default 槽
    /// 永不创建（旧版行为 = 首次触达人格页从 root 归档自愈出 default 槽）。
    /// 第三方槽原样保留，也不得凭空造本重构槽（不迁移存量）。
    #[test]
    fn re_onboard_legacy_store_keeps_lazy_init_armed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().join("persona-legacy-home");
        onboard_default(&home, false, OnboardMode::Seed).unwrap();

        // 构造存量形态：摘掉两槽与标记，只留第三方槽（bug-analyst 同构）
        fs::remove_dir_all(personas(&home).join("default")).unwrap();
        fs::remove_dir_all(personas(&home).join("custom")).unwrap();
        fs::remove_file(personas(&home).join("_active.json")).unwrap();
        fs::create_dir_all(personas(&home).join("bug-analyst")).unwrap();
        fs::write(
            personas(&home).join("bug-analyst").join("PERSONA.json"),
            r#"{"name":"bug-analyst"}"#,
        )
        .unwrap();

        onboard_default(&home, false, OnboardMode::Cli).unwrap();

        assert!(
            !personas(&home).join("_active.json").exists(),
            "外来 store 不得写 _active.json（懒初始化闸必须保持武装）"
        );
        assert!(
            !personas(&home).join("default").exists(),
            "存量 store 不得凭空造 default 槽（不迁移存量）"
        );
        assert!(
            !personas(&home).join("custom").exists(),
            "存量 store 不得凭空造 custom 槽（不迁移存量）"
        );
        assert!(
            personas(&home)
                .join("bug-analyst")
                .join("PERSONA.json")
                .exists(),
            "第三方槽原样保留"
        );
    }

    // ── 断言助手 ─────────────────────────────────────────────────────

    /// root 身份三件 = default/ 权威源（老贾版）——提取排除 + Step 9 天然
    /// 缺席安装的行为锚点（本重构消除的双真相源分裂）。
    fn assert_root_identity_is_jarvis(home: &Path) {
        for (file, expected) in [
            ("IDENTITY.md", crate::DEFAULT_IDENTITY),
            ("SOUL.md", crate::DEFAULT_SOUL),
            ("USER.md", crate::DEFAULT_USER),
        ] {
            let path = home.join("workspace").join(file);
            assert!(path.exists(), "{file} 必须落盘");
            assert_eq!(
                fs::read_to_string(&path).unwrap(),
                expected,
                "root {file} 必须 = default/ 权威源（老贾版）"
            );
        }
    }

    /// personas/ 恰两槽：default（贾维斯）+ custom（自定义人格），五件套
    /// 齐全、PERSONA.json 三字段、_active=default、无 bug-analyst。
    fn assert_two_slots_materialized(home: &Path) {
        let p = personas(home);
        assert!(p.join("default").is_dir(), "default 槽必须物化");
        assert!(p.join("custom").is_dir(), "custom 槽必须物化");
        assert!(
            !p.join("bug-analyst").exists(),
            "bug-analyst 不得预装落盘（内置人格必须手动下载）"
        );
        // 目录集合恰 = {custom, default}（read_dir 序是平台实现细节，排序后断言）
        let mut dirs: Vec<String> = fs::read_dir(&p)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        dirs.sort();
        assert_eq!(dirs, vec!["custom".to_string(), "default".to_string()]);

        // 两槽五件套齐全
        for slot in ["default", "custom"] {
            for f in FIVE_PIECE {
                assert!(
                    p.join(slot).join(f).exists(),
                    "{slot}/{f} 必须物化（五件套口径）"
                );
            }
        }

        // PERSONA.json：default=贾维斯/😄、custom=自定义人格/🧩（三字段形态）
        let dpj: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(p.join("default").join("PERSONA.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(dpj["name"], "贾维斯");
        assert_eq!(dpj["emoji"], "😄");
        assert!(dpj.get("description").is_some(), "description 字段在场");
        let cpj: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(p.join("custom").join("PERSONA.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cpj["name"], "自定义人格");
        assert_eq!(cpj["emoji"], "🧩");

        // 激活标记默认指向 default
        let active: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(p.join("_active.json")).unwrap()).unwrap();
        assert_eq!(active["name"], "default");
    }
}

// ---------------------------------------------------------------------------
// #5 安全模板（W2，2026-11-20）：场景解析表 + template 落盘 merge 语义。
// ---------------------------------------------------------------------------

#[test]
fn w5_template_scenario_parse_table() {
    use super::SecurityScenario;
    // 大小写不敏感 + 首尾空白容忍。
    assert_eq!(
        SecurityScenario::parse("personal"),
        Some(SecurityScenario::Personal)
    );
    assert_eq!(SecurityScenario::parse("DEV"), Some(SecurityScenario::Dev));
    assert_eq!(
        SecurityScenario::parse(" Server "),
        Some(SecurityScenario::Server)
    );
    assert_eq!(SecurityScenario::parse("iot"), Some(SecurityScenario::Iot));
    // 未知名 = None（CLI loud 报错列清单，不静默纠偏）。
    assert_eq!(SecurityScenario::parse("paranoid"), None);
    assert_eq!(SecurityScenario::parse(""), None);
}

#[test]
fn w5_template_scenario_level_table() {
    use super::SecurityScenario;
    // 档位映射：personal/iot=basic，dev=standard，server=strict。
    assert_eq!(SecurityScenario::Personal.level_str(), "basic");
    assert_eq!(SecurityScenario::Iot.level_str(), "basic");
    assert_eq!(SecurityScenario::Dev.level_str(), "standard");
    assert_eq!(SecurityScenario::Server.level_str(), "strict");
    // 说明文案非空（打印面是模板的价值所在）。
    for s in [
        SecurityScenario::Personal,
        SecurityScenario::Dev,
        SecurityScenario::Server,
        SecurityScenario::Iot,
    ] {
        assert!(
            !s.description().is_empty(),
            "{} description 非空",
            s.level_str()
        );
    }
}

#[test]
fn w5_template_writes_security_level_into_fresh_home() {
    use super::{SecurityScenario, onboard_template};
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("tpl-home");

    onboard_template(&home, false, SecurityScenario::Server).unwrap();

    // 12 步初始化照常发生（workspace/persona 等都在）。
    assert!(home.join("workspace").exists(), "workspace 目录已物化");
    // config.json 的 security.level = strict（merge 进完整 config，其余键在）。
    let cfg_path = super::common::config_path(&home);
    let cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(cfg["security"]["level"], serde_json::json!("strict"));
    // 档位表不物化：模板只写 level 一个键——流控/配额/executor 等缺席，
    // 由档位缺省真相源在运行期取值（防两处漂移的设计决策）。
    assert!(
        cfg["security"].get("rate_limit").is_none(),
        "模板不得物化档位表（rate_limit 应缺席，由档位缺省供给）"
    );
    assert!(
        cfg["security"].get("executor").is_none(),
        "模板不得物化档位表（executor 应缺席）"
    );
}

#[test]
fn w5_template_merges_into_existing_config_without_clobber() {
    use super::{SecurityScenario, onboard_template};
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().join("merge-home");
    // 先正常初始化，再手写一个用户自定义键，最后套模板——只动 security.level。
    super::onboard_default(&home, false, OnboardMode::Cli).unwrap();
    let cfg_path = super::common::config_path(&home);
    let mut cfg: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    cfg["custom_user_key"] = serde_json::json!("keep-me");
    cfg["model_list"] = serde_json::json!([{ "model_name": "user-model" }]);
    fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

    onboard_template(&home, false, SecurityScenario::Dev).unwrap();

    let after: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(after["security"]["level"], serde_json::json!("standard"));
    assert_eq!(
        after["custom_user_key"],
        serde_json::json!("keep-me"),
        "merge 语义：用户自定义键必须原样保留"
    );
    assert!(
        after["model_list"].as_array().map(|a| a.len()) == Some(1),
        "用户 model_list 不被模板重写"
    );
}
