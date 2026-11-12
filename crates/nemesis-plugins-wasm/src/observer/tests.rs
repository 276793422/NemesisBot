use super::*;
use crate::limits::PluginLimits;
use crate::manifest::PluginManifest;
use crate::registry::{PluginManager, ToolMetaSnapshot};
use crate::trust::PluginTrustState;
use nemesis_types::agent::AgentEvent;

fn tool_started() -> AgentEvent {
    AgentEvent::ToolStarted {
        session_key: "sess-1".into(),
        chat_id: "chat-1".into(),
        call_id: "call-1".into(),
        tool: "exec".into(),
        args_preview: "SECRET-CONTENT-BODY".into(),
    }
}

fn tool_finished() -> AgentEvent {
    AgentEvent::ToolFinished {
        session_key: "sess-1".into(),
        chat_id: "chat-1".into(),
        call_id: "call-1".into(),
        tool: "exec".into(),
        duration_ms: 5,
        ok: false,
        result_preview: "SECRET-CONTENT-BODY".into(),
    }
}

// -- 投影 ----------------------------------------------------------------

#[test]
fn project_event_tool_start_and_end_shapes() {
    let json = project_event(&tool_started()).expect("tool_start projects");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["type"], "tool_start");
    assert_eq!(v["session_key"], "sess-1");
    assert_eq!(v["tool"], "exec");
    assert_eq!(v["call_id"], "call-1");
    // 零内容体：args 预览与 chat_id 不出宿主。
    assert!(!json.contains("SECRET-CONTENT-BODY"));
    assert!(!json.contains("chat-1"));
    assert!(v["ts_ms"].as_u64().expect("ts") > 0);

    let json = project_event(&tool_finished()).expect("tool_end projects");
    let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
    assert_eq!(v["type"], "tool_end");
    assert_eq!(v["duration_ms"], 5);
    assert_eq!(v["ok"], false);
}

#[test]
fn project_event_non_broadcast_variants_return_none() {
    // v1 只投影 tool_start/tool_end 两型（schema 预留三型不投影）。
    assert!(
        project_event(&AgentEvent::ModeChanged {
            session_key: "s".into(),
            chat_id: "c".into(),
            mode: "plan".into(),
        })
        .is_none()
    );
}

// -- 事件泵 ----------------------------------------------------------------

/// 注册一个观察者插件（component=0 哨兵——本组用例只验投递面，不执行
/// guest）。
fn register_observer(mgr: &std::sync::Arc<PluginManager>) {
    let manifest = PluginManifest::parse(&format!(
            "api-version = 1\nslug = \"watcher\"\nkind = \"observer\"\nname = \"W\"\nversion = \"0.1.0\"\nwasm-sha256 = \"{}\"\n",
            "b".repeat(64)
        ))
        .expect("manifest")
        .0;
    mgr.register(
        manifest,
        PluginTrustState::ReviewRequired,
        0,
        None,
        None,
        None,
    )
    .expect("register observer");
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_pump_projects_and_closes_on_sender_drop() {
    let (_ws, mgr) = crate::test_support::fresh_manager(PluginLimits::default());
    register_observer(&mgr);
    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(8);
    let handle = spawn_pump(mgr.clone(), rx);

    tx.send(tool_started()).expect("send");
    tx.send(tool_finished()).expect("send");
    // 让泵消费完。
    for _ in 0..20 {
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    drop(tx); // 全部 sender drop → 泵退出
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("pump exits")
        .expect("join");
    assert!(mgr.is_enabled());
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_pump_reports_lag_and_survives() {
    let (_ws, mgr) = crate::test_support::fresh_manager(PluginLimits::default());
    register_observer(&mgr);
    let (tx, rx) = tokio::sync::broadcast::channel::<AgentEvent>(1);
    // 容量 1 + 先发 50 条（尚未 spawn 泵）→ 首次 recv 必 Lagged。
    for _ in 0..50 {
        let _ = tx.send(tool_started());
    }
    let handle = spawn_pump(mgr.clone(), rx);
    drop(tx); // 全部 sender drop → 泵退出（先 drop 再等，否则泵永不结束）
    tokio::time::timeout(std::time::Duration::from_secs(5), handle)
        .await
        .expect("pump survives lag")
        .expect("join");
}

// -- enqueue 闸链（不经泵，直投队列）---------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn enqueue_respects_master_switch_and_plugin_switch() {
    let (_ws, mgr) = crate::test_support::fresh_manager(PluginLimits::default());
    register_observer(&mgr);
    let reg = mgr.get("watcher").expect("registered");

    // 子系统总开关关 = 停投（fire-and-forget，无错误通道）。
    mgr.set_enabled(false);
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
    assert_eq!(
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    mgr.set_enabled(true);

    // 插件 enable 位关 = 停投。
    reg.enabled
        .store(false, std::sync::atomic::Ordering::SeqCst);
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
    assert_eq!(
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    reg.enabled.store(true, std::sync::atomic::Ordering::SeqCst);

    // 运行期总闸关 = 停投。
    crate::test_support::disable_runtime_gate(&_ws);
    mgr.enqueue_observer_event(r#"{"type":"tool_start"}"#.into());
    assert_eq!(
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[test]
fn enqueue_counts_queue_full_drops() {
    // 当前线程 runtime：worker 永不调度 → 队列必满，丢弃计数确定性见涨。
    let rt = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime");
    let mut limits = PluginLimits::default();
    limits.observer_queue_depth = 1;
    let _guard = rt.enter();
    let (_ws, mgr) = crate::test_support::fresh_manager(limits);
    register_observer(&mgr);
    let reg = mgr.get("watcher").expect("registered");
    mgr.enqueue_observer_event(r#"{"type":"tool_start","seq":1}"#.into());
    mgr.enqueue_observer_event(r#"{"type":"tool_start","seq":2}"#.into());
    assert_eq!(
        reg.dropped_events
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    drop(mgr);
}

// -- 其它杂项 --------------------------------------------------------------

#[test]
fn tool_meta_snapshot_roundtrip() {
    let meta = ToolMetaSnapshot {
        name: "plugin.demo.t".into(),
        base: "t".into(),
        title: "T".into(),
        description: "d".into(),
        parameters_json: "{}".into(),
        operation_type: "read".into(),
        min_tier: "big".into(),
    };
    let json = serde_json::to_string(&meta).expect("serialize");
    let back: ToolMetaSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.name, "plugin.demo.t");
}
