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

fn common_config(home: &std::path::Path) -> std::path::PathBuf {
    crate::common::config_path(home)
}

// ===========================================================================
// wave5 round2 batch-2（2026-09-25）：onboard_default 的 local=true 臂——
// agents.defaults.workspace 改写为相对路径 ".nemesisbot/workspace"
//（既有 onboard 测试全部 local=false，该分支从未执行）。
// ===========================================================================

mod w5b2 {
    use super::super::{OnboardMode, onboard_default};

    /// Cli 模式 + local=true：主 config 的 agents.defaults.workspace 必须
    /// 被改写为 ".nemesisbot/workspace"（--local 布局语义）。
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
// R55 持久化静默失败审计：人格文件安装（install_if_absent）此前是裸
// std::fs::write——(1) 漏掉 REL-002 原子写横扫（半截 IDENTITY.md = 截断
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
