//! 回归钉：models.* 条目引用三层查找（`entry_index_by_ref` 单一真相源，
//! ref_owner / resolver 同序）——此前 set_default/delete/copy/rename/test/
//! probe_effort 只认 `model_name` 显示别名，传 `model` 模型串（或裸尾段）
//! 恒报 not found（2026-11-27 自学习复测实测踩中：eval 条目别名
//! eval-gpt-6.2-sol，模型串 gpt-6.2-sol 不可用）。本钉锁三层序（别名精确
//! 高于模型串精确高于尾段软匹配）、命中后下游锚定条目别名（defaults.llm 写
//! 别名、热切传别名、回显别名）、默认守卫在模型串入参下语义不变。

use super::models::ModelsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
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
        quic_relay: None,
        mail_plugin: None,
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
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 两条目配置：alias=eval-x（model=vendor/x）+ alias=second（model=vendor/s）。
/// 端点全部指 127.0.0.1:1（connect refused，秒败不挂测试）。
fn two_entry_config() -> serde_json::Value {
    serde_json::json!({
        "model_list": [
            {
                "model_name": "eval-x", "model": "vendor/x",
                "api_base": "http://127.0.0.1:1/v1", "api_key": "k",
                "protocol": "openai",
            },
            {
                "model_name": "second", "model": "vendor/s",
                "api_base": "http://127.0.0.1:1/v1", "api_key": "k",
                "protocol": "openai",
            }
        ],
        "agents": { "defaults": { "llm": "eval-x" } }
    })
}

fn write_config(dir: &tempfile::TempDir, cfg: &serde_json::Value) {
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::to_string_pretty(cfg).unwrap(),
    )
    .unwrap();
}

fn read_config(dir: &tempfile::TempDir) -> serde_json::Value {
    let raw = std::fs::read_to_string(dir.path().join("config.json")).unwrap();
    serde_json::from_str(&raw).unwrap()
}

#[tokio::test]
async fn set_default_accepts_model_string_and_writes_alias() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    let out = h
        .handle_cmd(
            "set_default",
            Some(serde_json::json!({"name": "vendor/x"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // 回显身份 = 条目 model_name 别名（非调用方模型串）。
    assert_eq!(out["name"], "eval-x");
    let raw = read_config(&tmp);
    assert_eq!(raw.pointer("/agents/defaults/llm").unwrap(), "eval-x");
    assert_eq!(raw["model_list"][0]["model_name"], "eval-x");
}

#[tokio::test]
async fn set_default_accepts_bare_suffix_and_alias_still_first() {
    let tmp = tempfile::TempDir::new().unwrap();
    // third 层软匹配：裸尾段 "x" 命中 model=vendor/x；
    // 别名优先层：条目 alias=second 与另一条目 model 精同串时，别名层压倒。
    let mut cfg = two_entry_config();
    cfg["model_list"][1]["model"] = serde_json::json!("vendor-x-alias-holder");
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 裸尾段命中 eval-x 条目。
    let out = h
        .handle_cmd("set_default", Some(serde_json::json!({"name": "x"})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "eval-x");
    // 别名层压倒模型串层：alias="dup" 的条目排在 model="vendor/dup" 条目
    // 之前，传 "dup" 必须命中前者（别名精确层先于模型串精确层）。
    let cfg2 = serde_json::json!({
        "model_list": [
            {"model_name": "dup", "model": "other/m",
             "api_base": "http://127.0.0.1:1/v1", "api_key": "k", "protocol": "openai"},
            {"model_name": "primary", "model": "vendor/dup",
             "api_base": "http://127.0.0.1:1/v1", "api_key": "k", "protocol": "openai"}
        ],
        "agents": { "defaults": { "llm": "dup" } }
    });
    let tmp2 = tempfile::TempDir::new().unwrap();
    write_config(&tmp2, &cfg2);
    let h2 = ModelsHandler::new();
    let ctx2 = make_ctx(&tmp2);
    let out2 = h2
        .handle_cmd(
            "set_default",
            Some(serde_json::json!({"name": "dup"})),
            &ctx2,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out2["name"], "dup");
    assert_eq!(
        read_config(&tmp2).pointer("/agents/defaults/llm").unwrap(),
        "dup"
    );
}

#[tokio::test]
async fn set_default_not_found_still_loud() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    let err = h
        .handle_cmd(
            "set_default",
            Some(serde_json::json!({"name": "nope"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

#[tokio::test]
async fn delete_accepts_model_string_and_default_guard_holds() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 默认条目（eval-x）按模型串删 → 守卫拦（归属判定锚别名）。
    let err = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("cannot delete default"),
        "模型串入参的默认守卫必须拦，got: {err}"
    );
    // 非默认条目按模型串删 → 成功，回显别名。
    let out = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/s"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "second");
    let raw = read_config(&tmp);
    assert_eq!(raw["model_list"].as_array().unwrap().len(), 1);
    assert_eq!(raw["model_list"][0]["model_name"], "eval-x");
}

#[tokio::test]
async fn delete_position_guard_holds_when_default_llm_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cfg = two_entry_config();
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("");
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // defaults.llm 为空 → list[0] 位置兜底；模型串命中首条同样要拦。
    let err = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("cannot delete default"), "got: {err}");
    let out = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/s"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "second");
}

#[tokio::test]
async fn copy_accepts_model_string_and_echoes_alias() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    let out = h
        .handle_cmd(
            "copy",
            Some(serde_json::json!({"name": "vendor/x", "new_name": "copy-x"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "eval-x");
    assert_eq!(out["new_name"], "copy-x");
    let raw = read_config(&tmp);
    let list = raw["model_list"].as_array().unwrap();
    assert_eq!(list.len(), 3);
    assert_eq!(list[2]["model_name"], "copy-x");
    assert_eq!(list[2]["model"], "vendor/x");
}

#[tokio::test]
async fn rename_by_model_string_syncs_references_and_self_alias_noop_ok() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cfg = two_entry_config();
    cfg["agents"]["small_model"] = serde_json::json!("eval-x");
    cfg["tools"] = serde_json::json!({ "image_gen": { "model": "vendor/x" } });
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 按模型串改名：条目别名 eval-x → renamed；三处引用同趟同步
    //（defaults.llm/small_model 指旧别名、image_gen.model 指旧模型串——
    // ref_owner 归属判定都认到同一条目）。
    let out = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({"name": "vendor/x", "new_name": "renamed"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "eval-x");
    let raw = read_config(&tmp);
    assert_eq!(raw["model_list"][0]["model_name"], "renamed");
    assert_eq!(raw.pointer("/agents/defaults/llm").unwrap(), "renamed");
    assert_eq!(raw.pointer("/agents/small_model").unwrap(), "renamed");
    assert_eq!(
        raw.pointer("/tools/image_gen/model").unwrap(),
        "renamed",
        "指向旧模型串的引用同样归属该条目，必须同步"
    );
    // 自别名改名（name=模型串，new_name=条目现别名）= no-op 成功，不误报
    // already exists（重名检查按下标排除自身）。
    let out2 = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({"name": "vendor/x", "new_name": "renamed"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out2["renamed"], true);
}

#[tokio::test]
async fn update_field_accepts_model_string_and_bare_suffix() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 模型串入参写字段（tier 走 mtime 重读，无热切/后台探针副作用）。
    h.handle_cmd(
        "update_field",
        Some(serde_json::json!({"name": "vendor/s", "field": "model_tier", "value": "big"})),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    let raw = read_config(&tmp);
    assert_eq!(raw["model_list"][1]["model_tier"], "big");
    // 裸尾段入参同样命中。
    h.handle_cmd(
        "update_field",
        Some(serde_json::json!({"name": "s", "field": "model_tier", "value": "normal"})),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    let raw = read_config(&tmp);
    assert_eq!(raw["model_list"][1]["model_tier"], "normal");
}

#[tokio::test]
async fn probe_model_entry_accepts_model_string_and_bare_suffix() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &two_entry_config());
    // found（模型串命中）→ 走到探测层返回 status（连 127.0.0.1:1 秒败，
    // status=fail 但不再是 not found Err）。
    let out = super::models::probe_model_entry(
        tmp.path().to_string_lossy().as_ref(),
        "vendor/x",
        super::models::ProbeDepth::Interactive(Some(1)),
    )
    .await
    .unwrap();
    assert_eq!(out["name"], "eval-x");
    assert_eq!(out["status"], "fail");
    let out2 = super::models::probe_model_entry(
        tmp.path().to_string_lossy().as_ref(),
        "x",
        super::models::ProbeDepth::Interactive(Some(1)),
    )
    .await
    .unwrap();
    assert_eq!(out2["name"], "eval-x");
    // 未命中 → 诚实 Err。
    let err = super::models::probe_model_entry(
        tmp.path().to_string_lossy().as_ref(),
        "nope",
        super::models::ProbeDepth::Interactive(Some(1)),
    )
    .await
    .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

#[tokio::test]
async fn probe_effort_entry_accepts_model_string_via_honest_skip_lane() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut cfg = two_entry_config();
    // anthropic lane → probe_effort 诚实 skip（无网络），恰好做 found 证明面。
    cfg["model_list"][1]["protocol"] = serde_json::json!("anthropic");
    write_config(&tmp, &cfg);
    let out = super::models::probe_effort_entry(tmp.path().to_string_lossy().as_ref(), "vendor/s")
        .await
        .unwrap();
    assert_eq!(out["name"], "second");
    assert_eq!(out["status"], "skipped");
    let err = super::models::probe_effort_entry(tmp.path().to_string_lossy().as_ref(), "nope")
        .await
        .unwrap_err();
    assert!(err.contains("not found"), "got: {err}");
}

// === 无别名条目（手改 config legacy 形态，仅 model 键无 model_name）===
// 现代 add 路径恒写 model_name；本组钉存量手改配置：写回/热切/回显身份
// 走 entry_ref_identity 回退臂（模型串），归属判定按下标（多无别名条目
// 的归属名同为 ""，按名比对会互相误认）。逐行复查批（2026-11-27）修复面。

/// 单无别名条目。
fn alias_less_config() -> serde_json::Value {
    serde_json::json!({
        "model_list": [
            {
                "model": "vendor/x",
                "api_base": "http://127.0.0.1:1/v1", "api_key": "k",
                "protocol": "openai",
            }
        ],
        "agents": { "defaults": { "llm": "" } }
    })
}

#[tokio::test]
async fn alias_less_entry_set_default_writes_model_string_identity() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &alias_less_config());
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    // 修前回归形态：raw model_name 为空 → defaults.llm 落 ""（配置损坏）
    // + apply_runtime_swap("") resolve 失败，`?` 在写盘后传播成半状态 Err。
    let out = h
        .handle_cmd(
            "set_default",
            Some(serde_json::json!({"name": "vendor/x"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "vendor/x", "回显身份 = 模型串（非空串）");
    assert_eq!(
        read_config(&tmp).pointer("/agents/defaults/llm").unwrap(),
        "vendor/x",
        "无别名条目设默认必须写模型串身份，不得落空串"
    );
}

#[tokio::test]
async fn alias_less_entry_probe_runs_under_model_string_identity() {
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &alias_less_config());
    let out = super::models::probe_model_entry(
        tmp.path().to_string_lossy().as_ref(),
        "vendor/x",
        super::models::ProbeDepth::Interactive(Some(1)),
    )
    .await
    .unwrap();
    assert_eq!(out["name"], "vendor/x");
    assert_eq!(out["status"], "fail");
}

#[tokio::test]
async fn alias_less_multi_entry_delete_guard_is_index_anchored() {
    // 两条无别名条目，默认 = idx0（defaults.llm 指其模型串）：删 idx0 守卫
    // 拦；删 idx1（非默认）必须放行——按名比对两处归属名同为 ""，ref_owner
    // 旧法会误拦非默认条目；remove(idx) 必须真摘掉无别名条目。
    let mut cfg = alias_less_config();
    cfg["model_list"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "model": "vendor/s", "api_base": "http://127.0.0.1:1/v1",
            "api_key": "k", "protocol": "openai",
        }));
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("vendor/x");
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    let err = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("cannot delete default"), "got: {err}");
    let out = h
        .handle_cmd(
            "delete",
            Some(serde_json::json!({"name": "vendor/s"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "vendor/s", "非默认无别名条目放行且回显模型串");
    let raw = read_config(&tmp);
    let list = raw["model_list"].as_array().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["model"], "vendor/x");
}

#[tokio::test]
async fn alias_less_entry_update_field_hot_swap_resolves() {
    // 默认无别名条目改 api_base（构造期消费字段）→ 热切身份回退模型串、
    // canonical resolve 成功命令 Ok（修前 apply_runtime_swap("") 的 Err 在
    // 写盘后传播 = 半状态）。probe_effort_on_save 关掉防后台探针并发写盘。
    let mut cfg = alias_less_config();
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("vendor/x");
    cfg["agents"]["probe_effort_on_save"] = serde_json::json!(false);
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    h.handle_cmd(
        "update_field",
        Some(serde_json::json!({"name": "vendor/x", "field": "api_base",
             "value": "http://127.0.0.1:2/v1"})),
        &ctx,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        read_config(&tmp)["model_list"][0]["api_base"],
        "http://127.0.0.1:2/v1"
    );
    // 热切投影：AppState 快照字段同步为解析出的条目参数（身份可解析）。
    assert_eq!(*ctx.state.model_base.lock(), "http://127.0.0.1:2/v1");
}

#[tokio::test]
async fn alias_less_entry_rename_assigns_alias_and_syncs_refs() {
    // 无别名条目改名 = 顺手赋予别名；指向其模型串的默认引用按下标归属
    // 认领、同趟同步成新别名；was_default → 热切传 new_name（恒可解析）。
    let mut cfg = alias_less_config();
    cfg["agents"]["defaults"]["llm"] = serde_json::json!("vendor/x");
    let tmp = tempfile::TempDir::new().unwrap();
    write_config(&tmp, &cfg);
    let h = ModelsHandler::new();
    let ctx = make_ctx(&tmp);
    let out = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({"name": "vendor/x", "new_name": "named"})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["name"], "vendor/x");
    let raw = read_config(&tmp);
    assert_eq!(raw["model_list"][0]["model_name"], "named");
    assert_eq!(raw.pointer("/agents/defaults/llm").unwrap(), "named");
}
