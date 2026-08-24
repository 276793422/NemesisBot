//! 密钥体系生成与持久化（根 → CA → 发行方）。
//!
//! 信任链：`root` 签 `ca_cert` → `ca` 签 `issuer_cert` → `issuer` 签 exe。
//! `issuer_chain_bytes` = `[issuer_cert, ca_cert]`（leaf 在前，不含根证书），写入 envelope.cert_chain。
//!
//! 持久化为单个 JSON（hex 编码私钥 + 证书），revoke-server 启动加载 / 首次生成。

use crate::cert::{Certificate, issue_certificate, serialize_chain};
use crate::hex_util::{hex_decode_32, hex_decode_vec, hex_encode};
use anyhow::{Result, anyhow};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

/// JSON 持久化形式（hex 编码）。
#[derive(Debug, Serialize, Deserialize)]
pub struct KeyHierarchyJson {
    pub root_sk: String,
    pub ca_sk: String,
    pub ca_cert: String,
    pub issuer_sk: String,
    pub issuer_cert: String,
}

/// 完整密钥体系（内存，签发端持有）。
pub struct KeyHierarchy {
    pub root_sk: SigningKey,
    pub root_vk: VerifyingKey,
    pub ca_sk: SigningKey,
    pub ca_vk: VerifyingKey,
    pub ca_cert: Certificate,
    pub issuer_sk: SigningKey,
    pub issuer_vk: VerifyingKey,
    pub issuer_cert: Certificate,
    /// envelope.cert_chain 内容 = [issuer_cert, ca_cert] 序列化（leaf 在前，不含根）。
    pub issuer_chain_bytes: Vec<u8>,
}

/// 生成新密钥体系（有效期 `[valid_not_before, valid_not_after]`）。
pub fn generate_hierarchy(valid_not_before: u64, valid_not_after: u64) -> KeyHierarchy {
    let mut rng = OsRng;
    let root_sk = SigningKey::generate(&mut rng);
    let root_vk = root_sk.verifying_key();
    let ca_sk = SigningKey::generate(&mut rng);
    let ca_vk = ca_sk.verifying_key();
    let issuer_sk = SigningKey::generate(&mut rng);
    let issuer_vk = issuer_sk.verifying_key();

    let ca_cert = issue_certificate(
        &root_sk,
        &ca_vk.to_bytes(),
        b"CA",
        valid_not_before,
        valid_not_after,
    );
    let issuer_cert = issue_certificate(
        &ca_sk,
        &issuer_vk.to_bytes(),
        b"issuer",
        valid_not_before,
        valid_not_after,
    );
    let issuer_chain_bytes = serialize_chain(&[issuer_cert.clone(), ca_cert.clone()]);

    KeyHierarchy {
        root_sk,
        root_vk,
        ca_sk,
        ca_vk,
        ca_cert,
        issuer_sk,
        issuer_vk,
        issuer_cert,
        issuer_chain_bytes,
    }
}

impl KeyHierarchy {
    pub fn to_json(&self) -> KeyHierarchyJson {
        KeyHierarchyJson {
            root_sk: hex_encode(self.root_sk.to_bytes().as_ref()),
            ca_sk: hex_encode(self.ca_sk.to_bytes().as_ref()),
            ca_cert: hex_encode(&self.ca_cert.to_bytes()),
            issuer_sk: hex_encode(self.issuer_sk.to_bytes().as_ref()),
            issuer_cert: hex_encode(&self.issuer_cert.to_bytes()),
        }
    }

    pub fn from_json(j: &KeyHierarchyJson) -> Result<Self> {
        let root_sk = SigningKey::from_bytes(
            &hex_decode_32(&j.root_sk).map_err(|e| anyhow!("root_sk: {}", e))?,
        );
        let ca_sk =
            SigningKey::from_bytes(&hex_decode_32(&j.ca_sk).map_err(|e| anyhow!("ca_sk: {}", e))?);
        let issuer_sk = SigningKey::from_bytes(
            &hex_decode_32(&j.issuer_sk).map_err(|e| anyhow!("issuer_sk: {}", e))?,
        );
        let root_vk = root_sk.verifying_key();
        let ca_vk = ca_sk.verifying_key();
        let issuer_vk = issuer_sk.verifying_key();

        let ca_cert = Certificate::from_bytes(
            &hex_decode_vec(&j.ca_cert).map_err(|e| anyhow!("ca_cert: {}", e))?,
        )?;
        let issuer_cert = Certificate::from_bytes(
            &hex_decode_vec(&j.issuer_cert).map_err(|e| anyhow!("issuer_cert: {}", e))?,
        )?;
        let issuer_chain_bytes = serialize_chain(&[issuer_cert.clone(), ca_cert.clone()]);

        Ok(KeyHierarchy {
            root_sk,
            root_vk,
            ca_sk,
            ca_vk,
            ca_cert,
            issuer_sk,
            issuer_vk,
            issuer_cert,
            issuer_chain_bytes,
        })
    }

    pub fn save(&self, path: &str) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.to_json())?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub fn load(path: &str) -> Result<Self> {
        let data = std::fs::read(path)?;
        let j: KeyHierarchyJson = serde_json::from_slice(&data)?;
        Self::from_json(&j)
    }
}

#[cfg(test)]
mod tests;
