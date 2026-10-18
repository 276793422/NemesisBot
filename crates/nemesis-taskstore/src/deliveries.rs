//! deliveries 表 —— 交付账本（P1 建表；P4 交付门泛化：拒绝语义 + 查询面）。
//!
//! 幂等键 UNIQUE：同键重复 `record_delivery` 返回 `Ok(false)`（已记过，
//! 不重复记账）；`confirm_delivery` 升 delivered → confirmed；
//! `reject_delivery` 从任何非 rejected 状态进 rejected（终态，带原因
//! 留痕，幂等：已 rejected 再拒 = Ok(false)）。

use rusqlite::{OptionalExtension, params};

use crate::db::TaskDb;
use crate::tasks::now_rfc3339;

#[derive(Debug, Clone)]
pub struct DeliveryRecord {
    pub id: String,
    pub run_id: Option<String>,
    pub kind: String,
    pub idempotency_key: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub created_at: String,
    pub delivered_at: Option<String>,
    pub confirmed_at: Option<String>,
    pub rejected_at: Option<String>,
    pub reject_reason: Option<String>,
}

/// deliveries 查询列清单（与 row 映射共用，防 SELECT 与映射漂移）。
const DELIVERY_COLUMNS: &str = "id, run_id, kind, idempotency_key, status, payload, \
     created_at, delivered_at, confirmed_at, rejected_at, reject_reason";

fn row_to_delivery(r: &rusqlite::Row<'_>) -> rusqlite::Result<DeliveryRecord> {
    let payload_str: String = r.get(5)?;
    Ok(DeliveryRecord {
        id: r.get(0)?,
        run_id: r.get(1)?,
        kind: r.get(2)?,
        idempotency_key: r.get(3)?,
        status: r.get(4)?,
        payload: serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null),
        created_at: r.get(6)?,
        delivered_at: r.get(7)?,
        confirmed_at: r.get(8)?,
        rejected_at: r.get(9)?,
        reject_reason: r.get(10)?,
    })
}

impl TaskDb {
    /// 记一笔交付。幂等：同 idempotency_key 已存在 = `Ok(false)`（不覆盖
    /// 既有记录）；新记 = `Ok(true)`。落库即 `delivered` 状态。
    pub fn record_delivery(
        &self,
        kind: &str,
        run_id: Option<&str>,
        idempotency_key: &str,
        payload: &serde_json::Value,
    ) -> Result<bool, String> {
        self.with_conn(|conn| {
            let now = now_rfc3339();
            let n = conn
                .execute(
                    "INSERT OR IGNORE INTO deliveries (id, run_id, kind, idempotency_key, \
                     status, payload, created_at, delivered_at)
                     VALUES (?1, ?2, ?3, ?4, 'delivered', ?5, ?6, ?6)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        run_id,
                        kind,
                        idempotency_key,
                        payload.to_string(),
                        now,
                    ],
                )
                .map_err(|e| format!("tasks.db record delivery failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 确认交付（recorded/delivered → confirmed，幂等：已确认 = Ok(false)）。
    pub fn confirm_delivery(&self, idempotency_key: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET status = 'confirmed', confirmed_at = ?2 \
                     WHERE idempotency_key = ?1 AND status != 'confirmed'",
                    params![idempotency_key, now_rfc3339()],
                )
                .map_err(|e| format!("tasks.db confirm delivery failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 按幂等键查（不存在 = Ok(None)）。
    pub fn delivery_by_key(&self, idempotency_key: &str) -> Result<Option<DeliveryRecord>, String> {
        self.with_conn(|conn| {
            conn.query_row(
                &format!("SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE idempotency_key = ?1"),
                params![idempotency_key],
                row_to_delivery,
            )
            .optional()
            .map_err(|e| format!("tasks.db read delivery failed: {e}"))
        })
    }

    /// 拒绝交付（P4 交付门）：任何非 rejected 状态 → rejected（终态），
    /// 带 `reason` 留痕。幂等：已 rejected 再拒 = `Ok(false)`。
    /// 拒绝已 confirmed 的交付合法（人工复核推翻自动验收的入口）。
    pub fn reject_delivery(&self, idempotency_key: &str, reason: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET status = 'rejected', rejected_at = ?2, \
                     reject_reason = ?3 WHERE idempotency_key = ?1 AND status != 'rejected'",
                    params![idempotency_key, now_rfc3339(), reason],
                )
                .map_err(|e| format!("tasks.db reject delivery failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 浅合并交付 payload（G4，2026-10-18 批六：评审 anchor_summary 注记）。
    /// read-modify-write 单事务（同库连接内 BEGIN IMMEDIATE 语义——读与写
    /// 之间不被并发写插队）；`patch` 的顶层键逐一覆盖、其余键保留（不做
    /// 深合并——payload 顶层是扁平摘要字段）。返回合并后的完整 payload。
    /// 幂等键不存在 = `Err`（注记必须落在真实交付上，不凭空造行）。
    pub fn merge_delivery_payload(
        &self,
        idempotency_key: &str,
        patch: &serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.with_conn(|conn| {
            let tx = conn
                .unchecked_transaction()
                .map_err(|e| format!("tasks.db merge delivery begin failed: {e}"))?;
            let payload_str: String = tx
                .query_row(
                    "SELECT payload FROM deliveries WHERE idempotency_key = ?1",
                    params![idempotency_key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("tasks.db read delivery failed: {e}"))?
                .ok_or_else(|| format!("delivery {idempotency_key} not found"))?;
            let mut payload: serde_json::Value =
                serde_json::from_str(&payload_str).unwrap_or(serde_json::Value::Null);
            if let (Some(obj), Some(patch_obj)) = (payload.as_object_mut(), patch.as_object()) {
                for (k, v) in patch_obj {
                    obj.insert(k.clone(), v.clone());
                }
            }
            tx.execute(
                "UPDATE deliveries SET payload = ?2 WHERE idempotency_key = ?1",
                params![idempotency_key, payload.to_string()],
            )
            .map_err(|e| format!("tasks.db merge delivery failed: {e}"))?;
            tx.commit()
                .map_err(|e| format!("tasks.db merge delivery commit failed: {e}"))?;
            Ok(payload)
        })
    }

    /// 按执行 ID 列交付（run 维证据包查询面；created_at + id 稳定序）。
    pub fn deliveries_by_run(&self, run_id: &str) -> Result<Vec<DeliveryRecord>, String> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {DELIVERY_COLUMNS} FROM deliveries WHERE run_id = ?1 \
                     ORDER BY created_at ASC, id ASC"
                ))
                .map_err(|e| format!("tasks.db prepare delivery list failed: {e}"))?;
            let rows = stmt
                .query_map(params![run_id], row_to_delivery)
                .map_err(|e| format!("tasks.db list deliveries failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("tasks.db list deliveries failed: {e}"))
        })
    }

    /// 最近 N 笔交付（审计/仪表查询面；created_at DESC + id DESC 稳定序，
    /// `limit` 为 0 = 空列表不做全量拉取）。
    pub fn deliveries_recent(&self, limit: usize) -> Result<Vec<DeliveryRecord>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {DELIVERY_COLUMNS} FROM deliveries \
                     ORDER BY created_at DESC, id DESC LIMIT ?1"
                ))
                .map_err(|e| format!("tasks.db prepare delivery recent failed: {e}"))?;
            let rows = stmt
                .query_map(params![limit as i64], row_to_delivery)
                .map_err(|e| format!("tasks.db recent deliveries failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("tasks.db recent deliveries failed: {e}"))
        })
    }
}
