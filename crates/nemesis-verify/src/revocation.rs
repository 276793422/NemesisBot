//! 吊销检查（P2a：DLL 联网查 CRL，数据模式）。
//!
//! 流程：拉 `GET <NEMESIS_REVOCATION_URL>/v1/crl` → `SignedResponse<Crl>`（被根签）→
//! 用内置根公钥 `verify_response` → 缓存（TTL）→ 四维度查（key_fp/sig_hash/content_hash/publisher）。
//!
//! **数据模式**：CRL 被根私钥签，DLL 本地用根公钥验。云端被攻破 → 假 CRL 无根签 → 验不过 → 拒。
//!
//! 配置（环境变量，R7 同模式运行时配置）：
//! - `NEMESIS_REVOCATION_URL`：云端 base URL（如 `http://127.0.0.1:7878`）。
//! - `NEMESIS_STRICT_OFFLINE`：`1`/`true` = strict 模式（断网/拉取失败且无缓存 → `Unknown` → 调用方拒）。
//!
//! **soft-fail 默认**：未配置 URL / 拉取失败 → 用旧缓存；无缓存 → `Unknown`（调用方按 soft-fail 放行）。

use crate::{crl_match, hex_util::hex_encode, verify_response, Crl, CrlEntry, RevDim, SignedResponse};
use anyhow::Result;
use ed25519_dalek::VerifyingKey;
use std::sync::{Mutex, OnceLock};

/// 吊销查询结果（区分"未吊销"与"无法查询"）。
#[derive(Debug)]
pub enum RevocationResult {
    /// 查到 CRL，未命中吊销。
    NotRevoked,
    /// 查到 CRL，命中吊销条目。
    Revoked(CrlEntry),
    /// 无法查询（未配置 URL / 断网 strict 无缓存）。调用方按 soft-fail/strict 策略处置。
    Unknown,
}

/// CRL 缓存条目。
struct CrlCache {
    crl: Crl,
    fetched_at: u64,
}

static CRL_CACHE: OnceLock<Mutex<Option<CrlCache>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CrlCache>> {
    CRL_CACHE.get_or_init(|| Mutex::new(None))
}

/// CRL 缓存 TTL（秒）。过期强制重新拉。
const CRL_TTL_SECS: u64 = 3600;

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn revocation_url() -> Option<String> {
    std::env::var("NEMESIS_REVOCATION_URL")
        .ok()
        .filter(|s| !s.is_empty())
}

/// strict 模式（断网/拉取失败且无缓存 → 拒）。pub：verify_bytes 按此处置 Unknown。
pub fn strict_offline() -> bool {
    std::env::var("NEMESIS_STRICT_OFFLINE")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

/// 拉取并验证 CRL（被根签）。返回验过的 Crl。
fn fetch_crl(base_url: &str, root_pub: &VerifyingKey) -> Result<Crl> {
    let url = format!("{}/v1/crl", base_url.trim_end_matches('/'));
    let resp: SignedResponse<Crl> = reqwest::blocking::get(&url)?.json()?;
    if !verify_response(&resp, root_pub)? {
        anyhow::bail!("CRL signature invalid (root pubkey mismatch / tampered)");
    }
    Ok(resp.payload)
}

/// 取有效 CRL（缓存优先，过期/无则拉取）。soft-fail：拉失败用旧缓存。
fn get_crl(root_pub: &VerifyingKey) -> Option<Crl> {
    let now = now_secs();
    // 缓存命中（未过期）
    if let Ok(c) = cache().lock() {
        if let Some(cached) = c.as_ref() {
            if now < cached.fetched_at + CRL_TTL_SECS {
                return Some(cached.crl.clone());
            }
        }
    }
    // 缓存过期/无 → 拉取
    if let Some(base) = revocation_url() {
        match fetch_crl(&base, root_pub) {
            Ok(crl) => {
                if let Ok(mut c) = cache().lock() {
                    *c = Some(CrlCache {
                        crl: crl.clone(),
                        fetched_at: now,
                    });
                }
                Some(crl)
            }
            Err(_) => {
                // 拉取失败：strict 拒（None→Unknown）；soft-fail 用旧缓存
                if strict_offline() {
                    None
                } else if let Ok(c) = cache().lock() {
                    c.as_ref().map(|c| c.crl.clone())
                } else {
                    None
                }
            }
        }
    } else {
        // 未配置 URL：不查吊销（None→Unknown）
        None
    }
}

/// 查吊销：给定签名元数据（四维度），返回 [`RevocationResult`]。
pub fn check_revocation(
    key_fp: &[u8; 32],
    sig_hash: &[u8; 32],
    content_hash: &[u8; 32],
    publisher: Option<&str>,
    root_pub: &VerifyingKey,
) -> RevocationResult {
    let crl = match get_crl(root_pub) {
        Some(c) => c,
        None => return RevocationResult::Unknown,
    };
    let kf = hex_encode(key_fp);
    let sh = hex_encode(sig_hash);
    let ch = hex_encode(content_hash);
    match crl_match(&crl, RevDim::KeyFp, &kf)
        .or_else(|| crl_match(&crl, RevDim::SigHash, &sh))
        .or_else(|| crl_match(&crl, RevDim::FileHash, &ch))
        .or_else(|| publisher.and_then(|p| crl_match(&crl, RevDim::Publisher, p)))
        .cloned()
    {
        Some(e) => RevocationResult::Revoked(e),
        None => RevocationResult::NotRevoked,
    }
}

// ===== OCSP-like 单条查询（CRL 不可达时的实时 fallback，双轨的另一轨）=====

/// OCSP 单条查询请求（POST /v1/crl/query）。
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct OcspReq {
    pub key_fp: Option<String>,
    pub sig_hash: Option<String>,
    pub content_hash: Option<String>,
    pub publisher: Option<String>,
}

/// OCSP 单条查询响应（被根签）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OcspResp {
    /// "valid" / "revoked"
    pub code: String,
    /// 命中维度（仅 revoked 时有）
    pub dim: Option<crate::RevDim>,
    /// 命中值
    pub value: Option<String>,
    pub revoked_at: Option<u64>,
    pub reason: Option<String>,
    pub crl_ver: u64,
}

/// OCSP 单条查询：CRL 不可达时的实时 fallback。返回吊销条目（若吊销）。
///
/// strict 模式下 CRL 不可达 → 试 OCSP 单条；OCSP 也不可达 → 调用方拒。
/// soft-fail 不走此路径（直接放行）。
pub fn ocsp_check_single(
    key_fp: &[u8; 32],
    sig_hash: &[u8; 32],
    content_hash: &[u8; 32],
    publisher: Option<&str>,
    root_pub: &VerifyingKey,
) -> Option<CrlEntry> {
    let base = revocation_url()?;
    let url = format!("{}/v1/crl/query", base.trim_end_matches('/'));
    let req = OcspReq {
        key_fp: Some(hex_encode(key_fp)),
        sig_hash: Some(hex_encode(sig_hash)),
        content_hash: Some(hex_encode(content_hash)),
        publisher: publisher.map(String::from),
    };
    let client = reqwest::blocking::Client::new();
    let resp: SignedResponse<OcspResp> = client.post(&url).json(&req).send().ok()?.json().ok()?;
    if !verify_response(&resp, root_pub).ok()? {
        return None; // 验签失败 = 不可信，视作未查到（fallback 到 strict 拒）
    }
    if resp.payload.code == "revoked" {
        Some(CrlEntry {
            dim: resp.payload.dim.unwrap_or(crate::RevDim::KeyFp),
            value: resp.payload.value.unwrap_or_default(),
            revoked_at: resp.payload.revoked_at.unwrap_or(0),
            reason: resp.payload.reason.unwrap_or_default(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sign_response;
    use ed25519_dalek::SigningKey;

    fn keypair(seed: u8) -> (SigningKey, VerifyingKey) {
        let sk = SigningKey::from_bytes(&[seed; 32]);
        let vk = sk.verifying_key();
        (sk, vk)
    }

    /// 测试串行锁（全局 CRL_CACHE + env 并行竞争，参考 env-test-race-lock-pattern）。
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    /// 直接喂缓存一个 CRL（绕过联网），测四维度查询逻辑。
    fn seed_cache(crl: Crl) {
        *cache().lock().unwrap() = Some(CrlCache {
            crl,
            fetched_at: now_secs(),
        });
    }

    #[test]
    fn revoke_hit_key_fp() {
        let _g = TEST_LOCK.lock().unwrap();
        let (sk, vk) = keypair(1);
        let target_fp = [0xAAu8; 32];
        let signed = sign_response(
            &Crl {
                version: 1,
                valid_until: u64::MAX,
                entries: vec![CrlEntry {
                    dim: RevDim::KeyFp,
                    value: hex_encode(&target_fp),
                    revoked_at: 100,
                    reason: "leak".into(),
                }],
            },
            &sk,
        )
        .unwrap();
        seed_cache(signed.payload);
        match check_revocation(&target_fp, &[0u8; 32], &[0u8; 32], None, &vk) {
            RevocationResult::Revoked(e) => assert_eq!(e.reason, "leak"),
            o => panic!("expected Revoked, got {:?}", o),
        }
        match check_revocation(&[0xBBu8; 32], &[0u8; 32], &[0u8; 32], None, &vk) {
            RevocationResult::NotRevoked => {}
            o => panic!("expected NotRevoked, got {:?}", o),
        }
    }

    #[test]
    fn no_url_returns_unknown() {
        let _g = TEST_LOCK.lock().unwrap();
        unsafe {
            std::env::remove_var("NEMESIS_REVOCATION_URL");
        }
        *cache().lock().unwrap() = None;
        let (_, vk) = keypair(2);
        match check_revocation(&[0xAAu8; 32], &[0u8; 32], &[0u8; 32], None, &vk) {
            RevocationResult::Unknown => {}
            o => panic!("expected Unknown, got {:?}", o),
        }
    }
}
