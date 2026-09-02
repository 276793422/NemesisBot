//! CRUD operations for usage statistics.

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{Connection, ToSql, params, params_from_iter};

use crate::db;
use crate::models::{LogFilter, RequestLog, TrendPoint, UsageSummary};
use crate::pricing_store::PricingStore;

/// Thread-safe SQLite data store.
pub struct DataStore {
    conn: Mutex<Connection>,
    /// 分层价目表（自定义 > 下载 > 内置），与 db 同目录落盘
    /// （`{workspace}/data/pricing_custom.json` 等）。LLM 调用方经
    /// [`Self::compute_cost_usd`] 计价，管理端经 [`Self::pricing`] 增删改。
    pricing: PricingStore,
}

impl DataStore {
    /// Open (or create) the database at `db_path`. The layered pricing store
    /// lives next to the db (`db_path.parent()`).
    pub fn open(db_path: &Path) -> Result<Self, String> {
        let conn = db::init_db(db_path)?;
        let pricing = PricingStore::open(db_path.parent().unwrap_or_else(|| Path::new(".")))?;
        Ok(Self {
            conn: Mutex::new(conn),
            pricing,
        })
    }

    /// Layered pricing store accessor (custom CRUD / download replace / meta).
    pub fn pricing(&self) -> &PricingStore {
        &self.pricing
    }

    /// Layered cost computation for one LLM request
    /// (custom > downloaded > embedded; unknown model → `0.0`).
    pub fn compute_cost_usd(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> f64 {
        self.pricing.compute_cost_usd(
            model,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        )
    }

    /// Layered cost computation with per-component breakdown
    /// (A3 明细表分项成本). Unknown model → `None`（调用方记空
    /// `pricing_model` + 全 0 分项）。
    pub fn compute_cost_breakdown(
        &self,
        model: &str,
        input_tokens: i64,
        output_tokens: i64,
        cache_creation_tokens: i64,
        cache_read_tokens: i64,
    ) -> Option<crate::CostBreakdown> {
        self.pricing.compute_cost_breakdown(
            model,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        )
    }

    /// Record a single LLM request log.
    pub fn insert_request_log(&self, log: &RequestLog) -> Result<(), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO request_logs
                (trace_id, model, provider_type, input_tokens, output_tokens,
                 cache_creation_tokens, cache_read_tokens, total_cost_usd,
                 latency_ms, status_code, error_message, is_streaming, created_at,
                 pricing_model, input_cost_usd, output_cost_usd,
                 cache_creation_cost_usd, cache_read_cost_usd,
                 first_token_ms, session_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
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
                log.pricing_model,
                log.input_cost_usd,
                log.output_cost_usd,
                log.cache_creation_cost_usd,
                log.cache_read_cost_usd,
                log.first_token_ms,
                log.session_key,
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
                    COALESCE(SUM(CASE WHEN status_code = 200 THEN 1 ELSE 0 END), 0) as success_count,
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
        // DeepSeek/OpenAI: cache_creation = miss tokens, cache_read = hit tokens
        //   correct formula: cache_read / (cache_creation + cache_read)
        // Anthropic: cache_creation = write tokens, cache_read = read tokens
        //   correct formula: cache_read / (cache_creation + cache_read)
        // Both simplify to: hits / (hits + misses)
        let cacheable = summary.total_cache_creation_tokens + summary.total_cache_read_tokens;
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
            "hour" => {
                r#"
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
                GROUP BY ts ORDER BY ts"#
            }
            _ => {
                r#"
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
                GROUP BY ts ORDER BY ts"#
            }
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

    /// Query request logs with pagination and optional orthogonal filters
    /// (model 子串 / status 精确 / session_key 子串，A3 请求明细 tab)。
    pub fn query_logs(
        &self,
        start_ts: i64,
        end_ts: i64,
        page: i32,
        page_size: i32,
        filter: &LogFilter,
    ) -> Result<(Vec<RequestLog>, i64), String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;

        let offset = (page.max(1) - 1) * page_size;

        // WHERE 动态拼装：时间范围恒在（?1/?2），过滤条件按需追加。
        // 参数占位序号 = 绑定序（1-based），与 params_vec 压入顺序一致。
        let mut conds: Vec<String> = vec![
            "created_at >= ?1".to_string(),
            "created_at < ?2".to_string(),
        ];
        let mut bind: Vec<Box<dyn ToSql>> = vec![Box::new(start_ts), Box::new(end_ts)];
        if let Some(m) = filter.model.as_deref().filter(|s| !s.is_empty()) {
            bind.push(Box::new(format!("%{m}%")));
            conds.push(format!("model LIKE ?{}", bind.len()));
        }
        if let Some(s) = filter.status {
            bind.push(Box::new(s));
            conds.push(format!("status_code = ?{}", bind.len()));
        }
        if let Some(sk) = filter.session_key.as_deref().filter(|s| !s.is_empty()) {
            bind.push(Box::new(format!("%{sk}%")));
            conds.push(format!("session_key LIKE ?{}", bind.len()));
        }
        let where_clause = conds.join(" AND ");

        let count_sql = format!("SELECT COUNT(*) FROM request_logs WHERE {where_clause}");
        let total: i64 = conn
            .query_row(
                &count_sql,
                params_from_iter(bind.iter().map(|b| b.as_ref())),
                |row| row.get(0),
            )
            .map_err(|e| format!("count logs: {e}"))?;

        let select_sql = format!(
            "SELECT id, trace_id, model, provider_type, input_tokens, output_tokens,
                    cache_creation_tokens, cache_read_tokens, total_cost_usd,
                    latency_ms, status_code, error_message, is_streaming, created_at,
                    pricing_model, input_cost_usd, output_cost_usd,
                    cache_creation_cost_usd, cache_read_cost_usd,
                    first_token_ms, session_key
             FROM request_logs
             WHERE {where_clause}
             ORDER BY created_at DESC
             LIMIT ?{} OFFSET ?{}",
            bind.len() + 1,
            bind.len() + 2,
        );
        bind.push(Box::new(page_size));
        bind.push(Box::new(offset));

        let mut stmt = conn
            .prepare(&select_sql)
            .map_err(|e| format!("prepare logs: {e}"))?;

        let logs = stmt
            .query_map(
                params_from_iter(bind.iter().map(|b| b.as_ref())),
                row_to_log,
            )
            .map_err(|e| format!("query_logs: {e}"))?
            .filter_map(|r| r.ok())
            .collect();

        Ok((logs, total))
    }

    /// Fetch a single request-log row by id（A3 请求明细详情面板）。
    pub fn get_request_log(&self, id: i64) -> Result<Option<RequestLog>, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT id, trace_id, model, provider_type, input_tokens, output_tokens,
                        cache_creation_tokens, cache_read_tokens, total_cost_usd,
                        latency_ms, status_code, error_message, is_streaming, created_at,
                        pricing_model, input_cost_usd, output_cost_usd,
                        cache_creation_cost_usd, cache_read_cost_usd,
                        first_token_ms, session_key
                 FROM request_logs WHERE id = ?1",
            )
            .map_err(|e| format!("prepare get_request_log: {e}"))?;
        let mut rows = stmt
            .query_map(params![id], row_to_log)
            .map_err(|e| format!("get_request_log: {e}"))?;
        match rows.next() {
            Some(Ok(log)) => Ok(Some(log)),
            Some(Err(e)) => Err(format!("get_request_log row: {e}")),
            None => Ok(None),
        }
    }

    /// Roll up request logs older than 30 days into daily_rollups and delete originals.
    /// Retention sweep（A3 保留策略，可配置）：把 `retention_days` 天前的
    /// 明细行 rollup 进 `daily_rollups` 后删除（`None` = 跳过按天清理，
    /// 对应 config `usage.retention_days = 0`）；`max_rows` 给定时再按
    /// created_at 最旧优先裁掉超额行（防膨胀；聚合已由按天步做掉，
    /// 仅裁增量）。返回删除的总行数。
    ///
    /// 挂载点在 gateway 周期任务（启动时 + 每 6h）；此前 `rollup_old_logs`
    /// 从未被生产调用（只有测试），本次一并接上。
    pub fn retention_sweep(
        &self,
        retention_days: Option<i64>,
        max_rows: Option<i64>,
    ) -> Result<u64, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let mut deleted_total: u64 = 0;

        if let Some(days) = retention_days.filter(|d| *d > 0) {
            let cutoff = chrono::Local::now().timestamp() - days * 86400;

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
            deleted_total += deleted as u64;
        }

        // 条数上限：超出部分按最旧优先直接删（聚合已由按天步覆盖）。
        if let Some(cap) = max_rows.filter(|c| *c > 0) {
            let deleted = conn
                .execute(
                    "DELETE FROM request_logs WHERE id NOT IN
                        (SELECT id FROM request_logs ORDER BY created_at DESC LIMIT ?1)",
                    params![cap],
                )
                .map_err(|e| format!("max_rows trim: {e}"))?;
            deleted_total += deleted as u64;
        }

        if deleted_total > 0 {
            tracing::info!(
                deleted_total,
                ?retention_days,
                "[DataStore] Retention sweep removed old request logs"
            );
        }

        Ok(deleted_total)
    }
}

/// Row → [`RequestLog`]（列序与 query_logs / get_request_log 的 SELECT 一致）。
fn row_to_log(row: &rusqlite::Row<'_>) -> rusqlite::Result<RequestLog> {
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
        pricing_model: row.get(14)?,
        input_cost_usd: row.get(15)?,
        output_cost_usd: row.get(16)?,
        cache_creation_cost_usd: row.get(17)?,
        cache_read_cost_usd: row.get(18)?,
        first_token_ms: row.get(19)?,
        session_key: row.get(20)?,
    })
}
