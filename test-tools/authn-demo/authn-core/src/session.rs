//! 会话签发 / 过期 / 并发上限（验证项 ④）。
//!
//! 内存实现 + 最小存储 trait 轮廓：移植时产品侧把内存 HashMap 换成
//! 自己的存储（SQLite/Redis），接口形状不变。
//!
//! 安全注记：demo 内存 map 直接以 token 明文为键（进程内、即用即弃）；
//! 生产集成应在存储层只存 `SHA-256(token)`，本文件留有 `token_fingerprint`
//! 供移植时统一改写。

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use base64::Engine;
use rand::RngCore;
use serde::Serialize;
use thiserror::Error;

use crate::identity::Identity;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum SessionError {
    #[error("会话不存在或已吊销")]
    Invalid,
    #[error("会话已过期")]
    Expired,
    #[error("该用户并发会话数已达上限（{0}）")]
    LimitReached(usize),
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// 会话 TTL（默认 3600s）
    pub ttl_seconds: u64,
    /// 单用户并发会话上限（默认 4；超限拒绝新签发，不动旧会话）
    pub max_sessions_per_user: usize,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            ttl_seconds: 3600,
            max_sessions_per_user: 4,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionRecord {
    /// 会话令牌（base64url(32 随机字节)，122 bit 熵）
    pub token: String,
    pub subject: String,
    pub issued_at_unix: u64,
    pub expires_at_unix: u64,
}

/// 存储轮廓（移植锚点）：产品侧实现此 trait 即可替换内存存储。
/// demo 内不实例化 trait object——内存实现直接走同构的内联路径。
pub trait SessionStorage: Send + Sync {
    fn put(&self, record: SessionRecord) -> Result<(), SessionError>;
    fn get(&self, token: &str) -> Option<SessionRecord>;
    fn remove(&self, token: &str) -> bool;
    fn count_for_subject(&self, subject: &str) -> usize;
}

/// 存储侧只应保存令牌指纹而非明文（移植注记，demo 自身未启用）。
pub fn token_fingerprint(token: &str) -> String {
    // SHA-256 引入会拖依赖（本 crate 特意不引哈希库）；demo 用 base64 两次
    // 编码占位示意。移植时替换为 SHA-256 hex。
    let b64 = base64::engine::general_purpose::STANDARD;
    format!("demo-fp:{}", b64.encode(token.as_bytes()))
}

pub struct SessionStore {
    config: SessionConfig,
    sessions: std::sync::Mutex<HashMap<String, SessionRecord>>,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

impl SessionStore {
    pub fn new(config: SessionConfig) -> Self {
        Self {
            config,
            sessions: std::sync::Mutex::new(HashMap::new()),
        }
    }

    pub fn config(&self) -> &SessionConfig {
        &self.config
    }

    /// 为身份签发新会话。单用户并发超限时拒绝新签发（保旧拒新）。
    pub fn issue(&self, identity: &Identity) -> Result<SessionRecord, SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        // 先清该用户的过期会话，再数并发（过期不占额度）
        let now = now_unix();
        sessions.retain(|_, r| r.expires_at_unix > now);
        let live = sessions
            .values()
            .filter(|r| r.subject == identity.subject)
            .count();
        if live >= self.config.max_sessions_per_user {
            return Err(SessionError::LimitReached(
                self.config.max_sessions_per_user,
            ));
        }
        let issued = now;
        let expires = issued + self.config.ttl_seconds;
        let record = SessionRecord {
            token: new_token(),
            subject: identity.subject.clone(),
            issued_at_unix: issued,
            expires_at_unix: expires,
        };
        sessions.insert(record.token.clone(), record.clone());
        Ok(record)
    }

    /// 校验令牌：不存在 → `Invalid`；过期 → `Expired`（并顺带清除）。
    pub fn validate(&self, token: &str) -> Result<SessionRecord, SessionError> {
        let mut sessions = self.sessions.lock().unwrap();
        let record = sessions.get(token).cloned().ok_or(SessionError::Invalid)?;
        if now_unix() >= record.expires_at_unix {
            sessions.remove(token);
            return Err(SessionError::Expired);
        }
        Ok(record)
    }

    /// 吊销。返回是否存在过该会话。
    pub fn revoke(&self, token: &str) -> bool {
        self.sessions.lock().unwrap().remove(token).is_some()
    }

    /// 清扫全部过期会话，返回清扫数量（移植时对应存储层 TTL 任务）。
    pub fn purge_expired(&self) -> usize {
        let mut sessions = self.sessions.lock().unwrap();
        let now = now_unix();
        let before = sessions.len();
        sessions.retain(|_, r| r.expires_at_unix > now);
        before - sessions.len()
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// 32 随机字节 → base64url（无填充）。OS CSPRNG。
fn new_token() -> String {
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

/// 便于测试注入的 TTL 提示：demo CLI 演示过期用 `--ttl-secs 1`。
pub fn ttl_duration(config: &SessionConfig) -> Duration {
    Duration::from_secs(config.ttl_seconds)
}

#[cfg(test)]
mod tests;
