//! 证书与证书链（v3：公钥随签名走的信任链基础设施）。
//!
//! # 信任链
//! ```text
//! 根私钥（离线）→ 根证书（自签）+ 中间 CA 证书 + CRL
//!   └签→ 中间 CA 私钥 → 中间 CA 证书
//!          └签→ 发行方(leaf)私钥 → 发行方证书
//!                 └签→ exe（envelope 带 leaf pubkey + 完整链）
//! ```
//!
//! envelope.cert_chain = `[leaf_cert, intermediate_cert, ...]`（leaf 在前，**不含根证书**
//! ——根在验证端内置）。验证端用内置根公钥验最后一级证书的签发者。
//!
//! # Certificate 字节布局
//! ```text
//! subject_pubkey: 32B
//! valid_not_before: 8B LE
//! valid_not_after: 8B LE
//! issuer_key_fp: 32B（签发者公钥 SHA-256）
//! subject_meta_len: 2B LE
//! subject_meta: meta_len B（主体元数据，自由格式）
//! signature: 64B（签发者对 to_be_signed 的 Ed25519 签名）
//! ```
//! to_be_signed = `subject_pubkey | valid_not_before | valid_not_after | issuer_key_fp | subject_meta`
//!
//! # cert_chain 字节布局（envelope.cert_chain）
//! ```text
//! count: 2B LE
//! 对每个 cert: cert_len 4B LE + cert bytes
//! ```

use crate::crypto;
use anyhow::{Result, anyhow};
use ed25519_dalek::{SigningKey, Verifier, VerifyingKey};

/// 证书。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub subject_pubkey: [u8; 32],
    pub valid_not_before: u64,
    pub valid_not_after: u64,
    /// 签发者公钥 SHA-256 指纹（链上一级；最后一级指向根）。
    pub issuer_key_fp: [u8; 32],
    /// 主体元数据（自由格式字节，如 name/publisher，可选）。
    pub subject_meta: Vec<u8>,
    /// 签发者对 [`cert_tbs`] 的 Ed25519 签名。
    pub signature: [u8; 64],
}

/// 证书链验证错误。
#[derive(Debug, PartialEq)]
pub enum ChainError {
    Empty,
    /// leaf 证书的 subject_pubkey 与 envelope.pubkey 不符。
    LeafMismatch,
    /// 链断裂：cert[i] 的 issuer_key_fp 与 certs[i+1] 的 subject 指纹不符。
    BrokenChain,
    /// 最后一级的 issuer 在 root_pubs 里找不到（未到受信根）。
    NoRootForIssuer,
    /// 证书不在有效期内。
    Expired,
    /// 证书签名验不过。
    BadSignature,
    InvalidKey,
    ParseError(String),
}

fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}
fn rd_u64(b: &[u8], off: usize) -> u64 {
    u64::from_le_bytes([
        b[off],
        b[off + 1],
        b[off + 2],
        b[off + 3],
        b[off + 4],
        b[off + 5],
        b[off + 6],
        b[off + 7],
    ])
}

/// to_be_signed（签名覆盖范围，固定布局）。
fn cert_tbs(
    subject_pubkey: &[u8; 32],
    valid_not_before: u64,
    valid_not_after: u64,
    issuer_key_fp: &[u8; 32],
    subject_meta: &[u8],
) -> Vec<u8> {
    let mut b = Vec::with_capacity(32 + 8 + 8 + 32 + subject_meta.len());
    b.extend_from_slice(subject_pubkey);
    b.extend_from_slice(&valid_not_before.to_le_bytes());
    b.extend_from_slice(&valid_not_after.to_le_bytes());
    b.extend_from_slice(issuer_key_fp);
    b.extend_from_slice(subject_meta);
    b
}

impl Certificate {
    /// 序列化为字节。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(146 + self.subject_meta.len());
        b.extend_from_slice(&self.subject_pubkey);
        b.extend_from_slice(&self.valid_not_before.to_le_bytes());
        b.extend_from_slice(&self.valid_not_after.to_le_bytes());
        b.extend_from_slice(&self.issuer_key_fp);
        b.extend_from_slice(&(self.subject_meta.len() as u16).to_le_bytes());
        b.extend_from_slice(&self.subject_meta);
        b.extend_from_slice(&self.signature);
        b
    }

    /// 从字节解析。
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 146 {
            return Err(anyhow!("cert too short: {} < 146", bytes.len()));
        }
        let mut subject_pubkey = [0u8; 32];
        subject_pubkey.copy_from_slice(&bytes[0..32]);
        let valid_not_before = rd_u64(bytes, 32);
        let valid_not_after = rd_u64(bytes, 40);
        let mut issuer_key_fp = [0u8; 32];
        issuer_key_fp.copy_from_slice(&bytes[48..80]);
        let meta_len = rd_u16(bytes, 80) as usize;
        if bytes.len() < 82 + meta_len + 64 {
            return Err(anyhow!("cert truncated: meta_len {}", meta_len));
        }
        let subject_meta = bytes[82..82 + meta_len].to_vec();
        let mut signature = [0u8; 64];
        signature.copy_from_slice(&bytes[82 + meta_len..82 + meta_len + 64]);
        Ok(Certificate {
            subject_pubkey,
            valid_not_before,
            valid_not_after,
            issuer_key_fp,
            subject_meta,
            signature,
        })
    }

    fn tbs(&self) -> Vec<u8> {
        cert_tbs(
            &self.subject_pubkey,
            self.valid_not_before,
            self.valid_not_after,
            &self.issuer_key_fp,
            &self.subject_meta,
        )
    }
}

/// 序列化证书链（leaf 在前，不含根证书）。
pub fn serialize_chain(certs: &[Certificate]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&(certs.len() as u16).to_le_bytes());
    for c in certs {
        let cb = c.to_bytes();
        b.extend_from_slice(&(cb.len() as u32).to_le_bytes());
        b.extend_from_slice(&cb);
    }
    b
}

/// 解析证书链。
pub fn parse_chain(bytes: &[u8]) -> Result<Vec<Certificate>> {
    if bytes.len() < 2 {
        return Err(anyhow!("chain too short for count"));
    }
    let count = rd_u16(bytes, 0) as usize;
    let mut off = 2;
    let mut certs = Vec::with_capacity(count);
    for _ in 0..count {
        if off + 4 > bytes.len() {
            return Err(anyhow!("chain truncated at cert_len"));
        }
        let clen = rd_u32(bytes, off) as usize;
        off += 4;
        if off + clen > bytes.len() {
            return Err(anyhow!("chain truncated at cert bytes"));
        }
        certs.push(Certificate::from_bytes(&bytes[off..off + clen])?);
        off += clen;
    }
    Ok(certs)
}

/// 签发证书：`issuer_sk` 认证 `subject_pubkey`，有效期 `[not_before, not_after]`。
pub fn issue_certificate(
    issuer_sk: &SigningKey,
    subject_pubkey: &[u8; 32],
    subject_meta: &[u8],
    valid_not_before: u64,
    valid_not_after: u64,
) -> Certificate {
    let issuer_vk = issuer_sk.verifying_key();
    let issuer_key_fp = crypto::key_fp(&issuer_vk.to_bytes());
    let tbs = cert_tbs(
        subject_pubkey,
        valid_not_before,
        valid_not_after,
        &issuer_key_fp,
        subject_meta,
    );
    use ed25519_dalek::Signer;
    let sig: ed25519_dalek::Signature = issuer_sk.sign(&tbs);
    Certificate {
        subject_pubkey: *subject_pubkey,
        valid_not_before,
        valid_not_after,
        issuer_key_fp,
        subject_meta: subject_meta.to_vec(),
        signature: sig.to_bytes(),
    }
}

/// 验证证书链：`leaf_pubkey` 经 `chain` 一路签发到 `root_pubs` 中的受信根。
///
/// `chain` = `[leaf_cert, intermediate_cert, ...]`（leaf 在前，不含根证书）。
/// `now` 用于校验证书有效期。
pub fn verify_chain(
    leaf_pubkey: &[u8; 32],
    chain: &[Certificate],
    root_pubs: &[VerifyingKey],
    now: u64,
) -> Result<(), ChainError> {
    if chain.is_empty() {
        return Err(ChainError::Empty);
    }
    // leaf.subject_pubkey == envelope.pubkey
    if chain[0].subject_pubkey != *leaf_pubkey {
        return Err(ChainError::LeafMismatch);
    }

    for (i, cert) in chain.iter().enumerate() {
        // 找签发者公钥
        let issuer_pubkey: [u8; 32] = if i + 1 < chain.len() {
            // 中间级：下一级证书的 subject_pubkey，且其指纹须 == cert.issuer_key_fp
            let next = &chain[i + 1];
            let next_fp = crypto::key_fp(&next.subject_pubkey);
            if next_fp != cert.issuer_key_fp {
                return Err(ChainError::BrokenChain);
            }
            next.subject_pubkey
        } else {
            // 最后一级：签发者须是 root_pubs 中 fingerprint == cert.issuer_key_fp 的根
            let root = root_pubs
                .iter()
                .find(|r| crypto::key_fp(r.as_bytes()) == cert.issuer_key_fp)
                .ok_or(ChainError::NoRootForIssuer)?;
            *root.as_bytes()
        };

        // 有效期
        if now < cert.valid_not_before || now > cert.valid_not_after {
            return Err(ChainError::Expired);
        }

        // 验签
        let issuer_vk =
            VerifyingKey::from_bytes(&issuer_pubkey).map_err(|_| ChainError::InvalidKey)?;
        let tbs = cert.tbs();
        let sig = ed25519_dalek::Signature::from_bytes(&cert.signature);
        if issuer_vk.verify(&tbs, &sig).is_err() {
            return Err(ChainError::BadSignature);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
