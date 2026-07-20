//! axum 路由 handler（吊销 + 签发 + 用户管理）。

use crate::state::{now_secs, AppState};
use crate::store::{dim_str, status_str, AuditRecord, SignatureRecord, UserRecord};
use axum::body::Body;
use axum::extract::{Multipart, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::Json;
use rand::rngs::OsRng;
use rand::RngCore;
use revoke_common::{
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
    let signed = sign_response(&resp, &state.crkey).map_err(internal)?;
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
        .and_then(|v| crl_match(crl, RevDim::KeyId, v))
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
    let crl = state.store.list_crl().map_err(internal)?;
    let signed = sign_response(&crl, &state.crkey).map_err(internal)?;
    Ok(Json(signed))
}

pub async fn get_trusted_keys(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SignedResponse<TrustedKeyList>>, (StatusCode, String)> {
    let tkl = state.store.list_trusted_keys().map_err(internal)?;
    let signed = sign_response(&tkl, &state.crkey).map_err(internal)?;
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
    Json(req): Json<revoke_common::TrustedKey>,
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

    // codec 算 PE 感知 content_hash
    let codec = revoke_common::codec::detect_codec(&file_bytes);
    let format_tag = revoke_common::codec::detect_format(&file_bytes);
    let content_len = match codec.compute_l(&file_bytes).map_err(internal)? {
        Some(l) => l,
        None => file_bytes.len(),
    };
    let content_hash = codec.content_hash(&file_bytes, content_len).map_err(internal)?;

    // Ed25519 签名（signing_key 签 content_hash）
    let vk = state.signing_key.verifying_key();
    let fp: [u8; 32] = Sha256::digest(vk.to_bytes()).into();
    let signed_at = now_secs();
    let signed_meta = revoke_common::envelope::build_signed_meta(
        format_tag, 0u16, signed_at, 0, &fp, &content_hash, publisher.as_deref(), None,
    );
    let mut signing_msg = Vec::with_capacity(revoke_common::envelope::DOMAIN.len() + signed_meta.len());
    signing_msg.extend_from_slice(revoke_common::envelope::DOMAIN);
    signing_msg.extend_from_slice(&signed_meta);
    let signature = revoke_common::crypto::ed25519_sign(&state.signing_key, &signing_msg);
    let sig_hash: [u8; 32] = Sha256::digest(signature).into();

    // envelope 构造（与 exe-sign-tool sign 一致）
    let body_plain = revoke_common::envelope::build_body_plaintext(
        0u16, signed_at, 0, &fp, &signature, &content_hash,
        publisher.as_deref(), None, &sig_hash,
    );
    let sym_key = state.sym_key;
    let body_len = body_plain.len() + revoke_common::envelope::AEAD_TAG_LEN;
    let total_len = revoke_common::envelope::align_up(
        body_len + revoke_common::envelope::FOOTER_LEN,
        revoke_common::envelope::ENVELOPE_ALIGN,
    );
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let footer = revoke_common::envelope::build_footer(
        format_tag, total_len, body_len, content_len, &nonce,
    );
    let ciphertext = revoke_common::crypto::aead_seal(
        &sym_key, &nonce, &body_plain, &footer[..revoke_common::envelope::FOOTER_AAD_LEN],
    )
    .map_err(internal)?;
    let envelope = revoke_common::envelope::assemble_envelope(&ciphertext, &footer);

    // attach（签名文件 = 原始文件 + envelope）
    let mut signed_file = file_bytes;
    signed_file.extend_from_slice(&envelope);

    // registry（签发记录）
    state
        .store
        .add_signature(&SignatureRecord {
            sig_hash: hex_str(&sig_hash),
            key_fp: hex_str(&fp),
            publisher: publisher.clone(),
            signed_at,
            content_hash: hex_str(&content_hash),
            user_name: Some(user.name.clone()),
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
}

pub async fn admin_create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<CreateUserReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let mut token_bytes = [0u8; 32];
    OsRng.fill_bytes(&mut token_bytes);
    let token = hex_str(&token_bytes);
    state
        .store
        .add_user(&token, &req.name, req.publisher.as_deref(), now_secs())
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "token": token, "name": req.name })))
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
