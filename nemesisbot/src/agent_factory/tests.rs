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
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    });
    // Cluster::new only builds in-memory state (registry / task manager /
    // continuation store paths) — no sockets, safe in tests.
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
        node_name: String::new(),
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

/// E1 二期（全自动流转 P5）回归：cluster agent loop 必须接上用量账本。
///
/// T37① 真机抓到的产品 bug：`build_cluster_agent_loop` 漏了
/// `set_data_store`——B 端 LLM 调用不落 request_logs → 任务收尾
/// `extract_task_usage` 聚合恒零 → 回调 usage=None → master 诚实跳过
/// 记账，token 回传整链静默失效。修复后与 build_agent_loop 同源接线。
#[tokio::test]
async fn cluster_agent_loop_wires_usage_ledger_from_shared() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_mini_model_config(&home);

    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
        node_name: String::new(),
    }));

    // 有账本：shared.data_store=Some → loop 必须拿到同一份。
    let ds = Arc::new(
        nemesis_data::DataStore::open(
            &home
                .join("workspace")
                .join("data")
                .join("nemesisbot_data.db"),
        )
        .expect("open ledger"),
    );
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        data_store: Some(ds.clone()),
        ..Default::default()
    });
    let (agent_loop, _config, _observer) =
        build_cluster_agent_loop(&shared, cluster.clone()).expect("factory must succeed");
    assert!(
        agent_loop.data_store().is_some(),
        "cluster loop 必须接上用量账本（E1 二期 usage 提取的生产前提）"
    );

    // 无账本（DataStore 打开失败网关给 None）：loop 不炸、诚实 None——
    // 与 usage 提取的 honest-zero 边界一致。
    let (outbound_tx2, _rx2) = tokio::sync::mpsc::channel(16);
    let shared_no_ds = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx2,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    });
    let (loop_no_ds, _c2, _o2) =
        build_cluster_agent_loop(&shared_no_ds, cluster).expect("factory must succeed");
    assert!(loop_no_ds.data_store().is_none());
}

/// F-U3-2 接线回归（UAT U3 round-2 实证，T37① 同型）：`build_cluster_agent_loop`
/// 必须接上 `set_workspace_root`——缺了它 `cluster_agent::build_context` 的
/// tool_path_base 注入条件恒 None，档案管线任务的相对路径重写静默失效
/// （worker 文件落 workspace 根，变更集丢失，整条 E3 交付链空转）。
#[tokio::test]
async fn cluster_agent_loop_wires_workspace_root_from_shared() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_mini_model_config(&home);

    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "test-node-wsroot".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
        node_name: String::new(),
    }));

    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        ..Default::default()
    });
    let (agent_loop, _config, _observer) =
        build_cluster_agent_loop(&shared, cluster).expect("factory must succeed");
    assert_eq!(
        agent_loop.workspace_root().as_deref(),
        Some(shared.workspace_dir().as_path()),
        "cluster loop 必须接上工作区根（F-U3-2 tool_path_base 注入的生产前提）"
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
    // FT（2026-09-17）：人格段之后固定追加 Workspace 尾行（绝对路径随
    // tempfile 落点不定 → 前缀/收尾断言，不整串比对）。
    assert!(
        prompt.starts_with("我是集群节点\n\n---\n\n核心原则\n\n---\n\n**Workspace**: "),
        "{prompt}"
    );
    // 顺序：IDENTITY 在前 SOUL 在后，Workspace 收尾。
    assert!(prompt.ends_with("read_file/list_dir 等文件工具的相对路径以此为根。"));
    // Workspace 行指向 <home>/workspace（与装配处 set_workspace_root 同源）。
    assert!(prompt.contains(&format!(
        "**Workspace**: {}",
        tmp.path().join("workspace").display()
    )));
}

#[test]
fn cluster_prompt_skips_blank_files_and_single_file_works() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = cluster_dir(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    // 只有 IDENTITY（SOUL 缺失）→ 单文件也 Some（人格段 + Workspace 尾行）。
    std::fs::write(dir.join("IDENTITY.md"), "only identity").unwrap();
    let prompt = load_cluster_system_prompt(tmp.path()).expect("单文件 → Some");
    assert!(
        prompt.starts_with("only identity\n\n---\n\n**Workspace**: "),
        "{prompt}"
    );

    // 空白文件视为缺（trim 后为空跳过）→ 全空白 = None。
    let tmp2 = tempfile::TempDir::new().unwrap();
    let dir2 = cluster_dir(tmp2.path());
    std::fs::create_dir_all(&dir2).unwrap();
    std::fs::write(dir2.join("IDENTITY.md"), "  \n \n").unwrap();
    std::fs::write(dir2.join("SOUL.md"), "").unwrap();
    assert!(
        load_cluster_system_prompt(tmp2.path()).is_none(),
        "空白文件跳过 → None"
    );
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
                if let (Some(dst), Some(src)) = (
                    base.get_mut("agents").and_then(|a| a.as_object_mut()),
                    v.as_object(),
                ) {
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
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
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
async fn build_agent_loop_degrades_when_config_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    // 不写 config.json → load_config 走默认回落（zhipu 默认模型无 key）→
    // resolve 失败 → 无 LLM 降级装配（NullProvider，Ok 不 Err）——双击直启
    // 语义（2026-09-17）：启动不再强制配好 LLM；对话打到 NullProvider 得到
    // 诚实「未配置模型」报错，Dashboard 配好并设默认后热切恢复。
    let built = build_agent_loop(&make_shared(tmp.path()))
        .expect("missing config.json must degrade to NullProvider assembly");
    assert!(built.tool_count() > 0);
}

#[tokio::test]
async fn build_agent_loop_degrades_when_model_unresolvable() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    // agents.defaults.llm 指向不存在的条目 + model_list 为空 → resolve 失败
    // → 降级装配（Ok），不 panic / 不半装配（同 config-missing 语义）。
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

    let built = build_agent_loop(&make_shared(&home))
        .expect("unresolvable model must degrade to NullProvider assembly");
    assert!(built.tool_count() > 0);
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
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
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
    write_model_config(
        &home,
        serde_json::json!({ "executor": { "enabled": true } }),
    );

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

    let built = build_agent_loop(&shared).expect("executor Layer 1 config must build");
    assert!(
        built.tool_count() > 0,
        "MOVE tools replaced, count unchanged"
    );
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
        let sess_dir = home.join("workspace").join("sessions").join("cluster");
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

    /// hooks.json（K2/U14）：`{"hooks":{...}}` 包装形态 + 一条合法
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
        let built = build_agent_loop(&shared).expect("seeded config must build offline");
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
        fn r10_call_fn() -> Option<
            Arc<
                dyn Fn(
                        &str,
                        &str,
                        serde_json::Value,
                    ) -> std::pin::Pin<
                        Box<
                            dyn std::future::Future<Output = Result<serde_json::Value, String>>
                                + Send,
                        >,
                    > + Send
                    + Sync,
            >,
        > {
            Some(Arc::new(
                |_peer: &str, _method: &str, _args: serde_json::Value| {
                    Box::pin(async { Err("r10 offline stub".to_string()) })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<Output = Result<serde_json::Value, String>>
                                    + Send,
                            >,
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
            node_name: String::new(),
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
        let channel = crate::exec_world::build_executor_channel(&home, &workspace, store.handle())
            .expect("channel build must not error on unready sandbox");
        // 沙盒未就绪 ≠ 失败：降级为 stdio（Layer 1）通道仍然给出。
        assert!(
            channel.is_some(),
            "sandbox=true 但引擎未就绪必须降级为 stdio 通道（不是 None）"
        );
    }
}

// ===========================================================================
// ASM-08 装配矩阵（2026-09-16 横扫存量加固）：三 builder 关键件接线 + 断言
// helper 触发路径。
//
// 双层防线：
// ① builder 内部已调 assert_gateway_critical_wiring（漏接 = 启动即炸）——
//    本文件所有 build_* 成功本身即证明关键件齐；
// ② 以下测试再用 wiring_status() 直接断言 + 裸 AgentLoop::new 直测 helper
//    的 bail 文案——防将来有人删掉 builder 内的断言调用后回归无人知晓。
//
// security_plugin 一致性矩阵（helper 语义，有意为之勿「修直」）：
//   None/None（security.enabled=false 或 feature 裁剪）与 Some/Some（gateway
//   生产形态）都是合法态；fail 的只有「shared 有而 loop 没接」。
// =========================================================================

/// ASM-08 触发路径专用 stub：chat 恒 Err（装配测试不触 LLM）。
struct Asm08StubProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for Asm08StubProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Err("asm08 stub never chats".to_string())
    }
}

/// 裸构造一个关键件全缺的 loop（旧 standalone 构造器：estop/workspace_root/
/// config_path/pricing_store 全 None）——helper 触发路径的测试夹具。
fn fresh_unwired_loop() -> nemesis_agent::r#loop::AgentLoop {
    nemesis_agent::r#loop::AgentLoop::new(
        Box::new(Asm08StubProvider),
        nemesis_agent::types::AgentConfig {
            model: "asm08-stub".to_string(),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn asm08_main_loop_critical_wiring_matrix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_model_config(&home, serde_json::json!({}));

    let built = build_agent_loop(&make_shared(&home)).expect("factory must succeed");
    let status: std::collections::HashMap<&str, bool> = built.wiring_status().into_iter().collect();
    for key in ["estop", "workspace_root", "config_path", "pricing_store"] {
        assert!(
            status.get(key).copied().unwrap_or(false),
            "主 loop 关键件 `{key}` 未接线（ASM-08 矩阵回归）"
        );
    }
    // security_plugin 在 shared=None（默认 config）时 loop=None 合法——
    // 一致性断言的 None 分支；Some/Some 与 Some/None 分支见集群矩阵 +
    // helper 一致性测试。
}

#[tokio::test]
async fn asm08_cluster_loop_critical_wiring_matrix() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path().to_path_buf();
    write_mini_model_config(&home);

    // security_plugin = Some（gateway 生产形态，ASM-01）：security feature
    // 裁剪时字段退化为 Option<()>，Some(()) 同样走 None 分支语义。
    #[cfg(feature = "security")]
    let plugin = crate::security_setup::build_security_plugin(&home, true)
        .await
        .expect("security plugin must build offline");
    #[cfg(not(feature = "security"))]
    let plugin = ();

    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        security_plugin: Some(plugin),
        ..Default::default()
    });
    let cluster = Arc::new(nemesis_cluster::cluster::Cluster::new(ClusterConfig {
        node_id: "asm08-node".to_string(),
        bind_address: "127.0.0.1:0".to_string(),
        peers: Vec::new(),
        node_name: String::new(),
    }));
    let (agent_loop, _config, _observer) =
        build_cluster_agent_loop(&shared, cluster).expect("cluster factory must succeed");

    let status: std::collections::HashMap<&str, bool> =
        agent_loop.wiring_status().into_iter().collect();
    let mut keys = vec!["estop", "workspace_root", "config_path", "pricing_store"];
    #[cfg(feature = "security")]
    keys.push("security_plugin");
    for key in keys {
        assert!(
            status.get(key).copied().unwrap_or(false),
            "集群 loop 关键件 `{key}` 未接线（ASM-08 矩阵回归，含 ASM-01 安全管线）"
        );
    }
}

#[tokio::test]
async fn asm08_assert_helper_bails_and_names_missing_keys_on_fresh_loop() {
    let loop_ = fresh_unwired_loop();
    let shared = SharedResources::default();
    let err = assert_gateway_critical_wiring(&loop_, &shared, "ASM-08 矩阵")
        .expect_err("裸构造 loop 关键件全缺，helper 必须拦下（漏接不得静默）");
    let msg = err.to_string();
    assert!(msg.contains("ASM-08"), "bail 文案必须带 ASM-08 标记: {msg}");
    for key in ["estop", "workspace_root", "config_path"] {
        assert!(msg.contains(key), "bail 文案必须点名 `{key}`: {msg}");
    }
}

#[cfg(feature = "security")]
#[tokio::test]
async fn asm08_assert_helper_flags_shared_plugin_missing_on_loop() {
    // 一致性规则第三态：shared 有而 loop 没接 = 装配漏项（fail）。
    // （None/None 与 Some/Some 两个合法态分别由主/集群矩阵测试覆盖。）
    let tmp = tempfile::TempDir::new().unwrap();
    let plugin = crate::security_setup::build_security_plugin(tmp.path(), true)
        .await
        .expect("security plugin must build offline");
    let loop_ = fresh_unwired_loop();
    let shared = SharedResources {
        security_plugin: Some(plugin),
        ..Default::default()
    };
    let err = assert_gateway_critical_wiring(&loop_, &shared, "ASM-08 一致性")
        .expect_err("shared.security_plugin 有而 loop 未接必须拦下");
    assert!(
        err.to_string().contains("security_plugin"),
        "bail 必须点名 security_plugin: {err:#}"
    );
}

// =========================================================================
// 件2（2026-09-24 三合一收口）：spawn 闭包 SubagentStart/SubagentStop 触发点。
//
// 不发事件面（诚实边界，代码注记为准）：board_review / conflict_resolver /
// web board 的 run_detached 是内部 LLM 委托，不挂 spawn 闭包，天然不发。
// payload 字段全景由 nemesis-agent cc_hooks 单测钉住，这里只钉触发点接线。
// =========================================================================

fn refuse_cmd() -> String {
    if cfg!(windows) {
        "echo refused-by-hook 1>&2 & exit 2".to_string()
    } else {
        "echo refused-by-hook >&2; exit 2".to_string()
    }
}

fn append_marker_cmd(marker: &std::path::Path) -> String {
    let m = marker.to_string_lossy();
    if cfg!(windows) {
        format!("echo fired>>{m}")
    } else {
        format!("echo fired >> {m}")
    }
}

fn marker_lines_of(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path)
        .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
        .unwrap_or(0)
}

fn spawn_fixture(
    doc: &str,
) -> (
    Arc<nemesis_agent::r#loop::AgentLoop>,
    Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>>,
) {
    // keep：桥的 project_dir 是脚本 cwd——TempDir 句柄 drop 即删目录，脚本
    // 启动直接失败（非阻断放行），钩子永远不触发（cc_hooks 测试同款教训）。
    let tmp = tempfile::tempdir().expect("tempdir").keep();
    let bridge = Arc::new(
        nemesis_agent::cc_hooks::CcHookBridge::from_json(doc, tmp.clone()).expect("bridge"),
    );
    let loop_arc = Arc::new(fresh_unwired_loop());
    let slot: Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>> =
        Arc::new(std::sync::OnceLock::new());
    let bus = Arc::new(nemesis_bus::MessageBus::new());
    super::inject_spawn_fn(&loop_arc, &slot, &bus, Some(bridge));
    (loop_arc, slot)
}

/// 前台三路径：Start exit 2 = 拒绝（Err 带阻断文案，loop 不动）；Start 放行
/// + Stop 在 detached 轮次返回后落 marker（stub LLM 失败也发——观察型）。
#[tokio::test]
async fn spawn_closure_foreground_refuse_and_stop_fires() {
    // ① 拒绝路径。
    let doc = serde_json::json!({
        "hooks": {
            "SubagentStart": [{ "hooks": [{ "type": "command", "command": refuse_cmd() }] }]
        }
    })
    .to_string();
    let (_loop_arc, slot) = spawn_fixture(&doc);
    let spawn = slot.get().expect("closure injected");
    let err = spawn("agent-1", "do task", "", "", "", "readonly", 1, false)
        .await
        .expect_err("exit 2 must refuse spawn");
    assert!(err.contains("refused-by-hook"), "err={err}");

    // ② 放行路径：Stop 观察型——detached 轮次失败（stub LLM）也发。
    let tmp = tempfile::TempDir::new().unwrap();
    let stop_marker = tmp.path().join("stop.txt");
    let doc = serde_json::json!({
        "hooks": {
            "SubagentStart": [{ "hooks": [{ "type": "command", "command": "exit 0" }] }],
            "SubagentStop": [{ "hooks": [{ "type": "command", "command": append_marker_cmd(&stop_marker) }] }]
        }
    })
    .to_string();
    let (loop_arc, slot) = spawn_fixture(&doc);
    let spawn = slot.get().expect("closure injected");
    let _keep_alive = loop_arc; // Weak 升级需要 loop 活着
    let result = spawn("agent-1", "do task", "", "", "", "readonly", 1, false).await;
    assert!(result.is_err(), "stub provider never chats: {result:?}");
    assert!(
        marker_lines_of(&stop_marker) >= 1,
        "SubagentStop must fire even when the detached turn fails"
    );
}

/// 后台路径：Start 在 spawn 前落 marker；本调用立即返回 marker；后台任务
/// 完成后 Stop 落 marker（cancelled/failed/completed 三态判定见实现注记）。
#[tokio::test]
async fn spawn_closure_background_fires_start_then_stop() {
    let tmp = tempfile::TempDir::new().unwrap();
    let start_marker = tmp.path().join("start.txt");
    let stop_marker = tmp.path().join("stop.txt");
    let doc = serde_json::json!({
        "hooks": {
            "SubagentStart": [{ "hooks": [{ "type": "command", "command": append_marker_cmd(&start_marker) }] }],
            "SubagentStop": [{ "hooks": [{ "type": "command", "command": append_marker_cmd(&stop_marker) }] }]
        }
    })
    .to_string();
    let (loop_arc, slot) = spawn_fixture(&doc);
    let spawn = slot.get().expect("closure injected");
    let _keep_alive = loop_arc;
    let marker = spawn("agent-1", "bg task", "", "", "", "readonly", 1, true)
        .await
        .expect("background spawn returns marker");
    assert!(marker.starts_with("__BG_SPAWN__:"), "marker={marker}");
    assert_eq!(
        marker_lines_of(&start_marker),
        1,
        "SubagentStart must fire before the task spawns"
    );
    // 后台任务完成是异步的——轮询等 Stop marker（上限 ~5s）。
    for _ in 0..100 {
        if marker_lines_of(&stop_marker) >= 1 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("SubagentStop marker never appeared after background task completion");
}

/// 项目 loop 闭包：拒绝路径同主 spawn（loop_kind="project" 的 payload 字段
/// 由桥级单测覆盖；这里钉接线）。
#[tokio::test]
async fn project_spawn_closure_refuses_on_start_exit_2() {
    let tmp = tempfile::TempDir::new().unwrap();
    let doc = serde_json::json!({
        "hooks": {
            "SubagentStart": [{ "hooks": [{ "type": "command", "command": refuse_cmd() }] }]
        }
    })
    .to_string();
    let bridge = Arc::new(
        nemesis_agent::cc_hooks::CcHookBridge::from_json(&doc, tmp.path().to_path_buf())
            .expect("bridge"),
    );
    let loop_arc = Arc::new(fresh_unwired_loop());
    let slot: Arc<std::sync::OnceLock<nemesis_agent::loop_tools::SpawnFn>> =
        Arc::new(std::sync::OnceLock::new());
    super::inject_project_spawn_fn(&loop_arc, &slot, Some(bridge));
    let spawn = slot.get().expect("closure injected");
    let err = spawn("agent-1", "do task", "", "", "", "readonly", 1, false)
        .await
        .expect_err("project spawn must refuse on exit 2");
    assert!(err.contains("refused-by-hook"), "err={err}");
}

/// 件3 层2（2026-09-24）：延迟接线——真实 manager **后**挂（复刻 gateway 在
/// loop 构建之后才 set 的时序），观察任务轮询到即包 wrapper；之后从 auditor
/// 槽取到的 manager 已是 wrapper（委托 verdict 通过 + Notification 脚本
/// 真实执行）。Arc::ptr_eq 判「槽位已不是裸 stub」即包装完成。
#[cfg(feature = "security")]
#[tokio::test]
async fn approval_observer_wraps_manager_that_appears_later() {
    use nemesis_security::auditor::{ApprovalManager, ApprovalVerdict};

    struct StubManager;
    impl ApprovalManager for StubManager {
        fn is_running(&self) -> bool {
            true
        }
        fn request_approval_sync(
            &self,
            _request_id: &str,
            _operation: &str,
            _target: &str,
            _risk_level: &str,
            _reason: &str,
            _timeout_secs: u64,
        ) -> Result<ApprovalVerdict, String> {
            Ok(ApprovalVerdict::approved())
        }
    }

    // wrapper 委托时应触发的 Notification 脚本（追加 marker、exit 0）。
    let tmp = tempfile::tempdir().expect("tempdir").keep();
    let marker = tmp.join("obs.marker");
    let p = marker.to_string_lossy().to_string();
    let cmd = if cfg!(windows) {
        format!("echo N1>>{p}")
    } else {
        format!("echo N1 >> {p}")
    };
    let doc = serde_json::json!({
        "hooks": { "Notification": [{ "hooks": [{ "type": "command", "command": cmd }] }] }
    })
    .to_string();
    let bridge = Arc::new(
        nemesis_agent::cc_hooks::CcHookBridge::from_json(&doc, tmp.clone()).expect("bridge"),
    );

    let plugin = Arc::new(nemesis_security::pipeline::SecurityPlugin::new(
        nemesis_security::pipeline::SecurityPluginConfig::default(),
    ));

    // manager 尚未挂——观察任务进入轮询窗口。
    super::spawn_approval_observer_once(Arc::clone(&bridge), Arc::clone(&plugin));

    // 200ms 后才挂真实 manager（复刻 runtime 装配时序：loop 构建之后）。
    let stub: Arc<dyn ApprovalManager> = Arc::new(StubManager);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    plugin.auditor().set_approval_manager(Arc::clone(&stub));

    // 轮询等包装完成（槽位 Arc 不再指向裸 stub）。上限 ~5s。
    let mut wrapped = false;
    for _ in 0..100 {
        if let Some(mgr) = plugin.auditor().get_approval_manager()
            && !std::sync::Arc::ptr_eq(&mgr, &stub)
        {
            wrapped = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(wrapped, "observer never wrapped the late-arriving manager");

    // 包装后的槽位：调用一次 = 委托 verdict 通过 + Notification 脚本执行。
    let mgr = plugin
        .auditor()
        .get_approval_manager()
        .expect("wrapped slot");
    let verdict = mgr
        .request_approval_sync("r-1", "write_file", "/x", "high", "why", 30)
        .expect("delegated");
    assert!(verdict.approved);
    for _ in 0..100 {
        if std::fs::read_to_string(&marker)
            .map(|t| t.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
            >= 1
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(
        marker.exists(),
        "Notification must fire through the wrapped slot"
    );
}
