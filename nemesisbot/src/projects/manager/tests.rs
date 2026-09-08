//! ProjectLoopManager / 路由纯函数测试（L6++ G2，2026-09-08）。
//!
//! 覆盖：route_decision 三值穷举（system 全拒 / 按归属 / 无归属放行）、
//! session_key_from_stem 与 web 扫描端镜像、spawn-stop 生命周期（幂等 +
//! 诚实报错）、start_all 从注册表拉起 + 归属索引重建。

use std::path::PathBuf;
use std::sync::Arc;

use super::{route_decision, session_key_from_stem, ProjectLoopManager};
use crate::agent_factory::SharedResources;
use crate::projects::registry;
use nemesis_types::channel::InboundMessage;

// ---------------------------------------------------------------------------
// 夹具
// ---------------------------------------------------------------------------

fn unique_home(tag: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "nb_proj_mgr_{tag}_{}_{}_{}",
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

/// mini 档模型 config（provider 离线构造安全，与工厂测试同一形态）。
fn write_mini_model_config(home: &std::path::Path) {
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

fn msg(channel: &str, session_key: &str) -> InboundMessage {
    InboundMessage {
        channel: channel.to_string(),
        sender_id: "u1".to_string(),
        chat_id: "c1".to_string(),
        content: "hello".to_string(),
        media: Vec::new(),
        session_key: session_key.to_string(),
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

// ---------------------------------------------------------------------------
// route_decision 纯函数（G2 门：system 全拒 / web 按归属 / 无归属放行）
// ---------------------------------------------------------------------------

#[test]
fn route_decision_system_messages_never_reach_project_loops() {
    let m = msg("system", "agent:main:session:whatever");
    assert_eq!(
        route_decision(&m, Some("p_aaaaaaaa")),
        super::RouteDecision::System,
        "system 消息即使带项目归属也必须进主桥（内部回灌语义）"
    );
    assert_eq!(route_decision(&m, None), super::RouteDecision::System);
}

#[test]
fn route_decision_by_ownership() {
    let bound = msg("web", "agent:main:session:bound1");
    assert_eq!(
        route_decision(&bound, Some("p_aaaaaaaa")),
        super::RouteDecision::ToProject("p_aaaaaaaa".to_string()),
        "有归属 → 项目调度转发"
    );
    let unbound = msg("web", "agent:main:session:free1");
    assert_eq!(
        route_decision(&unbound, None),
        super::RouteDecision::ToMain,
        "无归属 → 主桥放行"
    );
    let bound_but_owner_none = msg("web", "agent:main:session:x");
    assert_eq!(
        route_decision(&bound_but_owner_none, None),
        super::RouteDecision::ToMain
    );
    // 非 web 通道同理（telegram 等 IM 通道同一矩阵——按 session_key 归属）。
    let im = msg("telegram", "telegram_12345");
    assert_eq!(
        route_decision(&im, Some("p_bbbbbbbb")),
        super::RouteDecision::ToProject("p_bbbbbbbb".to_string())
    );
    assert_eq!(route_decision(&im, None), super::RouteDecision::ToMain);
}

// ---------------------------------------------------------------------------
// session_key_from_stem（与 nemesis-web 扫描端镜像）
// ---------------------------------------------------------------------------

#[test]
fn session_key_from_stem_mirrors_web_scanner() {
    // web 固定前缀精确还原（保留 sid 内合法下划线）。
    assert_eq!(
        session_key_from_stem("agent_main_session_s1"),
        "agent:main:session:s1"
    );
    assert_eq!(
        session_key_from_stem("agent_main_session_my_sid"),
        "agent:main:session:my_sid",
        "sid 内下划线不能被还原成冒号"
    );
    // 非 web 会话 naive 还原。
    assert_eq!(session_key_from_stem("telegram_12345"), "telegram:12345");
    assert_eq!(session_key_from_stem("cron_foo_bar"), "cron:foo:bar");
}

// ---------------------------------------------------------------------------
// spawn / stop 生命周期
// ---------------------------------------------------------------------------

fn manager_for(home: &std::path::Path) -> ProjectLoopManager {
    let shared = Arc::new(SharedResources {
        home: home.to_path_buf(),
        ..Default::default()
    });
    let sess_dir = home.join("sessions");
    std::fs::create_dir_all(&sess_dir).unwrap();
    let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
        &sess_dir,
    ));
    ProjectLoopManager::new(shared, store, Arc::new(nemesis_bus::MessageBus::new()))
}

#[tokio::test]
async fn manager_spawn_stop_lifecycle() {
    let home = unique_home("lifecycle");
    write_mini_model_config(&home);
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let mgr = manager_for(&home);

    // 目录消失 → 诚实报错（不 spawn）。
    let ghost = registry::ProjectEntry {
        id: "p_ghost001".to_string(),
        name: "ghost".to_string(),
        path: home.join("no_such_dir"),
        created_at: "2026-09-08T00:00:00Z".to_string(),
    };
    assert!(mgr.spawn_project(&ghost).is_err());
    assert!(mgr.running_pids().is_empty());

    // 正常 spawn。
    let entry = registry::ProjectEntry {
        id: "p_life0001".to_string(),
        name: "lifecycle".to_string(),
        path: project_dir.clone(),
        created_at: "2026-09-08T00:00:00Z".to_string(),
    };
    mgr.spawn_project(&entry).expect("spawn succeeds");
    assert_eq!(mgr.running_pids(), vec!["p_life0001".to_string()]);
    assert!(mgr.project_loop("p_life0001").is_some());

    // 幂等：重复 spawn 不产生第二个实例。
    mgr.spawn_project(&entry).expect("idempotent spawn");
    assert_eq!(mgr.running_pids().len(), 1);

    // stop：第一次 true，第二次 false（摘除语义）。
    assert!(mgr.stop_project("p_life0001"));
    assert!(mgr.running_pids().is_empty());
    assert!(mgr.project_loop("p_life0001").is_none());
    assert!(!mgr.stop_project("p_life0001"));
}

// ---------------------------------------------------------------------------
// start_all（注册表驱动）+ 归属索引
// ---------------------------------------------------------------------------

#[tokio::test]
async fn manager_start_all_and_owner_index() {
    let home = unique_home("startall");
    write_mini_model_config(&home);
    let main_ws = home.join("workspace");
    let real_dir = home.join("proj_real");
    std::fs::create_dir_all(&real_dir).unwrap();

    let shared = Arc::new(SharedResources {
        home: home.clone(),
        ..Default::default()
    });
    let sess_dir = home.join("sessions");
    std::fs::create_dir_all(&sess_dir).unwrap();
    let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
        &sess_dir,
    ));
    let mgr = ProjectLoopManager::new(
        shared,
        store,
        Arc::new(nemesis_bus::MessageBus::new()),
    );
    assert_eq!(
        mgr.registry_path(),
        registry::registry_path(&main_ws).as_path(),
        "manager 的注册表路径必须从主 workspace 派生"
    );

    // 注册表预置 2 项目：一个目录真实、一个先注册后删目录（inactive 形态
    // ——registry 的 create 本身拒绝不存在的目录，删除发生在注册之后）。
    let real = registry::create_project(
        mgr.registry_path(),
        &main_ws,
        "真实项目",
        real_dir.to_str().unwrap(),
        4,
    )
    .expect("create real project");
    let gone_dir = home.join("proj_gone");
    std::fs::create_dir_all(&gone_dir).unwrap();
    let gone = registry::create_project(
        mgr.registry_path(),
        &main_ws,
        "消失项目",
        gone_dir.to_str().unwrap(),
        4,
    )
    .expect("create gone project");
    std::fs::remove_dir_all(&gone_dir).unwrap();

    // 归属 sidecar 预置：一个绑定 real 项目的 web 会话 + 一个无绑定会话。
    let logs_dir = nemesis_path::resolve_session_logs_dir_in_workspace(&main_ws);
    std::fs::create_dir_all(&logs_dir).unwrap();
    let bound_stem = "agent_main_session_bound1";
    std::fs::write(
        logs_dir.join(format!("{bound_stem}.meta.json")),
        serde_json::json!({
            "title": "bound",
            "project_id": real.id,
            "project_path": real.path.to_string_lossy()
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        logs_dir.join(format!("{bound_stem}.jsonl")),
        "{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":\"t\"}\n",
    )
    .unwrap();
    let free_stem = "agent_main_session_free1";
    std::fs::write(
        logs_dir.join(format!("{free_stem}.jsonl")),
        "{\"role\":\"user\",\"content\":\"hi\",\"timestamp\":\"t\"}\n",
    )
    .unwrap();

    // start_all：真实项目拉起、消失项目 warn 跳过、索引建好。
    mgr.start_all();
    assert!(mgr.project_loop(&real.id).is_some(), "real project loop must run");
    assert!(
        !mgr.running_pids().iter().any(|p| *p == gone.id),
        "missing-dir project must be skipped with a warning, not crash startup"
    );

    let bound_key = session_key_from_stem(bound_stem);
    assert_eq!(mgr.owner_of(&bound_key).as_deref(), Some(real.id.as_str()));
    assert_eq!(
        mgr.owner_of(&session_key_from_stem(free_stem)),
        None,
        "无 sidecar 的会话不进索引（落对话组）"
    );

    // 增量登记 + 查询。
    mgr.remember_session("agent:main:session:new1", &real.id);
    assert_eq!(mgr.owner_of("agent:main:session:new1").as_deref(), Some(real.id.as_str()));

    mgr.stop_all();
    assert!(mgr.running_pids().is_empty());
}

// ---------------------------------------------------------------------------
// bind / forget 全路径（G4）：sidecar 经 chat_log 全局单例写读——必须先
// 拿 GLOBAL_STATE_LOCK 并把单例 home 重定向到沙箱（crate::tests 纪律；
// Windows-form helper，Linux nightly 不编译此用例）。
// ---------------------------------------------------------------------------

#[cfg(windows)] // Windows-form（进程级单例重定向依赖 crate::tests 沙箱 home）
#[tokio::test]
async fn bind_and_forget_session_roundtrip_with_sidecar() {
    let _serial = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let home = crate::tests::singleton_test_home();
    let main_ws = home.join("workspace");
    std::fs::create_dir_all(&main_ws).unwrap();
    let project_dir = home.join("proj_bindfull");
    std::fs::create_dir_all(&project_dir).unwrap();
    let mgr = manager_for(&home);

    let entry = registry::create_project(
        mgr.registry_path(),
        &main_ws,
        "绑定全路径",
        project_dir.to_str().unwrap(),
        4,
    )
    .expect("create project");
    let key = "agent:main:session:bindfull1";

    // bind：unknown id / 注册后删目录（inactive 形态）→ 诚实报错
    // （此两臂不触 sidecar）。
    let err = mgr.bind_session(key, "p_nope000").unwrap_err();
    assert!(err.contains("不存在"), "unknown id must fail honestly: {err}");
    let ghost_dir = home.join("proj_bindghost");
    std::fs::create_dir_all(&ghost_dir).unwrap();
    let ghost = registry::create_project(
        mgr.registry_path(),
        &main_ws,
        "绑定幽灵",
        ghost_dir.to_str().unwrap(),
        4,
    )
    .expect("create ghost project");
    std::fs::remove_dir_all(&ghost_dir).unwrap();
    let err = mgr.bind_session(key, &ghost.id).unwrap_err();
    assert!(
        err.contains("目录不存在"),
        "missing dir must fail honestly: {err}"
    );
    assert_eq!(mgr.owner_of(key), None, "失败 bind 不得留下索引");

    // bind happy path：索引登记 + sidecar 烧 project 字段（meta 原先不存在
    // ——upsert 语义直接建）。
    mgr.bind_session(key, &entry.id).expect("bind succeeds");
    assert_eq!(mgr.owner_of(key).as_deref(), Some(entry.id.as_str()));
    let meta_path = nemesis_path::resolve_session_logs_dir_in_workspace(&main_ws)
        .join(format!(
            "{}.meta.json",
            key.replace(':', "_")
        ));
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    assert_eq!(meta["project_id"], entry.id.as_str());
    assert_eq!(
        meta["project_path"].as_str().unwrap(),
        entry.path.to_string_lossy().as_ref()
    );

    // forget：索引摘除 + sidecar 归属摘除（title 等保留；meta 文件不删）。
    assert!(mgr.forget_session(key), "首次 forget 返回 true");
    assert_eq!(mgr.owner_of(key), None);
    let meta: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&meta_path).unwrap()).unwrap();
    assert!(meta.get("project_id").and_then(|v| v.as_str()).is_none());
    assert!(meta.get("project_path").and_then(|v| v.as_str()).is_none());

    // 幂等：再 forget = false（归属已摘，no-op 不空写）。
    assert!(!mgr.forget_session(key), "二次 forget 必须 false");
}
