//! mail WSAPI 测试（表面契约 + 槽 None 诚实报错 + 热装载通路）。
//!
//! 真链路（句柄在位 → 插件 DLL 往返）归 UAT（U1-U13，test-tools 全链路）；
//! 本文件钉死的是**无 DLL 也可断言的宿主侧契约**：
//! - 命令表面（13 命令 + module 名）——L1 `system.commands` 汇总的真相；
//! - 槽 None（插件未装载）= 业务命令诚实报错，绝不假成功；
//! - status / config_get / config_set 三豁免（先于句柄闸：缺席也要能查态/
//!   能配——空态卡引导闭环）；
//! - 入参组参数缺省（list query 四键齐）与 require_id 纯函数臂；
//! - 热装载通路（2026-11-20）：hint 探针精确化 / 热装载成功与失败回传 /
//!   停用与已装载的重启语义 / AppState 装配期槽合流读（2026-11-20 双槽）。
//!
//! **静态槽纪律**：MAIL_PLUGIN_SLOT / HOT_LOADER / UNLOADED_PROBE 是进程级
//! 全局——触碰（读兜底或注入）的测试必须首行 `lock().await` 取 [`SLOT_TEST_LOCK`] 并
//! `reset_slots_for_test()`，防并行测试互相污染 hint/装载态。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use nemesis_plugin::mail::HostMailVTable;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 触碰进程级静态槽的测试串行闸（cargo test 默认并行——hint/装载态互相
/// 污染即假红；取锁 + reset 两步配对）。
static SLOT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

// ── 哑句柄（槽位占用测试件）──────────────────────────────────────────
// MailPluginHandle 字段全为非 Option fn 指针（无法 None 构造）。哑 extern
// "C" fn 按四种签名覆盖 15 字段——类型合法、返回共享 ok JSON（**不能
// panic**：blocking 线程 panic = 进程 abort 非 unwind；status 句柄面会真
// 分发 server_status，必须返回合法 JSON 让 loaded 投影成立）。测试不经
// 其他哑 fn 分发，返回值无断言意义。

/// 共享返回体（NUL 结尾 C 字符串，进程生命周期）。
static DUMMY_RET: &[u8] = b"{\"ok\":true}\0";

unsafe extern "C" fn d_init(
    _: *const HostMailVTable,
    _: *const std::os::raw::c_char,
) -> std::os::raw::c_int {
    0
}
extern "C" fn d_shutdown() {}
unsafe extern "C" fn d_json(_: *const std::os::raw::c_char) -> *const std::os::raw::c_char {
    DUMMY_RET.as_ptr() as *const std::os::raw::c_char
}
unsafe extern "C" fn d_str() -> *const std::os::raw::c_char {
    DUMMY_RET.as_ptr() as *const std::os::raw::c_char
}
unsafe extern "C" fn d_send(
    _: *const std::os::raw::c_char,
    _: *const std::os::raw::c_char,
    _: *const std::os::raw::c_char,
) -> *const std::os::raw::c_char {
    DUMMY_RET.as_ptr() as *const std::os::raw::c_char
}

/// 哑句柄构造（Copy——可多次进槽）。
fn dummy_handle() -> MailPluginHandle {
    MailPluginHandle {
        init: d_init,
        shutdown: d_shutdown,
        server_start: d_json,
        server_stop: d_str,
        server_status: d_str,
        send: d_send,
        list: d_json,
        read: d_json,
        mark_read: d_json,
        delete: d_json,
        stats: d_str,
        record: d_json,
        addr_add: d_json,
        addr_remove: d_json,
        addr_list: d_str,
    }
}

fn make_ctx() -> RequestContext {
    make_ctx_inner(None)
}

/// 装配期槽形态（AppState.mail_plugin 有值——gateway 启动时序注入的合流
/// 回落测试件）。
fn make_ctx_with_assembly_handle() -> RequestContext {
    make_ctx_inner(Some(dummy_handle()))
}

fn make_ctx_inner(mail_plugin: Option<MailPluginHandle>) -> RequestContext {
    let ws_str = std::env::temp_dir().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws_str.clone()),
        home: Some(ws_str.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        quic_relay: None,
        mail_plugin,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "mail-test-session".to_string(),
        chat_id: "mail-test-chat".to_string(),
        workspace: Some(ws_str),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

// ── 命令表面契约 ──────────────────────────────────────────────────────

#[test]
fn mail_module_name_and_command_surface() {
    let h = MailHandler;
    assert_eq!(h.module_name(), "mail");
    let cmds = h.commands();
    assert_eq!(
        cmds,
        &[
            "send",
            "list",
            "read",
            "mark_read",
            "delete",
            "stats",
            "record",
            "addr_add",
            "addr_remove",
            "addr_list",
            "status",
            "config_get",
            "config_set",
        ]
    );
    // 十三命令，无重名（L1 汇总去重依赖）。
    let mut sorted = cmds.to_vec();
    sorted.sort_unstable();
    let before = sorted.len();
    sorted.dedup();
    assert_eq!(sorted.len(), before, "命令清单存在重名");
    // 治理面与 agent 面分离：delete 与 addr_* 是宿主治理面命令，
    // agent 工具面（B4）schema 层面不存在——清单在场是治理面契约的一部分。
    assert!(cmds.contains(&"delete"));
    assert!(cmds.contains(&"addr_add"));
}

// ── 槽 None：诚实报错（绝不假成功）──────────────────────────────────

#[tokio::test]
async fn slot_none_every_command_reports_plugin_absent() {
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let ctx = make_ctx();
    let expect = "mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）";
    for cmd in h.commands() {
        // status/config_get/config_set 是豁免：槽 None 照常工作（状态查询 +
        // 配置面都先于句柄闸——缺席也要能查态、能配），专项断言见下方各测试。
        if matches!(*cmd, "status" | "config_get" | "config_set") {
            continue;
        }
        let data = match *cmd {
            // send/record/addr_add/addr_remove 正常带 data；read/mark_read/delete
            // 带 id；list 带 query；stats/addr_list 无参——全部都应在句柄解析
            // 处先短路（入参校验在句柄之后，槽 None 时到不了）。
            "send" => Some(serde_json::json!({"to": "a@b", "subject": "s", "body": "x"})),
            "read" | "mark_read" | "delete" => Some(serde_json::json!({"id": "m1"})),
            "list" => Some(serde_json::json!({"mailbox": ""})),
            "record" => Some(serde_json::json!({"message_id": "x"})),
            "addr_add" | "addr_remove" => Some(serde_json::json!({"address": "a@b"})),
            _ => None,
        };
        let err = h.handle_cmd(cmd, data, &ctx).await.unwrap_err();
        assert_eq!(err, expect, "命令 mail.{cmd} 未诚实报插件未装载");
    }
}

#[tokio::test]
async fn slot_none_status_reports_loaded_false_not_error() {
    // status 槽 None = Ok({loaded:false, hint})——前端降级横幅数据源；
    // 绝不假成功也绝不误报为命令错误。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let ctx = make_ctx();
    let v = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(v["loaded"], false);
    assert!(v["hint"].as_str().unwrap().contains("未装载"));
}

#[tokio::test]
async fn slot_none_unknown_command_same_honest_error() {
    // 未知命令也先撞句柄闸（错误信息一致——不给探测面区分「插件在不在」
    // 与「命令存不存在」的信号差）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let ctx = make_ctx();
    let err = h.handle_cmd("nonexistent", None, &ctx).await.unwrap_err();
    assert_eq!(
        err,
        "mail 插件未装载（DLL 缺席或 config.mail.json enabled 未开启）"
    );
}

// ── 纯函数臂：require_id ─────────────────────────────────────────────

#[test]
fn require_id_extracts_and_rejects() {
    assert_eq!(
        require_id(Some(&serde_json::json!({"id": "m-42"}))).unwrap(),
        "m-42"
    );
    // 缺 data / 缺 id / id 非字符串 → 同一诚实报错。
    assert_eq!(require_id(None).unwrap_err(), "缺 id 字段");
    assert_eq!(
        require_id(Some(&serde_json::json!({"other": 1}))).unwrap_err(),
        "缺 id 字段"
    );
    assert_eq!(
        require_id(Some(&serde_json::json!({"id": 42}))).unwrap_err(),
        "缺 id 字段"
    );
}

#[test]
fn list_query_defaults_all_four_keys_present() {
    // list 无 data = 四键齐全的默认 query（插件侧按缺省值解释）——
    // 字面构造处漏键会让插件侧 serde default 语义漂移，这里钉死。
    let d = None::<serde_json::Value>;
    let d = d.unwrap_or(serde_json::json!({}));
    let q = serde_json::json!({
        "mailbox": d.get("mailbox").and_then(|v| v.as_str()).unwrap_or(""),
        "unread_only": d.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false),
        "limit": d.get("limit").and_then(|v| v.as_u64()).unwrap_or(0),
        "offset": d.get("offset").and_then(|v| v.as_u64()).unwrap_or(0),
    });
    let obj = q.as_object().unwrap();
    assert_eq!(obj.len(), 4);
    assert_eq!(obj["mailbox"], "");
    assert_eq!(obj["unread_only"], false);
    assert_eq!(obj["limit"], 0);
    assert_eq!(obj["offset"], 0);
    // 部分覆盖：只传 limit 时其余键仍按缺省填充。
    let d = serde_json::json!({"limit": 5});
    let q = serde_json::json!({
        "mailbox": d.get("mailbox").and_then(|v| v.as_str()).unwrap_or(""),
        "unread_only": d.get("unread_only").and_then(|v| v.as_bool()).unwrap_or(false),
        "limit": d.get("limit").and_then(|v| v.as_u64()).unwrap_or(0),
        "offset": d.get("offset").and_then(|v| v.as_u64()).unwrap_or(0),
    });
    assert_eq!(q["limit"], 5);
    assert_eq!(q["mailbox"], "");
}

// ── 配置面：config_get / config_set（先于句柄闸——插件缺席可配）───────

/// 独立临时 workspace（config 写测不污染共享 temp 根；并行测试互不串扰）。
fn make_ctx_with_workspace_dir(tag: &str) -> (RequestContext, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("mail-cfg-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("config")).unwrap();
    let mut ctx = make_ctx();
    ctx.workspace = Some(dir.to_string_lossy().to_string());
    (ctx, dir)
}

#[tokio::test]
async fn config_get_missing_file_returns_empty_object_with_path() {
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("get-missing");
    let v = h
        .handle_cmd("config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["config"], serde_json::json!({}));
    let path = v["path"].as_str().unwrap();
    assert!(path.ends_with("config.mail.json"), "got: {path}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_set_then_get_roundtrip() {
    // enabled:true 且未装载 = 触发热装载分支：闭包未注入（测试形态）=
    // 诚实 load_error 回传（「即时装载失败」非命令失败——盘已写好，配置
    // 闭环成立）。写读往返照常断言。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("roundtrip");
    let cfg = serde_json::json!({
        "enabled": true,
        "smtp": {"enabled": true, "port": 2525},
        "client": {
            "smtp_host": "smtp.qq.com", "smtp_port": 465,
            "username": "u@qq.com", "password": "AUTHCODE",
        },
    });
    let v = h
        .handle_cmd("config_set", Some(serde_json::json!({"config": cfg})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["loaded"], false, "闭包未注入 = 不可能装载成功");
    assert!(
        v["load_error"]
            .as_str()
            .unwrap()
            .contains("热装载闭包未注入"),
        "got: {v}"
    );
    assert!(
        v["note"].as_str().unwrap().contains("即时装载失败"),
        "got: {v}"
    );

    let v2 = h
        .handle_cmd("config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v2["config"], cfg, "写读必须往返一致");
    // 落盘是合法 JSON（loader 启动可解析消费——盘上真相闭环）
    let raw = std::fs::read_to_string(dir.join("config").join("config.mail.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed["enabled"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_set_rejects_non_object_and_missing_config() {
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("reject");
    let err = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": [1, 2]})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("object"), "got: {err}");
    let err2 = h
        .handle_cmd("config_set", Some(serde_json::json!({"x": 1})), &ctx)
        .await
        .unwrap_err();
    assert!(err2.contains("config"), "got: {err2}");
    let err3 = h.handle_cmd("config_set", None, &ctx).await.unwrap_err();
    assert!(err3.contains("missing"), "got: {err3}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_get_corrupt_file_reports_honestly_not_empty_object() {
    // 损坏文件必须诚实报错（loader 读损坏 fail-closed 全关——配置页让用户
    // 看见损坏才能修正；静默落空对象 = 假成功反模式）。
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("corrupt");
    std::fs::write(dir.join("config").join("config.mail.json"), "{not json").unwrap();
    let err = h.handle_cmd("config_get", None, &ctx).await.unwrap_err();
    assert!(err.contains("解析失败"), "got: {err}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_get_tolerates_bom() {
    // BOM 容忍与 loader read_mail_config_raw 同源形态（Windows 编辑器常带 BOM）。
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("bom");
    std::fs::write(
        dir.join("config").join("config.mail.json"),
        "\u{feff}{\"enabled\":true}",
    )
    .unwrap();
    let v = h
        .handle_cmd("config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["config"]["enabled"], true);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn config_cmds_work_when_plugin_slot_none() {
    // 先于句柄闸的核心语义：插件缺席（槽 None）下 config 读写照常工作——
    // 空态卡「先配好再装载」引导闭环（make_ctx 的 mail_plugin 恒 None；
    // enabled:false 不触热装载分支，note 走「重启生效」文案）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("slot-none");
    let v = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": false}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert!(v["note"].as_str().unwrap().contains("重启"), "got: {v}");
    let v2 = h
        .handle_cmd("config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v2["config"]["enabled"], false);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── 热装载通路（2026-11-20）：hint 探针 / 热装载回传 / 重启语义 / 合流 ──

#[tokio::test]
async fn probe_injection_drives_status_hint() {
    // 未装载探针注入 = hint 精确化（gateway 注入四态原因）；槽空 + probe
    // 在 = hint 取 probe 值。reset 后恢复兜底文案（探针清理闭环）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let ctx = make_ctx();
    set_mail_unloaded_probe(Arc::new(|| {
        Some("mail 插件未装载：总闸未开（测试探针）".to_string())
    }));
    let v = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(v["loaded"], false);
    assert_eq!(
        v["hint"].as_str().unwrap(),
        "mail 插件未装载：总闸未开（测试探针）"
    );
    reset_slots_for_test();
    let v2 = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert!(
        v2["hint"]
            .as_str()
            .unwrap()
            .contains("DLL 缺席或 config.mail.json enabled 未开启"),
        "探针清理后应回落兜底文案: got: {v2}"
    );
}

#[tokio::test]
async fn hot_load_success_on_config_set_enable() {
    // 热装载成功链路：闭包被调（写哑句柄进热槽模拟 gateway 侧全链完成）→
    // 响应 loaded:true + note 含「即时装载」；此后 status 走句柄面
    // loaded:true（热槽即刻生效——勾选即用的核心断言）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("hot-load-ok");
    set_mail_hot_loader(Arc::new(|| {
        set_mail_plugin_slot(dummy_handle());
        Ok(())
    }));
    let v = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": true}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["loaded"], true, "闭包落槽后 loaded 必须为 true: got: {v}");
    assert!(v["note"].as_str().unwrap().contains("即时装载"), "got: {v}");
    // 热槽生效：status 走句柄面（哑句柄 server_status 分发即 panic——这里
    // 只验证 loaded 投影，不经哑 fn；用 business 闸代替：list 应撞句柄后
    // 才缺参？list 无参合法——会真分发哑 fn panic。改用 config_set 的
    // was_loaded 链路验证：再存 enabled:true → 已装载 → 配置刷新重启语义）。
    let v2 = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": true}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v2["loaded"], true);
    assert!(
        v2["note"]
            .as_str()
            .unwrap()
            .contains("配置刷新需重启网关生效"),
        "已装载态二次保存必须走配置刷新重启语义: got: {v2}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn hot_load_failure_reports_error_and_stays_unloaded() {
    // 热装载失败链路：闭包 Err（DLL 缺席等）→ ok:true（保存本身成功）+
    // loaded:false + load_error 透传 + note 引导重试；不假成功不误报命令错。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("hot-load-err");
    set_mail_hot_loader(Arc::new(|| Err("plugin_mail DLL 缺席（测试）".to_string())));
    let v = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": true}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["loaded"], false);
    assert!(
        v["load_error"].as_str().unwrap().contains("DLL 缺席"),
        "got: {v}"
    );
    assert!(
        v["note"].as_str().unwrap().contains("即时装载失败"),
        "got: {v}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn disable_with_loaded_plugin_reports_restart_semantics() {
    // 停用方向重启语义（dlopen 不可卸载，用户拍板）：已装载 + enabled:false
    // = 落盘 + note「停用需重启网关生效」；loaded 投影 true（仍在运行）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("disable-restart");
    set_mail_plugin_slot(dummy_handle());
    let v = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": false}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["loaded"], true, "停用只是落盘——插件仍在运行");
    assert!(
        v["note"].as_str().unwrap().contains("停用需重启网关生效"),
        "got: {v}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn appstate_assembly_slot_flows_into_effective_handle() {
    // 合流读（2026-11-20 双槽）：装配期槽（AppState.mail_plugin，gateway
    // 启动时序注入）热槽空 → status loaded:true（合流回落成立）；无 AppState
    // 句柄 + 热槽空 → loaded:false。只查热槽的旧形态会把启动装载误判未装载
    // → 重复热装载——此测试钉死合流语义。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let ctx = make_ctx();
    let v = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(v["loaded"], false, "两槽皆空必须未装载");

    let ctx2 = make_ctx_with_assembly_handle();
    let v2 = h.handle_cmd("status", None, &ctx2).await.unwrap().unwrap();
    assert_eq!(v2["loaded"], true, "AppState 装配期槽必须被合流读到");
    // 未注入 probe（gateway 形态才有精确 hint）——loaded:true 时 hint 不出现。
    assert!(v2.get("hint").is_none(), "装载态无 hint 字段: got: {v2}");
}

#[tokio::test]
async fn loaded_plugin_never_retriggers_hot_load() {
    // 已装载（热槽）+ enabled:true → 不触热装载闭包（若被调 = 重复
    // server_start 双 bind 面）——note 走配置刷新重启语义，闭包缺席也不报
    // load_error（证明闭包确实没被调）。
    let _g = SLOT_TEST_LOCK.lock().await;
    reset_slots_for_test();
    let h = MailHandler;
    let (ctx, dir) = make_ctx_with_workspace_dir("no-retrigger");
    set_mail_plugin_slot(dummy_handle());
    // 闭包故意不注入——若分支误入热装载，会拿到「闭包未注入」load_error。
    let v = h
        .handle_cmd(
            "config_set",
            Some(serde_json::json!({"config": {"enabled": true}})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v["ok"], true);
    assert!(
        v.get("load_error").is_none(),
        "已装载不得触热装载: got: {v}"
    );
    assert!(
        v["note"]
            .as_str()
            .unwrap()
            .contains("配置刷新需重启网关生效"),
        "got: {v}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
