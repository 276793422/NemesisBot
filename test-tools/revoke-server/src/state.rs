//! 服务端状态：`RevocationStore`（SQLite 默认）+ 密钥体系（根/发行锚/leaf）。
//!
//! v4：AppState 持有 X.509 `KeyHierarchy`（根私钥签云端响应/CRL + leaf 私钥签 exe
//! + 三级证书链）。

use crate::store::{RevocationStore, SqliteStore};
use anyhow::{Result, anyhow};
use nemesis_verify::keygen::KeyHierarchy;
use std::sync::Arc;

/// 服务端共享状态。
pub struct AppState {
    pub store: Arc<dyn RevocationStore>,
    /// 密钥体系：root_sk（签 CRL/云端响应）+ leaf_sk（签 exe）+ 三级 X.509 证书链。
    pub hierarchy: KeyHierarchy,
    /// admin 接口鉴权 token。
    pub admin_token: String,
}

impl AppState {
    pub fn new(db_url: &str, keys_file: &str, admin_token: String) -> Result<Arc<Self>> {
        let store: Arc<dyn RevocationStore> = Arc::new(SqliteStore::open(db_url)?);
        let hierarchy = KeyHierarchy::load(keys_file)
            .map_err(|e| anyhow!("load keys file {}: {}", keys_file, e))?;
        Ok(Arc::new(Self {
            store,
            hierarchy,
            admin_token,
        }))
    }
}

/// 当前 Unix 秒（审计时间戳用）。
pub fn now_secs() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

#[cfg(test)]
mod tests;
