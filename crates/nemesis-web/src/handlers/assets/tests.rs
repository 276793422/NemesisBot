//! assets 盘点器测试（W1 #11+#6–10）：诚实空清单 / skills 目录 ∪
//! lockfile 合流 / BOM 五类投影 / 快照落盘。聚合主函数（build_inventory
//! 需要完整 AppState——183 处字面量装配形态）归真链路（gateway 侧
//! integration-test / docs_generation 命令注册表覆盖），此处测可纯化的
//! 段函数与投影。

use super::{bom_view, build_skills_section, write_snapshot};
use serde_json::json;
use std::path::PathBuf;

fn temp_workspace(tag: &str) -> PathBuf {
    let base = std::env::temp_dir().join(format!(
        "assets-tests-{}-{}",
        tag,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&base).expect("create temp workspace");
    base
}

#[test]
fn empty_workspace_produces_honest_empty_sections() {
    let ws = temp_workspace("empty");
    let skills = build_skills_section(ws.to_str().unwrap());
    assert_eq!(skills["count"], 0, "无 skills 目录 = 诚实空清单");
    assert_eq!(
        skills["rows"].as_array().map(Vec::len),
        Some(0),
        "缺省态输出空数组而非 null/报错"
    );
    assert!(skills["note"].is_null());
}

#[test]
fn skills_section_merges_dirs_with_lockfile() {
    let ws = temp_workspace("merge");
    // ① alpha：目录 + lockfile 记账（来源/信任态来自记账）。
    let alpha = ws.join("skills").join("alpha");
    std::fs::create_dir_all(&alpha).unwrap();
    std::fs::write(
        alpha.join("SKILL.md"),
        "---\nname: alpha\ndescription: 测试技能 A\n---\nbody",
    )
    .unwrap();
    // ② beta：目录无记账（workspace-local）。
    let beta = ws.join("skills").join("beta");
    std::fs::create_dir_all(&beta).unwrap();
    // ③ ghost：lockfile 有记账、目录已删（可见性兜底，同 wasm.list 纪律）。
    let lockfile = json!({
        "version": 1,
        "skills": {
            "alpha": {
                "slug": "alpha",
                "source": "github:acme/skills",
                "commit": "abc123",
                "files": {"SKILL.md": "h1", "run.py": "h2"},
                "installed_at": 1_700_000_000i64,
                "verified_state": "verified",
            },
            "ghost": {
                "slug": "ghost",
                "source": "registry:main/x",
                "commit": "",
                "files": {"SKILL.md": "h1"},
                "installed_at": 1_700_000_100i64,
                "verified_state": "unsigned",
            }
        }
    });
    std::fs::write(
        ws.join("skills.lock.json"),
        serde_json::to_string(&lockfile).unwrap(),
    )
    .unwrap();

    let skills = build_skills_section(ws.to_str().unwrap());
    assert_eq!(skills["count"], 3, "alpha + beta + ghost = 3 行");
    let rows = skills["rows"].as_array().unwrap();
    // BTreeSet + 迭代 = 字典序确定性（beta < alpha？否——b > a，序：
    // alpha, beta, ghost）。
    let names: Vec<&str> = rows.iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["alpha", "beta", "ghost"]);

    let alpha_row = &rows[0];
    assert_eq!(alpha_row["source"], "github:acme/skills", "来源认 lockfile");
    assert_eq!(alpha_row["trust_state"], "verified", "信任态认 lockfile");
    assert_eq!(alpha_row["commit"], "abc123");
    assert_eq!(alpha_row["files_hashed"], 2);
    assert_eq!(
        alpha_row["description"], "测试技能 A",
        "描述来自 SKILL.md frontmatter"
    );

    let beta_row = &rows[1];
    assert_eq!(beta_row["source"], "workspace-local", "无记账 = 手工放置");
    assert!(
        beta_row["trust_state"].is_null(),
        "手工放置无信任态（诚实 null）"
    );
    assert!(beta_row["commit"].is_null());

    let ghost_row = &rows[2];
    assert_eq!(ghost_row["source"], "registry:main/x", "载荷已删仍可见");
}

#[test]
fn bom_view_projects_five_categories_with_honest_nulls() {
    // 最小 inventory 文档（与 build_inventory 产出同形）。
    let inventory = json!({
        "generated_at": 1_700_000_000i64,
        "bot_version": "1.2.3",
        "framework": [{ "name": "nemesisbot", "version": "1.2.3", "source": "builtin" }],
        "models": { "count": 1, "rows": [{
            "model": "test/testai-1.1", "provider_name": "test",
            "api_base": "http://127.0.0.1:8080/v1", "default": true,
        }]},
        "skills": { "count": 1, "rows": [{
            "name": "alpha", "source": "github:acme/skills",
            "commit": "abc123", "trust_state": "verified",
        }]},
        "plugins_wasm": { "count": 1, "plugins": [{
            "slug": "translate", "version": "0.1.0", "trust": "review-required",
        }]},
        "mcp": { "count": 1, "servers": [{
            "name": "fs", "transport_type": "stdio", "url": "npx-fs",
        }]},
    });
    let bom = bom_view(&inventory);
    let comps = bom["components"].as_array().unwrap();
    assert_eq!(
        comps.len(),
        5,
        "五类组件：framework/model/skill/tool_plugin/mcp"
    );
    assert_eq!(comps[0]["category"], "framework");
    assert_eq!(comps[0]["count"], 1);
    assert_eq!(comps[1]["category"], "model");
    let model_row = &comps[1]["rows"][0];
    assert_eq!(model_row["name"], "test/testai-1.1");
    assert_eq!(model_row["source"], "http://127.0.0.1:8080/v1");
    assert!(
        model_row["hash"].is_null(),
        "模型无文件 hash（诚实 null 不编造）"
    );
    assert!(model_row["signature"].is_null());
    assert_eq!(comps[2]["category"], "skill");
    let skill_row = &comps[2]["rows"][0];
    assert_eq!(
        skill_row["hash"], "abc123",
        "技能 hash = 锁定 commit（供应链 pin）"
    );
    assert_eq!(
        skill_row["signature"], "verified",
        "签名面 = lockfile 信任态"
    );
    assert_eq!(comps[3]["category"], "tool_plugin");
    let plugin_row = &comps[3]["rows"][0];
    assert_eq!(plugin_row["version"], "0.1.0");
    assert_eq!(plugin_row["signature"], "review-required");
    assert_eq!(comps[4]["category"], "mcp");
    assert_eq!(
        comps[4]["rows"][0]["source"], "stdio npx-fs",
        "MCP source = transport + endpoint"
    );
    assert_eq!(bom["bot_version"], "1.2.3");
}

#[test]
fn bom_view_on_empty_inventory_yields_zeroed_categories() {
    let empty = json!({ "generated_at": 0, "bot_version": "" });
    let bom = bom_view(&empty);
    let comps = bom["components"].as_array().unwrap();
    assert_eq!(comps.len(), 5);
    for c in comps {
        assert_eq!(
            c["count"], 0,
            "缺省态五类全空（诚实清单）: {}",
            c["category"]
        );
        assert_eq!(c["rows"].as_array().map(Vec::len), Some(0));
    }
}

#[test]
fn write_snapshot_creates_dirs_and_parses_back() {
    let ws = temp_workspace("snapshot");
    let doc = json!({ "generated_at": 42, "framework": [] });
    let path = write_snapshot(ws.to_str().unwrap(), &doc).expect("snapshot write");
    assert!(
        path.contains("logs") && path.contains("assets") && path.ends_with("inventory.json"),
        "落盘路径 = <workspace>/logs/assets/inventory.json: {path}"
    );
    let raw = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).expect("snapshot parses back");
    assert_eq!(parsed["generated_at"], 42);
    // 幂等重写（周期刷新同路径覆盖）。
    write_snapshot(ws.to_str().unwrap(), &doc).expect("rewrite is idempotent");
}

// BUG-20261122-042 回归：build_inventory 的 memory 统计 config_dir 错层/
// 空路径。修复前 assets.rs 手搓 `home.join("config")`（错层：<home>/config
// ≠ <home>/workspace/config 真相源——生产 home 恒 Some，每次资产页查询 /
// skills 安装刷新都在 <home>/config 下 load-or-create 默认 embedding 配置
// = 错位置写盘 + 统计读错文件）；home=None 臂更兜底 `Path::new("")`
// （load_embedding_config("") = 相对进程 cwd 落盘——crate 源码树被测试
// 进程撒 config.enhanced_memory.json 的实锤现场）。修复后与 memory.rs
// WSAPI 主路径同源（nemesis_path::workspace_config_dir(workspace)）。
// 本测试 home=None + tempdir workspace：断言 embedding 配置 load-or-create
// 落在 <workspace>/config/ 真相源、进程 cwd 无落盘。
// 字面量 AppState 构造打破本文件「不测聚合主函数」的既有口径，理由：
// 聚合函数存在路径副作用回归且缺陷恰在聚合装配段（段函数测不到）。
#[cfg(feature = "memory")]
#[test]
fn memory_stats_config_dir_follows_workspace_truth_source() {
    use crate::api_handlers::AppState;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: None, // 缺陷臂：home 缺失曾兜底空路径 → 相对 cwd 落盘
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        // AppState 双声明字段（workflow 门控真臂 / `Arc<()>` stub 臂）——
        // per-combo 构造让本测试在任意 feature 组合下编译（fork_route_tests
        // make_state 同款形态）。
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
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

    let cwd_file = std::env::current_dir()
        .expect("cwd")
        .join("config.enhanced_memory.json");
    let had_cwd_file = cwd_file.exists();

    let out = super::build_inventory(&state, &ws, None);

    assert_eq!(
        out["memory"]["enabled"], false,
        "memory_manager 未装配 = enabled false（诚实缺省）"
    );
    assert!(
        out["memory"]["active_tier"].is_string(),
        "stats 走通（active_tier 来自 embedding 配置，config_dir 由 workspace 真相源解析）: {}",
        out["memory"]
    );
    assert!(
        dir.path()
            .join("config")
            .join("config.enhanced_memory.json")
            .exists(),
        "embedding 配置 load-or-create 落在 <workspace>/config/ 真相源"
    );
    if !had_cwd_file {
        assert!(
            !cwd_file.exists(),
            "BUG-20261122-042 回归：默认 embedding 配置被写到进程 cwd（{}）",
            cwd_file.display()
        );
    }
}
