//! Auth credential storage.

use crate::token::AuthCredential;
use std::collections::HashMap;
use std::path::Path;
use std::sync::RwLock;

/// On-disk auth credential store.
pub struct AuthStore {
    path: String,
    creds: RwLock<HashMap<String, AuthCredential>>,
}

impl AuthStore {
    /// Create a new store backed by a file.
    pub fn new(path: &str) -> Self {
        let store = Self {
            path: path.to_string(),
            creds: RwLock::new(HashMap::new()),
        };
        if let Err(e) = store.load() {
            // BUG-20261025-39：构造期加载失败不能静默——盘上文件损坏时调用方
            // 只看到空 store，下次 save 会以「空 + 新条目」覆盖盘上残留，
            // 其余 provider 的 token 无声丢失。loud 留痕让损失可追溯。
            tracing::warn!(
                error = %e,
                "[AuthStore] 凭据存储加载失败：{path} 将按空存储继续（下次保存会覆盖盘上残留）"
            );
        }
        store
    }

    /// Save a credential.
    pub fn save(&self, provider: &str, cred: AuthCredential) -> Result<(), String> {
        self.creds
            .write()
            .unwrap()
            .insert(provider.to_string(), cred);
        self.persist()
    }

    /// Get a credential.
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.creds.read().unwrap().get(provider).cloned()
    }

    /// Remove a credential.
    pub fn remove(&self, provider: &str) -> Result<(), String> {
        self.creds.write().unwrap().remove(provider);
        self.persist()
    }

    /// List all providers.
    pub fn list_providers(&self) -> Vec<String> {
        self.creds.read().unwrap().keys().cloned().collect()
    }

    /// Delete all credentials by removing the store file.
    /// Mirrors Go DeleteAllCredentials.
    pub fn delete_all(&self) -> Result<(), String> {
        let path = Path::new(&self.path);
        if path.exists() {
            std::fs::remove_file(path).map_err(|e| format!("delete auth file: {}", e))?;
        }
        self.creds.write().unwrap().clear();
        Ok(())
    }

    fn load(&self) -> Result<(), String> {
        if !Path::new(&self.path).exists() {
            return Ok(());
        }
        let data =
            std::fs::read_to_string(&self.path).map_err(|e| format!("read auth store: {}", e))?;
        let creds: HashMap<String, AuthCredential> =
            serde_json::from_str(&data).map_err(|e| format!("parse auth store: {}", e))?;
        *self.creds.write().unwrap() = creds;
        Ok(())
    }

    fn persist(&self) -> Result<(), String> {
        let data = serde_json::to_string_pretty(&*self.creds.read().unwrap())
            .map_err(|e| format!("serialize: {}", e))?;
        // BUG-20261025-39：auth.json 存 OAuth access/refresh token（bearer
        // 凭据），落盘必须原子写 + 0o600（对齐 TrustStore 的 REL-002 纪律）。
        // 此前裸 std::fs::write 非原子：进程中断撕裂 = 半截 JSON，构造期
        // load 失败（现在会 loud）→ 空 store → 下次 save 覆盖即全量丢失；
        // 且默认权限在 Unix 下按 umask 常为 0644 世界可读。
        // write_file_atomic 自带父目录创建，错误统一 "atomic write ..." 前缀。
        nemesis_utils::write_file_atomic(&self.path, data.as_bytes(), 0o600)
            .map_err(|e| format!("write: {e}"))
    }
}

#[cfg(test)]
mod tests;
