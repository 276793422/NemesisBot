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

    // D2: spill root wired to U4 设计位 workspace/logs/spill（2026-08-31 迁移）。
    assert_eq!(
        agent_loop.spill_root_path(),
        Some(home.join("workspace").join("logs").join("spill"))
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
    // 真相源对齐（2026-08-27 WaveC 缺口修复）：生产 gateway 启动先
    // `ConfigStore::load(config_path)` 再把句柄交给工厂（gateway.rs 同款顺序），
    // 而 build_agent_loop 的 executor 通道装配读的是 shared.config_store.handle()
    // 运行时缓存，不是磁盘 config.json。此前夹具把 executor 段只写进磁盘、store
    // 却留在 Config::default() → build_executor_channel 恒返回 None → Layer 1
    // MOVE 工具替换循环从未被真实命中（假绿）。这里按 gateway 顺序把磁盘配置
    // 装进 store；load 失败（config.json 缺失/坏 JSON 时 load_config 有内嵌默认
    // 回落，Err 极罕见）退回默认 store，与旧夹具行为一致。
    // 每次调用独立建 store、不碰 set_global 单例 —— 测试间零共享状态。
    let config_store = match nemesis_config::ConfigStore::load(&home.join("config.json")) {
        Ok(store) => Arc::new(store),
        Err(_) => Arc::new(nemesis_config::ConfigStore::from_config(
            nemesis_config::Config::default(),
            home.join("config.json"),
        )),
    };
    Arc::new(SharedResources {
        home: home.to_path_buf(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(nemesis_cron::service::CronService::new(
            "",
        ))),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        config_store,
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
    // 2026-08-31 spill 根迁回 workspace（U4 设计指定位置）。
    assert_eq!(
        loop1.spill_root_path(),
        Some(home.join("workspace").join("logs").join("spill"))
    );

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
    //
    // 配置必须经 ConfigStore 注入（夹具真相源）：工厂的通道装配读
    // shared.config_store.handle()，不读磁盘。RemoteExecutorTool 的元数据故意
    // 全量委托给本地实现（prompt-cache 逐字节同源保证），注册边界无任何外部
    // 可观测物 —— 所以能钉住的确定性契约是「同一句柄喂给 build_executor_channel
    // 必须出 Some」：它失守即意味着有人把注入改回只写磁盘（替换循环随之静默失活）。
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(&home, serde_json::json!({ "executor": { "enabled": true } }));

    let shared = make_shared(&home);
    let channel = crate::exec_world::build_executor_channel(
        &home,
        &home.join("workspace"),
        shared.config_store.handle(),
    )
    .expect("channel build must not error offline");
    assert!(
        channel.is_some(),
        "fixture truth-source broken: executor.enabled=true did not reach the \
         ConfigStore the factory reads — Layer 1 replacement loop would be skipped"
    );

    let built =
        build_agent_loop(&shared).expect("executor Layer 1 config must build");
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

// ===========================================================================
// R10 补测批（coverage-95 goal）：tracing 懒求值族 + 种子文件族。
//
// 背景：tracing 宏的参数表达式只在 subscriber 存在且级别启用时才求值 ——
// 本文件既有测试从不装 logger，factory 内所有 info!(字段=..) 的参数在
// 覆盖图上都是未执行行。本批测试先 enable_tracing()（OnceLock 幂等）让
// 这些行真实落地；再按分支预置种子（过期 spill 文件 / 过期 session json /
// 过期 rpc_cache 快照 / hooks.json / auto_inject 配置 / web api_key），吃掉
// 各 startup-cleanup 分支和装配分支。
//
// 非确定性边界（诚实标注）：rpc_cache 启动清扫是 once-guard + tokio spawn
// —— 每个测试进程只有第一个 build_* 调用真正 spawn 立即清扫任务，并行
// 测试下谁赢 guard 不确定。rpc_cache 目录【不种子任何 *.json】——不是
// 因为会 panic（recover_to_manager 的内存判定已改 try_lock，见回归测试
// recover_from_disk_inside_async_runtime_does_not_panic），而是 once-guard
// 的 spawn 扫盘在并行测试下不确定会让断言 flake。扫盘分支代码
// 由空目录路径吃到（stale_task_ids 在空目录上返回空集）。spill/SessionStore
// 清扫（deleted>0 → info!）才是确定性断言目标。
// 798-817（log_cb tool_call 等臂）：callback 是私有闭包、仅经 loop 的
// emit_observer_sync 在真实 LLM 轮次触发，standalone 无法注入 trace_id
// —— 结构性放弃，不在此批覆盖。
// =========================================================================

mod r10 {
    use std::time::Duration;

    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 3600);

    /// 打开默认 INFO logger（crate::common 的 OnceLock 幂等实现）——使
    /// factory 内 info!/warn! 的参数表达式被求值（tracing 懒求值补偿）。
    fn enable_tracing() {
        crate::common::ensure_default_logger();
    }

    /// 把 `path` 的 mtime 往前拨 `age`（纯 std：OpenOptions + FileTimes，
    /// 不依赖 PowerShell）。失败静默 —— 种子失败时对应 info 行退化为不
    /// 触发，不影响正确性断言之外的流程。
    fn backdate(path: &std::path::Path, age: Duration) {
        use std::fs::{FileTimes, OpenOptions};
        let Some(modified) = std::time::SystemTime::now().checked_sub(age) else {
            return;
        };
        if let Ok(f) = OpenOptions::new().write(true).open(path) {
            let _ = f.set_times(FileTimes::new().set_modified(modified));
        }
    }

    /// 过期时间戳常量（远早于任何运行时刻，age 只会单调增长 —— 免时钟
    /// 依赖）。cleanup_old_sessions 解析 RFC3339 的 `updated` 字段比年龄。
    const AGED_RFC3339: &str = "2000-01-01T00:00:00+00:00";

    /// 主工厂种子：过期 spill 文件 + 过期 Main session json + 过期
    /// rpc_cache 快照（后者清扫是否真的跑取决于 once-guard 抽签）。
    fn seed_aged_main_files(home: &std::path::Path) {
        // spill：<workspace>/logs/spill/<sess>/<file>（cleanup_expired 只扫两层；
        // 2026-08-31 迁回 workspace，U4 设计位）。
        let spill_file = home
            .join("workspace")
            .join("logs")
            .join("spill")
            .join("r10-sess")
            .join("r10-part-0000.json");
        std::fs::create_dir_all(spill_file.parent().unwrap()).unwrap();
        std::fs::write(&spill_file, r#"{"tool":"exec","chars":99999}"#).unwrap();
        backdate(&spill_file, 10 * DAY);

        // Main SessionStore：cleanup_old_sessions 认 *.json 且必须带
        // `updated`（RFC3339）与可选 `key` 字段 —— 手工最小快照即可。
        let sess_dir = home.join("workspace").join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        std::fs::write(
            sess_dir.join("r10-aged-main.json"),
            serde_json::json!({
                "key": "r10-aged-main",
                "updated": AGED_RFC3339,
                "messages": []
            })
            .to_string(),
        )
        .unwrap();

        // rpc_cache 快照：本测试【不种子】任何 *.json ——不是会 panic
        // （build_agent_loop 内部 ContinuationManager::with_disk_store →
        // recover_to_manager 的内存判定已改 try_lock，见回归测试
        // recover_from_disk_inside_async_runtime_does_not_panic），而是
        // once-guard 的 spawn 扫盘在并行测试下不确定会让断言 flake。
        // 清扫分支代码改由【空目录】路径吃到：spawn_rpc_cache_cleanup
        // 照常 spawn，stale_task_ids 在空目录上照样执行（返回空集）。
    }

    /// 集群工厂种子：过期 cluster session json + 过期 spill（共享根）。
    fn seed_aged_cluster_files(home: &std::path::Path) {
        let sess_dir = home
            .join("workspace")
            .join("sessions")
            .join("cluster");
        std::fs::create_dir_all(&sess_dir).unwrap();
        std::fs::write(
            sess_dir.join("r10-aged-cluster.json"),
            serde_json::json!({
                "key": "node-x/r10-chat",
                "updated": AGED_RFC3339,
                "messages": []
            })
            .to_string(),
        )
        .unwrap();

        let spill_file = home
            .join("workspace")
            .join("logs")
            .join("spill")
            .join("r10-cluster-sess")
            .join("r10-part-0001.json");
        std::fs::create_dir_all(spill_file.parent().unwrap()).unwrap();
        std::fs::write(&spill_file, "{}").unwrap();
        backdate(&spill_file, 10 * DAY);
    }

    /// CC hooks.json（K2/U14）：`{"hooks":{...}}` 包装形态 + 一条合法
    /// command 脚本 → load_from_dir 出 Some → bridge.register 命中。
    fn seed_hooks_json(home: &std::path::Path) {
        let cfg_dir = home.join("config");
        std::fs::create_dir_all(&cfg_dir).unwrap();
        std::fs::write(
            cfg_dir.join("hooks.json"),
            serde_json::json!({
                "hooks": {
                    "PreToolUse": [{
                        "matcher": "",
                        "hooks": [{"type": "command", "command": "echo r10-hook"}]
                    }]
                }
            })
            .to_string(),
        )
        .unwrap();
    }

    /// P3.1 auto-inject 开关种子：config.enhanced_memory.json（memory
    /// feature 关闭时该文件只是没人读的多余文件，无害）。
    fn seed_auto_inject(home: &std::path::Path) {
        let ws_cfg = home.join("workspace").join("config");
        std::fs::create_dir_all(&ws_cfg).unwrap();
        std::fs::write(
            ws_cfg.join("config.enhanced_memory.json"),
            serde_json::json!({"auto_inject": true, "auto_inject_top_k": 7}).to_string(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn r10_main_factory_seeds_and_tracing_family() {
        enable_tracing();
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();
        write_model_config(
            &home,
            serde_json::json!({
                "agents": {"defaults": {"spill_retention_days": 7}},
                "tools": {"web": {
                    "brave": {"enabled": true, "api_key": "r10-brave-key"},
                    "perplexity": {"enabled": true, "api_key": "r10-perp-key"}
                }}
            }),
        );
        seed_aged_main_files(&home);
        seed_hooks_json(&home);
        seed_auto_inject(&home);

        let shared = make_shared(&home);
        let built =
            build_agent_loop(&shared).expect("seeded config must build offline");
        assert!(built.tool_count() > 0);

        // 同步清扫的确定性证据：过期 spill 文件与过期 Main session 已被
        // build_agent_loop 内联清掉（对应两条 deleted>0 → info! 臂）。
        assert!(
            !home
                .join("workspace")
                .join("logs")
                .join("spill")
                .join("r10-sess")
                .join("r10-part-0000.json")
                .exists(),
            "retention=7d 下 10 天龄 spill 文件必须在启动清扫中被删"
        );
        assert!(
            !home
                .join("workspace")
                .join("sessions")
                .join("r10-aged-main.json")
                .exists(),
            "TTL=7d 下 2000 年 updated 的 Main session 必须在启动清扫中被删"
        );
        // rpc_cache 启动清扫是 once-guard + tokio spawn，跨测试 guard 归属
        // 非确定；不删除断言、不强制等价（空目录路径下扫盘代码仍被执行）。
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    #[tokio::test]
    async fn r10_cluster_factory_seeds_and_tracing_family() {
        enable_tracing();
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();
        write_model_config(
            &home,
            serde_json::json!({"agents": {"defaults": {"spill_retention_days": 7}}}),
        );

        // 身份文件齐 → load_cluster_system_prompt Some 分支（info 字段行）。
        let dir = cluster_dir(&home);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("IDENTITY.md"), "R10 集群节点身份").unwrap();
        std::fs::write(dir.join("SOUL.md"), "R10 核心原则").unwrap();

        seed_aged_cluster_files(&home);

        // 离线 RPC 工具装配：call_fn 直接返回 Err（不触网），足以命中
        // set_rpc_call_fn + registered info 臂。
        fn r10_call_fn()
        -> Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>> + Send + Sync>>
        {
            Some(Arc::new(
                |_peer: &str, _method: &str, _args: serde_json::Value| {
                    Box::pin(async { Err("r10 offline stub".to_string()) })
                        as std::pin::Pin<
                            Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                        >
                },
            ))
        }

        // SharedResources 不是 Clone（Arc 堆栈 + vtable 字段），按字段从
        // make_shared 摊平，再补集群 RPC 两项 —— 与现有 D1 测试的字段面一致。
        let base = make_shared(&home);
        let shared = Arc::new(SharedResources {
            home: base.home.clone(),
            agent_outbound_tx: base.agent_outbound_tx.clone(),
            cron_service: base.cron_service.clone(),
            mcp_config_path: base.mcp_config_path.clone(),
            config_store: base.config_store.clone(),
            cluster_rpc_config: Some(nemesis_agent::loop_tools::ClusterRpcConfig::default()),
            cluster_rpc_call_fn: r10_call_fn(),
            ..Default::default()
        });

        let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
            node_id: "r10-node".to_string(),
            bind_address: "127.0.0.1:0".to_string(),
            peers: Vec::new(),
        }));

        let (loop_, _config, _observer) =
            build_cluster_agent_loop(&shared, cluster).expect("cluster factory must succeed");

        assert!(matches!(
            loop_.tier(),
            nemesis_types::capability::ModelTier::Mini
        ));
        // 同步清扫证据（集群侧）：过期 cluster session 与过期 spill 已删。
        assert!(
            !home
                .join("workspace")
                .join("sessions")
                .join("cluster")
                .join("r10-aged-cluster.json")
                .exists(),
            "TTL=7d 下 2000 年 updated 的 cluster session 必须被启动清扫删除"
        );
        assert!(
            !home
                .join("workspace")
                .join("logs")
                .join("spill")
                .join("r10-cluster-sess")
                .join("r10-part-0001.json")
                .exists(),
            "retention=7d 下 10 天龄集群 spill 文件必须被启动清扫删除"
        );
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    /// exec_world P5-2 家族（A 批残留）：executor.enabled+sandbox=true 但本机
    /// 没有 Sandboxie 引擎（target 测试 home 下无 Start.exe、SbieSvc 未跑）
    /// → 走完整探针序列（sandbox/strict live-probe 闭包 + service_state +
    /// will_attach 判定）后落 degraded warn 臂（strict=ON 版文案），最后仍
    /// 给出 stdio 传输的 Some(channel)。离线确定性：探针只读文件系统与
    /// 服务状态，不起引擎不触网。
    #[cfg(all(feature = "sandbox", target_os = "windows"))]
    #[tokio::test]
    async fn r10_executor_channel_sandbox_unready_warn_family() {
        enable_tracing();
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path().to_path_buf();
        write_model_config(
            &home,
            serde_json::json!({
                "executor": {"enabled": true, "sandbox": true, "strict": true}
            }),
        );
        let store = nemesis_config::ConfigStore::load(&home.join("config.json"))
            .expect("valid fixture config");
        let workspace = home.join("workspace");
        let channel = crate::exec_world::build_executor_channel(
            &home,
            &workspace,
            store.handle(),
        )
        .expect("channel build must not error on unready sandbox");
        // 沙盒未就绪 ≠ 失败：降级为 stdio（Layer 1）通道仍然给出。
        assert!(
            channel.is_some(),
            "sandbox=true 但引擎未就绪必须降级为 stdio 通道（不是 None）"
        );
    }
}
