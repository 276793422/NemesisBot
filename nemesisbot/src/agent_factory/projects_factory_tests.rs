//! L6++ 项目 loop 工厂测试（G2，2026-09-08）。
//!
//! 覆盖：构建成功 + 两 loop 共有工具 schema 字节一致（prompt cache 契约）+
//! cluster_rpc 剥离 + checkpoint 影子库两形态（.git 项目 / 纯目录）零污染 +
//! 目录缺失诚实拒绝。
//!
//! 独立夹具（不复用 tests.rs 的 write_mini_model_config——那边的模块顶有
//! `#![cfg(feature = "cluster")]`，跨模块引用会把本模块也锁到 cluster 门上）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{SharedResources, build_agent_loop, build_project_agent_loop, project_checkpoint_dir};
use crate::projects::registry::ProjectEntry;
use nemesis_agent::checkpoint::CheckpointBackend;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

/// 唯一临时目录（Windows SystemTime tick 粒度粗——键加 AtomicU64 序号 +
/// pid，绝不只靠时钟；见 memory test-isolation-debt 第 4 例）。
fn unique_home(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nb_proj_factory_{tag}_{}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        seq
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// mini 档模型 config（provider 离线构造安全：create_provider 只建 client
/// 结构，不发网络请求）。
fn write_mini_model_config(home: &Path) {
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [ {
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        } ]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

fn entry_for(id: &str, dir: &Path) -> ProjectEntry {
    ProjectEntry {
        id: id.to_string(),
        name: "测试项目".to_string(),
        path: dir.to_path_buf(),
        created_at: "2026-09-08T00:00:00Z".to_string(),
    }
}

/// SharedResources：离线默认件套 + cluster_rpc 配置（逼主 loop 注册
/// cluster_rpc，验证项目 loop 的剥离面）。
fn shared_for(home: &Path) -> Arc<SharedResources> {
    Arc::new(SharedResources {
        home: home.to_path_buf(),
        cluster_rpc_config: Some(nemesis_agent::loop_tools::ClusterRpcConfig::default()),
        cluster_rpc_call_fn: Some(Arc::new(
            |_node: &str, _method: &str, _payload: serde_json::Value| {
                Box::pin(async {
                    Err::<serde_json::Value, String>("offline test fixture".to_string())
                        as Result<serde_json::Value, String>
                })
                    as std::pin::Pin<
                        Box<
                            dyn std::future::Future<Output = Result<serde_json::Value, String>>
                                + Send,
                        >,
                    >
            },
        )
            as Arc<
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
            >),
        ..Default::default()
    })
}

fn dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect()
}

// ---------------------------------------------------------------------------
// 工厂构建 + schema 字节一致
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_factory_builds_loop_with_byte_identical_shared_schemas() {
    let home = unique_home("build");
    write_mini_model_config(&home);
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let session_store = main_loop
        .session_store()
        .cloned()
        .expect("main loop carries session store");
    let project_loop = build_project_agent_loop(
        &shared,
        &entry_for("p_factory01", &project_dir),
        session_store,
    )
    .expect("project loop builds");

    // 共享主 SessionStore：项目 loop 持有的 Arc 与主 loop 同一实例。
    let project_store = project_loop
        .session_store()
        .cloned()
        .expect("project loop carries session store");
    assert!(
        Arc::ptr_eq(
            &project_store,
            main_loop.session_store().expect("main store")
        ),
        "project loop must share the MAIN session store Arc"
    );

    // prompt cache 契约：项目 loop 的每个工具都必须存在于主 loop 且
    // parameters() 输出完全一致（serde_json::Value 相等 = schema 语义逐
    // 字节一致）。
    let project_tools = project_loop.tool_names();
    assert!(
        project_tools.len() > 10,
        "project loop should register the shared tool set, got {project_tools:?}"
    );
    for name in &project_tools {
        let p = project_loop.tool_parameters(name).expect("project schema");
        let m = main_loop
            .tool_parameters(name)
            .unwrap_or_else(|| panic!("tool '{name}' in project loop but not in main loop"));
        assert_eq!(p, m, "schema drift for tool '{name}'");
    }

    // 剥离面：cluster_rpc 只在主 loop（fixture 配了 cluster_rpc_config）。
    assert!(
        main_loop.tool_names().iter().any(|n| n == "cluster_rpc"),
        "fixture main loop should carry cluster_rpc"
    );
    assert!(
        !project_tools.iter().any(|n| n == "cluster_rpc"),
        "project loop must NOT register cluster_rpc (R1)"
    );

    // 核心交集抽样（schema 一致性已由上方全量循环覆盖，这里钉在场性）。
    for core in ["exec", "read_file", "write_file", "edit_file"] {
        assert!(
            project_tools.iter().any(|n| n == core),
            "core tool '{core}' missing from project loop"
        );
    }

    // 急停同源：项目 loop 绑的是 shared.estop 同一 Arc（estop 冻结两 loop）。
    // （绑定行为由 set_estop(shared.estop) 保证；Arc 同源性在 G3/G6 的
    // 行为级测试观察。）
}

// ---------------------------------------------------------------------------
// checkpoint 影子库两形态
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_checkpoint_shadow_repo_stays_out_of_project_dir() {
    // ── 形态一：项目带 .git 目录（git 影子库后端）──
    let home = unique_home("cp_git");
    write_mini_model_config(&home);
    let project_dir = home.join("proj");
    std::fs::create_dir_all(project_dir.join(".git")).unwrap();
    std::fs::write(project_dir.join("f.txt"), "hello").unwrap();
    let shared = shared_for(&home);
    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let loop_arc = build_project_agent_loop(
        &shared,
        &entry_for("p_cpgit0001", &project_dir),
        main_loop.session_store().cloned().unwrap(),
    )
    .expect("project loop builds");

    let store = loop_arc.checkpoint_store().expect("checkpoint store wired");
    assert_eq!(
        store.backend(),
        CheckpointBackend::Git,
        "project with .git dir must select the git shadow backend"
    );
    // 影子库在构造期即初始化于主 workspace（R7：绝不落用户项目目录）。
    let cp_dir = project_checkpoint_dir(&shared.workspace_dir(), "p_cpgit0001");
    let shadow = cp_dir.join("repo.git");
    assert!(
        shadow.is_dir(),
        "shadow repo must land under main workspace: {}",
        shadow.display()
    );
    // begin 落 turn 记录（同一 cp_dir——dir 挂载正确的直接证据）。
    store.begin(1, "turn one");
    let cp_entries = dir_names(&cp_dir);
    assert!(
        cp_entries.iter().any(|n| n.ends_with(".json")),
        "turn record must persist under main workspace cp_dir, got {cp_entries:?}"
    );
    // 用户项目目录零污染：只有自放的 .git + f.txt。
    let mut remaining = dir_names(&project_dir);
    remaining.sort();
    assert_eq!(
        remaining,
        vec![".git", "f.txt"],
        "project dir must stay untouched"
    );

    // ── 形态二：纯目录（无 .git）→ JSON 快照回落，同样零污染 ──
    let home2 = unique_home("cp_json");
    write_mini_model_config(&home2);
    let project_dir2 = home2.join("proj2");
    std::fs::create_dir_all(&project_dir2).unwrap();
    std::fs::write(project_dir2.join("a.md"), "content").unwrap();
    let shared2 = shared_for(&home2);
    let main2 = build_agent_loop(&shared2).expect("main loop builds");
    let loop2 = build_project_agent_loop(
        &shared2,
        &entry_for("p_cpjson001", &project_dir2),
        main2.session_store().cloned().unwrap(),
    )
    .expect("project loop builds");
    let store2 = loop2.checkpoint_store().expect("checkpoint store wired");
    assert_eq!(
        store2.backend(),
        CheckpointBackend::Json,
        "project without .git must fall back to JSON snapshots"
    );
    store2.begin(1, "turn one");
    // 纯目录项目零污染（JSON 记录按落盘纪律在首个真实文件变更时写入，
    // dir 挂载路径与形态一共用同一 persist 代码路径——上面已证）。
    assert_eq!(
        dir_names(&project_dir2),
        vec!["a.md"],
        "pure-dir project must stay untouched"
    );
}

// ---------------------------------------------------------------------------
// 目录缺失诚实拒绝
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_factory_rejects_missing_project_dir() {
    let home = unique_home("reject");
    write_mini_model_config(&home);
    let shared = shared_for(&home);
    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let gone = home.join("does_not_exist");
    let err = match build_project_agent_loop(
        &shared,
        &entry_for("p_gone0001", &gone),
        main_loop.session_store().cloned().unwrap(),
    ) {
        Ok(_) => panic!("missing project dir must be an honest error"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("项目目录不存在"),
        "error should name the missing dir, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// ASM-08 装配矩阵（2026-09-16 横扫存量加固）：项目 loop 关键件接线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn project_loop_critical_wiring_matrix() {
    let home = unique_home("asm08");
    write_mini_model_config(&home);
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let project_loop = build_project_agent_loop(
        &shared,
        &entry_for("p_asm080001", &project_dir),
        main_loop.session_store().cloned().unwrap(),
    )
    .expect("project loop builds");

    // builder 内部已跑 assert_gateway_critical_wiring（漏接 = 启动即炸）；
    // 这里再直接断言 wiring_status，防断言调用本身被将来误删后无人知晓。
    let status: std::collections::HashMap<&str, bool> =
        project_loop.wiring_status().into_iter().collect();
    for key in ["estop", "workspace_root", "config_path", "pricing_store"] {
        assert!(
            status.get(key).copied().unwrap_or(false),
            "项目 loop 关键件 `{key}` 未接线（ASM-08 矩阵回归）"
        );
    }
    // workspace_root 必须指向项目目录（不是主 workspace）——围栏根（D6）。
    assert_eq!(
        project_loop.workspace_root().as_deref(),
        Some(project_dir.as_path()),
        "项目 loop 的围栏根必须是项目目录"
    );
}

// ===========================================================================
// Coverage 追加（2026-09-24）：项目工厂装配旋钮臂——discipline 总开关 /
// small_model 通道（可解析 + 不可解析）/ 无 LLM 降级（ghost 模型 →
// NullProvider）/ 价目表打开失败降级。
// ===========================================================================

/// mini 档模型 config + 顶层/agents 段扩展（两层浅合并，同 tests.rs 手法）。
fn write_mini_model_config_extra(home: &Path, extra: serde_json::Value) {
    let mut cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "mini-model", "max_tool_iterations": 5 } },
        "model_list": [ {
            "model_name": "mini-model",
            "model": "testai/mini-model",
            "api_key": "test-key",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        } ]
    });
    if let (Some(base), Some(over)) = (cfg.as_object_mut(), extra.as_object()) {
        for (k, v) in over {
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
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&cfg).unwrap(),
    )
    .unwrap();
}

/// 项目工厂旋钮矩阵：discipline 开 + small_model 可解析 + event_tx 在场 →
/// 装配臂全命中，构建照常成功。
#[tokio::test]
async fn project_factory_wires_discipline_small_model_and_event_observers() {
    let home = unique_home("knobs");
    write_mini_model_config_extra(
        &home,
        serde_json::json!({
            "agents": {
                "defaults": { "spill_retention_days": 3, "restrict_to_workspace": true },
                "discipline": { "enabled": true },
                "small_model": "mini-model"
            }
        }),
    );
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let session_store = main_loop
        .session_store()
        .cloned()
        .expect("main loop carries session store");

    let project_loop = build_project_agent_loop(
        &shared,
        &entry_for("p_knobs01", &project_dir),
        session_store,
    )
    .expect("project loop builds with knobs on");
    assert!(project_loop.tool_count() > 0);
    assert!(matches!(
        project_loop.tier(),
        nemesis_types::capability::ModelTier::Mini
    ));
}

/// 项目工厂无 LLM 降级：llm 指向不存在条目 → resolve 失败 → NullProvider
/// 装配（Ok 不 Err，双击直启语义）。
#[tokio::test]
async fn project_factory_degrades_to_null_provider_on_unresolvable_model() {
    let home = unique_home("ghost");
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({
            "agents": { "defaults": { "llm": "ghost-model" } },
            "model_list": []
        })
        .to_string(),
    )
    .unwrap();
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds (degraded too)");
    let session_store = main_loop.session_store().cloned().unwrap();
    let project_loop = build_project_agent_loop(
        &shared,
        &entry_for("p_ghost01", &project_dir),
        session_store,
    )
    .expect("unresolvable model must degrade to NullProvider assembly, not fail");
    assert!(project_loop.tool_count() > 0);
}

/// small_model 指向不存在条目 → warn + 跳过（不阻断构建）。
#[tokio::test]
async fn project_factory_small_model_unresolvable_degrades_quietly() {
    let home = unique_home("smallghost");
    write_mini_model_config_extra(
        &home,
        serde_json::json!({ "agents": { "small_model": "ghost-small" } }),
    );
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let session_store = main_loop.session_store().cloned().unwrap();
    let project_loop =
        build_project_agent_loop(&shared, &entry_for("p_sg0001", &project_dir), session_store)
            .expect("unresolvable small model must not block project build");
    assert!(project_loop.tool_count() > 0);
}

/// 价目表打开失败（workspace/data 是文件）→ warn 降级，构建照常。
#[tokio::test]
async fn project_factory_pricing_store_open_failure_degrades() {
    let home = unique_home("pricing");
    write_mini_model_config(&home);
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    std::fs::write(home.join("workspace").join("data"), b"not a dir").unwrap();
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let shared = shared_for(&home);

    let main_loop = build_agent_loop(&shared).expect("main loop builds");
    let session_store = main_loop.session_store().cloned().unwrap();
    let project_loop =
        build_project_agent_loop(&shared, &entry_for("p_price1", &project_dir), session_store)
            .expect("pricing store open failure must degrade, not abort build");
    assert!(project_loop.tool_count() > 0);
}
