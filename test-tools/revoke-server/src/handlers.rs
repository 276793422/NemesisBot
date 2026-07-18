//! axum 路由 handler（store 集成版：CRL/trusted-keys/审计走 RevocationStore）。
//!
//! 所有公开响应（verify/crl/trusted-keys）用吊销根密钥签（SignedResponse），客户端验签防 MITM。
//! admin 操作（revoke/trusted-key）写审计（add_audit）。新增 GET /v1/audit 供 Web UI 查。

use crate::state::{now_secs, AppState};
use crate::store::AuditRecord;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use revoke_common::{
    crl_match, sign_response, Crl, CrlEntry, KeyStatus, RevDim, SignedResponse, TrustedKeyList,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

fn internal<E: std::fmt::Display>(e: E) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ── /v1/verify ──

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
    let (code, revoked_at, reason) = compute_status(&crl, &tkl, &req);
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
) -> (String, Option<u64>, Option<String>) {
    if !tkl.keys.is_empty() {
        if let Some(kf) = &req.key_fp {
            let active = tkl.keys.iter().any(|k| &k.key_fp == kf && k.status == KeyStatus::Active);
            if !active {
                return ("untrusted".into(), None, None);
            }
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

// ── /v1/crl + /v1/trusted-keys（带签名）──

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

// ── /v1/admin/*（admin token 鉴权 + 写审计）──

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
            dim: Some(format!("{:?}", req.dim)),
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
            reason: Some(format!("{:?}", req.status)),
            detail: None,
        })
        .map_err(internal)?;
    Ok(Json(serde_json::json!({ "trusted_keys_version": ver })))
}

// ── /v1/audit（Web UI 查审计日志，admin 鉴权）──

pub async fn get_audit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<AuditRecord>>, (StatusCode, String)> {
    check_admin(&state, &headers)?;
    let list = state.store.list_audit(200).map_err(internal)?;
    Ok(Json(list))
}

pub async fn health() -> &'static str {
    "ok"
}

fn check_admin(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    // 防 admin_token 为空时被空 Authorization 绕过（"" == ""）
    if state.admin_token.is_empty() {
        return Err((
            StatusCode::UNAUTHORIZED,
            "admin token not configured (refuse all admin ops)".into(),
        ));
    }
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let token = auth.strip_prefix("Bearer ").unwrap_or(auth);
    if token.is_empty() || token != state.admin_token {
        return Err((StatusCode::UNAUTHORIZED, "invalid admin token".into()));
    }
    Ok(())
}
