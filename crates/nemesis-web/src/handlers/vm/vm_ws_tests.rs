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
    /// B8c：try.exec 收到的 timeout 透传值（None=缺省臂）。
    try_exec_timeouts: Mutex<Vec<Option<u64>>>,
}

impl FakeBridge {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::new(Vec::new()),
            try_exec_timeouts: Mutex::new(Vec::new()),
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
    fn try_status(&self) -> serde_json::Value {
        self.note("try.status");
        serde_json::json!({ "agent_id": "try-dashboard", "enabled": false })
    }
    async fn try_start(&self) -> Result<serde_json::Value, String> {
        self.note("try.start");
        Ok(serde_json::json!({ "running": true }))
    }
    async fn try_exec(
        &self,
        command: &str,
        timeout_secs: Option<u64>,
    ) -> Result<serde_json::Value, String> {
        self.note("try.exec");
        self.try_exec_timeouts.lock().unwrap().push(timeout_secs);
        Ok(serde_json::json!({ "command": command, "output": "ok" }))
    }
    async fn try_stop(&self) -> Result<serde_json::Value, String> {
        self.note("try.stop");
        Ok(serde_json::json!({ "running": false }))
    }
    async fn sync_status(&self, project: &str) -> Result<serde_json::Value, String> {
        self.note("sync.status");
        Ok(serde_json::json!({ "project_id": project, "has_baseline": false }))
    }
    async fn sync_commit(&self, project: &str) -> Result<serde_json::Value, String> {
        self.note("commit");
        Ok(serde_json::json!({ "project_id": project, "changed": [], "deleted": [] }))
    }
    async fn sync_resync(&self, project: &str) -> Result<serde_json::Value, String> {
        self.note("resync");
        Ok(serde_json::json!({ "project_id": project, "files": 0 }))
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
        assert_eq!(
            names.len(),
            20,
            "命令族清单（实施计划锁定 + B3 自举族 + B6 试玩族 + B9-3 同步族）"
        );
        assert_eq!(h.module_name(), "vm");

        for cmd in names {
            let data = match *cmd {
                "config.set" => Some(serde_json::json!({ "vm": { "enabled": true } })),
                "instance.sleep" | "instance.wake" | "instance.kill" => {
                    Some(serde_json::json!({ "agent_id": "a1" }))
                }
                "try.exec" => Some(serde_json::json!({ "command": "uname -a" })),
                "sync.status" | "commit" | "resync" => Some(serde_json::json!({ "project": "p1" })),
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
            "try.status",
            "try.start",
            "try.exec",
            "try.stop",
            "sync.status",
            "commit",
            "resync",
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

// ---------------------------------------------------------------------------
// B6 快速试玩族
// ---------------------------------------------------------------------------

/// estop 触发态 ctx（make_ctx 基础上挂 engaged 的 EstopState——与 estop
/// WSAPI / 托盘 / CLI 四入口同源状态）。
fn make_estopped_ctx() -> RequestContext {
    let mut ctx = make_ctx();
    let estop = std::sync::Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    // ctx.state 刚构造（引用计数 1）——get_mut 安全。
    Arc::get_mut(&mut ctx.state).expect("fresh ctx state").estop = Some(estop);
    ctx
}

#[tokio::test]
async fn try_commands_route_through_bridge() {
    with_bridge(|b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        // status 只读探针。
        let res = h
            .handle_cmd("try.status", None, &ctx)
            .await
            .expect("status");
        assert_eq!(res.unwrap()["agent_id"], "try-dashboard");
        // start / stop 无参。
        h.handle_cmd("try.start", None, &ctx).await.expect("start");
        h.handle_cmd("try.stop", None, &ctx).await.expect("stop");
        // exec 转发命令原文。
        let res = h
            .handle_cmd(
                "try.exec",
                Some(serde_json::json!({ "command": "  uname -a  " })),
                &ctx,
            )
            .await
            .expect("exec");
        assert_eq!(res.unwrap()["command"], "uname -a", "命令 trim 后转发");
        let calls = b.calls.lock().unwrap();
        for expected in ["try.status", "try.start", "try.stop", "try.exec"] {
            assert!(calls.iter().any(|c| c == expected), "{expected} 未被调用");
        }
    })
    .await;
}

#[tokio::test]
async fn try_start_exec_refused_when_estop_engaged() {
    with_bridge(|b: Arc<FakeBridge>, _ctx| async move {
        let h = VmHandler;
        let ctx = make_estopped_ctx();
        let err = h.handle_cmd("try.start", None, &ctx).await.unwrap_err();
        assert!(err.contains("急停"), "start 必须被 estop 冻结: {err}");
        let err = h
            .handle_cmd(
                "try.exec",
                Some(serde_json::json!({ "command": "uname -a" })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("急停"), "exec 必须被 estop 冻结: {err}");
        // stop = 停机方向（安全方向）——estop 中放行。
        h.handle_cmd("try.stop", None, &ctx)
            .await
            .expect("stop 在急停中必须放行");
        // status 只读恒放行。
        h.handle_cmd("try.status", None, &ctx)
            .await
            .expect("status 在急停中必须放行");
        // bridge 只收到放行的两条。
        let calls = b.calls.lock().unwrap();
        assert!(!calls.iter().any(|c| c == "try.start"));
        assert!(!calls.iter().any(|c| c == "try.exec"));
        assert!(calls.iter().any(|c| c == "try.stop"));
        assert!(calls.iter().any(|c| c == "try.status"));
    })
    .await;
}

#[tokio::test]
async fn try_exec_input_validation() {
    with_bridge(|_b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        // 缺 data / 缺 command。
        let err = h.handle_cmd("try.exec", None, &ctx).await.unwrap_err();
        assert!(err.contains("missing data"), "{err}");
        // 空命令 / 纯空白拒绝。
        let err = h
            .handle_cmd(
                "try.exec",
                Some(serde_json::json!({ "command": "   " })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("命令为空"), "{err}");
        // 超长命令拒绝（试玩通道不承载长脚本）。
        let long = "x".repeat(2001);
        let err = h
            .handle_cmd(
                "try.exec",
                Some(serde_json::json!({ "command": long })),
                &ctx,
            )
            .await
            .unwrap_err();
        assert!(err.contains("过长"), "{err}");

        // B8c：timeout 透传——缺省 None / 显式值透传 / 超上限钳 600。
        h.handle_cmd(
            "try.exec",
            Some(serde_json::json!({ "command": "uname -a" })),
            &ctx,
        )
        .await
        .unwrap();
        h.handle_cmd(
            "try.exec",
            Some(serde_json::json!({ "command": "uname -a", "timeout": 300 })),
            &ctx,
        )
        .await
        .unwrap();
        h.handle_cmd(
            "try.exec",
            Some(serde_json::json!({ "command": "uname -a", "timeout": 99_999 })),
            &ctx,
        )
        .await
        .unwrap();
        h.handle_cmd(
            "try.exec",
            Some(serde_json::json!({ "command": "uname -a", "timeout": 0 })),
            &ctx,
        )
        .await
        .unwrap();
        let got = _b.try_exec_timeouts.lock().unwrap().clone();
        assert_eq!(
            got,
            vec![None, Some(300), Some(600), None],
            "timeout 透传/钳制/0=缺省"
        );
    })
    .await;
}

// ---------------------------------------------------------------------------
// B9-3 项目工作副本同步族
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_commands_route_project_through_bridge() {
    with_bridge(|b: Arc<FakeBridge>, ctx| async move {
        let h = VmHandler;
        // 三条命令都要求 project（缺参诚实报错，不打 bridge）。
        for cmd in ["sync.status", "commit", "resync"] {
            let err = h.handle_cmd(cmd, None, &ctx).await.unwrap_err();
            assert!(
                err.contains("missing data") || err.contains("project"),
                "{cmd}: {err}"
            );
        }
        // project 原文转发。
        for cmd in ["sync.status", "commit", "resync"] {
            let res = h
                .handle_cmd(cmd, Some(serde_json::json!({ "project": "NB-1" })), &ctx)
                .await
                .expect(cmd);
            assert_eq!(res.unwrap()["project_id"], "NB-1", "{cmd} project 未转发");
        }
        let calls = b.calls.lock().unwrap();
        for expected in ["sync.status", "commit", "resync"] {
            assert!(calls.iter().any(|c| c == expected), "{expected} 未被调用");
        }
    })
    .await;
}

#[tokio::test]
async fn sync_commit_resync_refused_when_estop_engaged_status_allowed() {
    with_bridge(|b: Arc<FakeBridge>, _ctx| async move {
        let h = VmHandler;
        let ctx = make_estopped_ctx();
        let data = Some(serde_json::json!({ "project": "p1" }));
        // commit / resync 是 agent 活动形态（写宿主 + 拽 VM）——急停冻结。
        for cmd in ["commit", "resync"] {
            let err = h.handle_cmd(cmd, data.clone(), &ctx).await.unwrap_err();
            assert!(err.contains("急停"), "{cmd} 必须被 estop 冻结: {err}");
        }
        // status 只读恒放行。
        h.handle_cmd("sync.status", data, &ctx)
            .await
            .expect("status 在急停中必须放行");
        let calls = b.calls.lock().unwrap();
        assert!(!calls.iter().any(|c| c == "commit"));
        assert!(!calls.iter().any(|c| c == "resync"));
        assert!(calls.iter().any(|c| c == "sync.status"));
    })
    .await;
}
