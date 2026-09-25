//! cov 补测（2026-09-25）：`init_services` 全相位驱动（assemble →
//! init_cluster(disabled) → init_web → init_agent → init_post_agent →
//! init_services），点亮姊妹 full_assembly 冒烟未触的分支：
//! - devices.enabled=true 的设备服务装配/启动臂；
//! - heartbeat.interval>0 的 interval 换算臂（×60）；
//! - board autopilot 启动同步 n>0 信息臂（store 先种规则）；
//! - real_port 回读 + gateway state 落盘断言。
//!
//! 纪律同 full_assembly：临时 home + NEMESISBOT_HOME + 全局锁互斥；网络面
//! 全 0（web/websocket/health/gateway 端口 0 = OS 分配，不占生产端口）；
//! 模型条目死端点（127.0.0.1:9 即刻拒绝，无外网）；集群关（无 UDP/RPC）；
//! 结束时 svc_mgr.shutdown() 让 web server 优雅退出（web_handle 可回收）。

use super::super::{
    ClusterHandoff, ClusterWiring, GatewayCtx, WebWiring, init_agent, init_cluster,
    init_post_agent, init_services, init_web,
};

struct SvcEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for SvcEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_services_full_phase_with_device_and_board_sync_arms() {
    let guard_env = {
        let g = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        SvcEnv {
            _guard: g,
            _tmp: tmp,
            home,
        }
    };

    // 配置：网络面归零 + 设备开 + 心跳 interval>0 + 死端点模型。
    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
    cfg["gateway"]["port"] = serde_json::json!(0);
    cfg["devices"]["enabled"] = serde_json::json!(true);
    cfg["heartbeat"]["interval"] = serde_json::json!(1); // >0 → ×60 换算臂
    // F-B10（2026-09-25）：interval>0 的换算在构造期完成，enabled=false
    // 保证 start() 直通不排 tick——插桩跑（本测 >60s）下真 tick 会驱动
    // process_heartbeat 对死端点 LLM 重试，runtime 关停与退避 sleep 竞态
    // 触发 tokio 关停断言 panic，毒化 GLOBAL_STATE_LOCK 级联 165 败。
    cfg["heartbeat"]["enabled"] = serde_json::json!(false);
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
    cfg["agents"]["defaults"]["workspace"] = serde_json::json!(
        guard_env
            .home
            .join("workspace")
            .to_string_lossy()
            .to_string()
    );
    cfg["model_list"] = serde_json::json!([{
        "model_name": "mini-model",
        "model": "openai/gpt-fake",
        "api_key": "k",
        "api_base": "http://127.0.0.1:9",
        "model_tier": "mini"
    }]);
    std::fs::write(guard_env.home.join("config.json"), cfg.to_string()).unwrap();

    // 全相位链（run() 的 B1–B6 编排骨架就地展开；B7 run_runtime 会停在
    // wait_for_shutdown，故不调用——init_services 产物即断言面）。
    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    // resolution.model_name = 底层模型 id（model 条目的 `model` 字段剥协议前缀），
    // 不是别名 mini-model。
    assert_eq!(ctx.model_name, "gpt-fake");

    // board 规则先种后同步：init_services 的 sync_autopilot_jobs 走 n>0 臂。
    // board_store 由 assemble 打开（workspace/board/board.db）。
    let board_store = ctx
        .board_store
        .clone()
        .expect("board feature 默认开 → store 在场");
    let _ap = board_store
        .create_autopilot(&nemesis_board::NewAutopilot {
            name: "服务同步规则".into(),
            cron: "0 9 * * *".into(),
            title: "svc {date}".into(),
            description: String::new(),
            priority: 2,
            project_id: None,
            target: String::new(),
            enabled: true,
            auto_plan: false,
            acceptance_criteria: None,
        })
        .expect("种 autopilot 规则");

    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster 关 = 廉价路径");
    let web: WebWiring = init_web(&ctx, &cluster).await.expect("init_web");
    assert_eq!(web.enabled_channels, vec!["web".to_string()]);

    let agent = init_agent(&ctx, &web, &cluster)
        .await
        .expect("agent 装配（死端点模型，无 LLM 调用）");
    assert!(agent.initial_tool_count > 0, "默认工具必须已注册");

    let mut web_server = web.web_server;
    let post = init_post_agent(
        &ctx,
        &mut web_server,
        web.web_bind_host.clone(),
        web.web_port,
        agent,
        cluster,
    )
    .await
    .expect("post_agent 装配");

    let cluster_handoff = ClusterHandoff {
        bridge_cluster_slot: ctx.autopilot_cluster_slot.clone(),
        cluster_adapter: post.cluster_adapter.clone(),
        cluster_arc_ref: post.cluster_arc_ref.clone(),
    };
    let services = init_services(
        &ctx,
        web_server,
        web.web_bind_host.clone(),
        web.web_display_host.clone(),
        web.web_port,
        web.bridge_client_launch.clone(),
        post.agent_adapter.clone(),
        10, // initial_tool_count 展示用
        cluster_handoff,
    )
    .await
    .expect("init_services");

    // web server 真实 bind（:0 → OS 分配临时端口）且 state 落盘回读一致。
    assert!(services.real_port > 0, "real_port 必须是真实绑定端口");
    let state_path = nemesis_path::resolve_gateway_state_path_in_workspace(
        &crate::common::workspace_path(&guard_env.home),
    );
    let state: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&state_path).expect("state 文件在场"))
            .expect("state json");
    assert_eq!(state["web_port"].as_i64(), Some(services.real_port));
    assert_eq!(state["web_host"], "127.0.0.1");
    // 同步后的规则应已登记 cron job（store 真相源 → cron 服务跟随）。
    let jobs = guard_last_job_count(&ctx);
    assert!(
        jobs >= 1,
        "autopilot 规则必须同步为 cron job（jobs={jobs}）"
    );

    // 优雅收尾：广播 shutdown → web server 退出 → web_handle 可回收。
    services.svc_mgr.shutdown();
    // web server 收到 shutdown 广播后退出；超时兜底防挂。
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), services.web_handle).await;
}

/// cron 服务当前 job 数（锁内快照；闭包数据不可 Clone，取计数即可）。
fn guard_last_job_count(ctx: &GatewayCtx) -> usize {
    ctx.cron_service.lock().unwrap().list_jobs(true).len()
}

/// enabled 集群形态补测（2026-09-25 wave5）：bridge 反向桥 spawn 块
/// （cluster 身份快照 + 桥 RPC 枢纽 + pump spawn）与资产对外基址 /
/// node_id 落盘（enabled cluster 必走；disabled 形态 cluster_adapter
/// 在场但 lan_ip 选择/落盘同样执行——本测聚焦 enabled + bridge Some）。
/// relay 用死端点 127.0.0.1:9（dial 即刻拒绝，泵自重连只打日志）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn init_services_enabled_cluster_bridge_identity_and_asset_persist() {
    let guard_env = {
        let g = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        SvcEnv {
            _guard: g,
            _tmp: tmp,
            home,
        }
    };

    // 配置：网络面归零（web :0 = OS 分配；cluster port/rpc_port :0 同理），
    // 死端点模型，无外网。
    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
    cfg["gateway"]["port"] = serde_json::json!(0);
    // F-B10：heartbeat 首拍 1s（interval 无关），关停窗口重叠即 tokio 关停
    // 断言 panic——插桩测量线的中毒源，测试域一律关掉（同文件 :61 同因）。
    cfg["heartbeat"]["enabled"] = serde_json::json!(false);
    // 集群主开关（config.json 的 cluster.enabled 与 config.cluster.json 的
    // enabled 是双闸——cluster_should_start = 两者相与）。
    cfg["cluster"]["enabled"] = serde_json::json!(true);
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
    cfg["agents"]["defaults"]["workspace"] = serde_json::json!(
        guard_env
            .home
            .join("workspace")
            .to_string_lossy()
            .to_string()
    );
    cfg["model_list"] = serde_json::json!([{
        "model_name": "mini-model",
        "model": "openai/gpt-fake",
        "api_key": "k",
        "api_base": "http://127.0.0.1:9",
        "model_tier": "mini"
    }]);
    std::fs::write(guard_env.home.join("config.json"), cfg.to_string()).unwrap();

    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");

    // enabled 集群 + 静态身份（node_id 断言锚点）。rpc_port 必须非 0
    // （0 = RPC server 不启动的语义值，bridge 身份块依赖它）——先用一次性
    // listener 占一个空闲端口再放手（:0 会撞语义）。
    let rpc_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("probe rpc port");
        l.local_addr().expect("rpc probe addr").port()
    };
    // 注意：写在 assemble 之后——assemble 会铺 workspace 模板（默认
    // config.cluster.json enabled=false），先写会被覆盖。
    let ws = guard_env.home.join("workspace");
    let ws_config = ws.join("config");
    std::fs::create_dir_all(&ws_config).unwrap();
    std::fs::write(
        ws_config.join("config.cluster.json"),
        format!(
            r#"{{"enabled": true, "port": 0, "rpc_port": {rpc_port}, "broadcast_interval": 1, "health_check_interval_secs": 0}}"#
        ),
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("cluster")).unwrap();
    std::fs::write(
        ws.join("cluster").join("peers.toml"),
        "[node]\n\
         id = \"cov-node-a\"\n\
         name = \"CovNodeA\"\n\
         role = \"worker\"\n\
         category = \"development\"\n\
         address = \"127.0.0.1:11950\"\n",
    )
    .unwrap();

    let cluster = init_cluster(&ctx).await.expect("init_cluster enabled");
    assert!(cluster.cluster_should_start, "enabled 形态必须起集群");
    let web = init_web(&ctx, &cluster).await.expect("init_web");
    let agent = init_agent(&ctx, &web, &cluster)
        .await
        .expect("agent 装配（死端点模型，无 LLM 调用）");
    let mut web_server = web.web_server;
    let post = init_post_agent(
        &ctx,
        &mut web_server,
        web.web_bind_host.clone(),
        web.web_port,
        agent,
        cluster,
    )
    .await
    .expect("post_agent 装配（enabled first_start Ok 臂）");

    let services = init_services(
        &ctx,
        web_server,
        web.web_bind_host.clone(),
        web.web_display_host.clone(),
        web.web_port,
        // bridge_client_launch 显式 Some：spawn 泵直连死中继（自重连无害）。
        Some(nemesis_config::BridgeClientConfig {
            enabled: true,
            relay_url: "ws://127.0.0.1:9".to_string(),
            token: "cov-token".to_string(),
            access_token: String::new(),
        }),
        post.agent_adapter.clone(),
        10,
        ClusterHandoff {
            bridge_cluster_slot: ctx.autopilot_cluster_slot.clone(),
            cluster_adapter: post.cluster_adapter.clone(),
            cluster_arc_ref: post.cluster_arc_ref.clone(),
        },
    )
    .await
    .expect("init_services enabled");

    // web 真实 bind；资产基址/node_id 落盘与集群身份一致。
    assert!(services.real_port > 0, "real_port 必须是真实绑定端口");
    let url = std::fs::read_to_string(nemesis_path::resolve_asset_node_url_path_in_workspace(&ws))
        .expect("asset url 落盘");
    assert!(
        url.starts_with("http://127.0.0.1:") && !url.ends_with(":0"),
        "回环绑定必须如实广告 127.0.0.1（G9），url={url}"
    );
    let node_id =
        std::fs::read_to_string(nemesis_path::resolve_asset_node_id_path_in_workspace(&ws))
            .expect("asset node id 落盘");
    assert_eq!(
        node_id, "cov-node-a",
        "node_id 必须取自 peers.toml 静态身份"
    );
    // RPC server 起服（:0 → 真实端口）→ bridge 身份快照与桥 RPC 枢纽非空。
    let cluster_arc = ctx
        .autopilot_cluster_slot
        .get()
        .expect("cluster arc 已入槽")
        .clone();
    assert!(
        cluster_arc.rpc_port() > 0,
        "enabled 集群 RPC 必须监听真实端口"
    );
    assert!(post.cluster_adapter.is_some(), "adapter 在场");

    services.svc_mgr.shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), services.web_handle).await;
}

// ---------------------------------------------------------------------------
// wave6（2026-09-25）：ctx.rs `set_on_job` 闭包体（assemble 装配后经
// execute_job 即时驱动，不等调度 tick）——
// - `board-ap:` 分流的急停跳过臂（estop.trigger → 诚实跳过、不建单）；
// - `board-ap:` 分流的正常触发臂（fire_board_autopilot → store 建单）；
// - message 任务的 InboundMessage 发布臂（channel/to 缺省、session_key
//   回落、max_rounds 元数据注入）。
// 复用本文件 SvcEnv 守卫；全网络面归零（web/gateway :0、死端点模型），
// 无 LLM 调用；闭包在 assemble 内接线，无需 init_services 全链。
// ---------------------------------------------------------------------------
mod wave6 {
    use crate::commands::gateway::{ClusterWiring, GatewayCtx, init_cluster};

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn w6_cron_on_job_covers_estop_skip_autopilot_fire_and_message_arms() {
        let guard_env = {
            let g = crate::GLOBAL_STATE_LOCK
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let tmp = tempfile::tempdir().unwrap();
            let home = tmp.path().join(".nemesisbot");
            std::fs::create_dir_all(&home).unwrap();
            unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
            super::SvcEnv {
                _guard: g,
                _tmp: tmp,
                home,
            }
        };

        // 配置：网络面归零 + 心跳关（F-B10 测试域禁真 tick）+ 死端点模型。
        let mut cfg: serde_json::Value =
            serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
        cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
        cfg["channels"]["web"]["port"] = serde_json::json!(0);
        cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
        cfg["gateway"]["port"] = serde_json::json!(0);
        cfg["heartbeat"]["enabled"] = serde_json::json!(false);
        cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
        cfg["agents"]["defaults"]["workspace"] = serde_json::json!(
            guard_env
                .home
                .join("workspace")
                .to_string_lossy()
                .to_string()
        );
        cfg["model_list"] = serde_json::json!([{
            "model_name": "mini-model",
            "model": "openai/gpt-fake",
            "api_key": "k",
            "api_base": "http://127.0.0.1:9",
            "model_tier": "mini"
        }]);
        std::fs::write(guard_env.home.join("config.json"), cfg.to_string()).unwrap();

        let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
            .await
            .expect("assemble ok")
            .expect("非 relay 必返 Some");
        // cluster 关（廉价路径）——autopilot 规则 target 为空，触发走纯本地建单。
        let _cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster 关 = 廉价路径");

        // 种一条本地 autopilot 规则并登记 cron job（name = `board-ap:{id}`）。
        let board_store = ctx.board_store.clone().expect("board store 在场");
        let ap = board_store
            .create_autopilot(&nemesis_board::NewAutopilot {
                name: "w6 闭包规则".into(),
                cron: "0 12 * * *".into(),
                title: "w6 {date}".into(),
                description: String::new(),
                priority: 2,
                project_id: None,
                target: String::new(),
                enabled: true,
                auto_plan: false,
                acceptance_criteria: None,
            })
            .expect("种规则");
        let cron = ctx.cron_service.clone();
        let n = nemesis_web::handlers::board::sync_autopilot_jobs(&cron, &board_store)
            .expect("同步登记");
        assert!(n > 0, "同步必须登记 >0 条");
        let ap_job_id = {
            let svc = cron.lock().unwrap();
            svc.list_jobs(true)
                .into_iter()
                .find(|j| j.name == format!("board-ap:{}", ap.id))
                .map(|j| j.id)
                .expect("board-ap job 已登记")
        };
        let issues_of = |ap_id: &str| {
            board_store
                .list_issues_by_origin("autopilot", ap_id, 50)
                .map(|v| v.len())
                .unwrap_or(0)
        };

        // ① 急停中触发 → 诚实跳过臂（execute Ok、不建单）。
        ctx.estop.trigger();
        assert!(ctx.estop.is_engaged(), "急停必须已触发");
        cron.lock()
            .unwrap()
            .execute_job(&ap_job_id)
            .expect("急停跳过臂 execute Ok");
        let issues_after_estop = issues_of(&ap.id.to_string());

        // ② 释放后触发 → fire_board_autopilot 正常建单臂。
        ctx.estop.release();
        cron.lock()
            .unwrap()
            .execute_job(&ap_job_id)
            .expect("正常触发臂 execute Ok");
        let issues_after_fire = issues_of(&ap.id.to_string());
        assert_eq!(issues_after_estop, 0, "急停臂不得建单");
        assert!(issues_after_fire > 0, "释放后触发必须新建 board issue");

        // ③ message 任务：channel/to/session_key 全缺省 → web + 空 chat_id 臂。
        let msg_job = {
            let svc = cron.lock().unwrap();
            svc.add_job(
                "w6-msg-default",
                nemesis_cron::CronSchedule {
                    kind: "cron".to_string(),
                    at_ms: None,
                    every_ms: None,
                    expr: Some("0 8 * * *".to_string()),
                    tz: None,
                },
                "w6 巡检消息",
                true,
                None,
                None,
            )
            .expect("登记 message job")
        };
        cron.lock()
            .unwrap()
            .execute_job(&msg_job.id)
            .expect("message 缺省臂 execute Ok");

        // ④ message 任务：session_key + to + max_rounds 齐备 → 路由 miss 回落
        //    `to`（conv_router 无绑定 tab）+ max_rounds 元数据注入臂。
        let msg_job2 = {
            let svc = cron.lock().unwrap();
            svc.add_job_ext(
                "w6-msg-session",
                nemesis_cron::CronSchedule {
                    kind: "cron".to_string(),
                    at_ms: None,
                    every_ms: None,
                    expr: Some("30 8 * * *".to_string()),
                    tz: None,
                },
                "w6 会话续跑消息",
                true,
                Some("cli"),
                Some("w6-direct"),
                Some("agent:w6-sess"),
                Some(3),
                true,
            )
            .expect("登记 session message job")
        };
        cron.lock()
            .unwrap()
            .execute_job(&msg_job2.id)
            .expect("message session 臂 execute Ok");

        {
            let svc = cron.lock().unwrap();
            for id in [&msg_job.id, &msg_job2.id] {
                let job = svc.get_job(id).expect("job 在");
                assert_eq!(
                    job.state.last_status.as_deref(),
                    Some("executed"),
                    "job {id} 必须执行成功"
                );
            }
        }
        // 无 web server 启动（未走 init_services）→ 无需 shutdown 等待；
        // estop 保持释放态（ctx 随测试结束销毁）。
    }
}

// wave6 续：enabled + master 角色的集群形态——点亮 post_agent.rs 的
// board 钩子装配区（cluster_ok 门内：BoardReviewDeps 构造、estop 恢复
// watcher、合并依赖注入、resume/retry/父单/项目/总结五钩注册、
// stuck-review 启动重放清扫）。coordinator 家族角色 = peers.toml
// [node] role = "master"；网络面照抄上方 enabled 形态（端口 0/预占）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6_boot_master_cluster_arms_board_review_hooks() {
    let guard_env = {
        let g = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        SvcEnv {
            _guard: g,
            _tmp: tmp,
            home,
        }
    };

    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
    cfg["gateway"]["port"] = serde_json::json!(0);
    cfg["heartbeat"]["enabled"] = serde_json::json!(false);
    cfg["cluster"]["enabled"] = serde_json::json!(true);
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("mini-model");
    cfg["agents"]["defaults"]["workspace"] = serde_json::json!(
        guard_env
            .home
            .join("workspace")
            .to_string_lossy()
            .to_string()
    );
    cfg["model_list"] = serde_json::json!([{
        "model_name": "mini-model",
        "model": "openai/gpt-fake",
        "api_key": "k",
        "api_base": "http://127.0.0.1:9",
        "model_tier": "mini"
    }]);
    std::fs::write(guard_env.home.join("config.json"), cfg.to_string()).unwrap();

    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");

    let rpc_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("probe rpc port");
        l.local_addr().expect("rpc probe addr").port()
    };
    let ws = guard_env.home.join("workspace");
    let ws_config = ws.join("config");
    std::fs::create_dir_all(&ws_config).unwrap();
    std::fs::write(
        ws_config.join("config.cluster.json"),
        format!(
            r#"{{"enabled": true, "port": 0, "rpc_port": {rpc_port}, "broadcast_interval": 1, "health_check_interval_secs": 0}}"#
        ),
    )
    .unwrap();
    std::fs::create_dir_all(ws.join("cluster")).unwrap();
    std::fs::write(
        ws.join("cluster").join("peers.toml"),
        "[node]\n\
         id = \"cov-node-m\"\n\
         name = \"CovNodeM\"\n\
         role = \"master\"\n\
         category = \"development\"\n\
         address = \"127.0.0.1:11951\"\n",
    )
    .unwrap();

    let cluster = init_cluster(&ctx)
        .await
        .expect("init_cluster enabled master");
    assert!(cluster.cluster_should_start, "master 形态必须起集群");
    let web = init_web(&ctx, &cluster).await.expect("init_web");
    let agent = init_agent(&ctx, &web, &cluster)
        .await
        .expect("agent 装配（死端点模型）");
    let mut web_server = web.web_server;
    let post = init_post_agent(
        &ctx,
        &mut web_server,
        web.web_bind_host.clone(),
        web.web_port,
        agent,
        cluster,
    )
    .await
    .expect("post_agent 装配（master 钩子装配区全走）");
    assert!(post.cluster_adapter.is_some(), "adapter 在场");

    let services = init_services(
        &ctx,
        web_server,
        web.web_bind_host.clone(),
        web.web_display_host.clone(),
        web.web_port,
        web.bridge_client_launch.clone(),
        post.agent_adapter.clone(),
        10,
        ClusterHandoff {
            bridge_cluster_slot: ctx.autopilot_cluster_slot.clone(),
            cluster_adapter: post.cluster_adapter.clone(),
            cluster_arc_ref: post.cluster_arc_ref.clone(),
        },
    )
    .await
    .expect("init_services master 形态");

    assert!(services.real_port > 0, "web 必须真实绑定");
    services.svc_mgr.shutdown();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(15), services.web_handle).await;
}

// wave6 续：assemble 内三区一次性点亮——
// ① 模型解析失败降级臂（ctx.rs 240-248）：agents.defaults.llm 指向不在
//    model_list 且不含任何可推断厂商关键词（claude/gpt/glm/...）的名字 →
//    resolve_model_config Err → warn + ProviderResolution::default() 降级
//    启动（双击直启「未配置模型」目标行为，不硬退）；
// ② U10 执行世界 Ok(Some) 臂（ctx.rs 375-377）：executor.enabled=true。
//    本 home 无 Sandboxie runtime（Start.exe 缺）→ build_executor_channel
//    落 stdio 车道（Layer 1），零引擎/UAC 触碰；
// ③ workflow 定义装载 n>0（ctx.rs 396-400）+ cron 触发器注册 n>0（413-419）：
//    definitions/ 先种 1 份合法 YAML（cron 触发）+ 1 份截断 JSON（解析
//    跳过只 warn，不影响 count）。
// assemble-only：三区全部在 assemble 内部，无需 init 链；无 web server
// 启动（端口不占）；workflow cron 触发的 tokio 任务随测试 runtime 结束
// 自然回收。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w6_boot_degraded_nullprovider_world_and_workflow_defs() {
    let guard_env = {
        let g = crate::GLOBAL_STATE_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(&home).unwrap();
        unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
        SvcEnv {
            _guard: g,
            _tmp: tmp,
            home,
        }
    };

    // workflow 定义先种（assemble 内 load_workflows_from_dir 消费）。
    let defs = guard_env
        .home
        .join("workspace")
        .join("workflow")
        .join("definitions");
    std::fs::create_dir_all(&defs).unwrap();
    std::fs::write(
        defs.join("w6-cron-wf.yaml"),
        r#"name: w6-cron-wf
version: "1.0.0"
description: wave6 cron seed
triggers:
  - trigger_type: cron
    config:
      schedule: "0 3 * * *"
nodes:
  - id: n1
    node_type: script
    config:
      language: bash
      script: echo hi
    is_terminal: true
edges: []
variables: {}
"#,
    )
    .unwrap();
    // 截断 JSON = parse 失败 → 引擎内 warn 跳过（装载计数只算合法那份）。
    std::fs::write(defs.join("w6-broken.json"), "{\"name\": \"w6-broken\", ").unwrap();

    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["gateway"]["host"] = serde_json::json!("127.0.0.1");
    cfg["gateway"]["port"] = serde_json::json!(0);
    cfg["heartbeat"]["enabled"] = serde_json::json!(false);
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("ghost-model-x");
    cfg["agents"]["defaults"]["workspace"] = serde_json::json!(
        guard_env
            .home
            .join("workspace")
            .to_string_lossy()
            .to_string()
    );
    cfg["model_list"] = serde_json::json!([]);
    cfg["executor"] = serde_json::json!({"enabled": true, "sandbox": true});
    std::fs::write(guard_env.home.join("config.json"), cfg.to_string()).unwrap();

    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok（模型解析失败必须降级不硬退）")
        .expect("非 relay 必返 Some");
    assert!(!ctx.estop.is_engaged(), "急停默认释放");
}
