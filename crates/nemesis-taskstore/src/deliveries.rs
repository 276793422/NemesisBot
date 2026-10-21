//! deliveries 表 —— 交付账本（P1 建表；P4 交付门泛化：拒绝语义 + 查询面；
//! 项 C 通知三段：notify_state 与出站失败重试箱）。
//!
//! 幂等键 UNIQUE：同键重复 `record_delivery` 返回 `Ok(false)`（已记过，
//! 不重复记账）；`mark_delivery_channel_accepted` 升 recorded → delivered
//! （通道接受时置位）；`confirm_delivery` 升 → confirmed；`reject_delivery`
//! 从任何非 rejected 状态进 rejected（终态，带原因留痕，幂等：已 rejected
//! 再拒 = Ok(false)）。
//!
//! 通知义务（notify_state，与 status 双维正交）：pending（有义务）→
//! channel_accepted → confirmed；none = 记账时声明无出站义务；legacy =
//! v4 前旧账（不参与通知管线）；dead = 重试耗尽死信终态。通道 send 失败
//! 的出站消息进 `notify_outbox` 指数退避重试（CD6 同款退避曲线）。

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
    /// 通知义务段位（项 C）：pending / channel_accepted / confirmed /
    /// none / legacy / dead。
    pub notify_state: String,
    /// 通知失败累计次数（重试债记账；成功任意处清零归位）。
    pub notify_attempts: i64,
    /// 最近一次通知失败原因（可观测）。
    pub notify_last_error: Option<String>,
    /// 下次重试到期时间（RFC3339 本地时；NULL = 不在重试队列）。
    pub notify_next_retry_at: Option<String>,
}

/// deliveries 查询列清单（与 row 映射共用，防 SELECT 与映射漂移）。
const DELIVERY_COLUMNS: &str = "id, run_id, kind, idempotency_key, status, payload, \
     created_at, delivered_at, confirmed_at, rejected_at, reject_reason, \
     notify_state, notify_attempts, notify_last_error, notify_next_retry_at";

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
        notify_state: r.get(11)?,
        notify_attempts: r.get(12)?,
        notify_last_error: r.get(13)?,
        notify_next_retry_at: r.get(14)?,
    })
}

// -- 通知重试参数（CD6 TransferOutbox 同款曲线，本地常量对齐）------------
// nemesis-taskstore 不依赖 nemesis-cluster（依赖方向），数值在此对齐；
// 两侧改动必须同步（漂移 = 两个重试面行为分叉）。

/// 通知失败退避起步（秒）——15s 起步指数爬升。
pub const NOTIFY_BACKOFF_BASE_SECS: u64 = 15;
/// 退避封顶（秒）——10 分钟（CD6 裁决⑩）。
pub const NOTIFY_BACKOFF_CAP_SECS: u64 = 600;
/// 死信 age 闸（秒）——7 天，对齐 task_result TTL（CD6 裁决⑨）。
pub const NOTIFY_DEAD_LETTER_AGE_SECS: i64 = 7 * 24 * 3600;
/// 死信 attempts 闸——与 age 先到先死（CD6 裁决⑨）。
pub const NOTIFY_DEAD_LETTER_MAX_ATTEMPTS: i64 = 1000;

/// 通知重试退避曲线（CD6 `push_backoff_secs` 同款）：15s ×
/// 2^(n-1) 封顶 600s。`attempts` 从 1 起计（首次失败 = 第 1 次 → 15s）。
pub fn notify_backoff_secs(attempts: i64) -> u64 {
    let shift = attempts.saturating_sub(1).clamp(0, 6) as u32;
    NOTIFY_BACKOFF_BASE_SECS
        .saturating_mul(1u64 << shift)
        .min(NOTIFY_BACKOFF_CAP_SECS)
}

/// 通知死信闸（CD6 同款双条件，任一命中即判死）：入箱年龄超 7 天，
/// 或累计失败 ≥1000 次。`age_secs` 为负（created_at 在未来/解析异常）
/// 按 0 处理——age 闸不因时钟抖动误判死。
pub fn notify_dead_lettered(age_secs: i64, attempts: i64) -> bool {
    age_secs >= NOTIFY_DEAD_LETTER_AGE_SECS || attempts >= NOTIFY_DEAD_LETTER_MAX_ATTEMPTS
}

/// notify_outbox 行（通道 send 失败暂存；message = JSON 序列化的
/// OutboundMessage，sweep 时反序列化后重投）。
#[derive(Debug, Clone)]
pub struct NotifyOutboxRow {
    pub id: String,
    pub delivery_key: String,
    pub message: String,
    pub created_at: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub next_retry_at: String,
}

const NOTIFY_OUTBOX_COLUMNS: &str =
    "id, delivery_key, message, created_at, attempts, last_error, next_retry_at";

fn row_to_notify_outbox(r: &rusqlite::Row<'_>) -> rusqlite::Result<NotifyOutboxRow> {
    Ok(NotifyOutboxRow {
        id: r.get(0)?,
        delivery_key: r.get(1)?,
        message: r.get(2)?,
        created_at: r.get(3)?,
        attempts: r.get(4)?,
        last_error: r.get(5)?,
        next_retry_at: r.get(6)?,
    })
}

impl TaskDb {
    /// 记一笔交付。幂等：同 idempotency_key 已存在 = `Ok(false)`（不覆盖
    /// 既有记录）；新记 = `Ok(true)`。
    ///
    /// 初始状态 `recorded`（项 C：delivered 由通道接受时置位，不再落库即
    /// delivered——落库只代表「义务已登记」，不代表「通道收下了」）。
    /// `notify_expected`：本笔交付是否有出站通知义务（true = pending 且
    /// 立即进入重试调度；false = none，通道接受/确认照常记账但不重试）。
    pub fn record_delivery(
        &self,
        kind: &str,
        run_id: Option<&str>,
        idempotency_key: &str,
        payload: &serde_json::Value,
        notify_expected: bool,
    ) -> Result<bool, String> {
        self.with_conn(|conn| {
            let now = now_rfc3339();
            let (notify_state, notify_next_retry_at) = if notify_expected {
                ("pending", Some(now.clone()))
            } else {
                ("none", None)
            };
            let n = conn
                .execute(
                    "INSERT OR IGNORE INTO deliveries (id, run_id, kind, idempotency_key, \
                     status, payload, created_at, notify_state, notify_next_retry_at)
                     VALUES (?1, ?2, ?3, ?4, 'recorded', ?5, ?6, ?7, ?8)",
                    params![
                        uuid::Uuid::new_v4().to_string(),
                        run_id,
                        kind,
                        idempotency_key,
                        payload.to_string(),
                        now,
                        notify_state,
                        notify_next_retry_at,
                    ],
                )
                .map_err(|e| format!("tasks.db record delivery failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 通道接受出站消息（项 C）：recorded → delivered（delivered_at 首次
    /// 置位）+ notify_state 升 channel_accepted + 清重试到期。forward-only
    /// 幂等：仅 `status='recorded'` 时生效——confirmed/rejected 后到达的
    /// 迟到接受是 no-op（拒绝/确认语义不被投递回执覆盖）。
    pub fn mark_delivery_channel_accepted(&self, idempotency_key: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET status = 'delivered', \
                     delivered_at = COALESCE(delivered_at, ?2), \
                     notify_state = 'channel_accepted', notify_next_retry_at = NULL \
                     WHERE idempotency_key = ?1 AND status = 'recorded'",
                    params![idempotency_key, now_rfc3339()],
                )
                .map_err(|e| format!("tasks.db mark delivery accepted failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 通知死信（项 C）：pending → dead（重试耗尽终态，停止调度）。
    /// 幂等：非 pending（none/legacy/已 confirmed/已 dead）= Ok(false)。
    pub fn mark_delivery_notify_dead(&self, idempotency_key: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET notify_state = 'dead', notify_next_retry_at = NULL \
                     WHERE idempotency_key = ?1 AND notify_state = 'pending'",
                    params![idempotency_key],
                )
                .map_err(|e| format!("tasks.db mark delivery notify dead failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 确认交付（recorded/delivered → confirmed，幂等：已确认 = Ok(false)）。
    /// 通知义务同步升 confirmed（义务走完终点；无论此前 pending 还是
    /// channel_accepted）。
    pub fn confirm_delivery(&self, idempotency_key: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET status = 'confirmed', confirmed_at = ?2, \
                     notify_state = 'confirmed', notify_next_retry_at = NULL \
                     WHERE idempotency_key = ?1 AND status != 'confirmed'",
                    params![idempotency_key, now_rfc3339()],
                )
                .map_err(|e| format!("tasks.db confirm delivery failed: {e}"))?;
            Ok(n > 0)
        })
    }

    /// 通知失败入箱（项 C）：UPSERT 按 delivery_key 幂等——同笔交付同时
    /// 至多一笔待重试；冲突 = 保留原行 id/created_at，覆盖 message、累计
    /// attempts、刷新 last_error 与退避后的 next_retry_at。首次入箱
    /// attempts=1。`delivery_key` 的交付必须存在且 notify_state='pending'
    /// （调用方守卫；无义务/已接受/已确认不产生重试债）。
    pub fn notify_enqueue_failure(
        &self,
        delivery_key: &str,
        message: &str,
        last_error: &str,
    ) -> Result<(), String> {
        self.with_conn(|conn| {
            let now = now_rfc3339();
            let existing: Option<i64> = conn
                .query_row(
                    "SELECT attempts FROM notify_outbox WHERE delivery_key = ?1",
                    params![delivery_key],
                    |r| r.get(0),
                )
                .optional()
                .map_err(|e| format!("tasks.db read notify outbox failed: {e}"))?;
            let attempts = existing.unwrap_or(0) + 1;
            let next_retry = crate::tasks::rfc3339_after_secs(notify_backoff_secs(attempts));
            conn.execute(
                "INSERT INTO notify_outbox (id, delivery_key, message, created_at, \
                 attempts, last_error, next_retry_at)
                 VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6)
                 ON CONFLICT(delivery_key) DO UPDATE SET \
                 message = excluded.message, attempts = ?7, \
                 last_error = excluded.last_error, next_retry_at = excluded.next_retry_at",
                params![
                    uuid::Uuid::new_v4().to_string(),
                    delivery_key,
                    message,
                    now,
                    last_error,
                    next_retry,
                    attempts,
                ],
            )
            .map_err(|e| format!("tasks.db enqueue notify retry failed: {e}"))?;
            // 交付侧记账：累计次数 + 最近错误（可观测面；调度仍以
            // notify_outbox.next_retry_at 为准）。
            conn.execute(
                "UPDATE deliveries SET notify_attempts = ?2, notify_last_error = ?3, \
                 notify_next_retry_at = ?4 WHERE idempotency_key = ?1",
                params![delivery_key, attempts, last_error, next_retry],
            )
            .map_err(|e| format!("tasks.db update delivery notify state failed: {e}"))?;
            Ok(())
        })
    }

    /// 到期重试行（next_retry_at <= now，升序 = 最先到期的先出）。
    pub fn notify_due(&self, now: &str, limit: usize) -> Result<Vec<NotifyOutboxRow>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare(&format!(
                    "SELECT {NOTIFY_OUTBOX_COLUMNS} FROM notify_outbox \
                     WHERE next_retry_at <= ?1 ORDER BY next_retry_at ASC, id ASC LIMIT ?2"
                ))
                .map_err(|e| format!("tasks.db prepare notify due failed: {e}"))?;
            let rows = stmt
                .query_map(params![now, limit as i64], row_to_notify_outbox)
                .map_err(|e| format!("tasks.db read notify due failed: {e}"))?;
            rows.collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("tasks.db read notify due failed: {e}"))
        })
    }

    /// 出箱（重试成功后清债）。
    pub fn notify_remove(&self, outbox_id: &str) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM notify_outbox WHERE id = ?1",
                params![outbox_id],
            )
            .map_err(|e| format!("tasks.db remove notify outbox failed: {e}"))?;
            Ok(())
        })
    }

    /// 按交付键清箱（通道在任意通道上成功接受时清该交付的重试债）。
    pub fn notify_remove_by_delivery(&self, delivery_key: &str) -> Result<(), String> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM notify_outbox WHERE delivery_key = ?1",
                params![delivery_key],
            )
            .map_err(|e| format!("tasks.db remove notify outbox failed: {e}"))?;
            Ok(())
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
    /// 拒绝同时终止通知义务（清重试到期；死信/箱内行由调用方 sweep 清）。
    pub fn reject_delivery(&self, idempotency_key: &str, reason: &str) -> Result<bool, String> {
        self.with_conn(|conn| {
            let n = conn
                .execute(
                    "UPDATE deliveries SET status = 'rejected', rejected_at = ?2, \
                     reject_reason = ?3, notify_next_retry_at = NULL \
                     WHERE idempotency_key = ?1 AND status != 'rejected'",
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
