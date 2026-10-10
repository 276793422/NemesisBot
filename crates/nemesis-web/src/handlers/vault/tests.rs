//! S3c（2026-10-09）— vault WSAPI handler 测试。
//!
//! 进程 env（NEMESISBOT_VAULT_PASSPHRASE）是全局的：触及它的用例持
//! [`TEST_LOCK`] 串行（PathRestore 先例）。Windows 默认 dpapi（open 即解
//! 锁，口令不参与）；Linux 默认 argon2id——fixture 创建与解锁显式带口令，
//! 两个平台同一套用例（锁定态用例在 dpapi 平台诚实跳过——dpapi 无锁态）。

use super::VaultHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_security::vault::{VaultMode, VaultStore};
use parking_lot::Mutex as PlMutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// 触及进程 env 的用例互斥（env 是进程全局）。
static TEST_LOCK: PlMutex<()> = PlMutex::new(());

const SECRET: &str = "sk-s3c-ROUNDTRIP-SECRET-do-not-echo";
const ALIAS: &str = "openai/prod-key";
const PW: &str = "s3c-test-passphrase";

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
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
        webhook_rate_limiter: Arc::new(()),
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

/// 在 `NEMESISBOT_VAULT_PASSPHRASE=<pw>` 在场的环境下跑 `f`，结束后摘除。
/// SAFETY：进程 env 全局——全部触及该变量的用例都持 TEST_LOCK 串行。
fn with_passphrase<T>(pw: &str, f: impl FnOnce() -> T) -> T {
    let _g = TEST_LOCK.lock();
    unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", pw) };
    let out = f();
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    out
}

/// 无口令环境下跑 `f`（锁定态用例）。
fn without_passphrase<T>(f: impl FnOnce() -> T) -> T {
    let _g = TEST_LOCK.lock();
    unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    f()
}

/// 建一个与平台默认模式一致的空 vault fixture（argon2id 带测试口令）。
fn create_fixture(dir: &tempfile::TempDir) {
    let path = nemesis_path::resolve_vault_path_in_workspace(dir.path());
    let mode = VaultStore::default_mode();
    let pw = (mode == VaultMode::Argon2id).then_some(PW);
    VaultStore::create(&path, mode, pw).expect("create vault fixture");
}

fn vault_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    nemesis_path::resolve_vault_path_in_workspace(dir.path())
}

// ---------------------------------------------------------------------------
// 缺失态诚实上报
// ---------------------------------------------------------------------------

#[tokio::test]
async fn status_and_list_on_missing_vault_are_honest() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let st = VaultHandler
        .handle_cmd("status", None, &ctx)
        .await
        .expect("status on missing vault should not error");
    assert_eq!(st.unwrap()["exists"], false);

    let ls = VaultHandler
        .handle_cmd("list", None, &ctx)
        .await
        .expect("list on missing vault should not error");
    let ls = ls.unwrap();
    assert_eq!(ls["exists"], false);
    assert_eq!(ls["entries"].as_array().unwrap().len(), 0);
}

/// workspace 未配置：诚实报错，不猜路径。
#[tokio::test]
async fn missing_workspace_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let mut ctx = make_ctx(&dir);
    ctx.workspace = None;
    let err = VaultHandler
        .handle_cmd("status", None, &ctx)
        .await
        .expect_err("no workspace must error");
    assert!(err.contains("workspace not configured"), "{err}");
}

// ---------------------------------------------------------------------------
// set → list/status → get 全链（值永不进响应）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_list_roundtrip_value_never_in_responses() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let resp = with_passphrase(PW, || async {
        VaultHandler
            .handle_cmd(
                "set",
                Some(serde_json::json!({
                    "alias": ALIAS, "value": SECRET,
                    "domain": "openai", "description": "S3c 测试条目"
                })),
                &ctx,
            )
            .await
            .expect("set creates vault on demand")
    })
    .await
    .expect("payload");
    assert_eq!(resp["alias"], ALIAS);
    assert_eq!(resp["rotated"], false, "首写不是轮换");

    // list：元数据齐全；完整响应 JSON 不含明文值。
    let ls = VaultHandler
        .handle_cmd("list", None, &ctx)
        .await
        .expect("list")
        .expect("payload");
    assert_eq!(ls["exists"], true);
    assert_eq!(ls["unlocked"], true, "口令在场应能解锁");
    let entries = ls["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["alias"], ALIAS);
    assert_eq!(entries[0]["domain"], "openai");
    assert_eq!(entries[0]["description"], "S3c 测试条目");
    assert!(
        entries[0]["created_at"].as_str().is_some(),
        "created_at 缺失"
    );
    assert_eq!(
        entries[0]["rotated_at"],
        serde_json::Value::Null,
        "首写不该有 rotated_at"
    );
    let dumped = serde_json::to_string(&ls).unwrap();
    assert!(!dumped.contains(SECRET), "list 响应泄漏明文值: {dumped}");

    // status：计数与解锁态。
    let st = VaultHandler
        .handle_cmd("status", None, &ctx)
        .await
        .expect("status")
        .expect("payload");
    assert_eq!(st["alias_count"], 1);
    assert_eq!(st["unlocked"], true);
    assert_eq!(
        st["mode"].as_str().unwrap(),
        VaultStore::default_mode().to_string()
    );
    let dumped = serde_json::to_string(&st).unwrap();
    assert!(!dumped.contains(SECRET), "status 响应泄漏明文值: {dumped}");

    // 落盘完整性：现开 fixture 取值 == 原值（复制语义链路真通）。
    let mut store = VaultStore::open(&vault_file(&dir)).expect("reopen");
    if store.mode() == VaultMode::Argon2id {
        store.unlock(PW).expect("unlock fixture");
    }
    assert_eq!(store.get(ALIAS).expect("get"), SECRET);
}

/// 覆盖（轮换）纪律：不带 force 拒绝；force:true 才轮换并保留 created_at。
#[tokio::test]
async fn set_overwrite_requires_force_and_rotates() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    with_passphrase(PW, || async {
        let set_args = |v: &str| serde_json::json!({ "alias": ALIAS, "value": v, "domain": "d" });
        VaultHandler
            .handle_cmd("set", Some(set_args(SECRET)), &ctx)
            .await
            .expect("first set")
            .expect("payload");

        // 不带 force：拒绝，文案指明 force。
        let err = VaultHandler
            .handle_cmd("set", Some(set_args("sk-NEW-VALUE")), &ctx)
            .await
            .expect_err("overwrite without force must be rejected");
        assert!(err.contains("force:true"), "{err}");

        // force:true：轮换成功，rotated=true。
        let mut args = set_args("sk-NEW-VALUE");
        args["force"] = serde_json::json!(true);
        let resp = VaultHandler
            .handle_cmd("set", Some(args), &ctx)
            .await
            .expect("forced set")
            .expect("payload");
        assert_eq!(resp["rotated"], true);
    })
    .await;

    // 值已换、created_at 保留、rotated_at 出现。
    let ls = VaultHandler
        .handle_cmd("list", None, &ctx)
        .await
        .expect("list")
        .expect("payload");
    let entry = &ls["entries"].as_array().unwrap()[0];
    assert!(entry["rotated_at"].as_str().is_some());
    let mut store = VaultStore::open(&vault_file(&dir)).expect("reopen");
    if store.mode() == VaultMode::Argon2id {
        store.unlock(PW).expect("unlock fixture");
    }
    assert_eq!(store.get(ALIAS).expect("get"), "sk-NEW-VALUE");
}

/// 空 secret 拒绝（镜像 CLI 纪律）。
#[tokio::test]
async fn set_empty_value_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = with_passphrase(PW, || async {
        VaultHandler
            .handle_cmd(
                "set",
                Some(serde_json::json!({ "alias": "a", "value": "" })),
                &ctx,
            )
            .await
    })
    .await
    .expect_err("empty secret must be rejected");
    assert!(err.contains("secret 为空"), "{err}");
}

/// 非法别名（首尾空白）透传 vault 校验，诚实报错。
#[tokio::test]
async fn set_invalid_alias_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = with_passphrase(PW, || async {
        VaultHandler
            .handle_cmd(
                "set",
                Some(serde_json::json!({ "alias": " bad alias ", "value": "v" })),
                &ctx,
            )
            .await
    })
    .await
    .expect_err("invalid alias must be rejected");
    assert!(err.contains("非法别名"), "{err}");
}

// ---------------------------------------------------------------------------
// remove
// ---------------------------------------------------------------------------

#[tokio::test]
async fn remove_flows() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    // 文件不存在：诚实报错（不悄悄造空 vault）。
    let err = VaultHandler
        .handle_cmd("remove", Some(serde_json::json!({"alias": ALIAS})), &ctx)
        .await
        .expect_err("remove on missing vault must error");
    assert!(err.contains("vault 文件不存在"), "{err}");

    with_passphrase(PW, || async {
        VaultHandler
            .handle_cmd(
                "set",
                Some(serde_json::json!({ "alias": ALIAS, "value": SECRET })),
                &ctx,
            )
            .await
            .expect("set")
            .expect("payload");

        // 未知别名：诚实报错。
        let err = VaultHandler
            .handle_cmd("remove", Some(serde_json::json!({"alias": "ghost"})), &ctx)
            .await
            .expect_err("unknown alias must error");
        assert!(err.contains("别名不存在"), "{err}");

        // 删除成功后 list 不再含该别名。
        let resp = VaultHandler
            .handle_cmd("remove", Some(serde_json::json!({"alias": ALIAS})), &ctx)
            .await
            .expect("remove existing")
            .expect("payload");
        assert_eq!(resp["removed"], true);
    })
    .await;

    let ls = VaultHandler
        .handle_cmd("list", None, &ctx)
        .await
        .expect("list")
        .expect("payload");
    assert_eq!(ls["entries"].as_array().unwrap().len(), 0);
}

// ---------------------------------------------------------------------------
// 锁定态诚实上报（仅 argon2id 平台——dpapi 无锁态）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn locked_vault_reports_unlocked_false_and_set_fails_loud() {
    if VaultStore::default_mode() != VaultMode::Argon2id {
        // Windows dpapi：open 即解锁，没有锁态——本用例语义不适用，诚实跳过。
        eprintln!("SKIP locked_vault: 平台默认 dpapi，无锁态");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    create_fixture(&dir);

    // 无口令环境：list 照常列（元数据明文）但 unlocked=false；set 诚实失败。
    without_passphrase(|| async {
        let ls = VaultHandler
            .handle_cmd("list", None, &ctx)
            .await
            .expect("list works locked")
            .expect("payload");
        assert_eq!(ls["unlocked"], false);

        let st = VaultHandler
            .handle_cmd("status", None, &ctx)
            .await
            .expect("status works locked")
            .expect("payload");
        assert_eq!(st["unlocked"], false);

        let err = VaultHandler
            .handle_cmd(
                "set",
                Some(serde_json::json!({ "alias": "x", "value": "v" })),
                &ctx,
            )
            .await
            .expect_err("set must fail loud when locked");
        assert!(
            err.contains("NEMESISBOT_VAULT_PASSPHRASE"),
            "错误应带口令指引: {err}"
        );

        // remove 不需要 DEK：锁定也可删（条目语义在元数据层）。
        VaultHandler
            .handle_cmd(
                "remove",
                Some(serde_json::json!({ "alias": "ghost" })),
                &ctx,
            )
            .await
            .expect_err("remove of unknown alias still errors");
    })
    .await;
}

// ---------------------------------------------------------------------------
// L1 注册表面
// ---------------------------------------------------------------------------

#[test]
fn module_surface() {
    assert_eq!(VaultHandler.module_name(), "vault");
    assert_eq!(
        VaultHandler.commands(),
        &["list", "set", "remove", "status"]
    );
}
