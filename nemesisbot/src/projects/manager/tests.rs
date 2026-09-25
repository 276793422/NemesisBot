//! ProjectLoopManager / 路由纯函数测试（L6++ G2，2026-09-08）。
//!
//! 覆盖：route_decision 三值穷举（system 全拒 / 按归属 / 无归属放行）、
//! session_key_from_stem 与 web 扫描端镜像、spawn-stop 生命周期（幂等 +
//! 诚实报错）、start_all 从注册表拉起 + 归属索引重建。

use std::path::PathBuf;
use std::sync::Arc;

use super::{ProjectLoopManager, route_decision, session_key_from_stem};
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

#[test]
fn route_decision_history_requests_exempt_from_project_routing() {
    // history 只读查询豁免（BUG 2026-09-23）：数据全在主 workspace，
    // 与项目 loop 存亡/忙闲无关——带项目归属也必须回主 loop。
    let mut bound = msg("web", "agent:main:session:projh1");
    bound.content = r#"{"request_id":"rq-1","limit":20}"#.to_string();
    bound
        .metadata
        .insert("request_type".to_string(), "history".to_string());
    assert_eq!(
        route_decision(&bound, Some("p_haaaaaaa")),
        super::RouteDecision::ToMain,
        "history + 项目归属 → 主 loop（不得被项目路由劫持）"
    );
    assert_eq!(
        route_decision(&bound, None),
        super::RouteDecision::ToMain,
        "history + 无归属 → 主 loop（与普通消息同向）"
    );
    // 非 history 的普通消息不受影响（对照：同 session_key 有归属仍进项目）。
    let plain = msg("web", "agent:main:session:projh1");
    assert_eq!(
        route_decision(&plain, Some("p_haaaaaaa")),
        super::RouteDecision::ToProject("p_haaaaaaa".to_string()),
        "豁免只对 request_type=history，普通消息矩阵不变"
    );
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
    let mgr = ProjectLoopManager::new(shared, store, Arc::new(nemesis_bus::MessageBus::new()));
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
    assert!(
        mgr.project_loop(&real.id).is_some(),
        "real project loop must run"
    );
    assert!(
        !mgr.running_pids().contains(&gone.id),
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
    assert_eq!(
        mgr.owner_of("agent:main:session:new1").as_deref(),
        Some(real.id.as_str())
    );

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
    let _serial = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
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
    assert!(
        err.contains("不存在"),
        "unknown id must fail honestly: {err}"
    );
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
        .join(format!("{}.meta.json", key.replace(':', "_")));
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

// ---------------------------------------------------------------------------
// reload_providers（BUG 2026-09-21）：模型热切联动到在跑项目 loop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn reload_providers_swaps_running_project_loop_model() {
    let home = unique_home("reload");
    write_mini_model_config(&home);
    let project_dir = home.join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();
    let mgr = manager_for(&home);
    let entry = registry::ProjectEntry {
        id: "p_reload01".to_string(),
        name: "reload".to_string(),
        path: project_dir.clone(),
        created_at: "2026-09-21T00:00:00Z".to_string(),
    };
    mgr.spawn_project(&entry).expect("spawn succeeds");
    let pid = entry.id.clone();

    // 起点形态：spawn 读到的默认模型（BUG 修复前项目 loop 永远停在这里）。
    let before = mgr.project_loop(&pid).expect("loop running").active_model();
    assert_eq!(before, "mini-model");

    // 盘上 config 换默认模型（= Dashboard set_default 写盘后的形态）。
    let swapped = serde_json::json!({
        "agents": { "defaults": { "llm": "fresh-model", "max_tool_iterations": 5 } },
        "model_list": [
            {
                "model_name": "fresh-model",
                "model": "testai/fresh-model",
                "api_key": "test-key",
                "api_base": "http://127.0.0.1:9",
                "model_tier": "mini"
            },
            {
                "model_name": "mini-model",
                "model": "testai/mini-model",
                "api_key": "test-key",
                "api_base": "http://127.0.0.1:9",
                "model_tier": "mini"
            }
        ]
    });
    std::fs::write(
        home.join("config.json"),
        serde_json::to_string_pretty(&swapped).unwrap(),
    )
    .unwrap();

    mgr.reload_providers();
    assert_eq!(
        mgr.project_loop(&pid)
            .expect("loop still running after swap")
            .active_model(),
        "fresh-model",
        "reload must hot-swap the running loop to the new default"
    );

    // 解析失败 = 保持现状（绝不把能用的 loop 换成 NullProvider）。
    let broken = serde_json::json!({
        "agents": { "defaults": { "llm": "missing-model" } },
        "model_list": []
    });
    std::fs::write(home.join("config.json"), broken.to_string()).unwrap();
    mgr.reload_providers();
    assert_eq!(
        mgr.project_loop(&pid)
            .expect("loop survives")
            .active_model(),
        "fresh-model",
        "broken config must keep the working provider"
    );

    // config.json 整个消失（读取失败分支）：同样保持现状，不 panic。
    std::fs::remove_file(home.join("config.json")).unwrap();
    mgr.reload_providers();
    assert_eq!(
        mgr.project_loop(&pid)
            .expect("loop survives")
            .active_model(),
        "fresh-model",
        "missing config must keep the working provider"
    );
}

// ---------------------------------------------------------------------------
// wave_a（2026-09-25）：route_one 转发/不可用回路、bridge_should_skip、
// display_label 三级回落、max_projects 读取链、ProjectsBridge list/rename/
// remove 薄委托。
// ---------------------------------------------------------------------------

mod wave_a {
    use super::*;

    /// 自建 manager（暴露 bus 以订阅出站——manager_for 不出 bus）。
    fn manager_with_bus(
        home: &std::path::Path,
    ) -> (ProjectLoopManager, Arc<nemesis_bus::MessageBus>) {
        let shared = Arc::new(SharedResources {
            home: home.to_path_buf(),
            ..Default::default()
        });
        let sess_dir = home.join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        let bus = Arc::new(nemesis_bus::MessageBus::new());
        (ProjectLoopManager::new(shared, store, bus.clone()), bus)
    }

    #[tokio::test]
    async fn route_one_forwards_bound_messages_into_project_channel() {
        let home = unique_home("routeone");
        let (mgr, _bus) = manager_with_bus(&home);
        mgr.remember_session("agent:main:session:fwd1", "p_fwd0001");
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        mgr.insert_test_channel("p_fwd0001", tx);

        let m = msg("web", "agent:main:session:fwd1");
        mgr.route_one(&m).await;
        let got = rx.recv().await.expect("绑定消息必须转发进项目通道");
        assert_eq!(got.session_key, "agent:main:session:fwd1");
        assert_eq!(got.channel, "web");
    }

    #[tokio::test]
    async fn route_one_unroutable_publishes_honest_error_on_bus() {
        let home = unique_home("unroutable");
        let (mgr, bus) = manager_with_bus(&home);
        let mut orx = bus.subscribe_outbound();
        // 归属命中但 channels 表无转发端 → 不可用出站（不静默丢）。
        mgr.remember_session("agent:main:session:gone1", "p_gone001");
        let m = msg("web", "agent:main:session:gone1");
        mgr.route_one(&m).await;
        let out = orx.recv().await.expect("不可用必须出站");
        assert_eq!(out.channel, "web");
        assert_eq!(out.chat_id, "c1");
        assert!(out.content.contains("不可用"), "实际：{}", out.content);
        // system 消息即使有归属也绝不进项目路径（无出站、无转发）。
        mgr.remember_session("agent:main:session:sys1", "p_gone001");
        let sys = msg("system", "agent:main:session:sys1");
        mgr.route_one(&sys).await;
        assert!(orx.try_recv().is_err(), "system 消息不得产生出站");
    }

    #[tokio::test]
    async fn route_one_closed_receiver_treated_as_unroutable() {
        let home = unique_home("closedtx");
        let (mgr, bus) = manager_with_bus(&home);
        let mut orx = bus.subscribe_outbound();
        mgr.remember_session("agent:main:session:closed1", "p_cls0001");
        let (tx, rx) = tokio::sync::mpsc::channel::<nemesis_types::channel::InboundMessage>(4);
        drop(rx); // 接收端已关（loop 刚 stop 的窗口）
        mgr.insert_test_channel("p_cls0001", tx);
        let m = msg("web", "agent:main:session:closed1");
        mgr.route_one(&m).await;
        let out = orx.recv().await.expect("接收端关闭必须按不可用出站");
        assert!(out.content.contains("不可用"), "实际：{}", out.content);
    }

    #[tokio::test]
    async fn bridge_should_skip_matches_route_decision_matrix() {
        let home = unique_home("skip");
        let (mgr, _bus) = manager_with_bus(&home);
        // system 永不跳过（主 loop 消费）。
        assert!(!mgr.bridge_should_skip(&msg("system", "agent:main:session:s9")));
        // 无归属放行。
        assert!(!mgr.bridge_should_skip(&msg("web", "agent:main:session:free2")));
        // 归属命中 → 跳过。
        mgr.remember_session("agent:main:session:bound2", "p_skip001");
        assert!(mgr.bridge_should_skip(&msg("web", "agent:main:session:bound2")));
        // history 豁免：即使带归属也不跳（走主 loop 应答）。
        let mut hist = msg("web", "agent:main:session:bound2");
        hist.metadata
            .insert("request_type".to_string(), "history".to_string());
        assert!(!mgr.bridge_should_skip(&hist), "history 不得被项目路由劫持");
    }

    #[tokio::test]
    async fn display_label_falls_back_registry_then_sidecar_tail_then_pid() {
        // bind_session 经 chat_log 全局单例烧 sidecar——必须重定向单例 home
        // 并持全局锁（与 bind_and_forget_session_roundtrip_with_sidecar 同纪律）。
        let _serial = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = crate::tests::singleton_test_home();
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let shared = Arc::new(SharedResources {
            home: home.clone(),
            ..Default::default()
        });
        let sess_dir = home.join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        let mgr = ProjectLoopManager::new(shared, store, Arc::new(nemesis_bus::MessageBus::new()));

        // ① 注册表名（目录真实的已注册项目）。
        let real_dir = home.join("proj_label_real");
        std::fs::create_dir_all(&real_dir).unwrap();
        let entry = registry::create_project(
            mgr.registry_path(),
            &main_ws,
            "标签真实项目",
            real_dir.to_str().unwrap(),
            4,
        )
        .unwrap();
        assert_eq!(
            mgr.display_label(&entry.id, "agent:main:session:any"),
            "标签真实项目",
            "注册表在时优先注册表名"
        );

        // ② sidecar project_path 尾段（项目已从注册表移除，但会话 sidecar
        //    还烧着绑定路径）。
        let session_key = "agent:main:session:label1x";
        mgr.bind_session(session_key, &entry.id).unwrap();
        registry::remove_project(mgr.registry_path(), &entry.id).unwrap();
        let label_from_sidecar = mgr.display_label(&entry.id, session_key);
        assert_eq!(
            label_from_sidecar,
            real_dir.file_name().unwrap().to_str().unwrap(),
            "注册表摘除后回落到 sidecar 路径尾段"
        );

        // ③ pid 兜底（既无注册表也无 sidecar）。
        assert_eq!(
            mgr.display_label("p_ghost999", "agent:main:session:none"),
            "p_ghost999"
        );
    }

    #[tokio::test]
    async fn max_projects_reads_config_live_with_safe_fallback() {
        let home = unique_home("maxproj");
        let (mgr, _bus) = manager_with_bus(&home);
        // 无 config：读取失败 → 回退默认。
        assert_eq!(
            mgr.max_projects(),
            nemesis_config::ProjectsConfig::default().max
        );
        // 有 config 无 projects 段：同样默认。
        std::fs::write(
            home.join("config.json"),
            r#"{"agents":{"defaults":{"llm":"m"}}}"#,
        )
        .unwrap();
        assert_eq!(
            mgr.max_projects(),
            nemesis_config::ProjectsConfig::default().max
        );
        // projects.max 显式配置：实时生效。
        std::fs::write(
            home.join("config.json"),
            r#"{"agents":{"defaults":{"llm":"m"}},"projects":{"max":9}}"#,
        )
        .unwrap();
        assert_eq!(mgr.max_projects(), 9);
    }

    #[tokio::test]
    async fn projects_bridge_list_rename_remove_roundtrip() {
        use nemesis_web::handlers::projects::ProjectsBridge as _;
        let home = unique_home("bridge");
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let (mgr, _bus) = manager_with_bus(&home);
        let dir_a = home.join("proj_bridge_a");
        std::fs::create_dir_all(&dir_a).unwrap();
        let a = registry::create_project(
            mgr.registry_path(),
            &main_ws,
            "桥接A",
            dir_a.to_str().unwrap(),
            4,
        )
        .unwrap();

        // list：条目在、未运行。
        let list = mgr.list();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, a.id);
        assert!(!list[0].running);

        // rename：名称换、id 不变、running 仍 false。
        let renamed = mgr.rename(&a.id, "桥接A改").expect("rename ok");
        assert_eq!(renamed.id, a.id);
        assert_eq!(renamed.name, "桥接A改");
        assert!(!renamed.running);

        // remove：registry 摘除 + stop（无 loop → false 不炸）。
        let removed = mgr.remove(&a.id).expect("remove ok");
        assert_eq!(removed.id, a.id);
        assert!(!removed.running);
        assert!(mgr.list().is_empty());
        assert!(mgr.remove(&a.id).is_err(), "二次 remove 诚实报错");
    }

    #[tokio::test]
    async fn project_path_and_is_running_and_loop_queries() {
        let home = unique_home("queries");
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let (mgr, _bus) = manager_with_bus(&home);
        let dir_q = home.join("proj_q");
        std::fs::create_dir_all(&dir_q).unwrap();
        let q = registry::create_project(
            mgr.registry_path(),
            &main_ws,
            "查询项目",
            dir_q.to_str().unwrap(),
            4,
        )
        .unwrap();
        assert_eq!(mgr.project_path(&q.id), Some(dir_q));
        assert_eq!(mgr.project_path("p_nope999"), None);
        assert!(!mgr.is_running(&q.id), "未 spawn = 不在运行");
        assert!(mgr.project_loop(&q.id).is_none());
        assert!(mgr.running_pids().is_empty());
    }
}

// ---------------------------------------------------------------------------
// wave5 round2（2026-09-25）：ProjectsBridge trait 全委托矩阵（create 发车
// 真跑 run_bus_arc / reload_provider_all 成功形态 swapped>0 / bind·forget·
// owner_of 单例 home 形态）、reload_providers config 坏档臂、
// max_projects Err 回退臂、start_all 目录消失 warn-skip 臂、归属索引重建
// 的坏 sidecar / bound>0 / 空 key 短路臂。
// ---------------------------------------------------------------------------

mod w5r2 {
    use super::*;

    /// 自建 manager（wave_a::manager_with_bus 是子模块私有，这里同款复制）。
    fn manager_with_bus(
        home: &std::path::Path,
    ) -> (ProjectLoopManager, Arc<nemesis_bus::MessageBus>) {
        let shared = Arc::new(SharedResources {
            home: home.to_path_buf(),
            ..Default::default()
        });
        let sess_dir = home.join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        let bus = Arc::new(nemesis_bus::MessageBus::new());
        (ProjectLoopManager::new(shared, store, bus.clone()), bus)
    }

    /// reload_providers：home config.json 坏档 → warn 保持现状（590-594）；
    /// 同一坏档下 max_projects 走 Err 回退默认（271-273）。
    #[tokio::test]
    async fn w5_reload_providers_config_parse_fail_and_max_projects_fallback() {
        let home = unique_home("w5reload");
        let (mgr, _bus) = manager_with_bus(&home);
        std::fs::write(home.join("config.json"), "{ not json").unwrap();
        mgr.reload_providers(); // 不得 panic：warn + return
        assert_eq!(
            mgr.max_projects(),
            nemesis_config::ProjectsConfig::default().max,
            "config 坏档 → 项目上限回退默认"
        );
    }

    /// start_all：注册表条目在、项目目录消失 → spawn_project 诚实报错 →
    /// warn 留痕跳过，不炸启动（153-159）。
    #[tokio::test]
    async fn w5_start_all_warns_and_skips_project_with_missing_dir() {
        let home = unique_home("w5startall");
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let (mgr, _bus) = manager_with_bus(&home);
        let real = home.join("proj_w5sa");
        std::fs::create_dir_all(&real).unwrap();
        registry::create_project(
            mgr.registry_path(),
            &main_ws,
            "W5 启动项目",
            real.to_str().unwrap(),
            4,
        )
        .unwrap();
        // 目录消失（注册表条目还在）→ start_all 只能 warn-skip。
        std::fs::remove_dir_all(&real).unwrap();
        mgr.start_all();
        assert!(mgr.running_pids().is_empty(), "目录消失的项目不得在跑");
    }

    /// 归属索引重建：有效 sidecar 命中 + bound>0 汇总（498）；损坏 sidecar
    /// warn 跳过不炸（485-489）；owner_of 空 key 短路（533）；
    /// display_label 空 session_key → sidecar 段短路（407）→ pid 兜底。
    #[tokio::test]
    async fn w5_owner_index_rebuild_arms_and_empty_key_edges() {
        let home = unique_home("w5idx");
        let main_ws = home.join("workspace");
        let logs = nemesis_path::resolve_session_logs_dir_in_workspace(&main_ws);
        std::fs::create_dir_all(&logs).unwrap();
        // 有效绑定（stem 还原 web 形态 key）。
        std::fs::write(logs.join("agent_main_session_w5a.jsonl"), "{}").unwrap();
        std::fs::write(
            logs.join("agent_main_session_w5a.meta.json"),
            r#"{"project_id":"p_w5idx001"}"#,
        )
        .unwrap();
        // 损坏 sidecar（坏 JSON）。
        std::fs::write(logs.join("agent_main_session_w5b.jsonl"), "{}").unwrap();
        std::fs::write(logs.join("agent_main_session_w5b.meta.json"), "{ not json").unwrap();

        let (mgr, _bus) = manager_with_bus(&home);
        mgr.reload_owner_index();
        assert_eq!(
            mgr.owner_of("agent:main:session:w5a").as_deref(),
            Some("p_w5idx001"),
            "有效 sidecar 必须进索引"
        );
        assert!(
            mgr.owner_of("agent:main:session:w5b").is_none(),
            "坏 sidecar 跳过"
        );
        // 空 key 短路（不走磁盘）。
        assert!(mgr.owner_of("").is_none());
        // display_label：注册表 miss + 空 session_key（sidecar 段短路）→ pid 兜底。
        assert_eq!(mgr.display_label("p_w5none1", ""), "p_w5none1");
    }

    /// ProjectsBridge trait 全委托①（独立 home 形态）：create 成功 → 真
    /// spawn 项目 loop（run_bus_arc 任务真被 poll）→ loop_for_session /
    /// project_path / display_label / reload_provider_all（swapped>0）/ remove
    /// 全链。
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn w5_bridge_trait_create_spawn_reload_remove_roundtrip() {
        let home = unique_home("w5trait");
        write_mini_model_config(&home); // create 发车需要可解析模型
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let (mgr, _bus) = manager_with_bus(&home);
        let mgr = Arc::new(mgr);
        let bridge: &dyn nemesis_web::handlers::projects::ProjectsBridge = &*mgr;

        let dir = home.join("proj_w5_trait");
        std::fs::create_dir_all(&dir).unwrap();
        let info = bridge
            .create("W5 trait 项目", dir.to_str().unwrap())
            .expect("create 成功并立即发车");
        assert!(info.running, "create 返回 running=true");
        assert!(mgr.is_running(&info.id));
        assert!(
            bridge.loop_for_session(&info.id).is_some(),
            "trait loop_for_session 委托到在跑 loop"
        );
        assert_eq!(bridge.project_path(&info.id), Some(dir.clone()));

        // 让出发车任务：多线程 runtime 上 poll 到 run_bus_arc 消费循环。
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        // 热切联动成功形态：盘上 config 可解析 → 每在跑 loop 热换 provider。
        bridge.reload_provider_all();

        assert_eq!(
            bridge.display_label(&info.id, "agent:main:session:whatever"),
            "W5 trait 项目",
            "trait display_label 注册表名优先"
        );

        let removed = bridge.remove(&info.id).expect("remove ok");
        assert!(!removed.running);
        assert!(!mgr.is_running(&info.id), "remove 必须连带 stop loop");
    }

    /// ProjectsBridge trait 全委托②（chat_log 单例 home 形态）：bind_session /
    /// owner_of / forget_session 委托链 + sidecar 烧入/清除联动。
    #[tokio::test]
    async fn w5_bridge_trait_bind_owner_forget_delegates() {
        let _serial = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = crate::tests::singleton_test_home();
        let main_ws = home.join("workspace");
        std::fs::create_dir_all(&main_ws).unwrap();
        let shared = Arc::new(SharedResources {
            home: home.clone(),
            ..Default::default()
        });
        let sess_dir = home.join("sessions");
        std::fs::create_dir_all(&sess_dir).unwrap();
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        let mgr = ProjectLoopManager::new(shared, store, Arc::new(nemesis_bus::MessageBus::new()));
        let bridge: &dyn nemesis_web::handlers::projects::ProjectsBridge = &mgr;

        let real_dir = home.join("proj_w5_bind");
        std::fs::create_dir_all(&real_dir).unwrap();
        let entry = registry::create_project(
            mgr.registry_path(),
            &main_ws,
            "W5 绑定项目",
            real_dir.to_str().unwrap(),
            4,
        )
        .unwrap();

        let sk = "agent:main:session:w5bind1";
        bridge.bind_session(sk, &entry.id).expect("bind ok");
        assert_eq!(
            bridge.owner_of(sk).as_deref(),
            Some(entry.id.as_str()),
            "bind 后 owner_of 内存索引命中"
        );
        bridge.forget_session(sk);
        assert!(bridge.owner_of(sk).is_none(), "forget 后索引清空");
        // 二次 forget：索引已空（bool false）但 trait 委托照常走。
        bridge.forget_session(sk);
    }
}
