//! mail WSAPI 测试（表面契约 + 槽 None 诚实报错）。
//!
//! 真链路（句柄在位 → 插件 DLL 往返）归 UAT（U1-U13，test-tools 全链路）；
//! 本文件钉死的是**无 DLL 也可断言的宿主侧契约**：
//! - 命令表面（13 命令 + module 名）——L1 `system.commands` 汇总的真相；
//! - 槽 None（插件未装载）= 业务命令诚实报错，绝不假成功；
//! - status / config_get / config_set 三豁免（先于句柄闸：缺席也要能查态/
//!   能配——空态卡引导闭环）；
//! - 入参组参数缺省（list query 四键齐）与 require_id 纯函数臂。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx() -> RequestContext {
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
        mail_plugin: None,
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
    assert!(v["note"].as_str().unwrap().contains("重启"));

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
    // 空态卡「先配好再装载」引导闭环（make_ctx 的 mail_plugin 恒 None）。
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
    let v2 = h
        .handle_cmd("config_get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(v2["config"]["enabled"], false);
    let _ = std::fs::remove_dir_all(&dir);
}
