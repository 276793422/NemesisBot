//! 存储抽象层：`RevocationStore` trait + SQLite 默认实现 + 审计记录。
//!
//! 多后端设计：trait 抽象存储，默认 [`SqliteStore`]（rusqlite bundled，无系统依赖）。
//! 后续 MySQL/PostgreSQL/JSON 文件各加一个 impl，上层 API 不变。
//!
//! 表结构（SQLite）：
//! - `meta`(key, value)：crl_version / crl_valid_until / trusted_keys_version
//! - `crl_entries`(dim, value, revoked_at, reason)
//! - `trusted_keys`(key_fp, status, not_after)
//! - `audit`(id, timestamp, action, operator, dim, value, reason, detail)

use anyhow::{anyhow, Result};
use parking_lot::Mutex;
use revoke_common::{Crl, CrlEntry, KeyStatus, RevDim, TrustedKey, TrustedKeyList};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// 审计记录（admin 操作全留痕）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditRecord {
    pub id: i64,
    pub timestamp: u64,
    /// 操作类型：revoke / trust_upsert / trust_revoke / ...
    pub action: String,
    /// 操作者（admin token 标识 / Web 用户）。
    pub operator: String,
    pub dim: Option<String>,
    pub value: Option<String>,
    pub reason: Option<String>,
    pub detail: Option<String>,
}

/// 存储抽象（多后端：SQLite 默认，后续 MySQL/PG/JSON）。
pub trait RevocationStore: Send + Sync {
    // CRL
    fn list_crl(&self) -> Result<Crl>;
    #[allow(dead_code)] // trait 查询接口，bin 入口尚未接
    fn query_revoke(&self, dim: RevDim, value: &str) -> Result<Option<CrlEntry>>;
    fn add_revoke(&self, entry: CrlEntry) -> Result<u64>; // 返新 crl_version
    // trusted-keys
    fn list_trusted_keys(&self) -> Result<TrustedKeyList>;
    fn upsert_trusted_key(&self, key: TrustedKey) -> Result<u64>; // 返新 trusted_keys_version
    // audit
    fn add_audit(&self, record: AuditRecord) -> Result<()>;
    fn list_audit(&self, limit: u32) -> Result<Vec<AuditRecord>>;
}

// ===================== SQLite 实现（默认） =====================

/// SQLite 存储后端。`Connection` 非 Sync，用 Mutex 串行化（吊销低频，足够）。
pub struct SqliteStore {
    conn: Mutex<Connection>,
}

impl SqliteStore {
    /// 打开/创建数据库（path 如 `revoke.db` 或 `:memory:`）。
    pub fn open(path: &str) -> Result<Self> {
        let conn = Connection::open(path)?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 内存库（测试用）。
    #[allow(dead_code)] // 测试辅助，bin 暂未用
    pub fn open_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init_schema(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn init_schema(conn: &Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS crl_entries (
                 dim TEXT NOT NULL, value TEXT NOT NULL,
                 revoked_at INTEGER NOT NULL, reason TEXT NOT NULL,
                 PRIMARY KEY(dim, value));
             CREATE TABLE IF NOT EXISTS trusted_keys (
                 key_fp TEXT PRIMARY KEY, status TEXT NOT NULL, not_after INTEGER);
             CREATE TABLE IF NOT EXISTS audit (
                 id INTEGER PRIMARY KEY AUTOINCREMENT, timestamp INTEGER NOT NULL,
                 action TEXT NOT NULL, operator TEXT NOT NULL,
                 dim TEXT, value TEXT, reason TEXT, detail TEXT);
             INSERT OR IGNORE INTO meta(key, value) VALUES('crl_version','1');
             INSERT OR IGNORE INTO meta(key, value) VALUES('trusted_keys_version','1');
             INSERT OR IGNORE INTO meta(key, value) VALUES('crl_valid_until','0');",
        )?;
        Ok(())
    }

    #[allow(dead_code)] // 与 meta_get 对称保留，待 add/upsert 改用统一入口时接入
    fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO meta(key, value) VALUES(?,?)",
            params![key, value],
        )?;
        Ok(())
    }
}

/// 读 meta 表（自由函数：调用方持 conn 锁，避免重入死锁——parking_lot Mutex 不可重入）。
fn meta_get(conn: &Connection, key: &str) -> Result<String> {
    let v: Option<String> = conn
        .query_row("SELECT value FROM meta WHERE key=?", params![key], |r| r.get(0))
        .optional()?;
    v.ok_or_else(|| anyhow!("meta key not found: {}", key))
}

impl RevocationStore for SqliteStore {
    fn list_crl(&self) -> Result<Crl> {
        let conn = self.conn.lock();
        let version: u64 = meta_get(&conn, "crl_version")?.parse().unwrap_or(1);
        let valid_until: u64 = meta_get(&conn, "crl_valid_until")?.parse().unwrap_or(0);
        let mut stmt = conn.prepare("SELECT dim, value, revoked_at, reason FROM crl_entries")?;
        let entries = stmt
            .query_map([], |row| {
                Ok(CrlEntry {
                    dim: parse_dim(&row.get::<_, String>(0)?),
                    value: row.get(1)?,
                    revoked_at: row.get(2)?,
                    reason: row.get(3)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(Crl {
            version,
            valid_until,
            entries,
        })
    }

    fn query_revoke(&self, dim: RevDim, value: &str) -> Result<Option<CrlEntry>> {
        let conn = self.conn.lock();
        let row = conn
            .query_row(
                "SELECT dim, value, revoked_at, reason FROM crl_entries WHERE dim=? AND value=?",
                params![dim_str(dim), value],
                |r| {
                    Ok(CrlEntry {
                        dim: parse_dim(&r.get::<_, String>(0)?),
                        value: r.get(1)?,
                        revoked_at: r.get(2)?,
                        reason: r.get(3)?,
                    })
                },
            )
            .optional()?;
        Ok(row)
    }

    fn add_revoke(&self, entry: CrlEntry) -> Result<u64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO crl_entries(dim, value, revoked_at, reason) VALUES(?,?,?,?)",
            params![dim_str(entry.dim), entry.value, entry.revoked_at, entry.reason],
        )?;
        conn.execute(
            "UPDATE meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key='crl_version'",
            [],
        )?;
        let ver: u64 = conn
            .query_row("SELECT value FROM meta WHERE key='crl_version'", [], |r| {
                r.get::<_, String>(0)
            })?
            .parse()
            .unwrap_or(1);
        Ok(ver)
    }

    fn list_trusted_keys(&self) -> Result<TrustedKeyList> {
        let conn = self.conn.lock();
        let version: u64 = meta_get(&conn, "trusted_keys_version")?.parse().unwrap_or(1);
        let mut stmt = conn.prepare("SELECT key_fp, status, not_after FROM trusted_keys")?;
        let keys = stmt
            .query_map([], |row| {
                Ok(TrustedKey {
                    key_fp: row.get(0)?,
                    status: parse_status(&row.get::<_, String>(1)?),
                    not_after: row.get(2)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(TrustedKeyList {
            version,
            valid_until: u64::MAX,
            keys,
        })
    }

    fn upsert_trusted_key(&self, key: TrustedKey) -> Result<u64> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT OR REPLACE INTO trusted_keys(key_fp, status, not_after) VALUES(?,?,?)",
            params![key.key_fp, status_str(key.status), key.not_after],
        )?;
        conn.execute(
            "UPDATE meta SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT) WHERE key='trusted_keys_version'",
            [],
        )?;
        let ver: u64 = conn
            .query_row("SELECT value FROM meta WHERE key='trusted_keys_version'", [], |r| {
                r.get::<_, String>(0)
            })?
            .parse()
            .unwrap_or(1);
        Ok(ver)
    }

    fn add_audit(&self, record: AuditRecord) -> Result<()> {
        let conn = self.conn.lock();
        conn.execute(
            "INSERT INTO audit(timestamp, action, operator, dim, value, reason, detail)
             VALUES(?,?,?,?,?,?,?)",
            params![
                record.timestamp,
                record.action,
                record.operator,
                record.dim,
                record.value,
                record.reason,
                record.detail
            ],
        )?;
        Ok(())
    }

    fn list_audit(&self, limit: u32) -> Result<Vec<AuditRecord>> {
        let conn = self.conn.lock();
        let mut stmt = conn.prepare(
            "SELECT id, timestamp, action, operator, dim, value, reason, detail
             FROM audit ORDER BY id DESC LIMIT ?",
        )?;
        let rows = stmt
            .query_map(params![limit as i64], |row| {
                Ok(AuditRecord {
                    id: row.get(0)?,
                    timestamp: row.get(1)?,
                    action: row.get(2)?,
                    operator: row.get(3)?,
                    dim: row.get(4)?,
                    value: row.get(5)?,
                    reason: row.get(6)?,
                    detail: row.get(7)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();
        Ok(rows)
    }
}

// ---- enum ↔ string 转换 ----
fn dim_str(d: RevDim) -> &'static str {
    match d {
        RevDim::KeyId => "key_id",
        RevDim::SigHash => "sig_hash",
        RevDim::FileHash => "file_hash",
        RevDim::Publisher => "publisher",
    }
}
fn parse_dim(s: &str) -> RevDim {
    match s {
        "sig_hash" => RevDim::SigHash,
        "file_hash" => RevDim::FileHash,
        "publisher" => RevDim::Publisher,
        _ => RevDim::KeyId,
    }
}
fn status_str(s: KeyStatus) -> &'static str {
    match s {
        KeyStatus::Active => "active",
        KeyStatus::Revoked => "revoked",
    }
}
fn parse_status(s: &str) -> KeyStatus {
    match s {
        "revoked" => KeyStatus::Revoked,
        _ => KeyStatus::Active,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> SqliteStore {
        SqliteStore::open_memory().unwrap()
    }

    #[test]
    fn crl_add_and_query() {
        let s = store();
        let entry = CrlEntry {
            dim: RevDim::KeyId,
            value: "abc".into(),
            revoked_at: 100,
            reason: "leak".into(),
        };
        let ver = s.add_revoke(entry.clone()).unwrap();
        assert_eq!(ver, 2); // 初始 version=1，add 后 2
        // 查
        let hit = s.query_revoke(RevDim::KeyId, "abc").unwrap().unwrap();
        assert_eq!(hit.value, "abc");
        assert_eq!(hit.reason, "leak");
        // 不命中
        assert!(s.query_revoke(RevDim::KeyId, "none").unwrap().is_none());
        // 列表
        let crl = s.list_crl().unwrap();
        assert_eq!(crl.version, 2);
        assert_eq!(crl.entries.len(), 1);
    }

    #[test]
    fn trusted_keys_upsert_list() {
        let s = store();
        let k = TrustedKey {
            key_fp: "ff".into(),
            status: KeyStatus::Active,
            not_after: None,
        };
        let v1 = s.upsert_trusted_key(k.clone()).unwrap();
        assert_eq!(v1, 2);
        // 更新同 key_fp → version 再 +1
        let k2 = TrustedKey {
            key_fp: "ff".into(),
            status: KeyStatus::Revoked,
            not_after: None,
        };
        let v2 = s.upsert_trusted_key(k2).unwrap();
        assert_eq!(v2, 3);
        let tkl = s.list_trusted_keys().unwrap();
        assert_eq!(tkl.keys.len(), 1);
        assert_eq!(tkl.keys[0].status, KeyStatus::Revoked);
    }

    #[test]
    fn audit_append_and_list() {
        let s = store();
        s.add_audit(AuditRecord {
            id: 0,
            timestamp: 1,
            action: "revoke".into(),
            operator: "tester".into(),
            dim: Some("key_id".into()),
            value: Some("abc".into()),
            reason: Some("leak".into()),
            detail: None,
        })
        .unwrap();
        s.add_audit(AuditRecord {
            id: 0,
            timestamp: 2,
            action: "trust_upsert".into(),
            operator: "tester".into(),
            dim: None,
            value: None,
            reason: None,
            detail: None,
        })
        .unwrap();
        let list = s.list_audit(10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].action, "trust_upsert"); // DESC → 最新在前
        assert_eq!(list[1].action, "revoke");
    }
}
