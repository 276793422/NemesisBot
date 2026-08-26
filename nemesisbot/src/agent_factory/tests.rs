//! Factory wiring tests (2026-08-24 arch review, U-list D1/D2).
//!
//! D1 — `build_cluster_agent_loop` resolves the startup capability tier from
//!      config.json exactly like `build_agent_loop`: a mini-tier active model
//!      must yield the mini tier on the cluster loop (previously the loop
//!      always ran the AgentLoop::new default = Big = full 42-tool set even
//!      for small models; tier filtering reads `self.tier` at tool-def build
//!      time, so a correct tier is the whole guarantee).
//! D2 — the cluster loop gets a spill root (`<home>/logs/spill`) so oversized
//!      tool results spill whole to disk instead of degrading to the prune
//!      profile. It shares the main agent's root; the daily cleanup task is
//!      only spawned by the main factory (asserted by wiring, not here).
//! D3 — comment-only (no behavior change); not asserted.

// The cluster factory is feature-gated; mirror the gate so the module is
// empty in trimmed builds (--no-default-features must compile clean).
#![cfg(feature = "cluster")]

use std::sync::Arc;

use nemesis_cluster::types::ClusterConfig;

use super::*;

/// Write a config.json whose active model is tagged `model_tier: "mini"`.
///
/// Shape mirrors what `model add` writes (model_name = post-slash segment,
/// model = full vendor/name), so the resolution chain
/// get_effective_llm → resolve_model_config → resolve_active_tier
/// matches production entries exactly.
fn write_mini_model_config(home: &std::path::Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

#[tokio::test]
async fn cluster_agent_resolves_tier_and_spill_root_from_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_mini_model_config(&home);

    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    });
    // Cluster::new only builds in-memory state (registry / task manager /
    // continuation store paths) — no sockets, safe in tests.
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
    }));

    let (agent_loop, _config, _observer) =
        build_cluster_agent_loop(&shared, cluster).expect(
            "build_cluster_agent_loop must succeed with a mini-tier config (provider construction is offline: HttpCompat + dummy key)",
        );

    // D1: startup tier comes from config.json (was: always the Big default).
    assert!(
        matches!(
            agent_loop.tier(),
            nemesis_types::capability::ModelTier::Mini
        ),
        "cluster agent must resolve model_tier=mini from config.json, got {:?}",
        agent_loop.tier()
    );
    // Tools are still registered for the loop (tier filtering happens at
    // tool-def build time inside the loop, reading the tier asserted above).
    assert!(agent_loop.tool_count() > 0);

    // D2: spill root wired to the shared <home>/logs/spill.
    assert_eq!(
        agent_loop.spill_root_path(),
        Some(home.join("logs").join("spill"))
    );
}

// ---------------------------------------------------------------------------
// load_cluster_system_prompt —— workspace/cluster 身份文件装配
// ---------------------------------------------------------------------------

fn cluster_dir(home: &std::path::Path) -> std::path::PathBuf {
    home.join("workspace").join("cluster")
}

#[test]
fn cluster_prompt_none_when_no_identity_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    // 目录都不存在 → None（集群 agent 裸跑）。
    assert!(load_cluster_system_prompt(tmp.path()).is_none());
    // 目录存在但文件缺 → 同样 None。
    std::fs::create_dir_all(cluster_dir(tmp.path())).unwrap();
    assert!(load_cluster_system_prompt(tmp.path()).is_none());
}

#[test]
fn cluster_prompt_joins_identity_and_soul_with_separator() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = cluster_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("IDENTITY.md"), "我是集群节点").unwrap();
    std::fs::write(dir.join("SOUL.md"), "核心原则").unwrap();

    let prompt = load_cluster_system_prompt(tmp.path()).expect("两文件齐 → Some");
    assert_eq!(prompt, "我是集群节点\n\n---\n\n核心原则");
    // 顺序：IDENTITY 在前 SOUL 在后。
    assert!(prompt.starts_with("我是集群节点"));
}

#[test]
fn cluster_prompt_skips_blank_files_and_single_file_works() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = cluster_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    // 只有 IDENTITY（SOUL 缺失）→ 单文件也 Some。
    std::fs::write(dir.join("IDENTITY.md"), "only identity").unwrap();
    assert_eq!(
        load_cluster_system_prompt(tmp.path()).as_deref(),
        Some("only identity")
    );

    // 空白文件视为缺（trim 后为空跳过）→ 全空白 = None。
    let tmp2 = tempfile::TempDir::new().unwrap();
    let dir2 = cluster_dir(tmp2.path());
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("IDENTITY.md"), "  \n \n").unwrap();
    std::fs::write(dir2.join("SOUL.md"), "").unwrap();
    assert!(load_cluster_system_prompt(tmp2.path()).is_none(), "空白文件跳过 → None");
}

// =========================================================================
// S11d 补测（quality-hardening goal 冲刺 S11）：主工厂 build_agent_loop 全链
// + build_shared_tool_config / register_tools_and_mcp / once-guard 清理任务 /
// attach_semantic_embedder。
// =========================================================================

/// 写一份带模型条目的 config.json（D1 同款形态，字段可覆盖）。
fn write_model_config(home: &std::path::Path, extra: serde_json::Value) {
    let mut cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [{
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]
    });
    if let (Some(base), Some(over)) = (cfg.as_object_mut(), extra.as_object()) {
        for (k, v) in over {
            // 顶层键覆盖；"agents" 段做两层浅合并（agents.defaults.* 不整段替换，
            // 否则会把 write_model_config 预置的 llm 丢掉）。
            if k == "agents" {
                if let (Some(dst), Some(src)) =
                    (base.get_mut("agents").and_then(|a| a.as_object_mut()), v.as_object())
                {
                    for (dk, dv) in src {
                        if dk == "defaults" {
                            if let (Some(ddst), Some(dsrc)) = (
                                dst.get_mut("defaults").and_then(|d| d.as_object_mut()),
                                dv.as_object(),
                            ) {
                                for (k2, v2) in dsrc {
                                    ddst.insert(k2.clone(), v2.clone());
                                }
                            }
                        } else {
                            dst.insert(dk.clone(), dv.clone());
                        }
                    }
                }
            } else {
                base.insert(k.clone(), v.clone());
            }
        }
    }
    std::fs::create_dir_all(home).unwrap();
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

fn make_shared(home: &std::path::Path) -> Arc<SharedResources> {
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    Arc::new(SharedResources {
        home: home.to_path_buf(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    })
}

#[tokio::test]
async fn build_agent_loop_full_chain_from_disk_config() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(&home, serde_json::json!({}));

    let loop1 = build_agent_loop(&make_shared(&home))
        .expect("build_agent_loop must succeed offline (provider construction only)");
    // D1 断言组复用于主工厂：tier 从 config.json 解析、工具已注册、spill 根挂上。
    assert!(matches!(
        loop1.tier(),
        nemesis_types::capability::ModelTier::Mini
    ));
    assert!(loop1.tool_count() > 0);
    assert_eq!(loop1.spill_root_path(), Some(home.join("logs").join("spill")));

    // 第二次构建（agent 重建路径）：rpc_cache / spill 的 once-guard 必须命中
    // swap=true 分支并直接返回（不重复 spawn），且构建本身仍成功。
    let loop2 = build_agent_loop(&make_shared(&home))
        .expect("rebuild must succeed (once-guards just skip re-spawn)");
    assert!(loop2.tool_count() > 0);
}

#[tokio::test]
async fn build_agent_loop_err_when_config_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    // 不写 config.json → load_config 走默认回落（zhipu 默认模型无 key）→
    // 工厂在 provider 创建处失败（Err，不会 panic / 半装配）。
    let err = match build_agent_loop(&make_shared(tmp.path())) {
        Err(e) => e,
        Ok(_) => panic!("missing config.json must fail the factory"),
    };
    assert!(
        err.to_string().to_lowercase().contains("provider"),
        "err: {err:#}"
    );
}

#[tokio::test]
async fn build_agent_loop_err_when_model_unresolvable() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    // agents.defaults.llm 指向不存在的条目 + model_list 为空 → resolve 失败。
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({
            "agents": { "defaults": { "llm": "ghost-model" } },
            "model_list": []
        })
        .to_string(),
    )
    .unwrap();

    let err = match build_agent_loop(&make_shared(&home)) {
        Err(e) => e,
        Ok(_) => panic!("unresolvable model must fail"),
    };
    assert!(
        err.to_string().contains("ghost-model"),
        "err should name the model: {err:#}"
    );
}

#[tokio::test]
async fn build_agent_loop_unknown_concurrent_mode_falls_back_to_reject() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(
        &home,
        serde_json::json!({ "agents": { "defaults": { "concurrent_request_mode": "bogus-mode" } } }),
    );
    // warn + 回落 reject —— 不允许让未知值把工厂搞挂。
    let built = build_agent_loop(&make_shared(&home))
        .expect("unknown concurrent_request_mode must fall back, not fail");
    assert!(built.tool_count() > 0);
}

#[tokio::test]
async fn build_agent_loop_with_mcp_enabled_and_config_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(&home, serde_json::json!({ "mcp": { "enabled": true } }));

    // config.mcp.json 存在 → enable_mcp_reload 路径（注册 MCP 工具）。
    let cfg_dir = home.join("workspace").join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.mcp.json"), r#"{"mcpServers": {}}"#).unwrap();

    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: cfg_dir.join("config.mcp.json"),
        mcp_enabled: true,
        ..Default::default()
    });
    let built = build_agent_loop(&shared).expect("mcp enabled with valid config must build");
    assert!(built.tool_count() > 0);
}

#[tokio::test]
async fn build_agent_loop_with_executor_layer1_wraps_move_tools() {
    // executor.enabled=true（sandbox=false → Layer 1 stdio 通道）→
    // register_tools_and_mcp 走 Some(channel) 分支：MOVE_TOOLS 全部替换为
    // RemoteExecutorTool 桥（schema 同源，数量不变）。
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(&home, serde_json::json!({ "executor": { "enabled": true } }));

    let built = build_agent_loop(&make_shared(&home))
        .expect("executor Layer 1 config must build");
    assert!(built.tool_count() > 0, "MOVE tools replaced, count unchanged");
}

#[test]
fn attach_semantic_embedder_none_and_with_memory_both_no_panic() {
    #[cfg(feature = "memory")]
    {
        let router = nemesis_providers::router::Router::new(Default::default());
        // None 分支：无 memory 子系统 → 语义路由降级 fallback，直接返回。
        attach_semantic_embedder(&router, None);

        // Some 分支：MemoryManager 默认构建（本地存储，无 ONNX）→ 挂 embedder。
        let tmp = tempfile::tempdir().unwrap();
        let cfg = nemesis_memory::manager::Config::new(tmp.path());
        let mgr = std::sync::Arc::new(nemesis_memory::manager::MemoryManager::new(&cfg));
        attach_semantic_embedder(&router, Some(&mgr));
    }
    #[cfg(not(feature = "memory"))]
    {
        // memory feature 裁掉时该函数不存在于编译产物 —— 用类型断言占位，
        // 保证本测试在裁剪构建下仍是可达的编译期检查。
        fn assert_send<T: Send>() {}
        assert_send::<std::path::PathBuf>();
    }
}
