//! 本地账号 + argon2 密码哈希（验证项 ③）。
//!
//! 账号文件是普通 JSON（`users.json`），密码只存 argon2id PHC 字符串。
//! `hash_password` 就是 demo CLI `hash-password` 子命令的内核。

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::OnceLock;

use argon2::password_hash::{rand_core::OsRng, SaltString};
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use serde::Deserialize;
use thiserror::Error;

use crate::identity::{AuthnSource, Identity};

#[derive(Debug, Error)]
pub enum LocalAuthError {
    #[error("用户不存在或密码错误")]
    BadCredentials,
    #[error("账号文件解析失败: {0}")]
    Parse(String),
    #[error("账号文件 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    #[error("账号文件格式非法: {0}")]
    Format(String),
    #[error("密码哈希失败: {0}")]
    Hash(String),
}

/// 手写 PartialEq（`io::Error` 本身无 PartialEq；Io 变体按同变体比较，
/// 只为测试断言服务）。
impl PartialEq for LocalAuthError {
    fn eq(&self, other: &Self) -> bool {
        use LocalAuthError::*;
        match (self, other) {
            (BadCredentials, BadCredentials) => true,
            (Io(_), Io(_)) => true,
            (Parse(a), Parse(b)) | (Format(a), Format(b)) | (Hash(a), Hash(b)) => a == b,
            _ => false,
        }
    }
}

/// 账号文件中的单条记录。
#[derive(Debug, Clone, Deserialize)]
pub struct LocalUserRecord {
    pub username: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub roles: Vec<String>,
    /// argon2id PHC 格式（`$argon2id$v=19$...`）
    pub password_hash: String,
}

/// 本地账号库。用户名按小写归一（登录大小写不敏感，文件里建议也写小写）。
#[derive(Debug, Clone)]
pub struct LocalUserDatabase {
    users: BTreeMap<String, LocalUserRecord>,
}

/// argon2id 哈希（默认参数：v19 / m=19456 / t=2 / p=1，RFC 9106 第一推荐档）。
pub fn hash_password(password: &str) -> Result<String, LocalAuthError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| LocalAuthError::Hash(e.to_string()))
}

/// 校验密码 vs PHC 哈希。
pub fn verify_password(password: &str, phc_hash: &str) -> Result<bool, LocalAuthError> {
    let parsed = PasswordHash::new(phc_hash)
        .map_err(|e| LocalAuthError::Hash(format!("PHC 解析失败: {e}")))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

/// 未知用户也做一次假校验，抹平"存在性"时序差异（与真实校验同代价）。
fn dummy_verify(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("timing-equalizer-demo").unwrap_or_default());
    if !hash.is_empty() {
        let _ = verify_password(password, hash);
    }
}

impl LocalUserDatabase {
    pub fn from_json_str(json: &str) -> Result<Self, LocalAuthError> {
        #[derive(Deserialize)]
        struct File {
            users: Vec<LocalUserRecord>,
        }
        let file: File =
            serde_json::from_str(json).map_err(|e| LocalAuthError::Parse(e.to_string()))?;
        let mut users = BTreeMap::new();
        for rec in file.users {
            if rec.username.trim().is_empty() {
                return Err(LocalAuthError::Format("存在空 username".into()));
            }
            let key = rec.username.to_lowercase();
            if users.insert(key, rec).is_some() {
                return Err(LocalAuthError::Format(
                    "username 重复（大小写归一后冲突）".into(),
                ));
            }
        }
        Ok(Self { users })
    }

    pub fn load_from_json_file(path: &Path) -> Result<Self, LocalAuthError> {
        let raw = std::fs::read_to_string(path)?;
        Self::from_json_str(&raw)
    }

    pub fn user_count(&self) -> usize {
        self.users.len()
    }

    /// 认证：成功 → 通用 [`Identity`]；失败统一 `BadCredentials`
    /// （不区分"用户不存在"和"密码错误"，不向攻击者泄露账号存在性）。
    pub fn authenticate(&self, username: &str, password: &str) -> Result<Identity, LocalAuthError> {
        match self.users.get(&username.to_lowercase()) {
            Some(rec) => {
                if verify_password(password, &rec.password_hash)? {
                    Ok(Identity {
                        subject: rec.username.clone(),
                        display_name: if rec.display_name.is_empty() {
                            rec.username.clone()
                        } else {
                            rec.display_name.clone()
                        },
                        roles: rec.roles.clone(),
                        source: AuthnSource::Local,
                    })
                } else {
                    Err(LocalAuthError::BadCredentials)
                }
            }
            None => {
                dummy_verify(password);
                Err(LocalAuthError::BadCredentials)
            }
        }
    }
}

#[cfg(test)]
mod tests;
