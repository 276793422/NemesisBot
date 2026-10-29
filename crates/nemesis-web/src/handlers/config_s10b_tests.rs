//! S10b (quality-hardening goal 冲刺, web 批次 2): ConfigHandler error arms and
//! CORS stubs the gated `mod tests` skips — missing config.json load failure,
//! invalid save payload, empty-path / invalid-result set_field, and the four
//! "CORS manager not connected" stub commands.

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::config::ConfigHandler;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state(ws: Option<String>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: ws.clone(),
        home: ws.clone(),
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
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    })
}

fn make_ctx(dir: &tempfile::TempDir, with_config: bool) -> RequestContext {
    if with_config {
        let config = nemesis_config::Config::default();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state: make_state(Some(dir.path().to_string_lossy().to_string())),
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn config_get_with_malformed_config_file_errors() {
    // Missing config.json falls back to defaults (Ok) — a present-but-broken
    // file is what trips the load error arm.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{definitely not json").unwrap();
    let ctx = make_ctx(&dir, false);
    let handler = ConfigHandler::new();
    let err = handler.handle_cmd("get", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to load config"), "{err}");
}

#[tokio::test]
async fn config_save_rejects_wrong_typed_payload() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd(
            "save",
            Some(serde_json::json!({ "model_list": "not-an-array" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config data"), "{err}");
}

#[tokio::test]
async fn config_set_field_empty_path_and_invalid_result_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let err = handler
        .handle_cmd("set_field", Some(serde_json::json!({ "path": "" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "empty path");

    // model_list must stay an array — a string breaks re-parse.
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list", "value": "oops" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config after field update"), "{err}");
}

#[tokio::test]
async fn config_cors_stubs_report_not_connected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let list = handler
        .handle_cmd("cors.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(list["origins"].as_array().unwrap().len(), 0);
    assert!(list["message"].as_str().unwrap().contains("not connected"));

    let added = handler
        .handle_cmd(
            "cors.add",
            Some(serde_json::json!({ "origin": "https://x.com" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(added["added"], false);

    let removed = handler
        .handle_cmd(
            "cors.remove",
            Some(serde_json::json!({ "origin": "https://x.com" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removed["removed"], false);

    let toggled = handler
        .handle_cmd(
            "cors.toggle",
            Some(serde_json::json!({ "enabled": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(toggled["toggled"], false);
}

// ---------------------------------------------------------------------------
// G6：agents.tool_doc_folding 开关 —— set_field 可写（typed round-trip），
// config.get 回读，盘上真实落盘；错型值 loud 拒绝。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_tool_doc_folding_toggle_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    // 默认关。
    let get = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(get["agents"]["tool_doc_folding"]["enabled"], false);

    // 开 → updated + get 回读 true。
    let out = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);
    let get = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(get["agents"]["tool_doc_folding"]["enabled"], true);

    // 盘上真实落盘（typed 序列化，不是只在内存）。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], true);
    // expand_top_n 保持 serde 默认 8。
    assert_eq!(raw["agents"]["tool_doc_folding"]["expand_top_n"], 8);

    // 关回去。
    handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": false })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
}

#[tokio::test]
async fn config_tool_doc_folding_rejects_wrong_typed_value() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    // enabled 必须是 bool —— 字符串 "on" 走 typed 反序列化失败，loud 拒绝。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled", "value": "on" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config after field update"), "{err}");

    // 拒绝后盘上仍是 false（默认值），没有被半写。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
}

// R1 真机验收发现：未知键曾走 generic set_field 静默丢弃（serde 忽略未知字段）
// 却谎报 updated:true —— 现按 G6「未知键必须 loud 拒绝」要求 round-trip 校验。
#[tokio::test]
async fn config_set_field_unknown_key_loud_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.nonexistent_key_xyz", "value": 1 })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown config field"), "{err}");

    // 盘上没有被半写（不存在中间对象被静默创建）。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert!(raw["agents"].get("nonexistent_key_xyz").is_none());

    // 深层未知路径同样拒绝（set_json_path 曾自动创建中间对象）。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.no_such_section.deeper", "value": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("unknown config field"), "{err}");
}

// BUG-20261029-174：set_field 路径中间/末端父节点是 scalar（bool/string/
// number）时，serde_json IndexMut 会 panic（cannot access key in JSON …）
// ——WS handler 任务崩，Dashboard attr 编辑器输错路径即可触发。修复 =
// set_json_path 显式 scalar 检查，loud Err 替代 panic。
#[tokio::test]
async fn config_set_field_scalar_parent_node_loud_rejects_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    // 末端父节点是 scalar：agents.tool_doc_folding.enabled 是 bool，
    // 路径多伸一层 → 修复前 panic，修复后 loud Err。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled.x", "value": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("parent node is not an object"), "{err}");

    // 中间节点是 scalar：再深一层，修复前在中间段 panic，修复后同 Err。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "agents.tool_doc_folding.enabled.x.y", "value": 1 })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("parent node is not an object"), "{err}");

    // 盘上没有被半写。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
}

// ---------------------------------------------------------------------------
// 数组下标路径（model_list.<i>.<field>）：set_json_path / json_path_get 必须
// 对称支持数字段（此前 get 对数组数字段返回 None → round-trip 误报
// "unknown config field"，set 本身也是静默 no-op）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_set_field_supports_array_index_paths() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    // 播种一个带条目的 model_list（Config 全字段 serde(default)，最小 JSON 可解析）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({ "model_list": [ { "model_name": "test/m1" } ] }).to_string(),
    )
    .unwrap();
    let handler = ConfigHandler::new();

    // 数组下标路径可写且 round-trip 通过（不再误报 unknown config field）。
    let out = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.0.model_name", "value": "test/m2" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["updated"], true);

    // get 回读 + 盘上真实落盘。
    let get = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(get["model_list"][0]["model_name"], "test/m2");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["model_list"][0]["model_name"], "test/m2");

    // 越界下标 loud 报错（无 append 语义），盘上没有被半写。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.5.model_name", "value": "x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("array index out of bounds"), "{err}");

    // 数组上的非数字段 loud 报错（不是静默 no-op）。
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list.zero", "value": "x" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not a valid array index"), "{err}");
}

// ---------------------------------------------------------------------------
// 闭包事务入口级并发回归钉（2026-10-13 RMW 收口三期）：锁在 update_* 入口内
// 取得，两个并发事务改**不同字段**时两个变更都必须存活——旧「调用点手锁」
// 形态的丢更新类（各自读到旧底、后写整体覆盖先写）在此结构上不可能复现。
// 一 typed 一 raw 两个入口混用，同钉「同族共用同一把进程级写串行锁」。
// ---------------------------------------------------------------------------

#[test]
fn config_update_entries_serialized_both_mutations_survive() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_string_lossy().to_string();
    // 最小可解析 config.json（Config 全字段 serde(default)）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({ "model_list": [ { "model_name": "seed" } ] }).to_string(),
    )
    .unwrap();

    let home_raw = home.clone();
    let home_typed = home.clone();
    std::thread::scope(|s| {
        let ra = s.spawn(move || {
            super::config::update_raw_config(&home_raw, |cfg| {
                cfg["agents"]["defaults"]["llm"] = serde_json::json!("from-raw");
                Ok(())
            })
        });
        let rb = s.spawn(move || {
            super::config::update_config(&home_typed, |cfg| {
                cfg.agents.tool_doc_folding.enabled = true;
                Ok(())
            })
        });
        ra.join().unwrap().expect("raw tx ok");
        rb.join().unwrap().expect("typed tx ok");
    });

    // 盘上两个变更都在（任一整体覆盖都会抹掉另一个的字段）。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["defaults"]["llm"], "from-raw");
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], true);
    // 播种的条目没有被 typed round-trip 或整体替换冲掉。
    assert_eq!(raw["model_list"][0]["model_name"], "seed");
}

// 事务 abort 语义钉：闭包 Err ⇒ 不落盘，盘上保持事务前状态（并发行下同样成立）。
#[test]
fn config_update_entry_closure_err_leaves_disk_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_string_lossy().to_string();
    // 种子里显式落 enabled=false：闭包把它改成 true 后 abort，断言盘上仍是 false
    //（若闭包改动泄漏到盘，这里就是 true）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({
            "model_list": [ { "model_name": "seed" } ],
            "agents": { "tool_doc_folding": { "enabled": false } }
        })
        .to_string(),
    )
    .unwrap();

    let err = super::config::update_config(&home, |cfg| {
        cfg.agents.tool_doc_folding.enabled = true;
        Err::<(), String>("deliberate abort".to_string())
    })
    .unwrap_err();
    assert_eq!(err, "deliberate abort");

    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["agents"]["tool_doc_folding"]["enabled"], false);
    assert_eq!(raw["model_list"][0]["model_name"], "seed");
}

// ---------------------------------------------------------------------------
// 掩码还原（2026-10-25 实机实证回归锁）：config.get 脱敏 → 编辑器原样保存，
// 旧实现把 model_list[].api_key 落盘成 '****'（真 key 丢失——channels.save /
// cluster.config.save 都有 restore_masked_named_fields，config.save 漏接）。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn config_save_restores_masked_sensitive_fields_from_existing() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let real_key = "sk-real-secret-key";
    // 盘上种一个带真 key 的模型条目（>8 字符：掩码形态 = 首4+****+尾4）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({
            "model_list": [ { "model_name": "seed", "model": "test/model", "api_key": real_key } ]
        })
        .to_string(),
    )
    .unwrap();
    let handler = ConfigHandler::new();

    // 1) get 响应脱敏：api_key 是掩码形态。
    let got = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let masked = got["model_list"][0]["api_key"]
        .as_str()
        .unwrap()
        .to_string();
    assert!(masked.contains("****"), "get 应脱敏：{masked}");
    assert_ne!(masked, real_key);

    // 2) 编辑器原样回存（不改任何东西）：修复前 api_key 落盘 '****'。
    handler
        .handle_cmd("save", Some(got), &ctx)
        .await
        .unwrap()
        .unwrap();

    // 3) 盘上真 key 原样保留。
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.path().join("config.json")).unwrap())
            .unwrap();
    assert_eq!(raw["model_list"][0]["api_key"].as_str().unwrap(), real_key);
    assert_eq!(raw["model_list"][0]["model_name"], "seed");
}

#[tokio::test]
async fn config_save_masked_value_without_counterpart_loud_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, false);
    // 盘上 api_key 本身就是掩码（历史脏数据）：含掩码的回存 loud 拒绝，
    // 绝不把掩码当真值再写一遍（与 channels.save 同一语义）。
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({
            "model_list": [ { "model_name": "seed", "api_key": "****" } ]
        })
        .to_string(),
    )
    .unwrap();
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd(
            "save",
            Some(serde_json::json!({
                "model_list": [ { "model_name": "seed", "api_key": "****" } ]
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("掩码值"), "{err}");
}
