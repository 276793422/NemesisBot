//! axum 路由 handler（吊销 + 签发 + 用户管理）。

use crate::state::{now_secs, AppState};
use crate::store::{dim_str, status_str, AuditRecord, IssuerRecord, SignatureRecord, UserRecord};
use axum::body::Body;
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use rand::rngs::OsRng;
use rand::RngCore;
use nemesis_verify::{
    crl_match, sign_response, Crl, CrlEntry, KeyStatus, RevDim, SignedResponse, TrustedKeyList,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn hex_str(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn extract_token(headers: &HeaderMap) -> &str {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    auth.strip_prefix("Bearer ").unwrap_or(auth)
}

// ===================== /v1/verify =====================

#[derive(Debug, Deserialize)]
pub struct VerifyReq {
    pub key_fp: Option<String>,
    pub sig_hash: Option<String>,
    pub content_hash: Option<String>,
    pub publisher: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VerifyResp {
    pub code: String,
    pub crl_ver: u64,
    pub trusted_keys_ver: u64,
    pub revoked_at: Option<u64>,
    pub reason: Option<String>,
    pub valid_until: u64,
}

pub async fn verify(
    State(state): State<Arc<AppState>>,
    Json(req): Json<VerifyReq>,
) -> Result<Json<SignedResponse<VerifyResp>>, (StatusCode, String)> {
    let crl = state.store.list_crl().map_err(internal)?;
    let tkl = state.store.list_trusted_keys().map_err(internal)?;
    let (code, revoked_at, reason) = compute_status(&crl, &tkl, &req, now_secs());
    let resp = VerifyResp {
        code,
        crl_ver: crl.version,
        trusted_keys_ver: tkl.version,
        revoked_at,
        reason,
        valid_until: crl.valid_until,
    };
    let signed = sign_response(&resp, &state.hierarchy.root_sk).map_err(internal)?;
    Ok(Json(signed))
}

fn compute_status(
    crl: &Crl,
    tkl: &TrustedKeyList,
    req: &VerifyReq,
    now: u64,
) -> (String, Option<u64>, Option<String>) {
    if !tkl.keys.is_empty() {
        let kf = match &req.key_fp {
            Some(k) => k,
            None => return ("untrusted".into(), None, None),
        };
        let active = tkl.keys.iter().any(|k| {
            &k.key_fp == kf
                && k.status == KeyStatus::Active
                && k.not_after.map(|t| now <= t).unwrap_or(true)
        });
        if !active {
            return ("untrusted".into(), None, None);
        }
    }
    let hit = req
        .key_fp
        .as_deref()
        .and_then(|v| crl_match(crl, RevDim::KeyFp, v))
        .or_else(|| req.sig_hash.as_deref().and_then(|v| crl_match(crl, RevDim::SigHash, v)))
        .or_else(|| req.content_hash.as_deref().and_then(|v| crl_match(crl, RevDim::FileHash, v)))
        .or_else(|| req.publisher.as_deref().and_then(|v| crl_match(crl, RevDim::Publisher, v)));
    if let Some(e) = hit {
        return ("revoked".into(), Some(e.revoked_at), Some(e.reason.clone()));
    }
    ("valid".into(), None, None)
}

// ===================== /v1/crl + /v1/trusted-keys =====================

pub async fn get_crl(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SignedResponse<Crl>>, (StatusCode, String)> {
    // 调试开关：模拟 /v1/crl 故障（测 OCSP fallback 路径：CRL 挂但 /v1/crl/query 活）。默认关。
    if std::env::var("NEMESIS_DEBUG_CRL_500")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, "debug: CRL forced 500".into()));
    }
    let crl = state.store.list_crl().map_err(internal)?;
    let signed = sign_response(&crl, &state.hierarchy.root_sk).map_err(internal)?;
    Ok(Json(signed))
}

pub async fn get_trusted_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SignedResponse<TrustedKeyList>>, (StatusCode, String)> {
    let tkl = state.store.list_trusted_keys().map_err(internal)?;
    let signed = sign_response(&tkl, &state.hierarchy.root_sk).map_err(internal)?;
    Ok(Json(signed))
}

// ===================== /v1/crl/query（OCSP-like 单条查询）=====================

/// OCSP-like 单条查询：纯查 CRL（不查 trusted_keys），返被根签的 OcspResp。
/// 与 `/v1/verify` 区别：verify 含 trusted_keys 准入检查；本端点纯吊销查询（DLL 用）。
pub async fn crl_query(
    State(state): State<Arc<AppState>>,
    Json(req): Json<nemesis_verify::revocation::OcspReq>,
) -> Result<Json<SignedResponse<nemesis_verify::revocation::OcspResp>>, (StatusCode, String)> {
    let crl = state.store.list_crl().map_err(internal)?;
    let hit = req
        .key_fp
        .as_deref()
        .and_then(|v| crl_match(&crl, RevDim::KeyFp, v))
        .or_else(|| req.sig_hash.as_deref().and_then(|v| crl_match(&crl, RevDim::SigHash, v)))
        .or_else(|| req.content_hash.as_deref().and_then(|v| crl_match(&crl, RevDim::FileHash, v)))
        .or_else(|| req.publisher.as_deref().and_then(|v| crl_match(&crl, RevDim::Publisher, v)));
    let resp = match hit {
        Some(e) => nemesis_verify::revocation::OcspResp {
            code: "revoked".into(),
            dim: Some(e.dim),
            value: Some(e.value.clone()),
            revoked_at: Some(e.revoked_at),
            reason: Some(e.reason.clone()),
            crl_ver: crl.version,
        },
        None => nemesis_verify::revocation::OcspResp {
            code: "valid".into(),
            dim: None,
            value: None,
            revoked_at: None,
            reason: None,
            crl_ver: crl.version,
        },
    };
    let signed = sign_response(&resp, &state.hierarchy.root_sk).map_err(internal)?;
    Ok(Json(signed))
}

// ===================== /v1/admin/revoke + trusted-key =====================

#[derive(Debug, Deserialize)]
pub struct RevokeReq {
    pub dim: RevDim,
    pub value: String,
    pub reason: String,
}

pub async fn admin_revoke(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<RevokeReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let entry = CrlEntry {
        dim: req.dim,
        value: req.value.clone(),
        reason: req.reason.clone(),
        revoked_at: now_secs(),
    };
    let ver = state.store.add_revoke(entry).map_err(internal)?;
    state
        .store
        .add_audit(AuditRecord {
            id: 0,
            timestamp: now_secs(),
            action: "revoke".into(),
            operator: "admin".into(),
            dim: Some(dim_str(req.dim).to_string()),
            value: Some(req.value),
            reason: Some(req.reason),
            detail: None,
        })
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "crl_version": ver })))
}

pub async fn admin_trusted_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<nemesis_verify::TrustedKey>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let ver = state.store.upsert_trusted_key(req.clone()).map_err(internal)?;
    state
        .store
        .add_audit(AuditRecord {
            id: 0,
            timestamp: now_secs(),
            action: "trust_upsert".into(),
            operator: "admin".into(),
            dim: None,
            value: Some(req.key_fp),
            reason: Some(status_str(req.status).to_string()),
            detail: None,
        })
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "trusted_keys_version": ver })))
}

// ===================== /v1/audit + /v1/signatures + /v1/admin/users =====================

pub async fn get_audit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditRecord>>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    Ok(Json(state.store.list_audit(200).map_err(internal)?))
}

pub async fn list_signatures(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<SignatureRecord>>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    Ok(Json(state.store.list_signatures(200).map_err(internal)?))
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserRecord>>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    Ok(Json(state.store.list_users().map_err(internal)?))
}

// ===================== /v1/sign（云端签发 — 文件上传）=====================

pub async fn sign_upload(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Response, (StatusCode, String)> {
    // 验 user token（签发鉴权，不同用户不同 token）
    let user_token = extract_token(&headers);
    let user = state
        .store
        .get_user_by_token(user_token)
        .map_err(internal)?
        .ok_or((StatusCode::UNAUTHORIZED, "invalid user token".into()))?;

    // multipart 解析：file（二进制）+ publisher（可选文本）
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut publisher_override: Option<String> = None;
    while let Some(field) = multipart.next_field().await.map_err(internal)? {
        match field.name() {
            Some("file") => {
                file_bytes = Some(field.bytes().await.map_err(internal)?.to_vec());
            }
            Some("publisher") => {
                publisher_override = Some(field.text().await.map_err(internal)?);
            }
            _ => {}
        }
    }
    let file_bytes = file_bytes.ok_or((StatusCode::BAD_REQUEST, "missing 'file' field".into()))?;
    let publisher = publisher_override.or(user.publisher);

    let signed_at = now_secs();

    // 按用户关联发行方选 issuer 私钥 + 链（default=keygen 默认 issuer；其他=动态发行方）
    let (issuer_sk, issuer_chain, issuer_pub): (
        ed25519_dalek::SigningKey,
        Vec<u8>,
        ed25519_dalek::VerifyingKey,
    ) = if user.issuer_name == "default" {
        (
            state.hierarchy.issuer_sk.clone(),
            state.hierarchy.issuer_chain_bytes.clone(),
            state.hierarchy.issuer_vk,
        )
    } else {
        let issuer = state
            .store
            .get_issuer_by_name(&user.issuer_name)
            .map_err(internal)?
            .ok_or((StatusCode::BAD_REQUEST, format!("issuer '{}' not found", user.issuer_name)))?;
        let sk = nemesis_verify::crypto::signing_key_from_hex(&issuer.issuer_sk).map_err(internal)?;
        let chain = nemesis_verify::hex_util::hex_decode_vec(&issuer.chain).map_err(internal)?;
        let vk = nemesis_verify::crypto::verifying_key_from_hex(&issuer.issuer_pub).map_err(internal)?;
        (sk, chain, vk)
    };

    // v3 签发：发行方私钥签 content，envelope 带 pubkey + 完整证书链（issuer→CA→root）
    let signed_file = nemesis_verify::verify::sign_content(
        &file_bytes,
        &issuer_sk,
        signed_at,
        Some(&issuer_chain),
        publisher.as_deref(),
        None,
        None,
    )
    .map_err(internal)?;

    // registry 元数据：content_hash（codec 算）+ key_fp（发行方公钥指纹）
    let codec = nemesis_verify::codec::detect_codec(&file_bytes);
    let content_len = match codec.compute_l(&file_bytes).map_err(internal)? {
        Some(l) => l,
        None => file_bytes.len(),
    };
    let content_hash: [u8; 32] = codec.content_hash(&file_bytes, content_len).map_err(internal)?;
    let key_fp: [u8; 32] = Sha256::digest(issuer_pub.to_bytes()).into();
    // sig_hash = SHA-256(signature)，从签名文件 envelope 解析（CRL 单签名吊销维度）。
    // sign_content 刚签完 envelope 必在，失败兜底 content_hash（理论上不触发）。
    let sig_hash: [u8; 32] = nemesis_verify::view::latest_sig_hash(&signed_file)
        .unwrap_or_else(|| Sha256::digest(content_hash).into());

    state
        .store
        .add_signature(&SignatureRecord {
            sig_hash: hex_str(&sig_hash),
            key_fp: hex_str(&key_fp),
            publisher: publisher.clone(),
            signed_at,
            content_hash: hex_str(&content_hash),
            user_name: Some(user.name.clone()),
            issuer_name: Some(user.issuer_name.clone()),
            registered_at: now_secs(),
        })
        .map_err(internal)?;

    // 返签名文件 binary（浏览器下载）
    Ok(Response::builder()
        .header("content-type", "application/octet-stream")
        .header("content-disposition", "attachment; filename=\"signed-file\"")
        .body(Body::from(signed_file))
        .map_err(|e| internal(e))?)
}

// ===================== /v1/admin/user（创建用户/发 token）=====================

#[derive(Debug, Deserialize)]
pub struct CreateUserReq {
    pub name: String,
    pub publisher: Option<String>,
    /// 关联发行方（缺省 default = keygen 默认 issuer）
    pub issuer_name: Option<String>,
}

pub async fn admin_create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateUserReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let issuer_name = req.issuer_name.clone().unwrap_or_else(|| "default".to_string());
    // 校验发行方存在（default 跳过；动态发行方要在 issuers 表）
    if issuer_name != "default" {
        let exists = state.store.get_issuer_by_name(&issuer_name).map_err(internal)?;
        if exists.is_none() {
            return Err((StatusCode::BAD_REQUEST, format!("issuer '{}' not found", issuer_name)));
        }
    }
    let mut token_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut token_bytes);
    let token = hex_str(&token_bytes);
    state
        .store
        .add_user(&token, &req.name, req.publisher.as_deref(), &issuer_name, now_secs())
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "token": token, "name": req.name, "issuer_name": issuer_name })))
}

// ===================== /v1/admin/issuer（创建动态发行方证书）=====================

#[derive(Debug, Deserialize)]
pub struct CreateIssuerReq {
    pub name: String,
}

/// 创建发行方：生成 Ed25519 keypair → CA 私钥签 issuer 证书 → 存 issuers 表。
/// 私钥 server 持有（开发者拿 token，签发时 server 用对应发行方私钥签）。
pub async fn admin_create_issuer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateIssuerReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    if req.name == "default" {
        return Err((StatusCode::BAD_REQUEST, "name 'default' is reserved".into()));
    }
    let kp = nemesis_verify::crypto::generate_key_pair();
    let issuer_vk = nemesis_verify::crypto::verifying_key_from_hex(&kp.public_key).map_err(internal)?;
    // CA 签 issuer 证书（有效期 [0, MAX]）
    let issuer_cert = nemesis_verify::cert::issue_certificate(
        &state.hierarchy.ca_sk,
        &issuer_vk.to_bytes(),
        req.name.as_bytes(),
        0,
        u64::MAX,
    );
    // chain = [issuer_cert, ca_cert]（leaf 在前，不含根证书）
    let chain = nemesis_verify::cert::serialize_chain(&[issuer_cert.clone(), state.hierarchy.ca_cert.clone()]);
    let rec = IssuerRecord {
        name: req.name.clone(),
        issuer_sk: kp.private_key,
        issuer_pub: kp.public_key,
        issuer_cert: hex_str(&issuer_cert.to_bytes()),
        chain: hex_str(&chain),
        created_at: now_secs(),
    };
    state.store.add_issuer(&rec).map_err(internal)?;
    state
        .store
        .add_audit(AuditRecord {
            id: 0,
            timestamp: now_secs(),
            action: "issuer_create".into(),
            operator: "admin".into(),
            dim: None,
            value: Some(req.name.clone()),
            reason: None,
            detail: Some(rec.issuer_pub.clone()),
        })
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "name": req.name, "issuer_pub": rec.issuer_pub })))
}

/// 列发行方（不返私钥）。
pub async fn list_issuers(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let issuers = state.store.list_issuers().map_err(internal)?;
    let out: Vec<serde_json::Value> = issuers
        .iter()
        .map(|i| {
            serde_json::json!({ "name": i.name, "issuer_pub": i.issuer_pub, "created_at": i.created_at })
        })
        .collect();
    Ok(Json(serde_json::Value::Array(out)))
}

// ===================== health + 鉴权 =====================

pub async fn health() -> &'static str {
    "ok"
}

fn check_admin(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    if state.admin_token.is_empty() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "admin token not configured (refuse all admin ops)".into(),
        ));
    }
    let token = extract_token(headers);
    if token.is_empty() || token != state.admin_token {
        return Err((StatusCode::UNAUTHORIZED, "invalid admin token".into()));
    }
    Ok(())
}
