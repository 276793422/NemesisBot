//! B4 — WebCredentialBroker 测试。
//!
//! 覆盖四条协议纪律：值不进事件载荷 / Stored 全链（respond→vault→重解析）/
//! 超时→Denied 且出册（迟到 respond 诚实 Err）/ 落库失败放回 pending。
//!
//! 形态注记：纯 sync `#[test]`（无 tokio runtime）——broker 的
//! `Handle::try_current` 探测落 Err 分支直接 `recv_timeout`，respond 从
//! `std::thread` 送裁决，规避 `block_in_place` 的 runtime 形态假设。
//!
//! env 纪律（vault handler 测试同族）：触及 `NEMESISBOT_VAULT_PASSPHRASE`
//! 的用例持 `crate::GLOBAL_STATE_LOCK` 串行；fixture 按平台
//! 默认模式创建（argon2id 带测试口令，dpapi 无口令）。
//! 锁必须与 connector_tools / vault_runtime / commands::vault 的 env 触及
//! 测试群同源——env 口令是进程级单例，文件级私有锁互斥不了同进程并行
//! 用例（Linux ET 实录：两把锁并行 set_var 互相覆盖口令 → DEK 解包认证
//! 失败假红）。

use super::*;
use nemesis_security::vault::VaultStore;

const SECRET: &str = "sk-b4-card-SECRET-do-not-echo";
const PW: &str = "b4-test-passphrase";

fn make_broker(
    dir: &tempfile::TempDir,
) -> (
    Arc<WebCredentialBroker>,
    tokio::sync::broadcast::Receiver<AgentEvent>,
) {
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    (
        Arc::new(WebCredentialBroker::new(dir.path().to_path_buf(), Some(tx))),
        rx,
    )
}

fn vault_file(dir: &tempfile::TempDir) -> std::path::PathBuf {
    nemesis_path::resolve_vault_path_in_workspace(dir.path())
}

/// 按平台默认模式创建空 vault fixture（argon2id 带口令；dpapi 无口令）。
fn create_fixture(dir: &tempfile::TempDir) {
    let mode = VaultStore::default_mode();
    let pw = (mode == nemesis_security::vault::VaultMode::Argon2id).then_some(PW);
    VaultStore::create(&vault_file(dir), mode, pw).expect("create vault fixture");
}

/// 在 `NEMESISBOT_VAULT_PASSPHRASE=<pw>`（argon2id 平台）在场的环境下执行
/// `f`。本文件全 sync 无 await，env 置位不需要跨 await。
fn with_passphrase<T>(f: impl FnOnce() -> T) -> T {
    let _g = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let mode = VaultStore::default_mode();
    let needs_pw = mode == nemesis_security::vault::VaultMode::Argon2id;
    if needs_pw {
        unsafe { std::env::set_var("NEMESISBOT_VAULT_PASSPHRASE", PW) };
    }
    let out = f();
    if needs_pw {
        unsafe { std::env::remove_var("NEMESISBOT_VAULT_PASSPHRASE") };
    }
    out
}

fn request(id: &str, timeout: u64) -> CredentialRequest {
    CredentialRequest {
        request_id: id.to_string(),
        service: "connector/test-conn".to_string(),
        key_name: "connector/test-conn/api_key".to_string(),
        reason: "test".to_string(),
        timeout_secs: timeout,
    }
}

// ---------------------------------------------------------------------------
// Stored 全链：respond → vault → 重解析
// ---------------------------------------------------------------------------

#[test]
fn stored_full_chain_value_lands_in_vault() {
    with_passphrase(|| {
        let dir = tempfile::tempdir().unwrap();
        let (broker, _rx) = make_broker(&dir);
        create_fixture(&dir);

        let responder = broker.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            responder.respond("cred-1", SECRET).unwrap()
        });
        let outcome = broker.request_credential(&request("cred-1", 5)).unwrap();
        let receipt = t.join().unwrap();

        assert_eq!(outcome, CredentialOutcome::Stored);
        assert_eq!(receipt["stored"], serde_json::json!(true));
        assert_eq!(receipt["rotated"], serde_json::json!(false));

        // 值真实落库且可解析（调用方原地重解析的依据）。
        let resolved = crate::vault_runtime::resolve_alias_at(
            &vault_file(&dir),
            "connector/test-conn/api_key",
        )
        .unwrap();
        assert_eq!(resolved, SECRET);
    });
}

// ---------------------------------------------------------------------------
// 值不进事件载荷
// ---------------------------------------------------------------------------

#[test]
fn requested_event_carries_metadata_only() {
    let dir = tempfile::tempdir().unwrap();
    let (broker, mut rx) = make_broker(&dir);

    let responder = broker.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        let _ = responder.respond("cred-evt", SECRET);
    });
    let outcome = broker.request_credential(&request("cred-evt", 5)).unwrap();
    assert_eq!(outcome, CredentialOutcome::Stored);

    let ev = rx.try_recv().unwrap();
    let payload = serde_json::to_value(&ev).unwrap();
    let inner = payload.get("data").unwrap();
    assert!(
        inner.get("value").is_none() && inner.get("secret").is_none(),
        "事件载荷不得携带值字段: {inner}"
    );
    assert_eq!(inner["request_id"], "cred-evt");
    assert_eq!(inner["key_name"], "connector/test-conn/api_key");
    assert_eq!(inner["timeout_secs"], 5);
    // 了结事件随后到达且无值。
    let ev2 = rx.try_recv().unwrap();
    let p2 = serde_json::to_value(&ev2).unwrap();
    assert_eq!(p2["data"]["decision"], "stored");
}

// ---------------------------------------------------------------------------
// 超时 → Denied + 出册 + 迟到 respond 诚实 Err
// ---------------------------------------------------------------------------

#[test]
fn timeout_denies_and_unregisters() {
    let dir = tempfile::tempdir().unwrap();
    let (broker, _rx) = make_broker(&dir);

    let outcome = broker.request_credential(&request("cred-to", 1)).unwrap();
    assert_eq!(outcome, CredentialOutcome::Denied, "超时语义 = 原错误路径");
    assert!(
        broker
            .pending_cards()
            .iter()
            .all(|c| c["request_id"] != "cred-to"),
        "超时后出册（不得泄漏 pending）"
    );

    // 迟到 respond：请求已出册 = 诚实 Err（不假成功）。
    let err = broker.respond("cred-to", SECRET).unwrap_err();
    assert!(err.contains("不存在"), "got: {err}");
}

// ---------------------------------------------------------------------------
// 落库失败 → 请求放回 pending（可修正重试）
// ---------------------------------------------------------------------------

#[test]
fn persist_failure_keeps_request_pending() {
    with_passphrase(|| {
        let dir = tempfile::tempdir().unwrap();
        let (broker, _rx) = make_broker(&dir);
        // 预写坏 vault 文件：open 失败 = persist 恒失败（确定性错误注入）。
        std::fs::create_dir_all(vault_file(&dir).parent().unwrap()).unwrap();
        std::fs::write(vault_file(&dir), b"{ not a vault }").unwrap();

        let responder = broker.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            responder.respond("cred-bad", SECRET).unwrap_err()
        });
        // 慢超时：等待侧还挂着，让失败路径先发生。
        let waiter = broker.clone();
        let h = std::thread::spawn(move || {
            let _ = waiter.request_credential(&request("cred-bad", 10));
        });
        let err = t.join().unwrap();
        assert!(err.contains("落库失败"), "got: {err}");
        // 请求放回 pending（等待者还在等）。
        assert!(
            broker
                .pending_cards()
                .iter()
                .any(|c| c["request_id"] == "cred-bad"),
            "落库失败后请求保留在 pending"
        );
        h.join().unwrap();
    });
}

// ---------------------------------------------------------------------------
// 空值拒绝 + 便捷入口（broker 缺席 = false）
// ---------------------------------------------------------------------------

#[test]
fn empty_value_rejected_without_consuming_request() {
    with_passphrase(|| {
        let dir = tempfile::tempdir().unwrap();
        let (broker, _rx) = make_broker(&dir);
        create_fixture(&dir);

        // 空值在校验层被拒（不动 pending），随后正式提交仍可成功。
        let err = broker.respond("cred-empty", "   ").unwrap_err();
        assert!(err.contains("为空"), "got: {err}");

        let responder = broker.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            responder.respond("cred-empty", SECRET).unwrap();
        });
        let outcome = broker
            .request_credential(&request("cred-empty", 5))
            .unwrap();
        assert_eq!(outcome, CredentialOutcome::Stored);
    });
}

#[test]
fn convenience_entry_false_when_broker_absent() {
    super::uninstall();
    assert!(!super::request_credential_card("svc", "alias", "why", 1));
}
