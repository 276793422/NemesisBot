//! `vm.*` WSAPI 用例（P5 第 4 步）：未装配诚实报错 / 各命令臂经 bridge /
//! config.set 缺参拒绝 / L1 `commands()` 清单。进程级 bridge 槽是共享态
//! ——全部用例持 [`VM_BRIDGE_TEST_LOCK`] 串行（AGENTS_BRIDGE 家族纪律）。
// 持锁跨 await 是刻意的测试串行化（槽位置换必须覆盖整个 handler await
// 窗口，否则并行用例互相置换 bridge 假绿）；agents_ws_tests 同款豁免。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 最小 RequestContext（agents_ws_tests make_ctx 同款；vm handler 不消费
/// ctx 状态，全字段 None/空即可）。TempDir 当场 drop——只用字符串。
fn make_ctx() -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 假 bridge（录制调用 + 固定应答）——handler 只消费 trait 面，不依赖
/// nemesisbot（依赖方向不许反向）。
struct FakeBridge {
    calls: Mutex<Vec<String>>,
}

impl FakeBridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
        })
    }
    fn note(&self, c: &str) {
        self.calls.lock().unwrap().push(c.to_string());
    }
}

#[async_trait::async_trait]
impl VmBridge for FakeBridge {
    fn overview(&self) -> serde_json::Value {
        self.note("overview");
        serde_json::json!({ "enabled": false })
    }
    fn config_get(&self) -> Option<serde_json::Value> {
        self.note("config.get");
        Some(serde_json::json!({ "enabled": true }))
    }
    fn config_set(&self, vm: serde_json::Value) -> Result<(), String> {
        self.note("config.set");
        if vm.get("enabled").is_none() {
            return Err("缺少 enabled".to_string());
        }
        Ok(())
    }
    fn image_list(&self) -> serde_json::Value {
        self.note("image.list");
        serde_json::json!({ "images": [] })
    }
    async fn image_build(&self, update: bool) -> Result<serde_json::Value, String> {
        self.note(if update {
            "image.build:update"
        } else {
            "image.build"
        });
        Ok(serde_json::json!({ "started": true }))
    }
    fn instance_list(&self) -> Vec<serde_json::Value> {
        self.note("instance.list");
        vec![serde_json::json!({ "agent_id": "a1", "running": true })]
    }
    async fn instance_sleep(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.note("instance.sleep");
        Ok(serde_json::json!({ "agent_id": agent_id, "slept": true }))
    }
    async fn instance_wake(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.note("instance.wake");
        Ok(serde_json::json!({ "agent_id": agent_id, "woken": true }))
    }
    async fn instance_kill(&self, agent_id: &str) -> Result<serde_json::Value, String> {
        self.note("instance.kill");
        Ok(serde_json::json!({ "agent_id": agent_id, "stopped": true }))
    }
    async fn gc_run(&self) -> serde_json::Value {
        self.note("gc.run");
        serde_json::json!({ "swept": 0 })
    }
    fn bootstrap_status(&self) -> serde_json::Value {
        self.note("bootstrap.status");
        serde_json::json!({ "qemu": { "state": "absent" } })
    }
    async fn bootstrap_install_qemu(&self) -> Result<serde_json::Value, String> {
        self.note("bootstrap.install_qemu");
        Ok(serde_json::json!({ "started": true }))
    }
    async fn bootstrap_enable_accel(&self) -> Result<serde_json::Value, String> {
        self.note("bootstrap.enable_accel");
        Ok(serde_json::json!({ "guidance": "n/a" }))
    }
}

// 持锁跨 await 见文件头豁免说明。
async fn with_bridge<T>(f: impl AsyncFnOnce(Arc<FakeBridge>, RequestContext) -> T) -> T {
    let _serial = VM_BRIDGE_TEST_LOCK.lock();
    let bridge = FakeBridge::new();
    set_vm_bridge_for_test(Some(bridge.clone()));
    let out = f(bridge, make_ctx()).await;
    set_vm_bridge_for_test(None);
    out
}

#[tokio::test]
async fn unregistered_bridge_is_honest_error() {
    let _serial = VM_BRIDGE_TEST_LOCK.lock();
    set_vm_bridge_for_test(None);
    let h = VmHandler;
    let err = h
        .handle_cmd("overview", None, &make_ctx())
        .await
        .unwrap_err();
    assert!(err.contains("未装配"), "{err}");
}

#[tokio::test]
async fn all_commands_route_through_bridge() {
    with_bridge(|_b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        let names = h.commands();
        assert_eq!(names.len(), 13, "命令族清单（实施计划锁定 + B3 自举族）");
        assert_eq!(h.module_name(), "vm");

        for cmd in names {
            let data = match *cmd {
                "config.set" => Some(serde_json::json!({ "vm": { "enabled": true } })),
                "instance.sleep" | "instance.wake" | "instance.kill" => {
                    Some(serde_json::json!({ "agent_id": "a1" }))
                }
                _ => None,
            };
            let res = h.handle_cmd(cmd, data, &ctx).await;
            assert!(res.is_ok(), "{cmd} 必须经 bridge 成功: {res:?}");
        }

        // 每条命令真的到了 bridge（配置臂以外全部录制在案）。
        let calls = _b.calls.lock().unwrap();
        for expected in [
            "overview",
            "config.get",
            "config.set",
            "image.list",
            "image.build",
            "instance.list",
            "instance.sleep",
            "instance.wake",
            "instance.kill",
            "gc.run",
            "bootstrap.status",
            "bootstrap.install_qemu",
            "bootstrap.enable_accel",
        ] {
            assert!(calls.iter().any(|c| c == expected), "{expected} 未被调用");
        }
    })
    .await;
}

#[tokio::test]
async fn image_build_update_flag_forwarded() {
    with_bridge(|b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        h.handle_cmd(
            "image.build",
            Some(serde_json::json!({ "update": true })),
            &ctx,
        )
        .await
        .expect("build");
        assert!(
            b.calls
                .lock()
                .unwrap()
                .contains(&"image.build:update".to_string())
        );
        // 缺省 = 非更新（首次构建）。
        h.handle_cmd("image.build", None, &ctx)
            .await
            .expect("build2");
        assert!(b.calls.lock().unwrap().contains(&"image.build".to_string()));
    })
    .await;
}

#[tokio::test]
async fn config_set_missing_vm_object_rejected() {
    with_bridge(|_b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        let err = h
            .handle_cmd("config.set", Some(serde_json::json!({})), &ctx)
            .await
            .unwrap_err();
        assert!(err.contains("vm"), "{err}");
        // bridge 侧校验失败同样上抛（typed validate 在实现侧）。
        let err = h
            .handle_cmd(
                "config.set",
                Some(serde_json::json!({ "vm": { "hypervisor": "kvm" } })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("enabled"), "{err}");
    })
    .await;
}

#[tokio::test]
async fn instance_ops_require_agent_id() {
    with_bridge(|_b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        for cmd in ["instance.sleep", "instance.wake", "instance.kill"] {
            let err = h.handle_cmd(cmd, None, &ctx).await.unwrap_err();
            assert!(err.contains("missing data"), "{cmd}: {err}");
        }
    })
    .await;
}
