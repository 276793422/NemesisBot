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
            // 构造期加载失败不能静默——盘上文件损坏时调用方
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
    ///
    /// BUG-20261114-058（BUG-20261114-041 同族）：持久化失败必须与 Err
    /// 同路径回滚内存态。旧实现「先 insert、再 persist」失败不回滚——
    /// 内存与盘上真相分叉：save 侧 = 进程重启丢 token（盘上是旧值）；
    /// remove 侧更糟 = **登出僵尸**（内存已清视为已登出，盘上 token 残留，
    /// 重启 load 复活已登出的凭据）。回滚选不可能失败的操作（纯内存
    /// 快照恢复），BUG-041 教训原文。
    pub fn save(&self, provider: &str, cred: AuthCredential) -> Result<(), String> {
        let mut creds = self.creds.write().unwrap();
        let prev = creds.insert(provider.to_string(), cred);
        if let Err(e) = self.persist_guarded(&creds) {
            match prev {
                Some(old) => {
                    creds.insert(provider.to_string(), old);
                }
                None => {
                    creds.remove(provider);
                }
            }
            return Err(e);
        }
        Ok(())
    }

    /// Get a credential.
    pub fn get(&self, provider: &str) -> Option<AuthCredential> {
        self.creds.read().unwrap().get(provider).cloned()
    }

    /// Remove a credential.（失败回滚同 [`Self::save`]，登出僵尸面。）
    pub fn remove(&self, provider: &str) -> Result<(), String> {
        let mut creds = self.creds.write().unwrap();
        let prev = creds.remove(provider);
        if let Err(e) = self.persist_guarded(&creds) {
            if let Some(old) = prev {
                creds.insert(provider.to_string(), old);
            }
            return Err(e);
        }
        Ok(())
    }

    /// List all providers (sorted — user-visible listings must be
    /// deterministic, not HashMap iteration order).
    pub fn list_providers(&self) -> Vec<String> {
        let mut providers: Vec<String> = self.creds.read().unwrap().keys().cloned().collect();
        providers.sort();
        providers
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

    /// 序列化 + 原子落盘。调用方必须已持 write guard 并传入 map 引用——
    /// 旧 `persist()` 自取 read lock，在 save/remove 持 write guard 期间
    /// 调用会同线程锁升级死锁；改为显式传参后，序列化在写锁内完成
    /// （快照一致性：并发 save 不可能把互相交叠的中间态写进盘文件），
    /// 也使失败回滚能在同一 guard 下恢复 prev。写文件期间持写锁 =
    /// 写者串行化，token 存储写频极低，可接受。
    fn persist_guarded(&self, creds: &HashMap<String, AuthCredential>) -> Result<(), String> {
        let data = serde_json::to_string_pretty(creds).map_err(|e| format!("serialize: {}", e))?;
        // auth.json 存 OAuth access/refresh token（bearer
        // 凭据），落盘必须原子写 + 0o600（对齐 TrustStore 的 纪律）。
        // 此前裸 std::fs:write 非原子：进程中断撕裂 = 半截 JSON，构造期
        // load 失败（现在会 loud）→ 空 store → 下次 save 覆盖即全量丢失；
        // 且默认权限在 Unix 下按 umask 常为 0644 世界可读。
        // write_file_atomic 自带父目录创建，错误统一 "atomic write..." 前缀。
        nemesis_utils::write_file_atomic(&self.path, data.as_bytes(), 0o600)
            .map_err(|e| format!("write: {e}"))
    }
}

#[cfg(test)]
mod tests;
