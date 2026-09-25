//! cov 补测（2026-09-25）：`init_web` 分支矩阵 + `GatewayCtx::assemble`
//! 缺配置 auto-init（seed）臂 + 模型解析失败降级臂。
//!
//! 形态（与姊妹 tests.rs 的 full_assembly 同纪律）：临时 home +
//! NEMESISBOT_HOME + crate::GLOBAL_STATE_LOCK 互斥；网络面归零（web/websocket
//! 端口 0 = OS 分配）；无真 LLM（model_list 留空 → 走解析失败 NullProvider
//! 降级臂，无外网调用）；SEC-001 拒绝臂用 TEST-NET 假地址（192.0.2.1，
//! init_web 只算字符串不真正 bind，无防火墙弹窗风险）。
//!
//! 与 full_assembly 的差异：本组直接驱动 assemble → init_cluster（disabled
//! 廉价路径）→ init_web，并在同一 ctx 上就地翻转 cfg 跑 11 个变体——每个
//! init_web 调用泄漏一个 mem::forget 的空 ChannelManager（生产语义照抄），
//! 随测试 runtime 结束一起销毁，无跨测试影响。

use super::super::{ClusterWiring, GatewayCtx, init_cluster};
use super::init_web;

/// env/home 夹具：持全局锁，drop 清 env。
struct CtxEnv {
    _guard: std::sync::MutexGuard<'static, ()>,
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for CtxEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn ctx_env() -> CtxEnv {
    let guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    CtxEnv {
        _guard: guard,
        _tmp: tmp,
        home,
    }
}

/// 写一份网络面归零、无模型条目的 config.json（模型解析走降级臂）。
fn write_bare_config(home: &std::path::Path) {
    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["agents"]["defaults"]["workspace"] =
        serde_json::json!(home.join("workspace").to_string_lossy().to_string());
    cfg["model_list"] = serde_json::json!([]);
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn init_web_branch_matrix_via_assemble() {
    let th = ctx_env();
    write_bare_config(&th.home);
    let mut ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    // 模型解析失败降级臂：model_list 空 → resolution 回落默认（NullProvider）。
    assert!(!ctx.model_name.is_empty(), "降级也要有诚实模型名");
    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster disabled 廉价路径");

    // 变体 A：基线（web on）。
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert_eq!(w.enabled_channels, vec!["web".to_string()]);
    assert_eq!(w.web_bind_host, "127.0.0.1");
    assert_eq!(w.web_display_host, "127.0.0.1");
    assert_eq!(w.web_port, 0);
    assert!(w.bridge_client_launch.is_none());
    drop(w);

    // 变体 B：web off → 空 enabled 列表（WebChannelConfig None 臂）。
    ctx.cfg.channels.web.enabled = false;
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert!(w.enabled_channels.is_empty());
    drop(w);

    // 变体 C：websocket on（回环 :0）→ Some 臂 + sync 目标登记。
    ctx.cfg.channels.websocket.enabled = true;
    ctx.cfg.channels.websocket.host = "127.0.0.1".into();
    ctx.cfg.channels.websocket.port = 0;
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert_eq!(w.enabled_channels, vec!["websocket".to_string()]);
    drop(w);

    // 变体 D：websocket 非回环 + 引导令牌 → SEC-001 拒绝（独立监听面闸臂）。
    ctx.cfg.channels.websocket.host = "192.0.2.1".into();
    let Err(err) = init_web(&ctx, &cluster).await else {
        panic!("websocket 非回环 + 引导令牌必须被 SEC-001 拒绝");
    };
    let msg = err.to_string();
    assert!(msg.contains("SEC-001"), "{msg}");
    assert!(
        msg.contains("channels.websocket.auth_token"),
        "错误要指名字段: {msg}"
    );

    // 变体 E：web 非回环 + 引导令牌 → SEC-001 拒绝（web 闸臂）。
    ctx.cfg.channels.websocket.enabled = false;
    ctx.cfg.channels.web.enabled = true;
    ctx.cfg.channels.web.host = "192.0.2.1".into();
    let Err(err) = init_web(&ctx, &cluster).await else {
        panic!("web 非回环 + 引导令牌必须被 SEC-001 拒绝");
    };
    assert!(err.to_string().contains("channels.web.auth_token"));

    // 变体 F：web 非回环 + 强令牌 → 过闸，host 原样透传（bind/display 同值臂）。
    ctx.cfg.channels.web.auth_token = "strong-secret-token".into();
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert_eq!(w.web_bind_host, "192.0.2.1");
    assert_eq!(w.web_display_host, "192.0.2.1");
    drop(w);
    ctx.cfg.channels.web.host = "127.0.0.1".into();
    ctx.cfg.channels.web.auth_token = String::new();

    // 变体 G：CORS 三态（坏文件 → 宽容默认 / dev 模式 / 正常 origins）。
    let cors_dir = th.home.join("config");
    std::fs::create_dir_all(&cors_dir).unwrap();
    let cors_path = cors_dir.join("cors.json");
    std::fs::write(&cors_path, "{not json").unwrap();
    let w = init_web(&ctx, &cluster).await.unwrap();
    drop(w);
    std::fs::write(
        &cors_path,
        r#"{"allowed_origins": [], "development_mode": true}"#,
    )
    .unwrap();
    let w = init_web(&ctx, &cluster).await.unwrap();
    drop(w);
    std::fs::write(
        &cors_path,
        r#"{"allowed_origins": ["http://localhost:5173"], "development_mode": false}"#,
    )
    .unwrap();
    let w = init_web(&ctx, &cluster).await.unwrap();
    drop(w);
    let _ = std::fs::remove_file(&cors_path);

    // 变体 H：bridge.server.token 配置 → 中继服务端装配臂（客户端仍关）。
    ctx.cfg.bridge = Some(nemesis_config::BridgeConfig {
        server: nemesis_config::BridgeServerConfig {
            token: "relay-secret".into(),
        },
        client: nemesis_config::BridgeClientConfig::default(),
    });
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert!(w.bridge_client_launch.is_none());
    drop(w);

    // 变体 I：桥客户端开但 relay_url 空 → 旁路不启动（诚实 error 臂）。
    ctx.cfg.bridge.as_mut().unwrap().client.enabled = true;
    let w = init_web(&ctx, &cluster).await.unwrap();
    assert!(w.bridge_client_launch.is_none());
    drop(w);

    // 变体 J：桥客户端配置完整 → launch 携带配置原值。
    ctx.cfg.bridge.as_mut().unwrap().client.relay_url = "http://127.0.0.1:1".into();
    ctx.cfg.bridge.as_mut().unwrap().client.token = "relay-secret".into();
    ctx.cfg.bridge.as_mut().unwrap().client.access_token = "panel-pw".into();
    let w = init_web(&ctx, &cluster).await.unwrap();
    let launch = w.bridge_client_launch.clone().expect("配置完整必须 Some");
    assert_eq!(launch.relay_url, "http://127.0.0.1:1");
    assert_eq!(launch.token, "relay-secret");
    assert_eq!(launch.access_token, "panel-pw");
    drop(w);
    ctx.cfg.bridge = None;

    // 注：不再断言 `signature_status_from_start_check().is_none()`——该查询读
    // 进程级 OnceLock 快照，全量套件中其他 lane 的签名类测试可能先行装配
    // （单测进程内其值不可隔离），两种返回值都合法，进程内不可断言。
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_auto_inits_seed_mode_when_config_missing() {
    let th = ctx_env();
    // 不写 config.json → assemble 走 auto-init（Seed）分支后继续。
    assert!(!th.home.join("config.json").exists());
    let ctx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    // auto-init 后：config 与 workspace 人格模板就位（seed = only-if-absent）。
    assert!(
        th.home.join("config.json").exists(),
        "seed 应产出 config.json"
    );
    assert!(
        th.home.join("workspace").join("IDENTITY.md").exists(),
        "seed 应提取工作空间人格模板"
    );
    assert!(!ctx.estop.is_engaged(), "estop 初始必须为释放态");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn assemble_coverage_arms_capture_off_board_db_failure_and_relay_gate() {
    let th = ctx_env();
    write_bare_config(&th.home);
    // ① capture 关闭臂：debug.capture.enabled=false（默认模板无该段，就地植入）。
    let mut cfg: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(th.home.join("config.json")).unwrap())
            .unwrap();
    cfg["debug"] = serde_json::json!({ "capture": { "enabled": false } });
    std::fs::write(th.home.join("config.json"), cfg.to_string()).unwrap();
    // ② board store 打开失败臂：把 board.db 位置摆成目录 → rusqlite 拒开
    //    → warn + board_store=None（不阻断装配）。
    let board_dir = th.home.join("workspace").join("board").join("board.db");
    std::fs::create_dir_all(&board_dir).unwrap();

    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    assert!(
        ctx.board_store.is_none(),
        "board.db 被目录占据时 store 必须诚实缺席"
    );

    // ③ relay 早退臂：--relay + 空 bridge.server.token → run_relay fail-closed
    //    （不 bind 任何端口，直接 Err 返回）。
    let Err(err) = GatewayCtx::assemble(false, true, &[]).await else {
        panic!("relay 模式在 bridge.server.token 为空时必须拒绝启动");
    };
    assert!(err.to_string().contains("bridge.server.token"), "{err}");
}

// ===========================================================================
// wave5 round2（2026-09-25）：CORS 新路径直写（修旧变体 G 的 legacy-copy
// 一次性语义缺陷——旧测写 legacy 路径，首跑即被拷贝到新路径，后续覆写
// 不再生效，dev/origins 两臂从未真正执行）、全通道 enabled push 矩阵、
// bridge.server.token 空值 info 臂、enabled cluster + bridge server token
// 的 hub RPC 枢纽装配臂、bus 出站 → ChannelManager 桥任务真转发。
// ===========================================================================

/// CORS dev-mode + origins 双臂（直写 cors_config_path 的**新路径**，
/// 绕开 legacy 拷贝一次性语义）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w5_init_web_cors_dev_and_origins_arms_via_real_path() {
    let th = ctx_env();
    write_bare_config(&th.home);
    let ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster disabled 廉价路径");

    // 直写新路径（cors_config_path 返回值即 web_init 实际读取处）。
    let cors_path = crate::common::cors_config_path(&th.home);
    std::fs::create_dir_all(cors_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cors_path,
        r#"{"allowed_origins": [], "development_mode": true}"#,
    )
    .unwrap();
    let w = init_web(&ctx, &cluster).await.expect("dev-mode CORS 不炸");
    drop(w);

    // origins 形态（development_mode false + 非空白名单）。
    std::fs::write(
        &cors_path,
        r#"{"allowed_origins": ["http://localhost:5173", "http://127.0.0.1:49000"], "development_mode": false}"#,
    )
    .unwrap();
    let w = init_web(&ctx, &cluster).await.expect("origins CORS 不炸");
    drop(w);
}

/// 全通道 enabled push 矩阵（telegram..external 11 条 push 线）+ external /
/// maixcam / line 三通道 Some 构造臂 + bus 出站桥任务真转发。
/// feature 关闭的通道名只在 allowed 列表（不会构造/启动实际通道实例）；
/// external input_exe 空 → spawn 异步失败（error 日志，无窗口无进程残留）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w5_init_web_all_channel_push_and_outbound_bridge_forward() {
    let th = ctx_env();
    write_bare_config(&th.home);
    let mut ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster disabled 廉价路径");

    // 全通道开启（网络面归零：maixcam/line 绑 127.0.0.1:0）。
    ctx.cfg.channels.telegram.enabled = true;
    ctx.cfg.channels.discord.enabled = true;
    ctx.cfg.channels.feishu.enabled = true;
    ctx.cfg.channels.slack.enabled = true;
    ctx.cfg.channels.whatsapp.enabled = true;
    ctx.cfg.channels.dingtalk.enabled = true;
    ctx.cfg.channels.qq.enabled = true;
    ctx.cfg.channels.line.enabled = true;
    ctx.cfg.channels.line.webhook_host = "127.0.0.1".into();
    ctx.cfg.channels.line.webhook_port = 0;
    ctx.cfg.channels.onebot.enabled = true;
    ctx.cfg.channels.maixcam.enabled = true;
    ctx.cfg.channels.maixcam.host = "127.0.0.1".into();
    ctx.cfg.channels.maixcam.port = 0;
    ctx.cfg.channels.external.enabled = true;

    let w = init_web(&ctx, &cluster).await.expect("全通道形态不炸");
    for name in [
        "telegram", "discord", "feishu", "slack", "whatsapp", "dingtalk", "qq", "line", "onebot",
        "maixcam", "external",
    ] {
        assert!(
            w.enabled_channels.iter().any(|c| c == name),
            "enabled 列表缺 {name}: {:?}",
            w.enabled_channels
        );
    }
    drop(w);

    // bus 出站桥任务（bus outbound → ChannelManager mpsc）：发布一条出站
    // 消息并让桥任务 poll 到（Ok 臂转发）。
    ctx.bus
        .publish_outbound(nemesis_types::channel::OutboundMessage::new(
            "web",
            "w5-bridge-probe",
            "出站桥探测",
        ));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
}

/// bridge.server.token 配置了但为空串 → 接入门不开放 info 臂（else 分支）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn w5_init_web_bridge_server_token_empty_takes_else_arm() {
    let th = ctx_env();
    write_bare_config(&th.home);
    let mut ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");
    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("cluster disabled 廉价路径");
    ctx.cfg.bridge = Some(nemesis_config::BridgeConfig {
        server: nemesis_config::BridgeServerConfig {
            token: String::new(),
        },
        client: nemesis_config::BridgeClientConfig::default(),
    });
    let w = init_web(&ctx, &cluster)
        .await
        .expect("空 token 走诚实 info 臂");
    assert!(w.bridge_client_launch.is_none());
}

/// enabled cluster（rpc_port>0）+ bridge.server.token → hub 身份 sink +
/// 桥帧 RPC 枢纽 + 成员表快照三件套装配（init_web 的 hub 侧深水臂）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn w5_init_web_bridge_hub_rpc_hub_assembly_with_enabled_cluster() {
    let th = ctx_env();
    // 集群主开关（config.json 侧双闸之一）。
    let mut cfg: serde_json::Value =
        serde_json::from_str(crate::CONFIG_DEFAULT).expect("parse CONFIG_DEFAULT");
    cfg["channels"]["web"]["host"] = serde_json::json!("127.0.0.1");
    cfg["channels"]["web"]["port"] = serde_json::json!(0);
    cfg["cluster"]["enabled"] = serde_json::json!(true);
    cfg["agents"]["defaults"]["workspace"] =
        serde_json::json!(th.home.join("workspace").to_string_lossy().to_string());
    cfg["model_list"] = serde_json::json!([]);
    std::fs::write(th.home.join("config.json"), cfg.to_string()).unwrap();

    let mut ctx: GatewayCtx = GatewayCtx::assemble(false, false, &[])
        .await
        .expect("assemble ok")
        .expect("非 relay 必返 Some");

    // enabled 集群 + 非 0 rpc_port（0 = RPC 不启动的语义值，枢纽装配依赖它）。
    let rpc_port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("probe rpc port");
        l.local_addr().expect("rpc probe addr").port()
    };
    let ws = th.home.join("workspace");
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

    let cluster: ClusterWiring = init_cluster(&ctx).await.expect("init_cluster enabled");
    assert!(cluster.cluster_should_start, "enabled 形态必须起集群");

    // bridge.server.token 非空 → 中继装配 + hub_cluster 在槽 → 深水臂。
    ctx.cfg.bridge = Some(nemesis_config::BridgeConfig {
        server: nemesis_config::BridgeServerConfig {
            token: "cov-hub-token".into(),
        },
        client: nemesis_config::BridgeClientConfig::default(),
    });
    let w = init_web(&ctx, &cluster).await.expect("hub 装配形态不炸");
    assert!(w.bridge_client_launch.is_none(), "client 关 → launch None");
}
