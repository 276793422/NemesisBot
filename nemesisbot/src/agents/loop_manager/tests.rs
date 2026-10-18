//! AgentLoopManager 行为用例（G1，2026-10-18 三方复评整改批三）：诚实拒绝
//! 面（不存在 / main / project / 未 active）+ NullProvider 全链路往返
//! （mpsc 注入 → loop 消费 → 出站回包到发起 chat_id）+ remove 联动与
//! 重建。全部 tempdir 真文件 + 内存可开的 tasks.db，LLM 面用无模型配置的
//! NullProvider 装配（确定性好，不依赖外部 LLM；错误出站即回包链证据）。

use std::sync::Arc;

use super::AgentLoopManager;
use crate::agent_factory::SharedResources;
use nemesis_agent::r#loop::AgentLoop;
use nemesis_taskstore::agents::{kind as agent_kind, status as agent_status};
use nemesis_taskstore::{AgentRecord, SqliteAgentsStore, TaskDb};
use nemesis_types::channel::{InboundMessage, OutboundMessage};

/// unwrap_err 需要 Ok 侧 Debug（AgentLoop 未实现）——测试内收敛一处。
fn expect_err(r: Result<Arc<AgentLoop>, String>) -> String {
    match r {
        Err(e) => e,
        Ok(_) => panic!("expected Err"),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    workspace: std::path::PathBuf,
    store: SqliteAgentsStore,
    manager: AgentLoopManager,
    /// loop 出站接收端（round-trip 断言用；make 时配对建立）。
    outbound_rx: tokio::sync::mpsc::Receiver<OutboundMessage>,
}

/// manager 夹具：tempdir + TaskDb + 最小 shared（config 在场，NullProvider
/// 形态；出站通道配对接收端保留）。config_path 缺省写无 model_list 的
/// config（工厂 NullProvider 降级装配——round-trip 不触真实 LLM）。
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let workspace = home.join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let cfg = serde_json::json!({
        "agents": { "defaults": { "llm": "missing-model", "max_tool_iterations": 2 } },
        "model_list": []
    });
    std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
    let db = TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
        &workspace,
    ))
    .unwrap();
    let store = SqliteAgentsStore::from_db(Arc::clone(&db));
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(64);
    let shared = Arc::new(SharedResources {
        home: home.clone(),
        workspace: workspace.clone(),
        agent_outbound_tx: outbound_tx,
        cron_service: Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(""),
        )),
        mcp_config_path: home.join("nonexistent-mcp.json"),
        config_store: Arc::new(
            nemesis_config::ConfigStore::load(&home.join("config.json")).unwrap_or_else(|_| {
                nemesis_config::ConfigStore::from_config(
                    nemesis_config::Config::default(),
                    home.join("config.json"),
                )
            }),
        ),
        ..Default::default()
    });
    let manager = AgentLoopManager::new(
        shared,
        db,
        Arc::new(nemesis_agent::session::SessionStore::new_in_memory()),
    );
    Fixture {
        _dir: dir,
        workspace,
        store,
        manager,
        outbound_rx,
    }
}

/// 落一条指定 kind/status 的记录（exec_root 指向已物化目录——构建面的
/// 存在性防御要求 exec_root 在场）。
fn put_record(
    store: &SqliteAgentsStore,
    workspace: &std::path::Path,
    id: &str,
    agent_kind: &str,
    status: &str,
) {
    let mut rec = AgentRecord::new_provisioning(id, &format!("agent-{id}"), agent_kind);
    rec.exec_root = workspace.to_string_lossy().to_string();
    rec.status = status.to_string();
    store.upsert(&rec).unwrap();
}

fn inbound(chat_id: &str, session_key: String) -> InboundMessage {
    InboundMessage {
        channel: "web".to_string(),
        sender_id: "user".to_string(),
        chat_id: chat_id.to_string(),
        content: "你好".to_string(),
        media: vec![],
        session_key,
        correlation_id: String::new(),
        metadata: Default::default(),
        voice_playback: None,
    }
}

#[tokio::test]
async fn resolve_or_build_honest_error_arms() {
    let fx = fixture();
    put_record(
        &fx.store,
        &fx.workspace,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        agent_kind::MAIN,
        agent_status::ACTIVE,
    );
    put_record(
        &fx.store,
        &fx.workspace,
        "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        agent_kind::PROJECT,
        agent_status::ACTIVE,
    );
    put_record(
        &fx.store,
        &fx.workspace,
        "cccccccccccccccccccccccccccccccc",
        agent_kind::SELFBUILT,
        agent_status::PROVISIONING,
    );
    // 不存在。
    let err = expect_err(fx.manager.loop_for_agent("no-such-agent"));
    assert!(err.contains("不存在"), "got: {err}");
    // main → 正门是对话组。
    let err = expect_err(
        fx.manager
            .loop_for_agent("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
    );
    assert!(err.contains("对话组"), "main 重定向文案: {err}");
    // project → 正门是 projects API。
    let err = expect_err(
        fx.manager
            .loop_for_agent("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"),
    );
    assert!(err.contains("项目"), "project 重定向文案: {err}");
    // provisioning → 不可会话（sweep 收敛语义在文案里）。
    let err = expect_err(
        fx.manager
            .loop_for_agent("cccccccccccccccccccccccccccccccc"),
    );
    assert!(err.contains("provisioning"), "状态拒绝文案: {err}");
    // 四臂全部拒绝面：无 loop 残留。
    assert!(!fx.manager.is_running("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));
    assert!(!fx.manager.is_running("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
    assert!(!fx.manager.is_running("cccccccccccccccccccccccccccccccc"));
}

#[tokio::test]
async fn send_message_missing_agent_honest_error() {
    let fx = fixture();
    let err = fx
        .manager
        .send_message(
            "ghost",
            inbound("web:conn-1", "agents/ghost:session:default".into()),
        )
        .await
        .unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
}

#[tokio::test]
async fn remove_without_loop_is_false_and_idempotent() {
    let fx = fixture();
    assert!(!fx.manager.remove("nobody"));
    assert!(
        !fx.manager.remove("nobody"),
        "幂等：二次 remove 不炸不翻 true"
    );
}

/// NullProvider 全链路往返：resolve_or_build 建真 loop → mpsc 注入 → loop
/// 消费 → LLM 失败（无模型）→ 错误出站回发起 chat_id。证明「注入面与
/// 出站链」通——回包寻址靠 chat_id（web 通道 → 发起方 WS 连接）。
#[tokio::test]
async fn null_provider_roundtrip_inject_to_outbound() {
    let mut fx = fixture();
    let id = "dddddddddddddddddddddddddddddddd";
    put_record(
        &fx.store,
        &fx.workspace,
        id,
        agent_kind::SELFBUILT,
        agent_status::ACTIVE,
    );
    fx.manager
        .send_message(
            id,
            inbound("web:conn-rt", format!("agents:{id}:session:default")),
        )
        .await
        .expect("inject");
    assert!(fx.manager.is_running(id), "send 后 loop 必须已构建在跑");
    // 出站回包（NullProvider 错误路径）到达发起 chat_id。
    let outbound = tokio::time::timeout(std::time::Duration::from_secs(30), fx.outbound_rx.recv())
        .await
        .expect("出站回包超时（30s 内 loop 错误路径应发布）")
        .expect("出站通道不应关闭");
    assert_eq!(outbound.channel, "web");
    assert_eq!(outbound.chat_id, "web:conn-rt", "回包必须寻址发起连接");
    assert!(!outbound.content.is_empty(), "错误回包内容不得为空");
    fx.manager.stop_all();
}

/// remove 联动 + 重建（gateway 重启后记录仍在、loop 可重建语义——本地化
/// 为本实例内 remove 后再 resolve_or_build 成功重建）。
#[tokio::test]
async fn remove_stops_loop_and_rebuild_works() {
    let fx = fixture();
    let id = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
    put_record(
        &fx.store,
        &fx.workspace,
        id,
        agent_kind::SELFBUILT,
        agent_status::ACTIVE,
    );
    let _loop1 = fx.manager.loop_for_agent(id).expect("first build");
    assert!(fx.manager.is_running(id));
    // G2：manager 按 agent home 派生浏览器 profile 目录（单一真相源
    // nemesis-path），构建产物必须携带。
    {
        let built = fx.manager.loop_for_agent(id).expect("probe build");
        let expect_profile =
            nemesis_path::resolve_agent_browser_profile_in_workspace(&fx.workspace, id);
        assert_eq!(
            built.browser_profile(),
            Some(expect_profile.to_string_lossy().as_ref()),
            "manager 派发的 browser_profile 必须贯通到 loop"
        );
    }
    assert!(fx.manager.remove(id), "remove 必须报告确有实例被停");
    assert!(!fx.manager.is_running(id));
    assert!(!fx.manager.remove(id), "幂等：二次 remove false");
    // 重建（记录仍在 store——remove 只停 loop 不删记录，记录删除是
    // AgentManager::remove 的职责）。
    let _loop2 = fx.manager.loop_for_agent(id).expect("rebuild");
    assert!(fx.manager.is_running(id));
    // teardown 收口（防 tokio 测试运行时退出时在跑任务噪音）。
    fx.manager.stop_all();
    assert!(!fx.manager.is_running(id));
}
