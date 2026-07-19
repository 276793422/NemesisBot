//! 服务端状态：`RevocationStore`（SQLite 默认）+ 吊销根私钥。
//!
//! store 集成后，CRL/trusted-keys/审计统一走 `RevocationStore` trait，
//! 后续换 MySQL/PG/JSON 只换 impl，上层不变。

use crate::store::{RevocationStore, SqliteStore};
use anyhow::{anyhow, Result};
use ed25519_dalek::SigningKey;
use std::sync::Arc;

/// 服务端共享状态。
pub struct AppState {
    pub store: Arc<dyn RevocationStore>,
    /// 吊销根私钥（签所有响应，防 MITM）。
    pub crkey: SigningKey,
    /// admin 接口鉴权 token。
    pub admin_token: String,
}

impl AppState {
    /// 打开 SQLite（db_url 如 `revoke.db`）+ 加载吊销根私钥。
    /// 后续多后端：据 db_url 前缀选 impl（sqlite:/mysql:/postgres:）。
    pub fn new(db_url: &str, crkey_hex: &str, admin_token: String) -> Result<Arc<Self>> {
        let store: Arc<dyn RevocationStore> = Arc::new(SqliteStore::open(db_url)?);
        let crkey = signing_key_from_hex(crkey_hex)?;
        Ok(Arc::new(Self {
            store,
            crkey,
            admin_token,
        }))
    }
}

/// 当前 Unix 秒（审计时间戳用）。
pub fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

/// hex（64 字符）→ SigningKey。
fn signing_key_from_hex(hex: &str) -> Result<SigningKey> {
    let hex = hex.trim();
    if hex.len() != 64 {
        return Err(anyhow!("crkey hex expected 64 chars, got {}", hex.len()));
    }
    let mut bytes = [0u8; 32];
    for i in 0..32 {
        bytes[i] =
            u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|e| anyhow!("crkey hex: {}", e))?;
    }
    Ok(SigningKey::from_bytes(&bytes))
}
