//! handlers 单测：直接调用 handler 函数（手工构造 `State`/`Json`/`HeaderMap`
//! 提取器——仓库既有范式见 `crates/nemesis-web/src/api_handlers/extra_tests.rs`），
//! 进程内完成，不 bind 端口。multipart 用 `FromRequest` 手工构造。
//!
//! env 说明：`get_crl` 读 `NEMESIS_DEBUG_CRL_500`。所有调 get_crl 的测试共享
//! CRL_ENV_LOCK 串行（env-test-race-lock 纪律：先清再设，防上轮残留泄漏）。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

use super::*;
use axum::extract::FromRequest;
use nemesis_verify::TrustedKey;
use nemesis_verify::revocation::OcspReq;
use nemesis_verify::verify::VerifyOutcome;
use nemesis_verify::verify_response;
use nemesis_verify::view::latest_sig_hash;
use std::sync::atomic::{AtomicU32, Ordering};

const ADMIN_TOKEN: &str = "test-admin-token";
const MP_BOUNDARY: &str = "nemesis-revoke-server-test-boundary";

/// get_crl 读 NEMESIS_DEBUG_CRL_500——所有调 get_crl 的测试先拿这把锁。
static CRL_ENV_LOCK: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
static KEYS_SEQ: AtomicU32 = AtomicU32::new(0);

fn setup_with_token(admin_token: &str) -> Arc<AppState> {
    let n = KEYS_SEQ.fetch_add(1, Ordering::SeqCst);
    let keys_path = std::env::temp_dir()
        .join(format!(
            "revoke_handlers_keys_{}_{}.json",
            std::process::id(),
            n
        ))
        .to_string_lossy()
        .into_owned();
    nemesis_verify::keygen::generate()
        .expect("generate test key hierarchy")
        .save(&keys_path)
        .expect("save test key hierarchy");
    let state = AppState::new(":memory:", &keys_path, admin_token.to_string())
        .expect("AppState::new for handler tests");
    // hierarchy 已载入内存，密钥文件即删（防残留）
    let _ = std::fs::remove_file(&keys_path);
    state
}

fn setup() -> Arc<AppState> {
    setup_with_token(ADMIN_TOKEN)
}

fn bearer_headers(token: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("authorization", format!("Bearer {token}").parse().unwrap());
    h
}

fn admin_headers() -> HeaderMap {
    bearer_headers(ADMIN_TOKEN)
}

fn verify_req(
    key_fp: Option<&str>,
    sig_hash: Option<&str>,
    content_hash: Option<&str>,
    publisher: Option<&str>,
) -> VerifyReq {
    VerifyReq {
        key_fp: key_fp.map(String::from),
        sig_hash: sig_hash.map(String::from),
        content_hash: content_hash.map(String::from),
        publisher: publisher.map(String::from),
    }
}

fn revoke_req(dim: RevDim, value: &str, reason: &str) -> RevokeReq {
    RevokeReq {
        dim,
        value: value.to_string(),
        reason: reason.to_string(),
    }
}

fn crl_entry(dim: RevDim, value: &str, revoked_at: u64, reason: &str) -> CrlEntry {
    CrlEntry {
        dim,
        value: value.to_string(),
        revoked_at,
        reason: reason.to_string(),
    }
}

fn crl_with(entries: Vec<CrlEntry>) -> Crl {
    Crl {
        version: 5,
        valid_until: u64::MAX,
        entries,
    }
}

fn tkl_with(keys: Vec<TrustedKey>) -> TrustedKeyList {
    TrustedKeyList {
        version: 3,
        valid_until: u64::MAX,
        keys,
    }
}

// ===================== health / internal / extract_token =====================

#[tokio::test]
async fn health_returns_ok() {
    assert_eq!(health().await, "ok");
}

#[test]
fn internal_maps_to_500_with_message() {
    let (code, msg) = internal("boom");
    assert_eq!(code, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(msg, "boom");
}

#[test]
fn extract_token_variants() {
    // 缺 header → 空
    assert_eq!(extract_token(&HeaderMap::new()), "");
    // Bearer 前缀剥掉
    let mut h = HeaderMap::new();
    h.insert("authorization", "Bearer abc".parse().unwrap());
    assert_eq!(extract_token(&h), "abc");
    // 无前缀裸串原样返回
    let mut h = HeaderMap::new();
    h.insert("authorization", "rawtoken".parse().unwrap());
    assert_eq!(extract_token(&h), "rawtoken");
    // 非 UTF-8 值 → to_str 失败 → 空
    let mut h = HeaderMap::new();
    h.insert(
        "authorization",
        axum::http::HeaderValue::from_bytes(&[0xFF, 0xFE]).unwrap(),
    );
    assert_eq!(extract_token(&h), "");
}

// ===================== check_admin =====================

#[test]
fn check_admin_refuses_all_when_unconfigured() {
    let state = setup_with_token("");
    let err = check_admin(&state, &admin_headers()).unwrap_err();
    assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    assert!(err.1.contains("not configured"), "msg: {}", err.1);
}

#[test]
fn check_admin_token_variants() {
    let state = setup();
    // 缺 header → 401
    let err = check_admin(&state, &HeaderMap::new()).unwrap_err();
    assert_eq!(
        err,
        (StatusCode::UNAUTHORIZED, "invalid admin token".to_string())
    );
    // 错 token → 401
    let err = check_admin(&state, &bearer_headers("wrong")).unwrap_err();
    assert_eq!(err.0, StatusCode::UNAUTHORIZED);
    // 正确 token（Bearer 前缀）→ Ok
    assert!(check_admin(&state, &admin_headers()).is_ok());
    // 无前缀裸串（strip_prefix 不匹配 → 原样比对）→ Ok
    let mut h = HeaderMap::new();
    h.insert("authorization", ADMIN_TOKEN.parse().unwrap());
    assert!(check_admin(&state, &h).is_ok());
}

// ===================== compute_status（纯函数全分支）=====================

#[test]
fn compute_status_empty_lists_valid() {
    let (code, at, reason) = compute_status(
        &crl_with(vec![]),
        &tkl_with(vec![]),
        &verify_req(None, None, None, None),
        1000,
    );
    assert_eq!(code, "valid");
    assert_eq!(at, None);
    assert_eq!(reason, None);
}

#[test]
fn compute_status_trusted_keys_admission() {
    // trusted_keys 非空 + 请求未带 key_fp → untrusted
    let tkl = tkl_with(vec![TrustedKey {
        key_fp: "aa".into(),
        status: KeyStatus::Active,
        not_after: None,
    }]);
    let (code, _, _) = compute_status(
        &crl_with(vec![]),
        &tkl,
        &verify_req(None, Some("sh"), None, None),
        1000,
    );
    assert_eq!(code, "untrusted");
    // 带了但不在列表 → untrusted
    let (code, _, _) = compute_status(
        &crl_with(vec![]),
        &tkl,
        &verify_req(Some("zz"), None, None, None),
        1000,
    );
    assert_eq!(code, "untrusted");
    // 在列表但状态 Revoked → untrusted
    let tkl = tkl_with(vec![TrustedKey {
        key_fp: "aa".into(),
        status: KeyStatus::Revoked,
        not_after: None,
    }]);
    let (code, _, _) = compute_status(
        &crl_with(vec![]),
        &tkl,
        &verify_req(Some("aa"), None, None, None),
        1000,
    );
    assert_eq!(code, "untrusted");
    // Active 但 not_after 已过（now > t）→ untrusted
    let tkl = tkl_with(vec![TrustedKey {
        key_fp: "aa".into(),
        status: KeyStatus::Active,
        not_after: Some(999),
    }]);
    let (code, _, _) = compute_status(
        &crl_with(vec![]),
        &tkl,
        &verify_req(Some("aa"), None, None, None),
        1000,
    );
    assert_eq!(code, "untrusted");
    // 边界 not_after == now（now <= t 放行）→ 空 CRL → valid
    let tkl = tkl_with(vec![TrustedKey {
        key_fp: "aa".into(),
        status: KeyStatus::Active,
        not_after: Some(1000),
    }]);
    let (code, _, _) = compute_status(
        &crl_with(vec![]),
        &tkl,
        &verify_req(Some("aa"), None, None, None),
        1000,
    );
    assert_eq!(code, "valid");
}

#[test]
fn compute_status_crl_hits_all_dims_with_priority() {
    let crl = crl_with(vec![
        crl_entry(RevDim::KeyFp, "kf1", 222, "by-key"),
        crl_entry(RevDim::SigHash, "sh1", 111, "by-sig"),
        crl_entry(RevDim::FileHash, "fh1", 333, "by-file"),
        crl_entry(RevDim::Publisher, "pub1", 444, "by-publisher"),
    ]);
    let empty_tkl = tkl_with(vec![]);
    // 四维度各自命中
    let cases = [
        (verify_req(Some("kf1"), None, None, None), 222u64, "by-key"),
        (verify_req(None, Some("sh1"), None, None), 111, "by-sig"),
        (verify_req(None, None, Some("fh1"), None), 333, "by-file"),
        (
            verify_req(None, None, None, Some("pub1")),
            444,
            "by-publisher",
        ),
    ];
    for (req, at, reason) in cases {
        let (code, got_at, got_reason) = compute_status(&crl, &empty_tkl, &req, 1000);
        assert_eq!(code, "revoked");
        assert_eq!(got_at, Some(at));
        assert_eq!(got_reason.as_deref(), Some(reason));
    }
    // key_fp 优先：key_fp + sig_hash 同时命中 → 返回 key_fp 条目（or_else 链短路）
    let (code, at, reason) = compute_status(
        &crl,
        &empty_tkl,
        &verify_req(Some("kf1"), Some("sh1"), None, None),
        1000,
    );
    assert_eq!(code, "revoked");
    assert_eq!(at, Some(222));
    assert_eq!(reason.as_deref(), Some("by-key"));
    // 全不命中 → valid
    let (code, _, _) = compute_status(
        &crl,
        &empty_tkl,
        &verify_req(Some("clean"), Some("clean"), None, None),
        1000,
    );
    assert_eq!(code, "valid");
}

// ===================== /v1/verify =====================

#[tokio::test]
async fn verify_empty_store_returns_valid_and_signed() {
    let state = setup();
    let Json(signed) = verify(
        State(state.clone()),
        Json(verify_req(None, None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(signed.payload.code, "valid");
    assert_eq!(signed.payload.crl_ver, 1);
    assert_eq!(signed.payload.trusted_keys_ver, 1);
    assert_eq!(signed.payload.revoked_at, None);
    assert_eq!(signed.payload.valid_until, u64::MAX);
    // 响应被根私钥签（客户端可验，防 MITM）
    assert!(verify_response(&signed, &state.hierarchy.root_vk()).unwrap());
}

#[tokio::test]
async fn verify_handler_revoked_by_key_fp() {
    let state = setup();
    let Json(v) = admin_revoke(
        State(state.clone()),
        admin_headers(),
        Json(revoke_req(RevDim::KeyFp, "deadbeef", "key leaked")),
    )
    .await
    .unwrap();
    assert_eq!(v["crl_version"], 2);
    // 被吊销 key → revoked + 原因
    let Json(signed) = verify(
        State(state.clone()),
        Json(verify_req(Some("deadbeef"), None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(signed.payload.code, "revoked");
    assert!(signed.payload.revoked_at.is_some());
    assert_eq!(signed.payload.reason.as_deref(), Some("key leaked"));
    assert_eq!(signed.payload.crl_ver, 2);
    assert_eq!(signed.payload.trusted_keys_ver, 1);
    assert!(verify_response(&signed, &state.hierarchy.root_vk()).unwrap());
    // 未吊销的其他 key → valid
    let Json(s2) = verify(
        State(state.clone()),
        Json(verify_req(Some("other-key"), None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(s2.payload.code, "valid");
}

// ===================== /v1/crl + /v1/trusted-keys + /v1/crl/query =====================

#[tokio::test]
async fn get_crl_signed_and_versioned() {
    let _g = CRL_ENV_LOCK.lock();
    // SAFETY: CRL_ENV_LOCK held——本二进制内所有走 get_crl 的测试共享这把锁；先清防上轮残留。
    unsafe { std::env::remove_var("NEMESIS_DEBUG_CRL_500") };
    let state = setup();
    let Json(empty) = get_crl(State(state.clone())).await.unwrap();
    assert_eq!(empty.payload.version, 1);
    assert!(empty.payload.entries.is_empty());
    assert_eq!(empty.payload.valid_until, u64::MAX);
    assert!(verify_response(&empty, &state.hierarchy.root_vk()).unwrap());
    // 加一条吊销 → version +1 且带签名
    state
        .store
        .add_revoke(crl_entry(RevDim::SigHash, "ab", 7, "r"))
        .unwrap();
    let Json(one) = get_crl(State(state.clone())).await.unwrap();
    assert_eq!(one.payload.version, 2);
    assert_eq!(one.payload.entries.len(), 1);
    assert_eq!(one.payload.entries[0].value, "ab");
    assert!(verify_response(&one, &state.hierarchy.root_vk()).unwrap());
}

#[tokio::test]
async fn get_crl_debug_env_switch_forces_500() {
    let _g = CRL_ENV_LOCK.lock();
    let state = setup();
    // =1 → 500
    // SAFETY: CRL_ENV_LOCK held；测试尾 remove_var 清理，且其他 get_crl 测试拿锁后先清。
    unsafe { std::env::set_var("NEMESIS_DEBUG_CRL_500", "1") };
    let err = get_crl(State(state.clone())).await.unwrap_err();
    assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(err.1, "debug: CRL forced 500");
    // =TRUE（大小写不敏感分支）
    unsafe { std::env::set_var("NEMESIS_DEBUG_CRL_500", "TRUE") };
    let err = get_crl(State(state.clone())).await.unwrap_err();
    assert_eq!(err.0, StatusCode::INTERNAL_SERVER_ERROR);
    // =0 → 不触发
    unsafe { std::env::set_var("NEMESIS_DEBUG_CRL_500", "0") };
    assert!(get_crl(State(state.clone())).await.is_ok());
    // 清理
    unsafe { std::env::remove_var("NEMESIS_DEBUG_CRL_500") };
}

#[tokio::test]
async fn crl_query_valid_then_revoked_by_multiple_dims() {
    let state = setup();
    // 无吊销 → valid，字段全空
    let Json(signed) = crl_query(
        State(state.clone()),
        Json(OcspReq {
            key_fp: Some("k1".into()),
            sig_hash: None,
            content_hash: None,
            publisher: None,
        }),
    )
    .await
    .unwrap();
    let r = &signed.payload;
    assert_eq!(r.code, "valid");
    assert_eq!(r.dim, None);
    assert_eq!(r.value, None);
    assert_eq!(r.revoked_at, None);
    assert_eq!(r.reason, None);
    assert_eq!(r.crl_ver, 1);
    assert!(verify_response(&signed, &state.hierarchy.root_vk()).unwrap());
    // key_fp 维度命中（固定 revoked_at，确定性）
    state
        .store
        .add_revoke(crl_entry(RevDim::KeyFp, "k1", 555, "leak"))
        .unwrap();
    let Json(s2) = crl_query(
        State(state.clone()),
        Json(OcspReq {
            key_fp: Some("k1".into()),
            sig_hash: None,
            content_hash: None,
            publisher: None,
        }),
    )
    .await
    .unwrap();
    let r = &s2.payload;
    assert_eq!(r.code, "revoked");
    assert_eq!(r.dim, Some(RevDim::KeyFp));
    assert_eq!(r.value.as_deref(), Some("k1"));
    assert_eq!(r.revoked_at, Some(555));
    assert_eq!(r.reason.as_deref(), Some("leak"));
    assert_eq!(r.crl_ver, 2);
    // publisher 维度命中（纯查 CRL，不走 trusted_keys 准入）
    state
        .store
        .add_revoke(crl_entry(RevDim::Publisher, "evil-corp", 666, "rogue"))
        .unwrap();
    let Json(s3) = crl_query(
        State(state.clone()),
        Json(OcspReq {
            key_fp: None,
            sig_hash: None,
            content_hash: None,
            publisher: Some("evil-corp".into()),
        }),
    )
    .await
    .unwrap();
    assert_eq!(s3.payload.code, "revoked");
    assert_eq!(s3.payload.dim, Some(RevDim::Publisher));
    assert_eq!(s3.payload.revoked_at, Some(666));
}

// ===================== admin 鉴权 + revoke/trusted-key 流 =====================

#[tokio::test]
async fn admin_endpoints_require_admin_token() {
    let state = setup();
    // 无 header → 401
    let (code, msg) = admin_revoke(
        State(state.clone()),
        HeaderMap::new(),
        Json(revoke_req(RevDim::Publisher, "bad-pub", "cleanup")),
    )
    .await
    .unwrap_err();
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(msg, "invalid admin token");
    // 错 token → 401
    let (code, _) = admin_revoke(
        State(state.clone()),
        bearer_headers("wrong"),
        Json(revoke_req(RevDim::Publisher, "bad-pub", "cleanup")),
    )
    .await
    .unwrap_err();
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    // 对 token → 200 + crl_version
    let Json(v) = admin_revoke(
        State(state.clone()),
        admin_headers(),
        Json(revoke_req(RevDim::Publisher, "bad-pub", "cleanup")),
    )
    .await
    .unwrap();
    assert_eq!(v["crl_version"], 2);
    // 审计留痕（revoke 行动 + 维度/值/原因全记）
    let Json(audit) = get_audit(State(state.clone()), admin_headers())
        .await
        .unwrap();
    let hit = audit
        .iter()
        .find(|a| a.action == "revoke")
        .expect("audit record for revoke");
    assert_eq!(hit.dim.as_deref(), Some("publisher"));
    assert_eq!(hit.value.as_deref(), Some("bad-pub"));
    assert_eq!(hit.reason.as_deref(), Some("cleanup"));
    // get_audit 本身也要 token
    assert_eq!(
        get_audit(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn trusted_key_lifecycle_through_handlers() {
    let state = setup();
    // 上架 Active → trusted_keys_version +1
    let tk = TrustedKey {
        key_fp: "fingerprint-1".into(),
        status: KeyStatus::Active,
        not_after: None,
    };
    let Json(v) = admin_trusted_key(State(state.clone()), admin_headers(), Json(tk.clone()))
        .await
        .unwrap();
    assert_eq!(v["trusted_keys_version"], 2);
    // trusted_keys 非空后：带该 fp 且未吊销 → valid（准入放行）
    let Json(s1) = verify(
        State(state.clone()),
        Json(verify_req(Some("fingerprint-1"), None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(s1.payload.code, "valid");
    // 同 fp 轮换为 Revoked → untrusted
    let tk2 = TrustedKey {
        key_fp: "fingerprint-1".into(),
        status: KeyStatus::Revoked,
        not_after: None,
    };
    let Json(v2) = admin_trusted_key(State(state.clone()), admin_headers(), Json(tk2))
        .await
        .unwrap();
    assert_eq!(v2["trusted_keys_version"], 3);
    let Json(s2) = verify(
        State(state.clone()),
        Json(verify_req(Some("fingerprint-1"), None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(s2.payload.code, "untrusted");
    // 不带 key_fp → untrusted（准入拦截）
    let Json(s3) = verify(
        State(state.clone()),
        Json(verify_req(None, None, None, None)),
    )
    .await
    .unwrap();
    assert_eq!(s3.payload.code, "untrusted");
    // get_trusted_keys 端点：版本 + 内容 + 签名
    let Json(tkl) = get_trusted_keys(State(state.clone())).await.unwrap();
    assert_eq!(tkl.payload.version, 3);
    assert_eq!(tkl.payload.keys.len(), 1);
    assert_eq!(tkl.payload.keys[0].status, KeyStatus::Revoked);
    assert!(verify_response(&tkl, &state.hierarchy.root_vk()).unwrap());
    // 审计 trust_upsert
    let Json(audit) = get_audit(State(state.clone()), admin_headers())
        .await
        .unwrap();
    assert!(
        audit
            .iter()
            .any(|a| a.action == "trust_upsert" && a.value.as_deref() == Some("fingerprint-1"))
    );
}

// ===================== 用户 / 发行方管理 =====================

async fn create_user(
    state: &Arc<AppState>,
    name: &str,
    publisher: Option<&str>,
    issuer_name: Option<&str>,
) -> String {
    let Json(v) = admin_create_user(
        State(state.clone()),
        admin_headers(),
        Json(CreateUserReq {
            name: name.to_string(),
            publisher: publisher.map(String::from),
            issuer_name: issuer_name.map(String::from),
        }),
    )
    .await
    .expect("create user via handler");
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn admin_create_user_default_and_missing_issuer() {
    let state = setup();
    // 未鉴权 → 401
    assert_eq!(
        admin_create_user(
            State(state.clone()),
            HeaderMap::new(),
            Json(CreateUserReq {
                name: "n".into(),
                publisher: None,
                issuer_name: None,
            }),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    // 缺省 issuer_name=default；token 为 32 字节 hex
    let Json(v) = admin_create_user(
        State(state.clone()),
        admin_headers(),
        Json(CreateUserReq {
            name: "dave".into(),
            publisher: Some("dave-pub".into()),
            issuer_name: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(v["name"], "dave");
    assert_eq!(v["issuer_name"], "default");
    let token = v["token"].as_str().unwrap();
    assert_eq!(token.len(), 64, "token 应为 32 字节 hex");
    assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    // store 中可按 token 查到
    let u = state
        .store
        .get_user_by_token(token)
        .unwrap()
        .expect("user stored");
    assert_eq!(u.name, "dave");
    assert_eq!(u.publisher.as_deref(), Some("dave-pub"));
    assert_eq!(u.issuer_name, "default");
    // 不存在的发行方 → 400
    let (code, msg) = admin_create_user(
        State(state.clone()),
        admin_headers(),
        Json(CreateUserReq {
            name: "x".into(),
            publisher: None,
            issuer_name: Some("ghost".into()),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(msg, "issuer 'ghost' not found");
    // 先建发行方再建用户 → 成功
    let _ = admin_create_issuer(
        State(state.clone()),
        admin_headers(),
        Json(CreateIssuerReq {
            name: "acme".into(),
        }),
    )
    .await
    .unwrap();
    let Json(v2) = admin_create_user(
        State(state.clone()),
        admin_headers(),
        Json(CreateUserReq {
            name: "eve".into(),
            publisher: None,
            issuer_name: Some("acme".into()),
        }),
    )
    .await
    .unwrap();
    assert_eq!(v2["issuer_name"], "acme");
}

#[tokio::test]
async fn admin_create_issuer_and_cert_chain_valid() {
    let state = setup();
    // default 保留名 → 400
    let (code, msg) = admin_create_issuer(
        State(state.clone()),
        admin_headers(),
        Json(CreateIssuerReq {
            name: "default".into(),
        }),
    )
    .await
    .unwrap_err();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(msg, "name 'default' is reserved");
    // 未鉴权 → 401
    assert_eq!(
        admin_create_issuer(
            State(state.clone()),
            HeaderMap::new(),
            Json(CreateIssuerReq { name: "x".into() }),
        )
        .await
        .unwrap_err()
        .0,
        StatusCode::UNAUTHORIZED
    );
    // 正常创建
    let Json(v) = admin_create_issuer(
        State(state.clone()),
        admin_headers(),
        Json(CreateIssuerReq {
            name: "acme".into(),
        }),
    )
    .await
    .unwrap();
    assert_eq!(v["name"], "acme");
    let issuer_pub = v["issuer_pub"].as_str().unwrap().to_string();
    // P-256 公钥 SEC1 uncompressed 65B → hex 130 字符
    assert_eq!(issuer_pub.len(), 130);
    // 落库字段完整 + 私钥也存（server 代签用；P-256 标量 32B → hex 64 字符）
    let rec = state
        .store
        .get_issuer_by_name("acme")
        .unwrap()
        .expect("issuer stored");
    assert_eq!(rec.issuer_pub, issuer_pub);
    assert_eq!(rec.issuer_sk.len(), 64);
    // 链可解析为 [leaf, issuing, root]（v4 含根），leaf 公钥绑定 + 全链验到根
    let chain_bytes = nemesis_verify::hex_util::hex_decode_vec(&rec.chain).unwrap();
    let chain = nemesis_verify::cert::parse_chain_blob(&chain_bytes).unwrap();
    assert_eq!(chain.len(), 3);
    let issuer_vk = nemesis_verify::crypto::verifying_key_from_hex(&issuer_pub).unwrap();
    // 链的 leaf 公钥 == 发行方公钥（签名键绑定）
    assert_eq!(
        nemesis_verify::crypto::public_key_bytes(&chain[0].subject_public_key().unwrap()),
        nemesis_verify::crypto::public_key_bytes(&issuer_vk)
    );
    // 全链验证（叶子→发行锚→根，逐级签名/窗口/KU/EKU）
    nemesis_verify::cert::verify_chain(&chain, &chain[2].sha256_fingerprint(), now_secs()).unwrap();
    // 链的根 == 本服务密钥体系的根
    assert_eq!(
        chain[2].sha256_fingerprint(),
        state.hierarchy.root_cert.sha256_fingerprint()
    );
    // 证书 subject CN == 发行方名
    assert_eq!(chain[0].subject_cn().unwrap().as_deref(), Some("acme"));
    // 审计留痕（issuer_create，detail=公钥）
    let Json(audit) = get_audit(State(state.clone()), admin_headers())
        .await
        .unwrap();
    let hit = audit
        .iter()
        .find(|a| a.action == "issuer_create")
        .expect("audit record for issuer_create");
    assert_eq!(hit.value.as_deref(), Some("acme"));
    assert_eq!(hit.detail.as_deref(), Some(issuer_pub.as_str()));
}

#[tokio::test]
async fn list_issuers_shape_and_no_private_key_leak() {
    let state = setup();
    // 未鉴权 401
    assert_eq!(
        list_issuers(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    // 空 → []
    let Json(v) = list_issuers(State(state.clone()), admin_headers())
        .await
        .unwrap();
    assert_eq!(v, serde_json::json!([]));
    // 建 2 个 → 数组 2 项，字段恰好 {name, issuer_pub, created_at}（不泄漏 issuer_sk/cert/chain）
    for name in ["i-one", "i-two"] {
        let _ = admin_create_issuer(
            State(state.clone()),
            admin_headers(),
            Json(CreateIssuerReq { name: name.into() }),
        )
        .await
        .unwrap();
    }
    let Json(v) = list_issuers(State(state.clone()), admin_headers())
        .await
        .unwrap();
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    for obj in arr {
        let obj = obj.as_object().unwrap();
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["created_at", "issuer_pub", "name"]);
        assert_eq!(obj["issuer_pub"].as_str().unwrap().len(), 130);
    }
}

#[tokio::test]
async fn list_users_endpoint_requires_admin_and_lists() {
    let state = setup();
    assert_eq!(
        list_users(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    let Json(v) = list_users(State(state.clone()), admin_headers())
        .await
        .unwrap();
    assert!(v.is_empty());
    create_user(&state, "carol", None, None).await;
    let Json(v) = list_users(State(state.clone()), admin_headers())
        .await
        .unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].name, "carol");
    assert_eq!(v[0].issuer_name, "default");
    assert!(v[0].active);
}

// ===================== /v1/sign（multipart 构造 + 全闭环）=====================

/// 手工拼 multipart/form-data 请求体（file 二进制 + 可选 publisher 文本字段）。
fn multipart_body(file: Option<&[u8]>, publisher: Option<&str>) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(bytes) = file {
        body.extend_from_slice(
            format!(
                "--{MP_BOUNDARY}\r\ncontent-disposition: form-data; \
                 name=\"file\"; filename=\"payload.bin\"\r\n\
                 content-type: application/octet-stream\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    if let Some(p) = publisher {
        body.extend_from_slice(
            format!(
                "--{MP_BOUNDARY}\r\ncontent-disposition: form-data; name=\"publisher\"\r\n\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(p.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{MP_BOUNDARY}--\r\n").as_bytes());
    body
}

/// 从手工构造的请求体建 Multipart 提取器（Multipart 无法直接 new，走 FromRequest）。
async fn multipart_of(body: Vec<u8>) -> Multipart {
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/sign")
        .header(
            "content-type",
            format!("multipart/form-data; boundary={MP_BOUNDARY}"),
        )
        .body(Body::from(body))
        .unwrap();
    Multipart::from_request(req, &())
        .await
        .expect("build Multipart from hand-crafted request")
}

async fn sign_file(
    state: &Arc<AppState>,
    token: &str,
    file: Option<&[u8]>,
    publisher: Option<&str>,
) -> Result<Response, (StatusCode, String)> {
    let mp = multipart_of(multipart_body(file, publisher)).await;
    sign_upload(State(state.clone()), bearer_headers(token), mp).await
}

async fn resp_bytes(resp: Response) -> Vec<u8> {
    axum::body::to_bytes(resp.into_body(), 8 << 20)
        .await
        .expect("read response body")
        .to_vec()
}

#[tokio::test]
async fn sign_upload_default_issuer_full_circle() {
    let state = setup();
    let token = create_user(&state, "alice", Some("alice-software"), None).await;
    // 错 user token → 401
    let (code, msg) = sign_file(&state, "totally-wrong", Some(b"payload-a"), None)
        .await
        .unwrap_err();
    assert_eq!(code, StatusCode::UNAUTHORIZED);
    assert_eq!(msg, "invalid user token");
    // 缺 file 字段 → 400
    let (code, msg) = sign_file(&state, &token, None, Some("whoever"))
        .await
        .unwrap_err();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(msg, "missing 'file' field");
    // 未知字段名被忽略（wildcard 分支）+ 仍缺 file → 400
    let unknown_field_body = format!(
        "--{MP_BOUNDARY}\r\ncontent-disposition: form-data; \
         name=\"unknown-field\"\r\n\r\nwhatever\r\n--{MP_BOUNDARY}--\r\n"
    );
    let mp = multipart_of(unknown_field_body.into_bytes()).await;
    let (code, _) = sign_upload(State(state.clone()), bearer_headers(&token), mp)
        .await
        .unwrap_err();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    // 正常签发（user.publisher 生效，无 multipart publisher 覆盖）
    let content: &[u8] = b"nemesis revoke-server sign test payload alpha";
    let resp = sign_file(&state, &token, Some(content), None)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap(),
        "application/octet-stream"
    );
    let disp = resp
        .headers()
        .get("content-disposition")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(disp.starts_with("attachment; filename="), "disp: {disp}");
    let signed = resp_bytes(resp).await;
    // content_hash 先行（v4_content_digest 单一真相源，与 handler registry 记账
    // 同口径——raw = SHA-256 全文件；PE 会走 authenticode digest，见 PE 专项测试）
    let expected_ch: [u8; 32] =
        nemesis_verify::verify::v4_content_digest(content).expect("v4_content_digest raw");
    // v4 CMS 签发（S5-2）：sig_hash = SHA-256(SignerInfo.signature DER) 必然可解析
    let sig_hash: [u8; 32] = latest_sig_hash(&signed).expect("v4 CMS 签名可解析出 sig_hash");
    // 端到端 Valid：v4 验证管线（链到根锚 + digest + signedAttrs 验签）全过，
    // pubkey = 签名者（keygen leaf）证书 SPKI
    let t0 = now_secs();
    match nemesis_verify::verify::verify_bytes(
        &signed,
        &[state.hierarchy.root_anchor_fingerprint()],
        now_secs(),
    ) {
        VerifyOutcome::Valid {
            signed_at, pubkey, ..
        } => {
            assert!(signed_at >= t0, "signed_at 应为服务端签发墙钟");
            assert_eq!(
                pubkey,
                nemesis_verify::crypto::public_key_bytes(&state.hierarchy.leaf_vk()),
            );
        }
        o => panic!("expected Valid, got {o:?}"),
    }
    // registry 记录逐字段核对
    let recs = state.store.list_signatures(10).unwrap();
    assert_eq!(recs.len(), 1);
    let rec = &recs[0];
    assert_eq!(rec.sig_hash, hex_str(&sig_hash));
    let expected_fp: [u8; 32] = nemesis_verify::crypto::key_fp(
        &nemesis_verify::crypto::public_key_bytes(&state.hierarchy.leaf_vk()),
    );
    assert_eq!(rec.key_fp, hex_str(&expected_fp));
    assert_eq!(rec.publisher.as_deref(), Some("alice-software"));
    assert_eq!(rec.user_name.as_deref(), Some("alice"));
    assert_eq!(rec.issuer_name.as_deref(), Some("default"));
    // content_hash 与 codec 直算一致
    assert_eq!(rec.content_hash, hex_str(&expected_ch));
    // 签发记录经 admin 端点可查（401 + 200）
    assert_eq!(
        list_signatures(State(state.clone()), HeaderMap::new())
            .await
            .unwrap_err()
            .0,
        StatusCode::UNAUTHORIZED
    );
    let Json(list) = list_signatures(State(state.clone()), admin_headers())
        .await
        .unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].sig_hash, rec.sig_hash);
}

#[tokio::test]
async fn sign_upload_publisher_override_and_user_fallback() {
    let state = setup();
    let token = create_user(&state, "bob", Some("bob-default-pub"), None).await;
    // multipart publisher 字段覆盖 user.publisher
    let resp = sign_file(
        &state,
        &token,
        Some(b"override-content-1"),
        Some("override-pub"),
    )
    .await
    .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    // 不同 content → 不同 sig_hash → REPLACE 不会吞行
    let resp2 = sign_file(&state, &token, Some(b"fallback-content-2"), None)
        .await
        .unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);
    let recs = state.store.list_signatures(10).unwrap();
    assert_eq!(recs.len(), 2);
    assert!(
        recs.iter()
            .any(|r| r.publisher.as_deref() == Some("override-pub")),
        "override publisher record missing"
    );
    assert!(
        recs.iter()
            .any(|r| r.publisher.as_deref() == Some("bob-default-pub")),
        "user fallback publisher record missing"
    );
}

#[tokio::test]
async fn sign_upload_with_dynamic_issuer_uses_its_key_and_chain() {
    let state = setup();
    // admin 创建动态发行方 acme → 用户绑定 acme → 签发走 acme 私钥 + 链
    let Json(v) = admin_create_issuer(
        State(state.clone()),
        admin_headers(),
        Json(CreateIssuerReq {
            name: "acme".into(),
        }),
    )
    .await
    .unwrap();
    let issuer_pub_hex = v["issuer_pub"].as_str().unwrap().to_string();
    let token = create_user(&state, "acme-dev", None, Some("acme")).await;
    let resp = sign_file(&state, &token, Some(b"dynamic issuer payload"), None)
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let signed = resp_bytes(resp).await;
    // 验签 Valid 且 pubkey == 动态发行方公钥（链经发行锚到根）
    let issuer_vk = nemesis_verify::crypto::verifying_key_from_hex(&issuer_pub_hex).unwrap();
    match nemesis_verify::verify::verify_bytes(
        &signed,
        &[state.hierarchy.root_anchor_fingerprint()],
        now_secs(),
    ) {
        VerifyOutcome::Valid { pubkey, .. } => assert_eq!(
            pubkey,
            nemesis_verify::crypto::public_key_bytes(&issuer_vk),
            "签名者 = 动态发行方 leaf 证书 SPKI"
        ),
        o => panic!("expected Valid, got {o:?}"),
    }
    // registry key_fp == 动态发行方公钥指纹
    let recs = state.store.list_signatures(10).unwrap();
    assert_eq!(recs.len(), 1);
    let expected_fp: [u8; 32] =
        nemesis_verify::crypto::key_fp(&nemesis_verify::crypto::public_key_bytes(&issuer_vk));
    assert_eq!(recs[0].key_fp, hex_str(&expected_fp));
    assert_eq!(recs[0].issuer_name.as_deref(), Some("acme"));
    // publisher 为 None（user 无 publisher 且无覆盖）
    assert_eq!(recs[0].publisher, None);
}

#[tokio::test]
async fn sign_upload_unknown_issuer_name_400() {
    let state = setup();
    // 绕过 handler 校验直接写 user，绑定不存在的发行方 → 签发时 400
    state
        .store
        .add_user("tok-ghost", "ghost", None, "ghost-issuer", 1)
        .unwrap();
    let (code, msg) = sign_file(&state, "tok-ghost", Some(b"x"), None)
        .await
        .unwrap_err();
    assert_eq!(code, StatusCode::BAD_REQUEST);
    assert_eq!(msg, "issuer 'ghost-issuer' not found");
}

// ===========================================================================
// PE 载体专项（S5-2）：/v1/sign 对 PE 输入产 Certificate Table 签名，registry
// content_hash 与 CMS messageDigest 同源（authenticode digest）——FileHash 吊销
// 维度的一致性契约。（原 S12b「compute_l Some 分支」覆盖随 v4 签发切换迁至
// nemesis-verify `sign_content_v4_elf_roundtrip_valid`——ELF 臂才走 compute_l。）
// ===========================================================================

/// 无证书表的最小 PE32+（端口自 nemesis-verify verify/tests.rs base_pe，模块私有
/// 不可跨文件）：nrva=16（Security 项全零 = 未签名）、1 个 section（0x200 @ 0x400）。
fn base_pe() -> Vec<u8> {
    let p: usize = 0x40;
    let size_of_opt: usize = 240;
    let sec_tbl = p + 24 + size_of_opt;
    let len = (sec_tbl + 40).max(0x400 + 0x200);
    let mut b = vec![0u8; len];
    b[0] = b'M';
    b[1] = b'Z';
    b[0x3C..0x40].copy_from_slice(&(p as u32).to_le_bytes());
    b[p..p + 4].copy_from_slice(b"PE\0\0");
    b[p + 6..p + 8].copy_from_slice(&1u16.to_le_bytes());
    b[p + 20..p + 22].copy_from_slice(&(size_of_opt as u16).to_le_bytes());
    b[p + 24..p + 26].copy_from_slice(&0x20bu16.to_le_bytes());
    b[p + 132..p + 136].copy_from_slice(&16u32.to_le_bytes()); // NumberOfRvaAndSizes
    b[sec_tbl + 16..sec_tbl + 20].copy_from_slice(&0x200u32.to_le_bytes());
    b[sec_tbl + 20..sec_tbl + 24].copy_from_slice(&0x400u32.to_le_bytes());
    b
}

#[tokio::test]
async fn sign_upload_pe_carrier_full_circle_digest_consistency() {
    let pe = base_pe();
    // 守卫：fixture 必须被判成 PE（否则本测在测假路径）
    assert_eq!(
        nemesis_verify::codec::detect_format(&pe),
        nemesis_verify::codec::FORMAT_TAG_PE,
        "fixture 应被判成 PE"
    );

    let state = setup();
    let token = create_user(&state, "pe-signer", None, None).await;
    let resp = sign_file(&state, &token, Some(&pe), None).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let signed = resp_bytes(resp).await;
    // v4 CMS 签名可 view 解析 + 验证管线 Valid（Certificate Table 定位 + PE
    // authenticode digest 比对全过）
    assert!(
        latest_sig_hash(&signed).is_some(),
        "PE Certificate Table 签名可解析"
    );
    assert!(matches!(
        nemesis_verify::verify::verify_bytes(
            &signed,
            &[state.hierarchy.root_anchor_fingerprint()],
            now_secs()
        ),
        VerifyOutcome::Valid { .. }
    ));
    // registry：content_hash == CMS 内嵌 messageDigest（authenticode digest）——
    // 同源对账：拿 registry 值吊销 FileHash 维，verify_bytes 必须查得中
    let recs = state.store.list_signatures(10).unwrap();
    assert_eq!(recs.len(), 1);
    let expected_hex = hex_str(&nemesis_verify::pe::authenticode_digest(&pe).unwrap()[..]);
    assert_eq!(recs[0].content_hash, expected_hex);
}
