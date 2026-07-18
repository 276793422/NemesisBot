//! 云端 client：查吊销服务端（`POST /v1/verify`），响应用吊销根公钥验签。
//!
//! 不可达（网络错 / HTTP 非 2xx / 验签失败）→ `Ok(None)`，调用方 soft-fail 本地兜底。
//! 这样 verify 流程在云端不可达时仍能用本地 policy 跑，只是 cloud_state=Unreachable。

use anyhow::Result;
use ed25519_dalek::VerifyingKey;
use revoke_common::{verify_response, SignedResponse};
use serde::{Deserialize, Serialize};

/// 客户端 → 服务端 /v1/verify 请求（envelope 四维度字段）。
#[derive(Debug, Serialize)]
pub struct CloudVerifyReq {
    pub key_fp: Option<String>,
    pub sig_hash: Option<String>,
    pub content_hash: Option<String>,
    pub publisher: Option<String>,
}

/// 服务端 → 客户端 /v1/verify 响应 payload（与 revoke-server VerifyResp 一致）。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CloudVerifyResp {
    pub code: String, // "valid" | "revoked" | "untrusted"
    pub crl_ver: u64,
    pub trusted_keys_ver: u64,
    pub revoked_at: Option<u64>,
    pub reason: Option<String>,
    #[allow(dead_code)]
    pub valid_until: u64,
}

/// 云端 client（阻塞 HTTP，验签响应防 MITM）。
pub struct CloudClient {
    url: String,
    crpub: VerifyingKey,
    http: reqwest::blocking::Client,
}

impl CloudClient {
    pub fn new(url: &str, crpub_hex: &str) -> Result<Self> {
        let crpub = crate::crypto::verifying_key_from_hex(crpub_hex)?;
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            crpub,
            http,
        })
    }

    /// 查签名状态。
    /// - `Ok(Some)` = 云端实时核实（响应已用 crpub 验签）
    /// - `Ok(None)` = 不可达/验签失败（调用方 soft-fail 本地兜底）
    pub fn verify(&self, req: &CloudVerifyReq) -> Result<Option<CloudVerifyResp>> {
        let resp = match self
            .http
            .post(format!("{}/v1/verify", self.url))
            .json(req)
            .send()
        {
            Ok(r) => r,
            Err(_) => return Ok(None), // 连接失败
        };
        if !resp.status().is_success() {
            return Ok(None);
        }
        let signed: SignedResponse<CloudVerifyResp> = match resp.json() {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        if !verify_response(&signed, &self.crpub)? {
            return Ok(None); // 响应签名无效 → 视为不可信/被篡改
        }
        Ok(Some(signed.payload))
    }
}
