//! `SqliteApprovalJournal` —— 审批台账 SQLite 实现（approvals 表）。
//!
//! 契约：`nemesis_types::approval_journal:ApprovalJournal`。
//! - `record_pending`：`INSERT OR IGNORE` 幂等（同 id 重复不覆盖不报错）；
//! - `record_decision`：`UPDATE... WHERE status='pending'` 条件更新
//!   先到先得（web / IM / 重启补裁并发竞态恰有一方生效）。

use std::sync::Arc;

#[cfg(feature = "sqlite")]
use rusqlite::{OptionalExtension, params};

#[cfg(feature = "sqlite")]
use nemesis_types::approval_journal::status;
use nemesis_types::approval_journal::{ApprovalDecision, ApprovalJournal, ApprovalJournalEntry};

use crate::db::TaskDb;
#[cfg(feature = "sqlite")]
use crate::tasks::now_rfc3339;

/// 审批台账（clone 廉价，共享同一 `Arc<TaskDb>`——与任务真相源同库）。
#[derive(Clone)]
pub struct SqliteApprovalJournal {
    /// stub 态（sqlite 关）下字段仅由 from_db 写入不被读——同型构造面保留。
    #[cfg_attr(not(feature = "sqlite"), allow(dead_code))]
    db: Arc<TaskDb>,
}

impl SqliteApprovalJournal {
    #[cfg(feature = "sqlite")]
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        Ok(Self {
            db: TaskDb::open(path)?,
        })
    }

    /// sqlite 未编译进本构建：同型 open，写读皆 no-op（warn 一次诚实注记）。
    #[cfg(not(feature = "sqlite"))]
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        TaskDb::open(path)?;
        Ok(Self {
            db: Arc::new(TaskDb {}),
        })
    }

    pub fn from_db(db: Arc<TaskDb>) -> Self {
        Self { db }
    }
}

#[cfg(feature = "sqlite")]
fn row_entry(r: &rusqlite::Row<'_>) -> rusqlite::Result<ApprovalJournalEntry> {
    let payload_str: String = r.get(7)?;
    Ok(ApprovalJournalEntry {
        id: r.get(0)?,
        broker: r.get(1)?,
        short_id: r.get(2)?,
        operation: r.get(3)?,
        target: r.get(4)?,
        risk_level: r.get(5)?,
        reason: r.get(6)?,
        payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
        status: r.get(8)?,
        created_at: r.get(9)?,
        decided_at: r.get(10)?,
        decision_note: r.get(11)?,
        decided_by: r.get(12)?,
    })
}

#[cfg(feature = "sqlite")]
const ENTRY_COLUMNS: &str = "id, broker, short_id, operation, target, risk_level, reason, \
     payload, status, created_at, decided_at, decision_note, decided_by";

impl ApprovalJournal for SqliteApprovalJournal {
    #[cfg(feature = "sqlite")]
    fn record_pending(&self, entry: &ApprovalJournalEntry) -> Result<(), String> {
        self.db.with_conn(|conn| {
            conn.execute(
                "INSERT OR IGNORE INTO approvals (id, broker, short_id, operation, target, \
                 risk_level, reason, payload, status, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    entry.id,
                    entry.broker,
                    entry.short_id,
                    entry.operation,
                    entry.target,
                    entry.risk_level,
                    entry.reason,
                    entry.payload.to_string(),
                    status::PENDING,
                    entry.created_at,
                ],
            )
            .map_err(|e| format!("tasks.db insert approval failed: {e}"))?;
            Ok(())
        })
    }

    /// 条件更新先到先得；无账 = 恒不生效（Ok(false)）。
    #[cfg(not(feature = "sqlite"))]
    fn record_pending(&self, _entry: &ApprovalJournalEntry) -> Result<(), String> {
        Ok(())
    }

    #[cfg(feature = "sqlite")]
    fn record_decision(
        &self,
        id: &str,
        decision: ApprovalDecision,
        note: Option<&str>,
        decided_by: &str,
    ) -> Result<bool, String> {
        self.db.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE approvals SET status = ?2, decided_at = ?3, decision_note = ?4, \
                     decided_by = ?5 WHERE id = ?1 AND status = 'pending'",
                    params![id, decision.as_str(), now_rfc3339(), note, decided_by,],
                )
                .map_err(|e| format!("tasks.db decide approval failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 条件更新恒不生效（无账可裁——Ok(false)）。
    #[cfg(not(feature = "sqlite"))]
    fn record_decision(
        &self,
        _id: &str,
        _decision: ApprovalDecision,
        _note: Option<&str>,
        _decided_by: &str,
    ) -> Result<bool, String> {
        Ok(false)
    }

    /// 查询恒空 = 台账项不存在（Err 文案与真实现一致）。
    #[cfg(not(feature = "sqlite"))]
    fn entry(&self, id: &str) -> Result<ApprovalJournalEntry, String> {
        Err(format!("approval not found: {id}"))
    }

    #[cfg(feature = "sqlite")]
    fn entry(&self, id: &str) -> Result<ApprovalJournalEntry, String> {
        self.db.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM approvals WHERE id = ?1"),
                params![id],
                row_entry,
            )
            .optional()
            .map_err(|e| format!("tasks.db read approval failed: {e}"))?
            .ok_or_else(|| format!("approval not found: {id}"))
        })
    }

    #[cfg(feature = "sqlite")]
    fn pending_entries(&self) -> Vec<ApprovalJournalEntry> {
        self.db
            .with_conn(|conn| {
                let mut stmt = conn
                    .prepare(&format!(
                        "SELECT {ENTRY_COLUMNS} FROM approvals WHERE status = 'pending' \
                         ORDER BY created_at, id"
                    ))
                    .map_err(|e| format!("tasks.db list approvals failed: {e}"))?;
                let rows = stmt
                    .query_map([], row_entry)
                    .map_err(|e| format!("tasks.db list approvals failed: {e}"))?;
                let mut out = Vec::new();
                for row in rows {
                    out.push(row.map_err(|e| format!("tasks.db approval decode failed: {e}"))?);
                }
                Ok(out)
            })
            .unwrap_or_else(|e| {
                tracing::warn!("tasks.db pending approvals unreadable: {e}");
                Vec::new()
            })
    }

    /// 查询恒空 = 无 pending 台账项。
    #[cfg(not(feature = "sqlite"))]
    fn pending_entries(&self) -> Vec<ApprovalJournalEntry> {
        Vec::new()
    }

    #[cfg(feature = "sqlite")]
    fn find_pending(&self, id_or_short: &str) -> Option<ApprovalJournalEntry> {
        self.db
            .with_conn(|conn| {
                conn.query_row(
                    &format!(
                        "SELECT {ENTRY_COLUMNS} FROM approvals \
                         WHERE status = 'pending' AND (id = ?1 OR short_id = ?1) LIMIT 1"
                    ),
                    params![id_or_short],
                    row_entry,
                )
                .optional()
                .map_err(|e| format!("tasks.db find approval failed: {e}"))
            })
            .unwrap_or_else(|e| {
                tracing::warn!("tasks.db find pending approval failed: {e}");
                None
            })
    }

    /// 查询恒空 = 找不到 pending 台账项。
    #[cfg(not(feature = "sqlite"))]
    fn find_pending(&self, _id_or_short: &str) -> Option<ApprovalJournalEntry> {
        None
    }
}
