//! CRUD operations for usage statistics.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};

use crate::db;
use crate::models::{RequestLog, TrendPoint, UsageSummary};

/// Thread-safe SQLite data store.
pub struct DataStore {
    conn: Mutex<Connection>,
}

impl DataStore {
    /// Open (or create) the database at `db_path`.
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn = db::init_db(db_path)?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Record a single LLM request log.
    pub fn insert_request_log(&self, log: &RequestLog) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO request_logs
                (trace_id, model, provider_type, input_tokens, output_tokens,
                 cache_creation_tokens, cache_read_tokens, total_cost_usd,
                 latency_ms, status_code, error_message, is_streaming, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![
                log.trace_id,
                log.model,
                log.provider_type,
                log.input_tokens,
                log.output_tokens,
                log.cache_creation_tokens,
                log.cache_read_tokens,
                log.total_cost_usd,
                log.latency_ms,
                log.status_code,
                log.error_message,
                log.is_streaming as i32,
                log.created_at,
            ],
        )
        .map_err(|e| format!("insert_request_log: {e}"))?;
        Ok(())
    }

    /// Query aggregated summary for a time range.
    pub fn query_summary(&self, start_ts: i64, end_ts: i64) -> Result<UsageSummary, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT
                    COUNT(*)                  as total_requests,
                    SUM(CASE WHEN status_code = 200 THEN 1 ELSE 0 END) as success_count,
                    COALESCE(SUM(input_tokens), 0)            as total_input_tokens,
                    COALESCE(SUM(output_tokens), 0)           as total_output_tokens,
                    COALESCE(SUM(cache_creation_tokens), 0)   as total_cache_creation_tokens,
                    COALESCE(SUM(cache_read_tokens), 0)       as total_cache_read_tokens,
                    COALESCE(SUM(total_cost_usd), 0.0)        as total_cost_usd,
                    COALESCE(AVG(CASE WHEN status_code = 200 THEN latency_ms END), 0.0) as avg_latency_ms
                 FROM request_logs
                 WHERE created_at >= ?1 AND created_at < ?2",
            )
            .map_err(|e| format!("prepare summary: {e}"))?;

        let summary = stmt
            .query_row(params![start_ts, end_ts], |row| {
                Ok(UsageSummary {
                    total_requests: row.get(0)?,
                    success_count: row.get(1)?,
                    total_input_tokens: row.get(2)?,
                    total_output_tokens: row.get(3)?,
                    total_cache_creation_tokens: row.get(4)?,
                    total_cache_read_tokens: row.get(5)?,
                    total_cost_usd: row.get(6)?,
                    avg_latency_ms: row.get(7)?,
                    cache_hit_rate: 0.0,
                })
            })
            .map_err(|e| format!("query_summary: {e}"))?;

        // Compute cache hit rate
        let cacheable = summary.total_input_tokens
            + summary.total_cache_creation_tokens
            + summary.total_cache_read_tokens;
        let cache_hit_rate = if cacheable > 0 {
            summary.total_cache_read_tokens as f64 / cacheable as f64
        } else {
            0.0
        };

        Ok(UsageSummary {
            cache_hit_rate,
            ..summary
        })
    }

    /// Query trend data grouped by hour or day.
    pub fn query_trends(
        &self,
        start_ts: i64,
        end_ts: i64,
        group_by: &str,
    ) -> Result<Vec<TrendPoint>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let sql = match group_by {
            "hour" => r#"
                SELECT
                    strftime('%Y-%m-%dT%H:00:00', created_at, 'unixepoch') as label,
                    (created_at / 3600) * 3600 as ts,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COUNT(*),
                    COALESCE(SUM(total_cost_usd), 0.0)
                FROM request_logs
                WHERE created_at >= ?1 AND created_at < ?2
                GROUP BY ts ORDER BY ts"#,
            "day" | _ => r#"
                SELECT
                    strftime('%Y-%m-%d', created_at, 'unixepoch') as label,
                    (created_at / 86400) * 86400 as ts,
                    COALESCE(SUM(input_tokens), 0),
                    COALESCE(SUM(output_tokens), 0),
                    COALESCE(SUM(cache_creation_tokens), 0),
                    COALESCE(SUM(cache_read_tokens), 0),
                    COUNT(*),
                    COALESCE(SUM(total_cost_usd), 0.0)
                FROM request_logs
                WHERE created_at >= ?1 AND created_at < ?2
                GROUP BY ts ORDER BY ts"#,
        };

        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("prepare trends: {e}"))?;

        let points = stmt
            .query_map(params![start_ts, end_ts], |row| {
                Ok(TrendPoint {
                    label: row.get(0)?,
                    timestamp: row.get(1)?,
                    input_tokens: row.get(2)?,
                    output_tokens: row.get(3)?,
                    cache_creation_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    request_count: row.get(6)?,
                    total_cost_usd: row.get(7)?,
                })
            })
            .map_err(|e| format!("query_trends: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok(points)
    }

    /// Query request logs with pagination.
    pub fn query_logs(
        &self,
        start_ts: i64,
        end_ts: i64,
        page: i32,
        page_size: i32,
    ) -> Result<(Vec<RequestLog>, i64), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let offset = (page.max(1) - 1) * page_size;

        let total: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM request_logs WHERE created_at >= ?1 AND created_at < ?2",
                params![start_ts, end_ts],
                |row| row.get(0),
            )
            .map_err(|e| format!("count logs: {e}"))?;

        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, model, provider_type, input_tokens, output_tokens,
                        cache_creation_tokens, cache_read_tokens, total_cost_usd,
                        latency_ms, status_code, error_message, is_streaming, created_at
                 FROM request_logs
                 WHERE created_at >= ?1 AND created_at < ?2
                 ORDER BY created_at DESC
                 LIMIT ?3 OFFSET ?4",
            )
            .map_err(|e| format!("prepare logs: {e}"))?;

        let logs = stmt
            .query_map(
                params![start_ts, end_ts, page_size, offset],
                |row| {
                    Ok(RequestLog {
                        id: row.get(0)?,
                        trace_id: row.get(1)?,
                        model: row.get(2)?,
                        provider_type: row.get(3)?,
                        input_tokens: row.get(4)?,
                        output_tokens: row.get(5)?,
                        cache_creation_tokens: row.get(6)?,
                        cache_read_tokens: row.get(7)?,
                        total_cost_usd: row.get(8)?,
                        latency_ms: row.get(9)?,
                        status_code: row.get(10)?,
                        error_message: row.get(11)?,
                        is_streaming: row.get::<_, i32>(12)? != 0,
                        created_at: row.get(13)?,
                    })
                },
            )
            .map_err(|e| format!("query_logs: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok((logs, total))
    }

    /// Roll up request logs older than 30 days into daily_rollups and delete originals.
    pub fn rollup_old_logs(&self) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cutoff = chrono::Local::now().timestamp() - 30 * 86400;

        conn.execute(
            "INSERT OR REPLACE INTO daily_rollups
                (date, model, request_count, success_count, input_tokens, output_tokens,
                 cache_creation_tokens, cache_read_tokens, total_cost_usd, avg_latency_ms)
             SELECT
                strftime('%Y-%m-%d', created_at, 'unixepoch'),
                model,
                COUNT(*),
                SUM(CASE WHEN status_code = 200 THEN 1 ELSE 0 END),
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_creation_tokens), 0),
                COALESCE(SUM(cache_read_tokens), 0),
                COALESCE(SUM(total_cost_usd), 0.0),
                COALESCE(AVG(CASE WHEN status_code = 200 THEN latency_ms END), 0.0)
             FROM request_logs
             WHERE created_at < ?1
             GROUP BY strftime('%Y-%m-%d', created_at, 'unixepoch'), model",
            params![cutoff],
        )
        .map_err(|e| format!("rollup insert: {e}"))?;

        let deleted = conn
            .execute(
                "DELETE FROM request_logs WHERE created_at < ?1",
                params![cutoff],
            )
            .map_err(|e| format!("rollup delete: {e}"))?;

        if deleted > 0 {
            tracing::info!(deleted, "[DataStore] Rolled up old request logs");
        }

        Ok(deleted as u64)
    }
}
