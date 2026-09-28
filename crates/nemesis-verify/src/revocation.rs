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

use crate::{
    Crl, CrlEntry, RevDim, SignedResponse, crl_match, hex_util::hex_encode, verify_response,
};
use anyhow::Result;
use p256::ecdsa::VerifyingKey;
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

/// 编译期固化 server URL（build 时 `NEMESIS_BUILD_REVOCATION_URL` 注入，如 `http://127.0.0.1:7878`）。
/// 固化优先（部署确定），fallback 运行时 `NEMESIS_REVOCATION_URL` 环境变量。
const BUILTIN_REVOCATION_URL: Option<&str> = option_env!("NEMESIS_BUILD_REVOCATION_URL");

fn revocation_url() -> Option<String> {
    if let Some(url) = BUILTIN_REVOCATION_URL
        && !url.is_empty()
    {
        return Some(url.to_string());
    }
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
    if let Ok(c) = cache().lock()
        && let Some(cached) = c.as_ref()
        && now < cached.fetched_at + CRL_TTL_SECS
    {
        return Some(cached.crl.clone());
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

// ===== 本地 CRL 快照支持（皮肤管理面 P3：无网络快照模式）=====
//
// 与上方「联网数据模式」的差异：CRL 文件由运维手动放置（release 附件
// 分发 → 保存为本地快照），装载/验签/查询全程无网络。信任链：快照 =
// 根签的 [`SignedResponse<Crl>`] JSON（`GET /v1/crl` 原样 body 存文件）；
// 根公钥来源 = 被检文件自己的证书链（[`root_pubkey_from_anchored`] 校验
// 链根指纹 ∈ 锚集后才给出公钥）。因此快照验签只在「存在可信签名包」时
// 才可能成立——没有可吊销的对象时快照自然不生效，与「吊销只对签名有
// 意义」同构。

use sha2::Digest as _;

/// 文件主签名的吊销元数据（四维；`None` = 该维缺失，诚实跳过不猜）。
#[derive(Debug, Default, Clone)]
pub struct RevocationMeta {
    /// 签名者 key_fp hex = SHA-256(SEC1 uncompressed 65B)。
    pub key_fp: Option<String>,
    /// sig_hash hex = SHA-256(SignerInfo.signature DER)。
    pub sig_hash: Option<String>,
    /// v4 内容摘要 hex（FileHash 维度；口径与签发/记账同源 =
    /// [`crate::verify::v4_content_digest`]）。
    pub content_hash: Option<String>,
    /// 签名者证书 subject CN（Publisher 维度）。
    pub publisher: Option<String>,
}

/// 主签名 CMS DER 定位（与 verify_bytes / view 同判据：PE 证书表首条目 /
/// ELF+raw v4 footer 载体）。无签名 / 结构坏 = Err。
fn primary_cms(bytes: &[u8]) -> Result<Vec<u8>> {
    if crate::codec::detect_format(bytes) == crate::codec::FORMAT_TAG_PE {
        let entries = crate::pe::read_certificate_entries(bytes)?;
        let first = entries
            .first()
            .ok_or_else(|| anyhow::anyhow!("证书表为空（无签名）"))?;
        if first.cert_type != crate::pe::WIN_CERT_TYPE_PKCS_SIGNED_DATA {
            anyhow::bail!("主条目类型 {:#06x} 非 PKCS_SIGNED_DATA", first.cert_type);
        }
        Ok(first.certificate.clone())
    } else {
        Ok(crate::envelope::extract_carrier_v4(bytes)?.cms_der)
    }
}

/// 提取文件主签名的吊销四维元数据（快照 CRL 查询用；无网络）。
///
/// 签名结构解析失败 → 各签名维 `None`（诚实缺维）；content_hash：PE =
/// authenticode_digest（排除证书表）；有 footer = 载体内嵌摘要（签发时刻
/// 口径，与记账一致）；都无 = [`crate::verify::v4_content_digest`]（未签
/// 文件的 best-effort 文件指纹）。
pub fn revocation_meta(bytes: &[u8]) -> RevocationMeta {
    let mut meta = RevocationMeta::default();
    if let Ok(cms) = primary_cms(bytes)
        && let Ok(ps) = crate::envelope::parse_signed_data(&cms)
    {
        meta.sig_hash = Some(hex_encode(&sha2::Sha256::digest(&ps.signature)));
        if let Some(signer) = ps
            .certs
            .iter()
            .find(|c| crate::verify::signer_matches(c, &ps))
        {
            if let Ok(vk) = signer.subject_public_key() {
                meta.key_fp = Some(hex_encode(&crate::crypto::key_fp(
                    &crate::crypto::public_key_bytes(&vk),
                )));
            }
            meta.publisher = signer.subject_cn().ok().flatten();
        }
    }
    meta.content_hash = if crate::codec::detect_format(bytes) == crate::codec::FORMAT_TAG_PE {
        crate::pe::authenticode_digest(bytes)
            .ok()
            .map(|d| hex_encode(&d))
    } else {
        match crate::envelope::extract_carrier_v4(bytes) {
            Ok(v) => Some(hex_encode(&v.content_digest)),
            Err(_) => crate::verify::v4_content_digest(bytes)
                .ok()
                .map(|d| hex_encode(&d)),
        }
    };
    meta
}

/// 四维匹配（快照模式；顺序与 [`check_revocation`] 一致，首个命中即返回）。
pub fn crl_match_meta<'a>(crl: &'a Crl, meta: &RevocationMeta) -> Option<&'a CrlEntry> {
    let hit =
        |dim: RevDim, v: &Option<String>| v.as_deref().and_then(|val| crl_match(crl, dim, val));
    hit(RevDim::KeyFp, &meta.key_fp)
        .or_else(|| hit(RevDim::SigHash, &meta.sig_hash))
        .or_else(|| hit(RevDim::FileHash, &meta.content_hash))
        .or_else(|| {
            meta.publisher
                .as_deref()
                .and_then(|p| crl_match(crl, RevDim::Publisher, p))
        })
}

/// 从一枚签名文件提取**锚定根公钥**（快照 CRL 验签用）。
///
/// 双重校验（fail-closed）：链完整上溯到自签根（AKI→SKI，与 verify 同源
/// 判据）；根证书 SHA-256 指纹 ∈ `anchors`。任一不满足 = Err——宁可放弃
/// 吊销检查，也不用未锚定的公钥去验 CRL。
pub fn root_pubkey_from_anchored(bytes: &[u8], anchors: &[[u8; 32]]) -> Result<VerifyingKey> {
    let cms = primary_cms(bytes)?;
    let ps = crate::envelope::parse_signed_data(&cms)?;
    let signer = ps
        .certs
        .iter()
        .find(|c| crate::verify::signer_matches(c, &ps))
        .ok_or_else(|| anyhow::anyhow!("sid 引用的签名者证书不在证书集"))?;
    let chain = crate::verify::order_chain_from(signer, &ps.certs)
        .ok_or_else(|| anyhow::anyhow!("证书链断裂（AKI 无法上溯）"))?;
    let root = chain.last().ok_or_else(|| anyhow::anyhow!("证书链为空"))?;
    let fp = root.sha256_fingerprint();
    if !anchors.contains(&fp) {
        anyhow::bail!("链根指纹不在锚集内（root={fp:02x?}）");
    }
    Ok(root.subject_public_key()?)
}

/// 装载并验签本地 CRL 快照（`GET /v1/crl` 原样 JSON 存为文件）。
pub fn load_crl_snapshot(json: &str, root_pub: &VerifyingKey) -> Result<Crl> {
    let resp: SignedResponse<Crl> = serde_json::from_str(json)
        .map_err(|e| anyhow::anyhow!("CRL 快照不是合法的 SignedResponse<Crl> JSON: {e}"))?;
    if verify_response(&resp, root_pub)? {
        Ok(resp.payload)
    } else {
        anyhow::bail!("CRL 快照签名验证失败（伪造/篡改/根不匹配）")
    }
}

#[cfg(test)]
mod tests;
